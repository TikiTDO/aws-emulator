// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutDeliverySource`, `GetDeliverySource` and
//! `DescribeDeliverySources`: the logical delivery sources an account holds.
//!
//! The rule under test: `PutDeliverySource` creates — or, under a name the account already
//! holds for the SAME resource, overwrites ("all the current delivery source parameters are
//! overwritten with the new parameter values that you specify") — one delivery source in
//! state from the parameters the request names, deriving its `arn` from the request's Region
//! and name and its `service` from the vendor of the `resourceArn` it represents, and reports
//! the stored record; a put that keeps the name and names a DIFFERENT resource is refused as
//! a conflict; `GetDeliverySource` then reports that same record and refuses a name the
//! account does not hold, and `DescribeDeliverySources` reports the same records — filtered to
//! the resources the request names, in source-name order, one `limit`-sized page at a time.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so what is
//! asserted here is what a client reads off the wire, and a response that is not computed
//! from the shared store cannot pass.
//!
//! A resource of a GLOBAL service is reached through its partition's one global endpoint, so
//! only that endpoint's Region can hold a delivery source for it and a put naming one from any
//! other Region is refused as invalid; a regional resource pins no Region at all.
//!
//! The table spans what the rule generalises over rather than one request: a sending resource
//! of several vendors (regional, global and region-and-account-less ARNs, and a wildcard ARN),
//! log types drawn from the `logType` member's own catalogue, a source configuration absent /
//! one entry / several entries / entries at both published key and value bounds, tag maps of
//! zero, one and several entries, names and log types at both published length bounds and one
//! past each, a put that creates followed by puts that overwrite, and concurrent puts of one
//! name.

use serde_json::{json, Value};

use service_com_amazonaws_cloudwatchlogs_logs_20140328::state::{
    delivery_destination_arn, delivery_source_arn, ComAmazonawsCloudwatchlogsLogs20140328State,
};

use crate::harness::{Logs, REGION};

/// The two operations under test.
const PUT: &str = "PutDeliverySource";
const GET: &str = "GetDeliverySource";
const DESCRIBE: &str = "DescribeDeliverySources";

/// The refusals the two operations model for the cases below.
const VALIDATION: &str = "ValidationException";
const CONFLICT: &str = "ConflictException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// One sending resource per vendor shape a `resourceArn` takes — "The ARN of the Amazon Web
/// Services resource that is generating and sending logs" — paired below with the `service`
/// its namespace implies: a regional resource, a global one (no Region segment), one with
/// neither Region nor account, a wildcard hub ARN ("For the SECURITY_FINDING_LOGS logType, use
/// a wildcard ARN for the hub resource") and a wildcard log-group ARN ("For the
/// INSIGHTS_QUERY_LOGS log type, use a wildcard log group ARN").
const WORKMAIL_ARN: &str =
    "arn:aws:workmail:eu-west-2:123456789012:organization/m-1234EXAMPLEabcd1234abcd1234abcd1234";
const CLOUDFRONT_ARN: &str = "arn:aws:cloudfront::123456789012:distribution/E1EXAMPLE1EXAMPLE";
const BUCKET_ARN: &str = "arn:aws:s3:::vended-logs-source";
const LOAD_BALANCER_ARN: &str =
    "arn:aws:elasticloadbalancing:eu-west-2:123456789012:loadbalancer/app/web/50dc6c495c0c9188";
const AGENTCORE_ARN: &str = "arn:aws:bedrock-agentcore:eu-west-2:123456789012:runtime/abcd1234";
const SECURITY_HUB_ARN: &str = "arn:aws:securityhub:eu-west-2:111122223333:hubv2/*";
const QUERY_LOG_GROUP_ARN: &str = "arn:aws:logs:eu-west-2:111122223333:log-group:*";

/// The Region CloudFront — a global service of the commercial partition — is reached through,
/// and so the only one that can hold a delivery source for a distribution.
const CLOUDFRONT_HOME_REGION: &str = "us-east-1";

/// Log types the `logType` member's own catalogue names — "For Amazon Web Services Amplify,
/// the valid values are ACCESS_LOGS and WAF_LOGS. For Application Load Balancer, the valid
/// values are ALB_ACCESS_LOGS, ALB_CONNECTION_LOGS, and ALB_HEALTH_CHECK_LOGS. For Amazon
/// Bedrock AgentCore [...] APPLICATION_LOGS and TRACES" — are the values a put may name FOR A
/// RESOURCE OF THAT SERVICE, so each case below pairs a resource with a log type its own
/// service sends. Source: the `logType` member of `PutDeliverySourceRequest`
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
///
/// A resource of a service the `logType` catalogue names NO log types for, so no log type can
/// be judged against it: Elemental MediaPackage is in that catalogue as a log sender, but this
/// emulator publishes no list for its namespace, which is what makes the rows using it about
/// the `LogType` LENGTH bounds rather than about which values one service sends.
const UNCATALOGUED_ARN: &str = "arn:aws:mediapackage:eu-west-2:123456789012:channels/vended";

/// One put, and what the service must then report for the delivery source it created.
#[derive(Debug)]
struct Case {
    /// What this row varies, for the assertion messages.
    label: &'static str,
    /// The Region the put is made in — the one its SigV4 credential scope names, and the one
    /// the source's own ARN is built from. [`REGION`] unless the resource is reached through
    /// another one.
    region: &'static str,
    /// `name` (`DeliverySourceName`, 1..60, `^[\w-]*$`).
    name: String,
    /// `resourceArn` — the resource whose logs this source represents.
    resource_arn: &'static str,
    /// `logType` (`LogType`, 1..255, `^[\w]*$`).
    log_type: String,
    /// `deliverySourceConfiguration`, zero / one / several entries.
    configuration: Vec<(String, String)>,
    /// `tags`, zero / one / several entries.
    tags: Vec<(&'static str, &'static str)>,
    /// The `service` the service must report: the vendor of `resource_arn`.
    expected_service: &'static str,
}

