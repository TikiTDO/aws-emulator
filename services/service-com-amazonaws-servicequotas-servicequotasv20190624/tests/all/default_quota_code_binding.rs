// SPDX-License-Identifier: CC0-1.0

//! Which quota a `QuotaCode` is BOUND to, over the wire.
//!
//! `QuotaCode` is the identity Service Quotas publishes a quota under: it is the key
//! `GetAWSDefaultServiceQuota`, `GetServiceQuota` and `RequestServiceQuotaIncrease`
//! take, and the one a client reads out of AWS's published catalog. So a row is not
//! free to carry another quota's facts under it. The rules asserted here, each over a
//! TABLE of rows spanning ten `ServiceCode`s rather than one recorded case:
//!
//! * a row published under a `QuotaCode` reports the `QuotaName`, `Value`, `Unit`,
//!   `Adjustable`, `Period`, `UsageMetric` and `Description` of the quota AWS publishes
//!   UNDER THAT CODE — a resource plane never carries a request rate's numbers and a
//!   rate never carries a plane's, whichever way round the two sort;
//! * every row of a service's published catalog carries a `Description`, which is the
//!   modelled member a client renders the quota by, and no two rows of one service share
//!   a `QuotaName`, because a name identifies the quota within the service;
//! * a per-API request-rate row is published with the sentence AWS publishes for that
//!   class, naming the operation it throttles, and is metered by that operation's
//!   `AWS/Usage` `CallCount` under the service's `AWS/Usage` display name.
//!
//! The harness (`client`, `emulator`, `default_quotas`) is the one `quota_workflow.rs`
//! owns.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas as defaults, emulator};
use crate::recorded_rows_events_through_imagebuilder::PUBLISHED_WITHOUT_A_DESCRIPTION;

