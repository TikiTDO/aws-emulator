// SPDX-License-Identifier: CC0-1.0

//! `GetFlowLogsIntegrationTemplate`: the CloudFormation template that integrates one stored
//! flow log with Amazon Athena, driven over the wire the service actually speaks — `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rules under test, each asserted over a SET of inputs rather than over one request:
//!
//! * The Athena table MAPS THE FLOW LOG'S OWN RECORD FORMAT: one column per `${field-id}`
//!   token of that flow log's `LogFormat`, in the order the format lays them out, named and
//!   typed the way AWS's published flow-log table names and types it. The rows differ in the
//!   format the flow log was created with — the documented default, a narrow one, a reordered
//!   one, transit gateway fields, and one that repeats a field.
//! * The NAMED QUERIES are the ones the table can answer: a query whose columns the format
//!   does not carry is not offered. The rows are the same formats.
//! * The PARTITION LOAD FREQUENCY decides the loader: `none` renders no Lambda function and
//!   no schedule at all, and each of the other three renders one on its own cron expression.
//! * The PARTITIONS LOADED UP FRONT are the days between `PartitionStartDate` and
//!   `PartitionEndDate`, inclusive. The rows differ in the span — one day, a few days, across
//!   a month boundary, and across a leap day.
//! * The table's LOCATION is the flow log's own Amazon S3 destination, under the object
//!   layout that flow log's `DestinationOptions` ask for. The rows differ in whether
//!   Hive-compatible prefixes were asked for.
//! * One `AthenaIntegration` is one Athena work group writing to the result location that
//!   entry named, so a request naming two of them gets two.
//! * Every refusal answers with ITS OWN code and message, and never with a 501 or a success:
//!   the rows span the flow log that does not exist, the one Athena cannot read, the ARNs
//!   that name no S3 location, the inverted partition range and the range past what this
//!   emulator renders.
//! * Generating a template is a READ: two identical calls answer the same template and the
//!   flow log store is untouched, and an authorized `DryRun` is answered before the flow log
//!   is looked up at all.

use serde_json::Value;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;

use super::ec2_query::{element, error_code, error_message, member};

/// The Region every call here is signed for — the Region the flow log records are published
/// under, so it is part of the table's location.
const REGION: &str = "us-east-1";

/// The bucket ARN the seeded flow logs publish to.
const LOGS_BUCKET_ARN: &str = "arn:aws:s3:::flow-logs-bucket/prefix";

/// Where the generated template is asked to be delivered.
const CONFIG_BUCKET_ARN: &str = "arn:aws:s3:::flow-logs-config-bucket";

/// Where an integration's Athena query results are asked to go.
const RESULTS_BUCKET_ARN: &str = "arn:aws:s3:::flow-logs-results-bucket/athena";

/// A well-formed flow log id this account holds no flow log for.
const ABSENT_FLOW_LOG_ID: &str = "fl-0123456789abcdef0";

/// The prefix of the S3 URI the seeded flow logs' records live under, without the calendar
/// part: `s3://<bucket>/<prefix>/AWSLogs/<account>/vpcflowlogs/<region>/` (Amazon VPC User
/// Guide, "Flow log files" — the layout of a flow log's objects in Amazon S3), written out
/// here rather than read from the crate so a wrong layout is caught.
fn records_root() -> String {
    format!(
        "s3://flow-logs-bucket/prefix/AWSLogs/{account}/vpcflowlogs/{REGION}/",
        account = engine::emulator_account_id(),
    )
}

/// The same layout with the Hive-compatible prefixes `DestinationOptions` can ask for (same
/// guide section).
fn hive_records_root() -> String {
    format!(
        "s3://flow-logs-bucket/prefix/AWSLogs/aws-account-id={account}/\
         aws-service=vpcflowlogs/aws-region={REGION}/",
        account = engine::emulator_account_id(),
    )
}