impl Case {
    fn of(
        label: &'static str,
        name: &str,
        resource_arn: &'static str,
        log_type: &str,
        expected_service: &'static str,
    ) -> Self {
        Self {
            label,
            region: REGION,
            name: name.to_string(),
            resource_arn,
            log_type: log_type.to_string(),
            configuration: Vec::new(),
            tags: Vec::new(),
            expected_service,
        }
    }

    /// This put, made in `region` instead of [`REGION`].
    fn in_region(mut self, region: &'static str) -> Self {
        self.region = region;
        self
    }

    fn with_configuration(mut self, configuration: &[(&str, &str)]) -> Self {
        self.configuration = configuration
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        self
    }

    fn with_tags(mut self, tags: &[(&'static str, &'static str)]) -> Self {
        self.tags = tags.to_vec();
        self
    }

    /// The request body this case puts.
    fn body(&self) -> Value {
        let mut body = json!({
            "name": self.name,
            "resourceArn": self.resource_arn,
            "logType": self.log_type,
        });
        if !self.configuration.is_empty() {
            body["deliverySourceConfiguration"] = map_value(
                &self
                    .configuration
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect::<Vec<_>>(),
            );
        }
        if !self.tags.is_empty() {
            body["tags"] = map_value(&self.tags);
        }
        body
    }

    /// The `DeliverySource` the service must report for this case, through the put and
    /// through every later read.
    fn expected(&self) -> Value {
        let mut expected = json!({
            "name": self.name,
            "arn": delivery_source_arn(self.region, &self.name),
            "resourceArns": [self.resource_arn],
            "service": self.expected_service,
            "logType": self.log_type,
        });
        if !self.configuration.is_empty() {
            expected["deliverySourceConfiguration"] = map_value(
                &self
                    .configuration
                    .iter()
                    .map(|(key, value)| (key.as_str(), value.as_str()))
                    .collect::<Vec<_>>(),
            );
        }
        if !self.tags.is_empty() {
            expected["tags"] = map_value(&self.tags);
        }
        expected
    }
}

/// A string map as the wire carries one (`tags`, `deliverySourceConfiguration`).
fn map_value(entries: &[(&str, &str)]) -> Value {
    Value::Object(
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), json!(value)))
            .collect(),
    )
}

/// The `deliverySource` a response carries.
fn reported(operation: &str, body: &Value, response: &Value) -> Value {
    response.get("deliverySource").cloned().unwrap_or_else(|| {
        panic!("{operation} {body}: the response must carry deliverySource, got {response}")
    })
}

/// The source the service reports under `name`, read through [`REGION`]'s endpoint.
async fn get(logs: &Logs, name: &str) -> Value {
    get_in_region(logs, REGION, name).await
}

/// The source `region`'s endpoint reports under `name` — a delivery source is a regional
/// resource, so the Region a read is addressed to is part of naming one.
async fn get_in_region(logs: &Logs, region: &str, name: &str) -> Value {
    let body = json!({"name": name});
    let response = logs.ok_in_region(region, GET, &body).await;
    reported(GET, &body, &response)
}

/// The corpus the reporting cases below read: one source per vendor shape, per configuration
/// shape, per tag-set size, and at both published name bounds.
fn cases() -> Vec<Case> {
    // The published bounds of `DeliverySourceConfigurationKey` / `Value` (`@length` 1..255).
    let longest_key = "k".repeat(255);
    let longest_value = "v".repeat(255);
    vec![
        Case::of(
            "a regional resource, no configuration and no tags",
            "workmail-logs",
            WORKMAIL_ARN,
            "ACCESS_CONTROL_LOGS",
            "workmail",
        ),
        // A resource of a global service, which is reached only through its partition's global
        // endpoint Region — so this row is put there rather than in `REGION`.
        Case::of(
            "a global resource (no Region segment), one tag",
            "cloudfront_access_logs",
            CLOUDFRONT_ARN,
            "ACCESS_LOGS",
            "cloudfront",
        )
        .in_region(CLOUDFRONT_HOME_REGION)
        .with_tags(&[("team", "platform")]),
        Case::of(
            "a resource ARN with neither Region nor account, several tags",
            "s3-access-logs",
            BUCKET_ARN,
            "S3_SERVER_ACCESS_LOGS",
            "s3",
        )
        .with_tags(&[("env", "prod"), ("owner", "obs"), ("tier", "gold")]),
        Case::of(
            "one configuration entry",
            "alb-access-logs",
            LOAD_BALANCER_ARN,
            "ALB_ACCESS_LOGS",
            "elasticloadbalancing",
        )
        .with_configuration(&[("samplingRate", "50")]),
        Case::of(
            "several configuration entries and tags together",
            "agentcore-traces",
            AGENTCORE_ARN,
            "TRACES",
            "bedrock-agentcore",
        )
        .with_configuration(&[("samplingRate", "100"), ("includeHeaders", "true")])
        .with_tags(&[("env", "dev")]),
        Case::of(
            "a wildcard hub resource ARN",
            "security-findings",
            SECURITY_HUB_ARN,
            "SECURITY_FINDING_LOGS",
            "securityhub",
        ),
        // Both published bounds of `DeliverySourceName` (`@length` 1..60), and the
        // configuration key and value bounds.
        Case::of(
            "the shortest name the model admits, configuration at both key and value bounds",
            "a",
            QUERY_LOG_GROUP_ARN,
            "INSIGHTS_QUERY_LOGS",
            "logs",
        )
        .with_configuration(&[(longest_key.as_str(), longest_value.as_str())]),
        // One (resource, log type) pairing is represented by ONE source, so this row — which
        // is about the name's upper bound — names a pairing no row above holds.
        Case::of(
            "the longest name the model admits",
            &"z".repeat(60),
            AGENTCORE_ARN,
            "USAGE_LOGS",
            "bedrock-agentcore",
        ),
    ]
}