/// One published default-quota row, as the recorded default-quota listing of its
/// `ServiceCode` reports it.
struct Bound {
    service_code: &'static str,
    quota_code: &'static str,
    quota_name: &'static str,
    value: f64,
    unit: &'static str,
    adjustable: bool,
    /// The published description, where the evidence states its text; `None` pins none
    /// (a wrong assertion would be worse than no assertion), and the
    /// every-row-is-described rule below still covers it.
    description: Option<&'static str>,
    /// `(PeriodValue, PeriodUnit)` of a quota AWS publishes over a window.
    period: Option<(i32, &'static str)>,
    /// `(MetricName, MetricStatisticRecommendation, Service, Resource, Type)` of the
    /// `AWS/Usage` metric, for a quota CloudWatch publishes usage for.
    metric: Option<(
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        &'static str,
    )>,
}

/// The `AWS/Usage` `CallCount` metric of one API, under a service's `AWS/Usage` display
/// name: the shape every per-API request-rate row is metered by.
const fn call_count(
    usage_service: &'static str,
    api: &'static str,
) -> Option<(
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
)> {
    Some(("CallCount", "Sum", usage_service, api, "API"))
}

/// The rows this suite pins, across the ten `ServiceCode`s whose catalogs bind these
/// codes.
///
/// SOURCE: the recorded `ListAWSDefaultServiceQuotas` response of each of those service
/// codes, which states for each code the quota published under it — its name, default
/// value, unit, adjustability, window, usage metric and, where quoted, its description
/// text.
///
/// The rows differ along everything the rule generalises over: a counted resource plane
/// (`fms` protocol list, `fsx` ONTAP backups), a measured one (`glacier` archive size in
/// megabytes, `gamelift` script capacity in gigabytes, `imagebuilder` component size in
/// kilobytes, `events` event-pattern size in characters), a scheduling window in days
/// (`groundstation`) and a retention window in days (`guardduty`), a per-API request
/// rate metered by a call count (`firehose`, `events`), a resource plane metered by a
/// resource count over a five-minute window (`events` EventsV2), adjustable and
/// not-adjustable rows, and values from 1 to 1,048,576.
const BOUND_ROWS: &[Bound] = &[
    // events — the size of a rule's event pattern, published in characters, under the
    // code this catalog bound to an invocation rate.
    Bound {
        service_code: "events",
        quota_code: "L-664C5505",
        quota_name: "Event pattern size",
        value: 2048.0,
        unit: "None",
        adjustable: true,
        description: Some("Maximum size of an event pattern, in characters."),
        period: None,
        metric: None,
    },
    Bound {
        service_code: "events",
        quota_code: "L-388D1D08",
        quota_name: "Targets per rule",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    // A RESOURCE count, so its metric and window are a resource plane's and not an
    // API's.
    Bound {
        service_code: "events",
        quota_code: "L-DF788473",
        quota_name: "[EventsV2] Event Buses",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: Some((5, "MINUTE")),
        metric: Some((
            "ResourceCount",
            "Maximum",
            "EventBridge",
            "EventsV2/EventBus",
            "Resource",
        )),
    },
    Bound {
        service_code: "events",
        quota_code: "L-CB72B930",
        quota_name: "CreateEndpoint throttle limit in transactions per second",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of requests per second for CreateEndpoint API. Additional \
             requests are throttled.",
        ),
        period: Some((1, "SECOND")),
        metric: call_count("EventBridge", "CreateEndpoint"),
    },
    // firehose — the rate rows, which this service publishes as not adjustable, under
    // the `AWS/Usage` display name its usage is published with.
    Bound {
        service_code: "firehose",
        quota_code: "L-3A800C3A",
        quota_name: "Rate of DeleteDeliveryStream requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of DeleteDeliveryStream requests you can make per second in \
             this account in the current Region.",
        ),
        period: Some((1, "SECOND")),
        metric: call_count("Kinesis Firehose", "DeleteDeliveryStream"),
    },
    Bound {
        service_code: "firehose",
        quota_code: "L-040D05FF",
        quota_name: "Rate of CreateDeliveryStream requests",
        value: 5.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of CreateDeliveryStream requests you can make per second in \
             this account in the current Region.",
        ),
        period: Some((1, "SECOND")),
        metric: call_count("Kinesis Firehose", "CreateDeliveryStream"),
    },
    Bound {
        service_code: "firehose",
        quota_code: "L-14BB0BE7",
        quota_name: "Delivery streams",
        value: 5000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of delivery streams you can create in this account in the \
             current Region.",
        ),
        period: Some((5, "MINUTE")),
        metric: Some((
            "ResourceCount",
            "Maximum",
            "Firehose",
            "DeliveryStreams",
            "Resource",
        )),
    },
    // fms — resource-count planes under codes this catalog bound to API rates.
    Bound {
        service_code: "fms",
        quota_code: "L-742188BD",
        quota_name: "Protocols per protocol list",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "fms",
        quota_code: "L-573ADF04",
        quota_name: "Resource sets per Firewall Manager admin account",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "fms",
        quota_code: "L-1608788B",
        quota_name: "Partner rule groups per AWS WAF policy",
        value: 1.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "fms",
        quota_code: "L-0745D646",
        quota_name: "Applications per application list",
        value: 50.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of applications that you can define in an application list.",
        ),
        period: None,
        metric: None,
    },
    // fsx — the per-file-system-type planes, each under its own code.
    Bound {
        service_code: "fsx",
        quota_code: "L-C431DBA3",
        quota_name: "ONTAP backups",
        value: 10_000.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "fsx",
        quota_code: "L-4E6C2FB3",
        quota_name: "OpenZFS provisioned SSD read cache storage capacity",
        value: 262_144.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "fsx",
        quota_code: "L-15D9FE87",
        quota_name: "Lustre Cache_1 storage capacity",
        value: 100_800.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "fsx",
        quota_code: "L-E2C89679",
        quota_name: "ONTAP SSD storage capacity",
        value: 1_048_576.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    // gamelift — a capacity plane in gigabytes beside counted planes.
    Bound {
        service_code: "gamelift",
        quota_code: "L-0536A98D",
        quota_name: "Compute per Anywhere fleet",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of registered compute resources allowed per Anywhere fleet.",
        ),
        period: None,
        metric: None,
    },
    Bound {
        service_code: "gamelift",
        quota_code: "L-ED58E8F8",
        quota_name: "Script capacity",
        value: 100.0,
        unit: "Gigabytes",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "gamelift",
        quota_code: "L-C6F4238C",
        quota_name: "Custom locations per region",
        value: 20.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "gamelift",
        quota_code: "L-90D24F1B",
        quota_name: "Builds per region",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    // glacier — the size planes, published in megabytes, and the vault plane; this
    // service publishes its whole catalog as not adjustable.
    Bound {
        service_code: "glacier",
        quota_code: "L-E0D19AD1",
        quota_name: "Archive size.",
        value: 4.0,
        unit: "Megabytes",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "glacier",
        quota_code: "L-153F5D45",
        quota_name: "Number of vault tags.",
        value: 50.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "glacier",
        quota_code: "L-0D9D2530",
        quota_name: "Number of random restore requests.",
        value: 35.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "glacier",
        quota_code: "L-D1C67346",
        quota_name: "Vaults per account",
        value: 1000.0,
        unit: "None",
        adjustable: false,
        description: Some("The maximum number of vaults an account can have."),
        period: None,
        metric: None,
    },
    // glue — the job plane and a label-file size in megabytes, under codes this catalog
    // bound to catalog-API rates.
    Bound {
        service_code: "glue",
        quota_code: "L-611FDDE4",
        quota_name: "Max jobs per account",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        description: Some("The maximum number of jobs in your account."),
        period: None,
        metric: None,
    },
    Bound {
        service_code: "glue",
        quota_code: "L-B78896B9",
        quota_name: "Label file size",
        value: 10.0,
        unit: "Megabytes",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "glue",
        quota_code: "L-4F4B522D",
        quota_name: "Global s3 path max length",
        value: 2000.0,
        unit: "None",
        adjustable: true,
        description: Some("Catalog: global s3 path max length"),
        period: None,
        metric: None,
    },
    // groundstation — the scheduling limits, which are windows and counts rather than
    // the per-config-type counters this catalog bound these codes to.
    Bound {
        service_code: "groundstation",
        quota_code: "L-09DEC198",
        quota_name: "Contact Lead Time Maximum",
        value: 7.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "groundstation",
        quota_code: "L-5CCF0BC2",
        quota_name: "Config limit",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "groundstation",
        quota_code: "L-DF7B6DEC",
        quota_name: "Scheduled Contacts Limit",
        value: 100.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "groundstation",
        quota_code: "L-DE376FC5",
        quota_name: "Ephemeris Validation limit",
        value: 10.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "groundstation",
        quota_code: "L-FED20749",
        quota_name: "Scheduled Minutes Limit",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    // guardduty — the detector plane, the two member-account planes (which AWS keys
    // separately) and the retention window in days.
    Bound {
        service_code: "guardduty",
        quota_code: "L-FA6D6E3D",
        quota_name: "Detectors",
        value: 1.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "guardduty",
        quota_code: "L-8B0D7135",
        quota_name: "Member accounts through AWS Organizations",
        value: 50_000.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "guardduty",
        quota_code: "L-F51A2830",
        quota_name: "Member accounts by invitation",
        value: 5000.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "guardduty",
        quota_code: "L-9ABF7A23",
        quota_name: "Filters",
        value: 100.0,
        unit: "None",
        adjustable: false,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "guardduty",
        quota_code: "L-373487E5",
        quota_name: "Finding retention period",
        value: 90.0,
        unit: "None",
        adjustable: false,
        description: Some(
            "The maximum number of days a finding is retained. After 90 days findings are \
             deleted.",
        ),
        period: None,
        metric: None,
    },
    // imagebuilder — the recipe planes, a component size in kilobytes and a workflow
    // parameter plane, under codes this catalog bound to image-API rates.
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-28A502FD",
        quota_name: "Container recipes",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: Some(
            "The maximum number of container recipes that you can create in an account in the \
             current Region.",
        ),
        period: None,
        metric: None,
    },
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-1DF98342",
        quota_name: "Image recipes",
        value: 1000.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-D5DC1FB3",
        quota_name: "Component size",
        value: 64.0,
        unit: "Kilobytes",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-E49825CC",
        quota_name: "Parameters per image workflow",
        value: 25.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
    Bound {
        service_code: "imagebuilder",
        quota_code: "L-D5665818",
        quota_name: "Launch templates modified per distribution configuration Region",
        value: 5.0,
        unit: "None",
        adjustable: true,
        description: None,
        period: None,
        metric: None,
    },
];