/// One form field.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A VPC created over the wire, by id — so a flow log can monitor a VPC the account really
/// has without reaching into the store.
async fn create_vpc(router: &MultiServiceRouter, cidr_block: &str) -> String {
    let params = [param("CidrBlock", cidr_block)];
    let (status, body) = super::ec2_query::call(router, REGION, "CreateVpc", &params).await;
    assert_eq!(
        status, 200,
        "CreateVpc {cidr_block}: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "vpcId").unwrap_or_else(|| panic!("CreateVpc answered no vpcId: {body}"))
}

/// One flow log on `vpc_id` publishing to Amazon S3, created over the wire, by id.
///
/// `log_format` is the record format the flow log is created with, or `None` for the
/// documented default; `hive_compatible` asks for Hive-compatible object prefixes.
async fn create_s3_flow_log(
    router: &MultiServiceRouter,
    case: &str,
    vpc_id: &str,
    log_format: Option<&str>,
    hive_compatible: bool,
) -> String {
    let mut params = vec![
        param("ResourceType", "VPC"),
        param("ResourceId.1", vpc_id),
        param("TrafficType", "ALL"),
        param("LogDestinationType", "s3"),
        param("LogDestination", LOGS_BUCKET_ARN),
    ];
    if let Some(log_format) = log_format {
        params.push(param("LogFormat", log_format));
    }
    if hive_compatible {
        params.push(param("DestinationOptions.HiveCompatiblePartitions", "true"));
    }
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateFlowLogs", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateFlowLogs expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "item")
        .unwrap_or_else(|| panic!("case {case:?}: CreateFlowLogs answered no flow log id: {body}"))
}

/// One flow log on `vpc_id` publishing to CloudWatch Logs — the destination an Athena
/// integration cannot read.
async fn create_cloud_watch_logs_flow_log(router: &MultiServiceRouter, vpc_id: &str) -> String {
    let params = vec![
        param("ResourceType", "VPC"),
        param("ResourceId.1", vpc_id),
        param("TrafficType", "ALL"),
        param("LogDestinationType", "cloud-watch-logs"),
        param("LogGroupName", "flow-logs"),
        param(
            "DeliverLogsPermissionArn",
            "arn:aws:iam::000000000000:role/flow-logs-delivery",
        ),
    ];
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateFlowLogs", &params).await;
    assert_eq!(
        status, 200,
        "CreateFlowLogs on CloudWatch Logs: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "item").unwrap_or_else(|| panic!("CreateFlowLogs answered no id: {body}"))
}

/// The form fields of one `AthenaIntegration` entry at `position` (counting from 1).
fn integration_params(
    position: usize,
    results_arn: &str,
    frequency: &str,
    start: Option<&str>,
    end: Option<&str>,
) -> Vec<(String, String)> {
    let prefix = format!("IntegrateService.AthenaIntegration.{position}");
    let mut fields = vec![
        param(
            &format!("{prefix}.IntegrationResultS3DestinationArn"),
            results_arn,
        ),
        param(&format!("{prefix}.PartitionLoadFrequency"), frequency),
    ];
    if let Some(start) = start {
        fields.push(param(&format!("{prefix}.PartitionStartDate"), start));
    }
    if let Some(end) = end {
        fields.push(param(&format!("{prefix}.PartitionEndDate"), end));
    }
    fields
}

/// The form fields of a whole request: the flow log, where the template is delivered, and the
/// integrations.
fn request_params(flow_log_id: &str, integrations: &[Vec<(String, String)>]) -> Vec<(String, String)> {
    let mut params = vec![
        param("FlowLogId", flow_log_id),
        param("ConfigDeliveryS3DestinationArn", CONFIG_BUCKET_ARN),
    ];
    for integration in integrations {
        params.extend(integration.iter().cloned());
    }
    params
}

/// One `GetFlowLogsIntegrationTemplate` call, as its HTTP status and XML body.
async fn get_template(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, "GetFlowLogsIntegrationTemplate", params).await
}

/// The generated template of a served call, parsed as the JSON CloudFormation document it is.
async fn template(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Value {
    let (status, body) = get_template(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let rendered = member(&body, "result")
        .or_else(|| member(&body, "Result"))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the response must carry the generated template: {body}")
        });
    let parsed: Value = serde_json::from_str(&rendered).unwrap_or_else(|error| {
        panic!("case {case:?}: the template must be JSON ({error}): {rendered}")
    });
    assert_eq!(
        parsed["AWSTemplateFormatVersion"].as_str(),
        Some("2010-09-09"),
        "case {case:?}: expected a CloudFormation template, got {parsed}"
    );
    parsed
}

/// The resources of `template` whose `Type` is `type_name`, by logical id, in logical-id
/// order.
fn resources_of_type(template: &Value, type_name: &str) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> = template["Resources"]
        .as_object()
        .map(|resources| {
            resources
                .iter()
                .filter(|(_, resource)| resource["Type"].as_str() == Some(type_name))
                .map(|(id, resource)| (id.clone(), resource.clone()))
                .collect()
        })
        .unwrap_or_default();
    found.sort_by(|(left, _), (right, _)| left.cmp(right));
    found
}