/// A put stores the source it describes, reports it, and the read reports the SAME record
/// afterwards — for a sending resource of every vendor shape, every optional member present
/// and absent, and tag sets of every size.
#[tokio::test]
async fn a_put_delivery_source_is_reported_and_read_back() {
    let logs = Logs::new();
    let cases = cases();

    for case in &cases {
        let body = case.body();
        let response = logs.ok_in_region(case.region, PUT, &body).await;
        let put_reported = reported(PUT, &body, &response);
        let expected = case.expected();
        assert_eq!(
            put_reported, expected,
            "case {:?}: expected the put to report {expected}, got {put_reported}",
            case.label
        );

        let read = get_in_region(&logs, case.region, &case.name).await;
        assert_eq!(
            read, expected,
            "case {:?}: expected the read-back to report {expected}, got {read}",
            case.label
        );

        // Reporting is a READ: asking twice reports the same record.
        let again = get_in_region(&logs, case.region, &case.name).await;
        assert_eq!(
            again, read,
            "case {:?}: a second read must report the same record, got {again} then {read}",
            case.label
        );
    }

    // Every source put above is still reported as it was put: one put does not disturb
    // another name's record.
    for case in &cases {
        let read = get_in_region(&logs, case.region, &case.name).await;
        let expected = case.expected();
        assert_eq!(
            read, expected,
            "case {:?}: after every other put, expected {expected}, got {read}",
            case.label
        );
    }
}

/// Every log type the `logType` member's catalogue names FOR THE RESOURCE SENDING IT is stored
/// and reported as given, and so is one at either published length bound for a resource whose
/// service that catalogue names none for; one past the maximum is refused by the modelled
/// constraint. Source: the model's `LogType` shape (`@length` 1..255, `@pattern ^[\w]*$`) and
/// the `logType` member's own catalogue
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
#[tokio::test]
async fn every_published_log_type_is_stored_and_the_length_bounds_hold() {
    let logs = Logs::new();

    // (the sending resource, a log type its own service sends) — the regional and Region-less
    // senders of the catalogue, every log type each of them names.
    let mut accepted: Vec<(&str, String)> = Vec::new();
    for (resource_arn, log_types) in [
        (
            WORKMAIL_ARN,
            [
                "ACCESS_CONTROL_LOGS",
                "AUTHENTICATION_LOGS",
                "WORKMAIL_AVAILABILITY_PROVIDER_LOGS",
                "WORKMAIL_MAILBOX_ACCESS_LOGS",
                "WORKMAIL_PERSONAL_ACCESS_TOKEN_LOGS",
            ]
            .as_slice(),
        ),
        (BUCKET_ARN, ["S3_SERVER_ACCESS_LOGS"].as_slice()),
        (
            LOAD_BALANCER_ARN,
            [
                "ALB_ACCESS_LOGS",
                "ALB_CONNECTION_LOGS",
                "ALB_HEALTH_CHECK_LOGS",
                "NLB_ACCESS_LOGS",
            ]
            .as_slice(),
        ),
        (
            AGENTCORE_ARN,
            ["APPLICATION_LOGS", "USAGE_LOGS", "TRACES"].as_slice(),
        ),
        (SECURITY_HUB_ARN, ["SECURITY_FINDING_LOGS"].as_slice()),
        (QUERY_LOG_GROUP_ARN, ["INSIGHTS_QUERY_LOGS"].as_slice()),
        // A resource whose service the catalogue names no log types for: its log type is
        // kept as given, which is what puts the published LENGTH bounds under test here.
        (UNCATALOGUED_ARN, ["L", "ACCESS_LOGS"].as_slice()),
    ] {
        for log_type in log_types {
            accepted.push((resource_arn, (*log_type).to_string()));
        }
    }
    accepted.push((UNCATALOGUED_ARN, "L".repeat(255)));

    for (index, (resource_arn, log_type)) in accepted.iter().enumerate() {
        let name = format!("log-type-{index}");
        let body = json!({
            "name": name,
            "resourceArn": resource_arn,
            "logType": log_type,
        });
        let response = logs.ok(PUT, &body).await;
        let put_reported = reported(PUT, &body, &response);
        assert_eq!(
            put_reported.get("logType").and_then(Value::as_str),
            Some(log_type.as_str()),
            "case {log_type:?}: expected the put to report that logType, got {put_reported}"
        );

        let read = get(&logs, &name).await;
        assert_eq!(
            read.get("logType").and_then(Value::as_str),
            Some(log_type.as_str()),
            "case {log_type:?}: expected the read-back to report that logType, got {read}"
        );
    }

    // One character past the published maximum.
    let too_long = "L".repeat(256);
    let code = logs
        .err(
            PUT,
            &json!({
                "name": "log-type-too-long",
                "resourceArn": UNCATALOGUED_ARN,
                "logType": too_long,
            }),
        )
        .await;
    assert_eq!(
        code, VALIDATION,
        "case a logType one past the published maximum: expected {VALIDATION}, got {code}"
    );

    // A log type the sending resource's service does NOT send is refused, whatever its
    // length: which values are legal is the sending service's, not the member's.
    let code = logs
        .err(
            PUT,
            &json!({
                "name": "log-type-not-sent",
                "resourceArn": BUCKET_ARN,
                "logType": "ACCESS_CONTROL_LOGS",
            }),
        )
        .await;
    assert_eq!(
        code, VALIDATION,
        "case a logType the resource's service does not send: expected {VALIDATION}, got {code}"
    );
}