/// The `ServiceCode`s whose catalogs are covered, and the name `AWS/Usage` publishes
/// each one's usage metrics under.
///
/// SOURCE: the `Service` dimension of the `AWS/Usage` metrics the recorded default-quota
/// listing of each code carries. `groundstation` and `guardduty` publish no metered
/// quota, so they carry no dimension here.
const COVERED_SERVICES: &[(&str, Option<&str>)] = &[
    ("events", Some("EventBridge")),
    ("firehose", Some("Kinesis Firehose")),
    ("fms", Some("Firewall Manager")),
    ("fsx", Some("FSx")),
    ("gamelift", Some("GameLift")),
    ("glacier", Some("Glacier")),
    ("glue", Some("Glue")),
    ("groundstation", None),
    ("guardduty", None),
    ("imagebuilder", Some("EC2 Image Builder")),
];

/// Every pinned code is bound to its own quota: the row published under it reports that
/// quota's name, value, unit, adjustability, window, usage metric and description,
/// through the listing and through the single read alike.
#[tokio::test]
async fn a_quota_code_is_bound_to_the_quota_aws_publishes_under_it() {
    let engine = emulator();
    let client = client(&engine, "us-west-2");

    let mut listings: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for row in BOUND_ROWS {
        let case = format!("{}/{}", row.service_code, row.quota_code);
        let pair = json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code});
        let quota = client.ok("GetAWSDefaultServiceQuota", pair).await["Quota"].clone();

        assert_eq!(
            (
                quota["QuotaName"].clone(),
                quota["Value"].clone(),
                quota["Unit"].clone(),
                quota["Adjustable"].clone(),
            ),
            (
                json!(row.quota_name),
                json!(row.value),
                json!(row.unit),
                json!(row.adjustable),
            ),
            "case {case}: expected the quota AWS publishes under this code, got {quota}"
        );
        if let Some(description) = row.description {
            assert_eq!(
                quota["Description"],
                json!(description),
                "case {case}: expected the published description, got {quota}"
            );
        }
        match row.period {
            Some((value, unit)) => assert_eq!(
                quota["Period"],
                json!({"PeriodValue": value, "PeriodUnit": unit}),
                "case {case}: expected the window AWS publishes this quota over, got {quota}"
            ),
            None => assert!(
                quota.get("Period").is_none_or(Value::is_null),
                "case {case}: this quota is published over no window, got {quota}"
            ),
        }
        match row.metric {
            Some((metric_name, statistic, service, resource, usage_type)) => assert_eq!(
                quota["UsageMetric"],
                json!({
                    "MetricNamespace": "AWS/Usage",
                    "MetricName": metric_name,
                    "MetricStatisticRecommendation": statistic,
                    "MetricDimensions": {
                        "Service": service,
                        "Resource": resource,
                        "Type": usage_type,
                        "Class": "None",
                    },
                }),
                "case {case}: expected the usage metric AWS publishes for this quota, got \
                 {quota}"
            ),
            None => assert!(
                quota.get("UsageMetric").is_none_or(Value::is_null),
                "case {case}: this quota publishes no usage metric, got {quota}"
            ),
        }

        let listed = match listings.get(row.service_code) {
            Some(listed) => listed,
            None => {
                let listed = defaults(&client, row.service_code).await;
                listings.entry(row.service_code).or_insert(listed)
            }
        };
        assert!(
            listed.contains(&quota),
            "case {case}: the listing must carry the record the single read returned, got \
             {listed:?}"
        );

        // The applied plane is keyed on the same code, so a client that read the code out
        // of the default catalog resolves the same quota's value in force.
        let applied = client
            .ok(
                "GetServiceQuota",
                json!({"ServiceCode": row.service_code, "QuotaCode": row.quota_code}),
            )
            .await["Quota"]
            .clone();
        assert_eq!(
            (applied["QuotaName"].clone(), applied["Value"].clone()),
            (json!(row.quota_name), json!(row.value)),
            "case {case}: the applied read must resolve the same quota, got {applied}"
        );
    }
}