/// The one resource of `template` whose `Type` is `type_name`, asserting there is exactly one.
fn only_resource(case: &str, template: &Value, type_name: &str) -> Value {
    let found = resources_of_type(template, type_name);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {type_name}, got {ids:?} in {template}",
        ids = found.iter().map(|(id, _)| id.clone()).collect::<Vec<String>>(),
    );
    found
        .first()
        .map(|(_, resource)| resource.clone())
        .unwrap_or(Value::Null)
}

/// The `(name, type)` of every column of the generated table, in table order.
fn table_columns(case: &str, template: &Value) -> Vec<(String, String)> {
    let table = only_resource(case, template, "AWS::Glue::Table");
    table["Properties"]["TableInput"]["StorageDescriptor"]["Columns"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: the table must carry columns: {table}"))
        .iter()
        .map(|column| {
            (
                column["Name"].as_str().unwrap_or_default().to_string(),
                column["Type"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The `Name` of every named query of `template`, sorted.
fn named_query_names(template: &Value) -> Vec<String> {
    let mut names: Vec<String> = resources_of_type(template, "AWS::Athena::NamedQuery")
        .iter()
        .filter_map(|(_, query)| {
            query["Properties"]["Name"]
                .as_str()
                .map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// The `QueryString` of the named query that loads the historical partitions of integration
/// `position`.
fn historical_partitions_query(case: &str, template: &Value, position: usize) -> String {
    let expected = format!("VpcFlowLogsLoadHistoricalPartitions{position}");
    resources_of_type(template, "AWS::Athena::NamedQuery")
        .iter()
        .find(|(_, query)| query["Properties"]["Name"].as_str() == Some(expected.as_str()))
        .and_then(|(_, query)| query["Properties"]["QueryString"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a {expected} named query in {template}")
        })
}

/// The Athena identifiers the integration of `flow_log_id` uses, derived here independently
/// of the handler: an Athena identifier carries only lowercase letters, digits and `_`.
fn athena_names(flow_log_id: &str) -> (String, String) {
    let stem = flow_log_id.replace('-', "_");
    (
        format!("vpc_flow_logs_{stem}"),
        format!("vpc_flow_logs_{stem}_records"),
    )
}

// ---------------------------------------------------------------------------
// The table maps the flow log's own record format
// ---------------------------------------------------------------------------

/// One row of the record-format table: the format the flow log is created with, the columns
/// its Athena table must then carry, and the named queries that table can answer.
struct FormatCase {
    name: &'static str,
    /// The `LogFormat` the flow log is created with, or `None` for the documented default.
    log_format: Option<&'static str>,
    /// The columns, in table order, as `(name, Athena type)`.
    columns: &'static [(&'static str, &'static str)],
    /// The named queries the template must offer for integration 1.
    queries: &'static [&'static str],
}

/// Every column of the default record format, with the type AWS's published Athena table
/// gives it (Amazon VPC User Guide, "Query flow logs using Amazon Athena" — the
/// `CREATE EXTERNAL TABLE` column list). Written out here as the test's own expectation.
const DEFAULT_FORMAT_COLUMNS: &[(&str, &str)] = &[
    ("version", "int"),
    ("account_id", "string"),
    ("interface_id", "string"),
    ("srcaddr", "string"),
    ("dstaddr", "string"),
    ("srcport", "int"),
    ("dstport", "int"),
    ("protocol", "bigint"),
    ("packets", "bigint"),
    ("bytes", "bigint"),
    ("start", "bigint"),
    ("end", "bigint"),
    ("action", "string"),
    ("log_status", "string"),
];

/// The named queries a table carrying the whole default format can answer.
const ALL_QUERIES: &[&str] = &[
    "VpcFlowLogsAcceptedTraffic1",
    "VpcFlowLogsRejectedTraffic1",
    "VpcFlowLogsSshRdpTraffic1",
    "VpcFlowLogsTopTalkers1",
    "VpcFlowLogsTotalBytesTransferred1",
    "VpcFlowLogsTrafficByProtocol1",
];

/// The Athena table is the flow log's record format: one column per `${field-id}` token, in
/// the format's own order, named and typed as AWS's published flow-log table is — and the
/// named queries offered are exactly those whose columns that table has.
#[tokio::test]
async fn the_table_maps_the_flow_logs_own_record_format() {
    let cases: &[FormatCase] = &[
        FormatCase {
            name: "the documented default format",
            log_format: None,
            columns: DEFAULT_FORMAT_COLUMNS,
            queries: ALL_QUERIES,
        },
        FormatCase {
            name: "a narrow format",
            log_format: Some("${srcaddr} ${dstaddr} ${bytes}"),
            columns: &[
                ("srcaddr", "string"),
                ("dstaddr", "string"),
                ("bytes", "bigint"),
            ],
            // Every other query reads a field this format does not carry.
            queries: &["VpcFlowLogsTotalBytesTransferred1"],
        },
        FormatCase {
            name: "a reordered format",
            log_format: Some("${bytes} ${packets} ${srcaddr} ${dstaddr}"),
            columns: &[
                ("bytes", "bigint"),
                ("packets", "bigint"),
                ("srcaddr", "string"),
                ("dstaddr", "string"),
            ],
            queries: &["VpcFlowLogsTopTalkers1", "VpcFlowLogsTotalBytesTransferred1"],
        },
        FormatCase {
            name: "transit gateway record fields",
            log_format: Some("${tgw-id} ${packets-lost-no-route} ${action}"),
            columns: &[
                ("tgw_id", "string"),
                ("packets_lost_no_route", "bigint"),
                ("action", "string"),
            ],
            queries: &["VpcFlowLogsAcceptedTraffic1", "VpcFlowLogsRejectedTraffic1"],
        },
        FormatCase {
            name: "a format that repeats a field",
            log_format: Some("${action} ${action} ${srcaddr}"),
            // A table cannot carry one column twice, so the repeat is one column.
            columns: &[("action", "string"), ("srcaddr", "string")],
            queries: &["VpcFlowLogsAcceptedTraffic1", "VpcFlowLogsRejectedTraffic1"],
        },
        FormatCase {
            name: "a format reading a tag of the monitored resource",
            log_format: Some("${srcaddr} ${aws:instance-tag:Owner}"),
            columns: &[("srcaddr", "string"), ("aws_instance_tag_owner", "string")],
            queries: &[],
        },
    ];

    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.240.0.0/16").await;

    for row in cases {
        let case = row.name;
        let flow_log_id =
            create_s3_flow_log(&router, case, &vpc_id, row.log_format, false).await;
        let params = request_params(
            &flow_log_id,
            &[integration_params(1, RESULTS_BUCKET_ARN, "none", None, None)],
        );
        let rendered = template(&router, case, &params).await;

        let expected: Vec<(String, String)> = row
            .columns
            .iter()
            .map(|(name, data_type)| (name.to_string(), data_type.to_string()))
            .collect();
        let actual = table_columns(case, &rendered);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the columns {expected:?}, got {actual:?}"
        );

        let (database, table) = athena_names(&flow_log_id);
        let glue_table = only_resource(case, &rendered, "AWS::Glue::Table");
        assert_eq!(
            glue_table["Properties"]["TableInput"]["Name"].as_str(),
            Some(table.as_str()),
            "case {case:?}: expected the table {table}, got {glue_table}"
        );
        assert_eq!(
            glue_table["Properties"]["DatabaseName"].as_str(),
            Some(database.as_str()),
            "case {case:?}: expected the database {database}, got {glue_table}"
        );

        // The named queries the table can answer, and nothing else — the historical
        // partition loader is counted beside them.
        let mut expected_queries: Vec<String> =
            row.queries.iter().map(|name| (*name).to_string()).collect();
        expected_queries.push("VpcFlowLogsLoadHistoricalPartitions1".to_string());
        expected_queries.sort();
        let actual_queries = named_query_names(&rendered);
        assert_eq!(
            actual_queries, expected_queries,
            "case {case:?}: expected the named queries {expected_queries:?}, got \
             {actual_queries:?}"
        );
        // Each one reads the table this flow log implies.
        for (id, query) in resources_of_type(&rendered, "AWS::Athena::NamedQuery") {
            let statement = query["Properties"]["QueryString"]
                .as_str()
                .unwrap_or_default();
            let names_the_table =
                statement.contains(&format!("{database}.{table}")) || statement.contains(&table);
            assert!(
                names_the_table,
                "case {case:?}: the query {id} must read {database}.{table}, got {statement:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The partition load frequency decides the loader
// ---------------------------------------------------------------------------

/// Each `PartitionLoadFrequency` member and the schedule its loader runs on: `none` asks for
/// no loader at all, and the other three for one on their own cron expression.
#[tokio::test]
async fn the_partition_load_frequency_decides_the_loader_and_its_schedule() {
    // Each row: the frequency, and the cron expression the generated schedule must carry —
    // `None` for the frequency that renders no loader.
    let cases: &[(&str, Option<&str>)] = &[
        ("none", None),
        ("daily", Some("cron(0 0 * * ? *)")),
        ("weekly", Some("cron(0 0 ? * 1 *)")),
        ("monthly", Some("cron(0 0 1 * ? *)")),
    ];

    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.241.0.0/16").await;
    let flow_log_id = create_s3_flow_log(&router, "loader", &vpc_id, None, false).await;
    let (database, table) = athena_names(&flow_log_id);

    for (frequency, expected_schedule) in cases {
        let case = format!("the {frequency} frequency");
        let params = request_params(
            &flow_log_id,
            &[integration_params(1, RESULTS_BUCKET_ARN, frequency, None, None)],
        );
        let rendered = template(&router, &case, &params).await;

        let functions = resources_of_type(&rendered, "AWS::Lambda::Function");
        let rules = resources_of_type(&rendered, "AWS::Events::Rule");
        let roles = resources_of_type(&rendered, "AWS::IAM::Role");
        match expected_schedule {
            None => {
                for (kind, found) in [
                    ("AWS::Lambda::Function", &functions),
                    ("AWS::Events::Rule", &rules),
                    ("AWS::IAM::Role", &roles),
                ] {
                    assert!(
                        found.is_empty(),
                        "case {case:?}: a frequency that asks for no loader must render no \
                         {kind}, got {ids:?}",
                        ids = found.iter().map(|(id, _)| id.clone()).collect::<Vec<String>>(),
                    );
                }
            }
            Some(schedule) => {
                let rule = only_resource(&case, &rendered, "AWS::Events::Rule");
                assert_eq!(
                    rule["Properties"]["ScheduleExpression"].as_str(),
                    Some(*schedule),
                    "case {case:?}: expected the schedule {schedule}, got {rule}"
                );
                let function = only_resource(&case, &rendered, "AWS::Lambda::Function");
                let variables = &function["Properties"]["Environment"]["Variables"];
                assert_eq!(
                    variables["DATABASE_NAME"].as_str(),
                    Some(database.as_str()),
                    "case {case:?}: the loader must be told its database, got {function}"
                );
                assert_eq!(
                    variables["TABLE_NAME"].as_str(),
                    Some(table.as_str()),
                    "case {case:?}: the loader must be told its table, got {function}"
                );
                let location = variables["PARTITION_LOCATION_TEMPLATE"]
                    .as_str()
                    .unwrap_or_default();
                assert!(
                    location.starts_with(&records_root()),
                    "case {case:?}: the loader must point at {root}, got {location:?}",
                    root = records_root(),
                );
                // The rule invokes the function this template created.
                let targets = rule["Properties"]["Targets"].to_string();
                assert!(
                    targets.contains("VPCFlowLogsPartitionLoader1"),
                    "case {case:?}: the schedule must target the loader, got {targets}"
                );
                // The loader runs under a role of its own, scoped to the two buckets.
                let role = only_resource(&case, &rendered, "AWS::IAM::Role").to_string();
                for expected in [
                    "arn:aws:s3:::flow-logs-bucket",
                    "arn:aws:s3:::flow-logs-results-bucket",
                ] {
                    assert!(
                        role.contains(expected),
                        "case {case:?}: the loader's role must name {expected}, got {role}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The partitions loaded up front
// ---------------------------------------------------------------------------

/// One row of the partition-range table: the two timestamps, and the calendar days the
/// template must load a partition for.
struct RangeCase {
    name: &'static str,
    start: &'static str,
    end: &'static str,
    /// Each day as `(year, month, day)`.
    days: &'static [(&'static str, &'static str, &'static str)],
}

/// The partitions a template loads up front are the days between `PartitionStartDate` and
/// `PartitionEndDate` inclusive, each pointing at that day's own prefix.
#[tokio::test]
async fn the_loaded_partitions_are_the_days_between_the_two_timestamps() {
    let cases: &[RangeCase] = &[
        RangeCase {
            name: "a single day",
            start: "2026-03-04T00:00:00Z",
            end: "2026-03-04T23:59:59Z",
            days: &[("2026", "03", "04")],
        },
        RangeCase {
            name: "three days",
            start: "2026-03-04T06:00:00Z",
            end: "2026-03-06T06:00:00Z",
            days: &[("2026", "03", "04"), ("2026", "03", "05"), ("2026", "03", "06")],
        },
        RangeCase {
            name: "across a month boundary",
            start: "2026-01-30T00:00:00Z",
            end: "2026-02-02T00:00:00Z",
            days: &[
                ("2026", "01", "30"),
                ("2026", "01", "31"),
                ("2026", "02", "01"),
                ("2026", "02", "02"),
            ],
        },
        RangeCase {
            // 2024 is a leap year, so February has a 29th.
            name: "across a leap day",
            start: "2024-02-27T00:00:00Z",
            end: "2024-03-01T00:00:00Z",
            days: &[
                ("2024", "02", "27"),
                ("2024", "02", "28"),
                ("2024", "02", "29"),
                ("2024", "03", "01"),
            ],
        },
        RangeCase {
            // 2026 is not, so the day after the 28th is March.
            name: "across the end of a common February",
            start: "2026-02-27T00:00:00Z",
            end: "2026-03-01T00:00:00Z",
            days: &[
                ("2026", "02", "27"),
                ("2026", "02", "28"),
                ("2026", "03", "01"),
            ],
        },
    ];

    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.242.0.0/16").await;
    let flow_log_id = create_s3_flow_log(&router, "partitions", &vpc_id, None, false).await;

    for row in cases {
        let case = row.name;
        let params = request_params(
            &flow_log_id,
            &[integration_params(
                1,
                RESULTS_BUCKET_ARN,
                "daily",
                Some(row.start),
                Some(row.end),
            )],
        );
        let rendered = template(&router, case, &params).await;
        let statement = historical_partitions_query(case, &rendered, 1);

        assert_eq!(
            statement.matches("PARTITION (").count(),
            row.days.len(),
            "case {case:?}: expected {count} partitions, got {statement}",
            count = row.days.len(),
        );
        for (year, month, day) in row.days {
            let keys = format!("PARTITION (year='{year}', month='{month}', day='{day}')");
            let location = format!("LOCATION '{root}{year}/{month}/{day}/'", root = records_root());
            for expected in [&keys, &location] {
                assert!(
                    statement.contains(expected.as_str()),
                    "case {case:?}: expected {expected:?} in {statement}"
                );
            }
        }
    }
}

/// The table's location, and the location of every partition, follow the object layout the
/// flow log's own `DestinationOptions` ask for.
#[tokio::test]
async fn the_object_layout_follows_the_flow_logs_destination_options() {
    // Each row: whether the flow log asked for Hive-compatible prefixes, and the S3 prefix
    // its records are then under.
    let cases: &[(&str, bool, String)] = &[
        ("the default prefixes", false, records_root()),
        ("hive-compatible prefixes", true, hive_records_root()),
    ];

    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.243.0.0/16").await;

    for (case, hive_compatible, expected_root) in cases {
        let flow_log_id =
            create_s3_flow_log(&router, case, &vpc_id, None, *hive_compatible).await;
        let params = request_params(
            &flow_log_id,
            &[integration_params(
                1,
                RESULTS_BUCKET_ARN,
                "none",
                Some("2026-03-04T00:00:00Z"),
                Some("2026-03-04T00:00:00Z"),
            )],
        );
        let rendered = template(&router, case, &params).await;

        let table = only_resource(case, &rendered, "AWS::Glue::Table");
        assert_eq!(
            table["Properties"]["TableInput"]["StorageDescriptor"]["Location"].as_str(),
            Some(expected_root.as_str()),
            "case {case:?}: expected the table at {expected_root}, got {table}"
        );
        let statement = historical_partitions_query(case, &rendered, 1);
        let expected_location = if *hive_compatible {
            format!("LOCATION '{expected_root}year=2026/month=03/day=04/'")
        } else {
            format!("LOCATION '{expected_root}2026/03/04/'")
        };
        assert!(
            statement.contains(&expected_location),
            "case {case:?}: expected {expected_location:?} in {statement}"
        );
    }
}

// ---------------------------------------------------------------------------
// One integration, one work group
// ---------------------------------------------------------------------------

/// Each `AthenaIntegration` of the request gets its own Athena work group writing to the
/// result location that entry named, over the ONE table the flow log implies.
#[tokio::test]
async fn each_athena_integration_gets_its_own_work_group() {
    let (router, _engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.244.0.0/16").await;
    let flow_log_id = create_s3_flow_log(&router, "two integrations", &vpc_id, None, false).await;

    let second_results = "arn:aws:s3:::flow-logs-results-bucket/second";
    let params = request_params(
        &flow_log_id,
        &[
            integration_params(1, RESULTS_BUCKET_ARN, "daily", None, None),
            integration_params(2, second_results, "none", None, None),
        ],
    );
    let case = "two integrations";
    let rendered = template(&router, case, &params).await;

    let work_groups = resources_of_type(&rendered, "AWS::Athena::WorkGroup");
    let locations: Vec<String> = work_groups
        .iter()
        .map(|(_, group)| {
            group["Properties"]["WorkGroupConfiguration"]["ResultConfiguration"]
                ["OutputLocation"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    assert_eq!(
        locations,
        vec![
            "s3://flow-logs-results-bucket/athena/".to_string(),
            "s3://flow-logs-results-bucket/second/".to_string(),
        ],
        "case {case:?}: each integration must write its results where it asked, got \
         {locations:?}"
    );
    // One table for both, and one loader — only the first integration asked for a schedule.
    assert_eq!(
        resources_of_type(&rendered, "AWS::Glue::Table").len(),
        1,
        "case {case:?}: both integrations read the one table of the flow log: {rendered}"
    );
    assert_eq!(
        resources_of_type(&rendered, "AWS::Lambda::Function").len(),
        1,
        "case {case:?}: only the integration that asked for a schedule gets a loader: \
         {rendered}"
    );
    // Each integration carries its own historical partition query and its own query set.
    for position in [1, 2] {
        let statement = historical_partitions_query(case, &rendered, position);
        assert!(
            statement.starts_with("ALTER TABLE "),
            "case {case:?}: integration {position} must load its partitions, got {statement}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// One row of the refusal table: the request, and the code and message it must be refused
/// with.
struct RefusalCase {
    name: &'static str,
    params: Vec<(String, String)>,
    expected_code: &'static str,
    /// A fragment the message must carry, or the whole message where AWS fixes it.
    expected_message: String,
}

/// Every refusal answers with the code and the message of what is wrong with the request —
/// never with the scaffold's 501, and never with a success. Nothing is written on any of
/// these paths.
#[tokio::test]
async fn every_refusal_answers_its_own_code_and_writes_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.245.0.0/16").await;
    let flow_log_id = create_s3_flow_log(&router, "refusals", &vpc_id, None, false).await;
    let cloud_watch_id = create_cloud_watch_logs_flow_log(&router, &vpc_id).await;
    let integration = integration_params(1, RESULTS_BUCKET_ARN, "none", None, None);

    let cases: Vec<RefusalCase> = vec![
        RefusalCase {
            name: "a flow log this account does not hold",
            params: request_params(ABSENT_FLOW_LOG_ID, std::slice::from_ref(&integration)),
            expected_code: "InvalidFlowLogId.NotFound",
            expected_message: format!("Flow Log Id Not Found or Not Active.{ABSENT_FLOW_LOG_ID}"),
        },
        RefusalCase {
            name: "a flow log that publishes to CloudWatch Logs",
            params: request_params(&cloud_watch_id, std::slice::from_ref(&integration)),
            expected_code: "UnsupportedOperation",
            expected_message: "cloud-watch-logs".to_string(),
        },
        RefusalCase {
            name: "a config delivery destination that is not an S3 ARN",
            params: {
                let mut params =
                    request_params(&flow_log_id, std::slice::from_ref(&integration));
                params.retain(|(name, _)| name != "ConfigDeliveryS3DestinationArn");
                params.push(param("ConfigDeliveryS3DestinationArn", "flow-logs-config"));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_message: "ConfigDeliveryS3DestinationArn".to_string(),
        },
        RefusalCase {
            name: "a result destination naming no bucket",
            params: request_params(
                &flow_log_id,
                &[integration_params(1, "arn:aws:s3:::A", "none", None, None)],
            ),
            expected_code: "InvalidParameterValue",
            expected_message: "IntegrationResultS3DestinationArn".to_string(),
        },
        RefusalCase {
            name: "a result destination in another service",
            params: request_params(
                &flow_log_id,
                &[integration_params(
                    1,
                    "arn:aws:athena:us-east-1:000000000000:workgroup/primary",
                    "none",
                    None,
                    None,
                )],
            ),
            expected_code: "InvalidParameterValue",
            expected_message: "IntegrationResultS3DestinationArn".to_string(),
        },
        RefusalCase {
            name: "a partition range that ends before it starts",
            params: request_params(
                &flow_log_id,
                &[integration_params(
                    1,
                    RESULTS_BUCKET_ARN,
                    "daily",
                    Some("2026-03-04T00:00:00Z"),
                    Some("2026-03-01T00:00:00Z"),
                )],
            ),
            expected_code: "InvalidParameterCombination",
            expected_message: "PartitionEndDate".to_string(),
        },
        RefusalCase {
            name: "a partition range past what one template loads",
            params: request_params(
                &flow_log_id,
                &[integration_params(
                    1,
                    RESULTS_BUCKET_ARN,
                    "daily",
                    Some("2000-01-01T00:00:00Z"),
                    Some("2026-01-01T00:00:00Z"),
                )],
            ),
            expected_code: "InvalidParameterValue",
            expected_message: "daily".to_string(),
        },
        RefusalCase {
            // `IntegrateServices` is `@required`, so a request that carries no integration
            // at all is refused by the boundary validator before the handler runs — which is
            // the modelled constraint answering, not this operation.
            name: "a request naming no service integration",
            params: vec![
                param("FlowLogId", &flow_log_id),
                param("ConfigDeliveryS3DestinationArn", CONFIG_BUCKET_ARN),
            ],
            expected_code: "MalformedInput",
            expected_message: "integrateServices".to_string(),
        },
    ];

    let before = engine.state::<Ec2State>().lock().flow_logs.clone();
    for row in &cases {
        let case = row.name;
        let (status, body) = get_template(&router, &row.params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            row.expected_code,
            "case {case:?}: expected {expected}, got {body}",
            expected = row.expected_code,
        );
        let message = error_message(&body);
        assert!(
            message.contains(&row.expected_message),
            "case {case:?}: expected a message naming {expected:?}, got {message:?}",
            expected = row.expected_message,
        );
        let after = engine.state::<Ec2State>().lock().flow_logs.clone();
        assert_eq!(
            after.keys().collect::<Vec<&String>>(),
            before.keys().collect::<Vec<&String>>(),
            "case {case:?}: a refused call must write nothing"
        );
    }
}

/// An authorized `DryRun` is answered with `DryRunOperation` at HTTP 412 and reports no
/// template — whether or not the flow log it names exists, since the flag asks about the
/// CALLER's permissions and not about the account's resources.
#[tokio::test]
async fn a_dry_run_is_answered_before_the_flow_log_is_looked_up() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.246.0.0/16").await;
    let flow_log_id = create_s3_flow_log(&router, "dry run", &vpc_id, None, false).await;
    let integration = integration_params(1, RESULTS_BUCKET_ARN, "daily", None, None);

    for (case, named) in [
        ("a stored flow log", flow_log_id.as_str()),
        ("a flow log this account does not hold", ABSENT_FLOW_LOG_ID),
    ] {
        let mut params = request_params(named, std::slice::from_ref(&integration));
        params.push(param("DryRun", "true"));
        let (status, body) = get_template(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: unexpected message in {body}"
        );
    }

    assert_eq!(
        engine.state::<Ec2State>().lock().flow_logs.len(),
        1,
        "a dry run must leave the store exactly as it was"
    );
}

/// Generating the template is a READ: the same request answers the same template twice, and
/// the flow log store is untouched.
#[tokio::test]
async fn generating_the_template_changes_nothing_a_later_caller_sees() {
    let (router, engine) = super::ec2_query::emulator();
    let vpc_id = create_vpc(&router, "10.247.0.0/16").await;
    let flow_log_id = create_s3_flow_log(&router, "a read", &vpc_id, None, false).await;
    let params = request_params(
        &flow_log_id,
        &[integration_params(
            1,
            RESULTS_BUCKET_ARN,
            "monthly",
            Some("2026-03-01T00:00:00Z"),
            Some("2026-03-03T00:00:00Z"),
        )],
    );

    let before = engine.state::<Ec2State>().lock().flow_logs.clone();
    let first = template(&router, "first read", &params).await;
    let second = template(&router, "second read", &params).await;
    assert_eq!(
        first, second,
        "two identical calls must answer the same template"
    );
    let after = engine.state::<Ec2State>().lock().flow_logs.clone();
    assert_eq!(
        after.len(),
        before.len(),
        "generating a template must not write to the flow log store"
    );
    assert_eq!(
        after.keys().collect::<Vec<&String>>(),
        before.keys().collect::<Vec<&String>>(),
        "generating a template must not write to the flow log store"
    );

    // The template records which flow log it integrates and where it was asked to be
    // delivered, so a caller reading it back knows what it was generated from.
    let metadata = &first["Metadata"]["VPCFlowLogsIntegration"];
    assert_eq!(
        metadata["FlowLogId"].as_str(),
        Some(flow_log_id.as_str()),
        "the template must name the flow log it integrates, got {metadata}"
    );
    assert_eq!(
        metadata["ConfigDeliveryS3Destination"].as_str(),
        Some("s3://flow-logs-config-bucket/"),
        "the template must name where it was asked to be delivered, got {metadata}"
    );
}