/// A put under a name the account holds for the SAME resource OVERWRITES that source's
/// parameters: "all the current delivery source parameters are overwritten with the new
/// parameter values that you specify." The tag set is the resource's rather than one of those
/// parameters, so a put that names none leaves it as it was and a put that names tags assigns
/// exactly those.
#[tokio::test]
async fn a_put_under_a_held_name_overwrites_the_source_parameters() {
    let logs = Logs::new();
    let name = "overwritten-source";

    let created = Case::of("created", name, WORKMAIL_ARN, "ACCESS_CONTROL_LOGS", "workmail")
        .with_configuration(&[("samplingRate", "50"), ("includeHeaders", "true")])
        .with_tags(&[("env", "prod")]);
    logs.ok(PUT, &created.body()).await;

    // A second put of the same resource with another logType and NO configuration: the
    // configuration the first put named is gone, and the tag set it assigned stays.
    let overwritten = Case::of("overwritten", name, WORKMAIL_ARN, "AUTHENTICATION_LOGS", "workmail")
        .with_tags(&[("env", "prod")]);
    let body = overwritten.body();
    let response = logs.ok(PUT, &body).await;
    let put_reported = reported(PUT, &body, &response);
    let expected = overwritten.expected();
    assert_eq!(
        put_reported, expected,
        "case overwritten: expected the put to report {expected}, got {put_reported}"
    );
    let read = get(&logs, name).await;
    assert_eq!(
        read, expected,
        "case overwritten: expected the read-back to report {expected}, got {read}"
    );

    // A third put naming tags assigns exactly those, and a configuration again.
    let retagged = Case::of("retagged", name, WORKMAIL_ARN, "AUTHENTICATION_LOGS", "workmail")
        .with_configuration(&[("samplingRate", "10")])
        .with_tags(&[("env", "staging"), ("owner", "obs")]);
    logs.ok(PUT, &retagged.body()).await;
    let read = get(&logs, name).await;
    let expected = retagged.expected();
    assert_eq!(
        read, expected,
        "case retagged: expected the read-back to report {expected}, got {read}"
    );

    // One name, one record: overwriting never adds a second source.
    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored: Vec<String> = state
        .lock()
        .delivery_sources_in(REGION)
        .map(|source| source.name.clone())
        .collect();
    assert_eq!(
        stored,
        vec![name.to_string()],
        "case overwritten: the account must hold exactly one source, got {stored:?}"
    );
}

/// A put that keeps a held name and names a DIFFERENT resource is refused as the conflict it
/// is, and the stored source is left exactly as it was — whatever else about the request
/// changes. A put naming the SAME resource is no conflict: it is the update above.
#[tokio::test]
async fn a_put_under_a_held_name_naming_another_resource_is_a_conflict() {
    let logs = Logs::new();
    let name = "conflicted-source";

    let held = Case::of("held", name, WORKMAIL_ARN, "ACCESS_CONTROL_LOGS", "workmail")
        .with_tags(&[("env", "prod")]);
    logs.ok(PUT, &held.body()).await;
    let expected = held.expected();

    // (case, the other resource the put names, the logType it names)
    let conflicting: [(&str, &str, &str); 4] = [
        ("another vendor's resource", LOAD_BALANCER_ARN, "ALB_ACCESS_LOGS"),
        ("another resource of the same vendor", "arn:aws:workmail:eu-west-2:123456789012:organization/m-0000EXAMPLEabcd1234abcd1234abcd1234", "ACCESS_CONTROL_LOGS"),
        ("another resource and another logType", BUCKET_ARN, "S3_SERVER_ACCESS_LOGS"),
        ("a wildcard ARN of the same vendor", "arn:aws:workmail:eu-west-2:123456789012:organization/*", "ACCESS_CONTROL_LOGS"),
    ];

    for (case, resource_arn, log_type) in conflicting {
        let body = json!({"name": name, "resourceArn": resource_arn, "logType": log_type});
        let code = logs.err(PUT, &body).await;
        assert_eq!(
            code, CONFLICT,
            "case {case:?}: expected {CONFLICT}, got {code}"
        );

        let read = get(&logs, name).await;
        assert_eq!(
            read, expected,
            "case {case:?}: a refused put must leave the source as it was, expected {expected}, \
             got {read}"
        );
    }

    // The same resource under the same name is accepted — the conflict is about the resource,
    // not about the name being held.
    let again = logs.ok(PUT, &held.body()).await;
    let put_reported = reported(PUT, &held.body(), &again);
    assert_eq!(
        put_reported, expected,
        "case a repeated put of the same pairing: expected {expected}, got {put_reported}"
    );
}

/// Concurrent puts of ONE name and resource all succeed and leave one source: the pairing is
/// idempotent, so no racer is refused and none of them doubles the account's state.
#[tokio::test]
async fn concurrent_puts_of_one_pairing_all_succeed_and_leave_one_source() {
    let logs = Logs::new();
    let name = "raced-source";
    let case = Case::of("raced", name, LOAD_BALANCER_ARN, "ALB_ACCESS_LOGS", "elasticloadbalancing")
        .with_configuration(&[("samplingRate", "50")]);
    let body = case.body();

    let (one, two, three, four, five, six) = tokio::join!(
        logs.call(PUT, &body),
        logs.call(PUT, &body),
        logs.call(PUT, &body),
        logs.call(PUT, &body),
        logs.call(PUT, &body),
        logs.call(PUT, &body),
    );

    let expected = case.expected();
    for (racer, (status, response)) in [one, two, three, four, five, six].iter().enumerate() {
        assert_eq!(
            *status, 200,
            "case racer {racer}: expected HTTP 200, got {status} {response}"
        );
        let put_reported = reported(PUT, &body, response);
        assert_eq!(
            put_reported, expected,
            "case racer {racer}: expected {expected}, got {put_reported}"
        );
    }

    let read = get(&logs, name).await;
    assert_eq!(
        read, expected,
        "case raced: expected the read-back to report {expected}, got {read}"
    );

    let state = logs
        .engine()
        .state::<ComAmazonawsCloudwatchlogsLogs20140328State>();
    let stored: Vec<String> = state
        .lock()
        .delivery_sources_in(REGION)
        .map(|source| source.name.clone())
        .collect();
    assert_eq!(
        stored,
        vec![name.to_string()],
        "case raced: the account must hold exactly one source, got {stored:?}"
    );
}