/// Every row of every covered service's catalog carries a published `Description` —
/// except the rows no source available here states a sentence for, which carry none
/// rather than one this catalog would word itself ([`PUBLISHED_WITHOUT_A_DESCRIPTION`])
/// — and no two rows of one service share a `QuotaName`.
#[tokio::test]
async fn every_published_row_is_described_once_under_its_own_name() {
    let engine = emulator();
    let client = client(&engine, "eu-west-1");

    for (service_code, _) in COVERED_SERVICES {
        let listed = defaults(&client, service_code).await;
        assert!(
            !listed.is_empty(),
            "case {service_code}: the service must publish its catalog, got {listed:?}"
        );

        let mut names: BTreeSet<&str> = BTreeSet::new();
        for quota in &listed {
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            let description = quota["Description"].as_str().unwrap_or_default();
            let stated = !PUBLISHED_WITHOUT_A_DESCRIPTION.contains(&(*service_code, code));
            assert_eq!(
                !description.is_empty(),
                stated,
                "case {case}: every published row carries the description AWS publishes for \
                 it, and a row no source states one for carries none, got {quota}"
            );
            let name = quota["QuotaName"].as_str().unwrap_or_default();
            assert!(
                names.insert(name),
                "case {case}: a quota name identifies one quota of a service, got a second row \
                 named {name:?} in {listed:?}"
            );
        }
    }
}