/// A `resourceArn` that is not an ARN at all names no resource to read the source's `service`
/// off, so it is refused, and the refusal names the member. A well-formed ARN of any vendor is
/// accepted: the operation documents no existence requirement for the resource it names.
#[tokio::test]
async fn a_resource_arn_that_is_not_an_arn_is_refused() {
    let logs = Logs::new();

    // (case, the resourceArn it names)
    let unparsable: [(&str, &str); 4] = [
        ("a bare resource name", "my-distribution"),
        ("an ARN missing its prefix", "aws:workmail:eu-west-2:123456789012:organization/m-1"),
        ("a URL", "https://example.invalid/distribution"),
        ("the empty string", ""),
    ];

    for (case, resource_arn) in unparsable {
        let (code, message) = logs
            .refusal(
                PUT,
                &json!({"name": "unparsable", "resourceArn": resource_arn, "logType": "ACCESS_LOGS"}),
            )
            .await;
        assert_eq!(
            code, VALIDATION,
            "case {case:?}: expected {VALIDATION}, got {code}"
        );
        assert!(
            message.contains("resourceArn"),
            "case {case:?}: the refusal must name the member, got {message:?}"
        );
    }

    // Nothing was stored by any of them.
    let code = logs.err(GET, &json!({"name": "unparsable"})).await;
    assert_eq!(
        code, NOT_FOUND,
        "case a refused put: expected {NOT_FOUND}, got {code}"
    );
}

/// A resource of a GLOBAL service — one that lives in its partition rather than in a Region,
/// so its ARN carries no Region segment — is reached through that partition's single global
/// endpoint, so only that endpoint's Region can hold a delivery source for it: a put naming
/// one from any other Region is refused with the modelled validation error and stores nothing.
///
/// The rows span every global sender the `logType` catalogue names (not CloudFront alone) and
/// every partition that publishes a global endpoint, each put from several Regions that are not
/// the one it is reached through, and then from the one that is — where it is stored, reported
/// and read back carrying an ARN of that Region.
#[tokio::test]
async fn a_global_resource_source_lives_only_in_its_partitions_global_region() {
    let logs = Logs::new();

    // (case, the resourceArn, the Region the resource is reached through, a logType)
    //
    // Source: these are the global senders the `logType` member's catalogue names — "For
    // CloudFront, the valid value is ACCESS_LOGS", "For Route 53 Global Resolver, the valid
    // value is GLOBAL_RESOLVER_LOGS", "For Amazon Web Services Shield Advanced, the valid
    // value is FLOW_LOGS"
    // (smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")) — the
    // documented ARN form of each carries no Region, and each partition reaches them through
    // one global endpoint: the commercial partition's in US East (N. Virginia), the China
    // partitions' in Beijing, the AWS GovCloud (US) partition's in US-Gov-West.
    let global: [(&str, &str, &str, &str); 7] = [
        (
            "a CloudFront distribution",
            CLOUDFRONT_ARN,
            "us-east-1",
            "ACCESS_LOGS",
        ),
        (
            "a Route 53 hosted zone",
            "arn:aws:route53:::hostedzone/Z1EXAMPLE1EXAMP",
            "us-east-1",
            "GLOBAL_RESOLVER_LOGS",
        ),
        (
            "a Shield Advanced protection",
            "arn:aws:shield::123456789012:protection/abcd1234-1234-abcd-1234-abcd1234abcd",
            "us-east-1",
            "FLOW_LOGS",
        ),
        (
            "a CloudFront distribution of the China partition",
            "arn:aws-cn:cloudfront::123456789012:distribution/E2EXAMPLE2EXAMPLE",
            "cn-north-1",
            "ACCESS_LOGS",
        ),
        (
            "a Route 53 hosted zone of the China partition",
            "arn:aws-cn:route53:::hostedzone/Z2EXAMPLE2EXAMP",
            "cn-north-1",
            "GLOBAL_RESOLVER_LOGS",
        ),
        (
            "a CloudFront distribution of the GovCloud partition",
            "arn:aws-us-gov:cloudfront::123456789012:distribution/E3EXAMPLE3EXAMPLE",
            "us-gov-west-1",
            "ACCESS_LOGS",
        ),
        (
            "a Shield Advanced protection of the GovCloud partition",
            "arn:aws-us-gov:shield::123456789012:protection/dcba4321-4321-dcba-4321-dcba4321dcba",
            "us-gov-west-1",
            "FLOW_LOGS",
        ),
    ];

    for (index, (case, resource_arn, home_region, log_type)) in global.iter().enumerate() {
        // Every other partition's global Region, plus two ordinary ones: none of them serves
        // this resource.
        let elsewhere: Vec<&str> = ["us-east-1", "cn-north-1", "us-gov-west-1", REGION, "us-west-2"]
            .into_iter()
            .filter(|region| region != home_region)
            .collect();

        for region in &elsewhere {
            let name = format!("global-{index}-{region}");
            let body =
                json!({"name": name, "resourceArn": resource_arn, "logType": log_type});
            let (code, message) = logs.refusal_in_region(region, PUT, &body).await;
            assert_eq!(
                code, VALIDATION,
                "case {case:?} put in {region}: expected {VALIDATION}, got {code}"
            );
            // The refusal is the service's fixed sentence for an action it does not serve for
            // this kind of log in the Region the request reached, whose only interpolated
            // value is that Region. Its wording, and what it must NOT carry, is the subject of
            // `tests/all/delivery_source_region_support.rs`.
            let expected =
                format!("{PUT} action is not supported for this LogType in region {region}");
            assert_eq!(
                message, expected,
                "case {case:?} put in {region}: expected {expected:?}, got {message:?}"
            );

            // A refused put stores nothing — not in the Region it was refused in, and not
            // anywhere else.
            let code = logs.err_in_region(region, GET, &json!({"name": name})).await;
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?} put in {region}: a refused put must store nothing, got {code}"
            );
        }

        // From the Region the resource IS reached through, the put is served and the source is
        // stored under an ARN of that Region.
        let name = format!("global-{index}-home");
        let body = json!({"name": name, "resourceArn": resource_arn, "logType": log_type});
        let response = logs.ok_in_region(home_region, PUT, &body).await;
        let put_reported = reported(PUT, &body, &response);
        let read = get_in_region(&logs, home_region, &name).await;
        assert_eq!(
            read, put_reported,
            "case {case:?}: expected the read-back to report {put_reported}, got {read}"
        );
        let expected_arn = delivery_source_arn(home_region, &name);
        assert_eq!(
            read.get("arn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: expected the stored source to carry {expected_arn}, got {read}"
        );
    }
}

/// A resource whose ARN pins NO Region is accepted from every Region: the rule is about the
/// vendor being one of the global log senders, never about the ARN merely carrying an empty
/// Region segment (an Amazon S3 bucket ARN carries none and a bucket is regional), and a
/// regional resource's Region is the caller's to name.
#[tokio::test]
async fn a_resource_that_pins_no_region_is_accepted_from_every_region() {
    let logs = Logs::new();

    // (case, the resourceArn, a logType)
    let unpinned: [(&str, &str, &str); 7] = [
        (
            "a regional resource of the request's own Region",
            WORKMAIL_ARN,
            "ACCESS_CONTROL_LOGS",
        ),
        (
            "a regional resource of another Region",
            "arn:aws:workmail:ap-southeast-2:123456789012:organization/m-9999EXAMPLEabcd1234abcd1234abcd",
            "AUTHENTICATION_LOGS",
        ),
        (
            "an S3 bucket, Region-less but not global",
            BUCKET_ARN,
            "S3_SERVER_ACCESS_LOGS",
        ),
        (
            "a wildcard log group of another Region",
            "arn:aws:logs:us-east-1:111122223333:log-group:*",
            "INSIGHTS_QUERY_LOGS",
        ),
        (
            "a global sender of a partition with no global endpoint of its own",
            "arn:aws-iso:cloudfront::123456789012:distribution/E4EXAMPLE4EXAMPLE",
            "ACCESS_LOGS",
        ),
        // A global service the `logType` catalogue does NOT name as a log sender pins no
        // Region here: Global Accelerator's own global endpoint is not the partition's usual
        // one, so no Region is required for it rather than the wrong one.
        (
            "a Global Accelerator accelerator, a global service that sends no logs",
            "arn:aws:globalaccelerator::123456789012:accelerator/1234abcd-abcd-1234-abcd-1234abcdefab",
            "ACCESS_LOGS",
        ),
        (
            "an IAM role, a global service that sends no logs",
            "arn:aws:iam::123456789012:role/service-role/vended-logs",
            "ACCESS_LOGS",
        ),
    ];

    for (index, (case, resource_arn, log_type)) in unpinned.iter().enumerate() {
        for region in [REGION, "us-east-1", "us-west-2", "cn-north-1"] {
            let name = format!("unpinned-{index}-{region}");
            let body = json!({"name": name, "resourceArn": resource_arn, "logType": log_type});
            let response = logs.ok_in_region(region, PUT, &body).await;
            let put_reported = reported(PUT, &body, &response);
            let expected_arn = delivery_source_arn(region, &name);
            assert_eq!(
                put_reported.get("arn").and_then(Value::as_str),
                Some(expected_arn.as_str()),
                "case {case:?} put in {region}: expected {expected_arn}, got {put_reported}"
            );
            let read = get_in_region(&logs, region, &name).await;
            assert_eq!(
                read, put_reported,
                "case {case:?} put in {region}: expected the read-back to report \
                 {put_reported}, got {read}"
            );
        }
    }
}

/// A `name` at either published bound is accepted and one past each is refused by the modelled
/// constraint. Source: the model's `DeliverySourceName` shape (`@length` 1..60, `@pattern
/// ^[\w-]*$`), which both operations' `name` targets.
#[tokio::test]
async fn the_published_name_bounds_are_accepted_and_one_past_each_is_refused() {
    let logs = Logs::new();

    // One (resource, log type) pairing is represented by ONE source, so each bound names its
    // own pairing: what varies across these two rows is the name's length.
    for (name, resource_arn, log_type) in [
        ("n".to_string(), BUCKET_ARN, "S3_SERVER_ACCESS_LOGS"),
        ("n".repeat(60), WORKMAIL_ARN, "ACCESS_CONTROL_LOGS"),
    ] {
        let name = name.as_str();
        let body = json!({"name": name, "resourceArn": resource_arn, "logType": log_type});
        let response = logs.ok(PUT, &body).await;
        let put_reported = reported(PUT, &body, &response);
        assert_eq!(
            put_reported.get("name").and_then(Value::as_str),
            Some(name),
            "case {name:?}: expected the put to report that name, got {put_reported}"
        );
        let read = get(&logs, name).await;
        assert_eq!(
            read, put_reported,
            "case {name:?}: expected the read-back to report {put_reported}, got {read}"
        );
    }

    for (case, name) in [
        ("one character past the maximum", "n".repeat(61)),
        ("one character short of the minimum", String::new()),
    ] {
        let put_code = logs
            .err(
                PUT,
                &json!({"name": name, "resourceArn": BUCKET_ARN, "logType": "S3_SERVER_ACCESS_LOGS"}),
            )
            .await;
        assert_eq!(
            put_code, VALIDATION,
            "case {case:?}: expected {PUT} to answer {VALIDATION}, got {put_code}"
        );
        let get_code = logs.err(GET, &json!({"name": name})).await;
        assert_eq!(
            get_code, VALIDATION,
            "case {case:?}: expected {GET} to answer {VALIDATION}, got {get_code}"
        );
    }
}

/// Reading a delivery source the account does not hold is refused with the error the operation
/// models, whatever else the account holds.
#[tokio::test]
async fn reading_a_source_the_account_does_not_hold_is_refused() {
    let logs = Logs::new();

    let code = logs.err(GET, &json!({"name": "absent"})).await;
    assert_eq!(
        code, NOT_FOUND,
        "case an empty account: expected {NOT_FOUND}, got {code}"
    );

    let held = Case::of("held", "held-source", BUCKET_ARN, "S3_SERVER_ACCESS_LOGS", "s3");
    logs.ok(PUT, &held.body()).await;

    // A name is a name: nothing but the one the account holds is reported, and a source is not
    // reached by the resource it represents either.
    for (case, name) in [
        ("another name", "other-source"),
        ("the held name in another case", "HELD-SOURCE"),
        ("the held name with a suffix", "held-source-2"),
        ("a prefix of the held name", "held"),
    ] {
        let code = logs.err(GET, &json!({"name": name})).await;
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {code}"
        );
    }

    // The held source is still reported, so the refusals above read the same store.
    let read = get(&logs, "held-source").await;
    assert_eq!(
        read,
        held.expected(),
        "case held: expected {}, got {read}",
        held.expected()
    );
}