/// A per-API request-rate row is published with the sentence AWS publishes for that
/// class, naming the operation it throttles, and is metered by that operation's
/// `AWS/Usage` `CallCount` under the service's `AWS/Usage` display name.
#[tokio::test]
async fn a_request_rate_row_names_the_operation_it_throttles() {
    let engine = emulator();
    let client = client(&engine, "ap-northeast-1");

    let mut rate_rows = 0_usize;
    for (service_code, usage_service) in COVERED_SERVICES {
        for quota in defaults(&client, service_code).await {
            if quota["UsageMetric"]["MetricName"] != json!("CallCount") {
                continue;
            }
            let code = quota["QuotaCode"].as_str().unwrap_or_default();
            let case = format!("{service_code}/{code}");
            let api = quota["UsageMetric"]["MetricDimensions"]["Resource"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                !api.is_empty(),
                "case {case}: a call count names the API it meters, got {quota}"
            );
            assert_eq!(
                quota["UsageMetric"]["MetricDimensions"]["Service"],
                json!(usage_service.expect("a metered service has an AWS/Usage name")),
                "case {case}: expected the AWS/Usage display name of the service, got {quota}"
            );
            // Each service words the rate class the way its own recorded rows word it:
            // EventBridge publishes a number of requests per second FOR THAT API, and
            // every other covered service a number of requests the account may make.
            // EventBridge's own INVOCATION series is the one row of the class metered by a
            // call count that is not one API's rate, and its recorded listing describes it
            // by what an invocation IS.
            let expected = if *service_code == "events" && api == "Invocations" {
                "An invocation is an event matching a rule and being sent on to the rule's \
                 targets. After the limit is reached, the invocations are throttled; that \
                 is, they still happen but they are delayed."
                    .to_string()
            } else if *service_code == "events" {
                format!(
                    "The maximum number of requests per second for {api} API. Additional \
                     requests are throttled."
                )
            } else {
                format!(
                    "The maximum number of {api} requests you can make per second in this \
                     account in the current Region."
                )
            };
            assert_eq!(
                quota["Description"],
                json!(expected),
                "case {case}: expected the sentence AWS publishes for a request-rate quota, \
                 got {quota}"
            );
            assert_eq!(
                quota["Period"],
                json!({"PeriodValue": 1, "PeriodUnit": "SECOND"}),
                "case {case}: a request rate is published over one second, got {quota}"
            );
            rate_rows += 1;
        }
    }

    assert!(
        rate_rows > COVERED_SERVICES.len(),
        "case catalog: the rule must be asserted over the request-rate rows of these \
         catalogs, got {rate_rows} rows"
    );
}