/// The sources one Region's endpoint reports, as `DescribeDeliverySources` reports them: one
/// page per call, in the order the store enumerates that Region's sources (by source name).
async fn describe_in_region(
    logs: &Logs,
    region: &str,
    body: &Value,
) -> (Vec<Value>, Option<String>) {
    let response = logs.ok_in_region(region, DESCRIBE, body).await;
    let page = response
        .get("deliverySources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE} {body}: the response must carry deliverySources, got {response}"))
        .clone();
    let token = response
        .get("nextToken")
        .and_then(Value::as_str)
        .map(ToString::to_string);
    (page, token)
}

/// The same, read through [`REGION`]'s endpoint.
async fn describe(logs: &Logs, body: &Value) -> (Vec<Value>, Option<String>) {
    describe_in_region(logs, REGION, body).await
}

/// Every source a Region holds is reported through THAT Region's endpoint as it was put, in
/// source-name order, and `resourceArns` "filters the results to delivery sources associated
/// with ANY specified ARN". A delivery source is a regional resource, so each endpoint reports
/// its own Region's sources: the corpus below is put across two Regions and each is described
/// through the Region it was put in (region scoping itself is the subject of
/// `tests/all/delivery_region_isolation.rs`).
#[tokio::test]
async fn describe_reports_every_stored_source_and_filters_by_resource_arn() {
    let logs = Logs::new();
    let cases = cases();
    for case in &cases {
        logs.ok_in_region(case.region, PUT, &case.body()).await;
    }

    // Every Region the corpus was put across, each described through its own endpoint.
    let mut regions: Vec<&str> = cases.iter().map(|case| case.region).collect();
    regions.sort_unstable();
    regions.dedup();

    // (case, the resourceArns the request filters by)
    let filters: [(&str, Vec<&str>); 5] = [
        ("one ARN", vec![WORKMAIL_ARN]),
        ("another one ARN", vec![BUCKET_ARN]),
        ("several ARNs", vec![WORKMAIL_ARN, CLOUDFRONT_ARN, BUCKET_ARN]),
        ("an ARN no source represents", vec![AGENTCORE_ARN.trim_end_matches('4')]),
        (
            "every ARN the account's sources represent",
            vec![
                WORKMAIL_ARN,
                CLOUDFRONT_ARN,
                BUCKET_ARN,
                LOAD_BALANCER_ARN,
                AGENTCORE_ARN,
                SECURITY_HUB_ARN,
                QUERY_LOG_GROUP_ARN,
            ],
        ),
    ];

    for region in regions {
        // No filter: every source of this Region, in name order.
        let mut expected: Vec<(String, Value)> = cases
            .iter()
            .filter(|case| case.region == region)
            .map(|case| (case.name.clone(), case.expected()))
            .collect();
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        let expected_all: Vec<Value> = expected.iter().map(|(_, value)| value.clone()).collect();

        let (page, token) = describe_in_region(&logs, region, &json!({"limit": 50})).await;
        assert_eq!(
            page, expected_all,
            "case no filter in {region}: expected {expected_all:?}, got {page:?}"
        );
        assert_eq!(
            token, None,
            "case no filter in {region}: one page holding every source carries no nextToken, \
             got {token:?}"
        );

        for (case, arns) in &filters {
            let selected: Vec<Value> = cases
                .iter()
                .filter(|candidate| {
                    candidate.region == region && arns.contains(&candidate.resource_arn)
                })
                .map(|candidate| (candidate.name.clone(), candidate.expected()))
                .collect::<std::collections::BTreeMap<String, Value>>()
                .into_values()
                .collect();
            let (page, token) =
                describe_in_region(&logs, region, &json!({"resourceArns": arns, "limit": 50}))
                    .await;
            assert_eq!(
                page, selected,
                "case {case:?} in {region}: expected {selected:?}, got {page:?}"
            );
            assert_eq!(
                token, None,
                "case {case:?} in {region}: one page holding every selected source carries no \
                 nextToken, got {token:?}"
            );
        }
    }
}

/// `DescribeDeliverySources` pages the enumeration: each page holds at most `limit` sources,
/// a `nextToken` is handed back exactly while sources remain, and the pages concatenated are
/// the whole enumeration whatever the page size. A token issued for another filter is refused.
/// The enumeration one endpoint pages is its own Region's sources, so each Region the corpus
/// was put across is paged through its own endpoint.
#[tokio::test]
async fn describe_pages_the_enumeration_whatever_the_page_size() {
    let logs = Logs::new();
    let cases = cases();
    for case in &cases {
        logs.ok_in_region(case.region, PUT, &case.body()).await;
    }

    let mut regions: Vec<&str> = cases.iter().map(|case| case.region).collect();
    regions.sort_unstable();
    regions.dedup();

    for region in regions {
        let mut names: Vec<String> = cases
            .iter()
            .filter(|case| case.region == region)
            .map(|case| case.name.clone())
            .collect();
        names.sort();

        // Every page size from one source to more than the Region holds, plus the default.
        for limit in [None, Some(1), Some(2), Some(3), Some(names.len()), Some(50)] {
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut body = json!({});
                if let Some(limit) = limit {
                    body["limit"] = json!(limit);
                }
                if let Some(token) = &token {
                    body["nextToken"] = json!(token);
                }
                let (page, next) = describe_in_region(&logs, region, &body).await;
                if let Some(limit) = limit {
                    assert!(
                        page.len() <= limit,
                        "case limit {limit} in {region}: a page holds at most {limit} sources, \
                         got {}",
                        page.len()
                    );
                }
                for source in &page {
                    seen.push(
                        source
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_else(|| panic!("case limit {limit:?} in {region}: a reported source carries a name, got {source}"))
                            .to_string(),
                    );
                }
                match next {
                    Some(next) => token = Some(next),
                    None => break,
                }
            }
            assert_eq!(
                seen, names,
                "case limit {limit:?} in {region}: expected the pages to enumerate {names:?}, \
                 got {seen:?}"
            );
        }
    }

    // A token is a position within ONE filtered enumeration, so continuing it under another
    // filter is refused rather than silently answered from a different list.
    let (_, token) = describe(&logs, &json!({"limit": 1})).await;
    let token = token.expect("a first page of one source out of several carries a nextToken");
    let code = logs
        .err(
            DESCRIBE,
            &json!({"limit": 1, "nextToken": token, "resourceArns": [BUCKET_ARN]}),
        )
        .await;
    assert_eq!(
        code, VALIDATION,
        "case a token continued under another filter: expected {VALIDATION}, got {code}"
    );

    // A value that is not a token this service minted is refused too.
    let code = logs
        .err(DESCRIBE, &json!({"limit": 1, "nextToken": "not-a-token"}))
        .await;
    assert_eq!(
        code, VALIDATION,
        "case a token this service never minted: expected {VALIDATION}, got {code}"
    );
}

/// A delivery source holds ONE tag set: the one `PutDeliverySource$tags` assigns ("An optional
/// list of key-value pairs to associate with the resource"), the one the tagging operations
/// read and write under its ARN, and the one its own reporters report. The same holds for the
/// delivery destination, the other resource of this family whose model shape carries `tags`.
#[tokio::test]
async fn a_delivery_source_and_a_delivery_destination_each_carry_one_tag_set() {
    let logs = Logs::new();

    // (case, the ARN the tagging operations name, the operation that reports the resource, the
    // request body that reports it, the member the reported resource is carried under)
    let source_name = "tagged-source";
    let destination_name = "tagged-dest";
    logs.ok(
        PUT,
        &json!({
            "name": source_name,
            "resourceArn": WORKMAIL_ARN,
            "logType": "ACCESS_CONTROL_LOGS",
            "tags": {"env": "prod"},
        }),
    )
    .await;
    logs.ok(
        "PutDeliveryDestination",
        &json!({
            "name": destination_name,
            "deliveryDestinationConfiguration": {"destinationResourceArn": BUCKET_ARN},
        }),
    )
    .await;

    let source_arn = delivery_source_arn(REGION, source_name);
    let destination_arn = delivery_destination_arn(REGION, destination_name);

    let resources: [(&str, &str, &str, Value, &str); 2] = [
        (
            "a delivery source",
            source_arn.as_str(),
            GET,
            json!({"name": source_name}),
            "deliverySource",
        ),
        (
            "a delivery destination",
            destination_arn.as_str(),
            "GetDeliveryDestination",
            json!({"name": destination_name}),
            "deliveryDestination",
        ),
    ];

    for (case, arn, reader, reader_body, member) in resources {
        // Each tag set starts as the put left it, and the tagging operations read the SAME set.
        let put_tags = tags_of(&logs, reader, &reader_body, member).await;
        let listed = listed_tags(&logs, arn).await;
        assert_eq!(
            listed, put_tags,
            "case {case:?}: the listed tag set must be the one the resource reports, expected \
             {put_tags:?}, got {listed:?}"
        );

        // Tagging appends, and the resource's own reporter reports the appended set.
        logs.ok(
            "TagResource",
            &json!({"resourceArn": arn, "tags": {"owner": "obs", "tier": "gold"}}),
        )
        .await;
        let mut expected = put_tags.clone();
        expected.insert("owner".to_string(), "obs".to_string());
        expected.insert("tier".to_string(), "gold".to_string());
        let listed = listed_tags(&logs, arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listed tag set to be {expected:?}, got {listed:?}"
        );
        let reported = tags_of(&logs, reader, &reader_body, member).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: one resource holds ONE tag set, expected {expected:?}, got \
             {reported:?}"
        );

        // Untagging removes exactly the keys it names.
        logs.ok(
            "UntagResource",
            &json!({"resourceArn": arn, "tagKeys": ["owner"]}),
        )
        .await;
        expected.remove("owner");
        let listed = listed_tags(&logs, arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the listed tag set to be {expected:?} after untagging, got \
             {listed:?}"
        );
    }
}

/// The tag set `ListTagsForResource` reports for one resource, as a key-ordered map.
async fn listed_tags(logs: &Logs, resource_arn: &str) -> std::collections::BTreeMap<String, String> {
    let response = logs
        .ok("ListTagsForResource", &json!({"resourceArn": resource_arn}))
        .await;
    let tags = response.get("tags").unwrap_or_else(|| {
        panic!("case {resource_arn}: ListTagsForResource must report a tag set, got {response}")
    });
    string_map(resource_arn, tags)
}

/// The tag set a resource's own reporter reports, as a key-ordered map.
async fn tags_of(
    logs: &Logs,
    reader: &str,
    reader_body: &Value,
    member: &str,
) -> std::collections::BTreeMap<String, String> {
    let response = logs.ok(reader, reader_body).await;
    let resource = response
        .get(member)
        .unwrap_or_else(|| panic!("{reader} {reader_body}: expected {member}, got {response}"));
    match resource.get("tags") {
        Some(tags) => string_map(reader, tags),
        None => std::collections::BTreeMap::new(),
    }
}

/// A reported string map, keyed in a stable order so two of them compare.
fn string_map(case: &str, tags: &Value) -> std::collections::BTreeMap<String, String> {
    tags.as_object()
        .unwrap_or_else(|| panic!("case {case}: tags must be a map, got {tags}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: the value of tag {key} must be a string, got {value}")
                    })
                    .to_string(),
            )
        })
        .collect()
}

