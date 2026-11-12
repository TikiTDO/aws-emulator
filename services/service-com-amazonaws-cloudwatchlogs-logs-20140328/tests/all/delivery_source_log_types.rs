// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which `logType` a delivery source may send.
//!
//! The rule under test: `logType` "defines the type of log that the source is sending", and
//! which values are legal is a property of the SERVICE whose resource sends them rather than
//! of the member — the `logType` member's own catalogue lists them per service — so a
//! `PutDeliverySource` naming a type that resource's service does not send is refused 400
//! `ValidationException` and nothing is stored; a type that service sends is accepted and
//! reported back. A resource of a service this emulator publishes no list for is no basis to
//! refuse a type, so its `logType` is stored as given.
//!
//! Which sentence the refusal carries turns on whether the type names a log source at all:
//! a type SOME service in the catalogue sends is a log source this sending service is not
//! allowed for, and the refusal says exactly that and nothing more; a type no service sends
//! is no log source, and the refusal says the provided type is not valid and lists the
//! options that service does support.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than the one request a report
//! happens to cite: every service this emulator publishes a log-type list for, each with
//! EVERY type that list names (so a service sending one type and a service sending several
//! are both covered), each tried with a type another service in the catalogue sends, with a
//! type no service sends, and with a type differing from a legal one only in case; the ARN
//! forms those resources take (regional, Region-less, wildcard, and a global sender reached
//! through its partition's one global endpoint); and a resource of a service the catalogue
//! does not name.
//!
//! Source for every list below: the `logType` member of `PutDeliverySourceRequest`
//! (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`), whose
//! catalogue names the log types each service sends.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// The operations under test: the put the rule governs, and the read that shows what it
/// stored.
const PUT: &str = "PutDeliverySource";
const GET: &str = "GetDeliverySource";

/// The refusals `PutDeliverySource` models for the cases below.
const VALIDATION: &str = "ValidationException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// One sending resource: the ARN a put names it by, the Region that ARN is reached through,
/// and every `logType` its service sends.
struct Sender {
    /// What this row varies, named in every assertion.
    label: &'static str,
    resource_arn: &'static str,
    /// The Region the put must be made in: a resource of a global service is reached through
    /// its partition's one global endpoint, and every other resource through the caller's own
    /// Region.
    region: &'static str,
    /// Every log type the catalogue names for this resource's service, in the order it lists
    /// them — which is the order the refusal states them in.
    log_types: &'static [&'static str],
}

/// The services this emulator publishes a log-type list for, one resource each.
fn senders() -> Vec<Sender> {
    vec![
        Sender {
            label: "an Amplify app",
            resource_arn: "arn:aws:amplify:eu-west-2:123456789012:apps/d1example",
            region: REGION,
            // "For Amazon Web Services Amplify, the valid values are ACCESS_LOGS and
            // WAF_LOGS."
            log_types: &["ACCESS_LOGS", "WAF_LOGS"],
        },
        Sender {
            label: "a WorkMail organization (a regional ARN)",
            resource_arn:
                "arn:aws:workmail:eu-west-2:123456789012:organization/m-1234EXAMPLEabcd1234abcd",
            region: REGION,
            // "For Amazon WorkMail, the valid values are ACCESS_CONTROL_LOGS,
            // AUTHENTICATION_LOGS, WORKMAIL_AVAILABILITY_PROVIDER_LOGS,
            // WORKMAIL_MAILBOX_ACCESS_LOGS, and WORKMAIL_PERSONAL_ACCESS_TOKEN_LOGS."
            log_types: &[
                "ACCESS_CONTROL_LOGS",
                "AUTHENTICATION_LOGS",
                "WORKMAIL_AVAILABILITY_PROVIDER_LOGS",
                "WORKMAIL_MAILBOX_ACCESS_LOGS",
                "WORKMAIL_PERSONAL_ACCESS_TOKEN_LOGS",
            ],
        },
        Sender {
            label: "an S3 bucket (an ARN with neither Region nor account)",
            resource_arn: "arn:aws:s3:::vended-logs-source",
            region: REGION,
            // "For Amazon S3, the valid value is S3_SERVER_ACCESS_LOGS."
            log_types: &["S3_SERVER_ACCESS_LOGS"],
        },
        Sender {
            label: "an Application Load Balancer",
            resource_arn:
                "arn:aws:elasticloadbalancing:eu-west-2:123456789012:loadbalancer/app/web/50dc6c49",
            region: REGION,
            // "For Application Load Balancer, the valid values are ALB_ACCESS_LOGS,
            // ALB_CONNECTION_LOGS, and ALB_HEALTH_CHECK_LOGS. [...] For Network Load
            // Balancer, the valid value is NLB_ACCESS_LOGS." — one service namespace, so one
            // list.
            log_types: &[
                "ALB_ACCESS_LOGS",
                "ALB_CONNECTION_LOGS",
                "ALB_HEALTH_CHECK_LOGS",
                "NLB_ACCESS_LOGS",
            ],
        },
        Sender {
            label: "a Bedrock AgentCore runtime",
            resource_arn: "arn:aws:bedrock-agentcore:eu-west-2:123456789012:runtime/abcd1234",
            region: REGION,
            // "For Amazon Bedrock AgentCore Gateway, the valid values are APPLICATION_LOGS and
            // TRACES. [...] For Amazon Bedrock AgentCore Runtime, the valid values are
            // APPLICATION_LOGS, USAGE_LOGS, and TRACES." — one service namespace covering
            // several kinds of resource, so one list, stated in the order the catalogue holds
            // those kinds in.
            log_types: &["APPLICATION_LOGS", "TRACES", "USAGE_LOGS"],
        },
        Sender {
            label: "a wildcard Security Hub hub ARN",
            resource_arn: "arn:aws:securityhub:eu-west-2:111122223333:hubv2/*",
            region: REGION,
            // "For Amazon Web Services Security Hub, the valid value is
            // SECURITY_FINDING_LOGS."
            log_types: &["SECURITY_FINDING_LOGS"],
        },
        Sender {
            label: "a wildcard log group ARN (Logs Insights query logs)",
            resource_arn: "arn:aws:logs:eu-west-2:111122223333:log-group:*",
            region: REGION,
            // "For query execution logs from CloudWatch Logs Insights, the valid value is
            // INSIGHTS_QUERY_LOGS."
            log_types: &["INSIGHTS_QUERY_LOGS"],
        },
        Sender {
            label: "an SES mail manager resource",
            resource_arn: "arn:aws:ses:eu-west-2:123456789012:mailmanager/vended",
            region: REGION,
            // "For Amazon SES mail manager, the valid values are APPLICATION_LOGS and
            // TRAFFIC_POLICY_DEBUG_LOGS."
            log_types: &["APPLICATION_LOGS", "TRAFFIC_POLICY_DEBUG_LOGS"],
        },
        Sender {
            label: "a CloudFront distribution, reached through the global endpoint",
            resource_arn: "arn:aws:cloudfront::123456789012:distribution/E1EXAMPLE1EXAMPLE",
            // CloudFront is global: the commercial partition reaches it through us-east-1, so
            // a delivery source for a distribution lives only there.
            region: "us-east-1",
            // "For CloudFront, the valid value is ACCESS_LOGS."
            log_types: &["ACCESS_LOGS"],
        },
    ]
}

/// A resource of a service the `logType` catalogue is not reflected here for, so no list
/// exists to judge a type against: Elemental MediaPackage sends logs on AWS, but this
/// emulator publishes no log types for its namespace.
const UNCATALOGUED_ARN: &str =
    "arn:aws:mediapackage:eu-west-2:123456789012:channels/vended";

/// The whole message a type that is NO log source of this catalogue is refused with: the fixed
/// sentence, and the options the sending service does support as its one variable part.
fn invalid_log_type_message(supported: &[&str]) -> String {
    format!(
        "Provided LogType is not valid. Supported options are [{}].",
        supported.join(", ")
    )
}

/// The whole message a log type ANOTHER service sends is refused with: entirely fixed, naming
/// neither the type, nor the resource, nor the service, nor the options.
const SERVICE_NOT_ALLOWED_MESSAGE: &str = "This service is not allowed for this LogSource.";

/// Log types the `logType` member's catalogue names for a sender this suite's table does NOT
/// hold a row for, so each is a log source of the delivery plane that no sender below sends.
/// Source: the `logType` member of `PutDeliverySourceRequest`
/// (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`) — "For
/// Amazon Web Services Shield Advanced, the valid value is FLOW_LOGS", "For Route 53 Global
/// Resolver, the valid value is GLOBAL_RESOLVER_LOGS", "For PCS, the valid values are
/// PCS_SCHEDULER_LOGS [...]", "For Amazon ECS, the valid value is ACTION_LOGS".
const LOG_TYPES_OF_OTHER_SENDERS: [&str; 4] = [
    "FLOW_LOGS",
    "GLOBAL_RESOLVER_LOGS",
    "PCS_SCHEDULER_LOGS",
    "ACTION_LOGS",
];

/// Values that name no log source of this catalogue at all, at and inside the published bounds
/// of `LogType` (`@length` 1..255, `@pattern ^[\w]*$`).
fn values_that_are_no_log_source(a_legal_type: &str) -> Vec<String> {
    vec![
        "NOT_A_LOG_TYPE".to_string(),
        // A log type is matched exactly, so one differing from a legal one only in case names
        // no log source either.
        a_legal_type.to_lowercase(),
        "L".to_string(),
        "L".repeat(255),
    ]
}

/// The `deliverySource` a response carries.
fn reported(operation: &str, body: &Value, response: &Value) -> Value {
    response.get("deliverySource").cloned().unwrap_or_else(|| {
        panic!("{operation} {body}: the response must carry deliverySource, got {response}")
    })
}

/// Every log type a sender's own service sends is accepted, stored and read back.
#[tokio::test]
async fn every_log_type_the_sending_service_sends_is_stored() {
    let logs = Logs::new();

    for (index, sender) in senders().iter().enumerate() {
        for (type_index, log_type) in sender.log_types.iter().enumerate() {
            let case = (sender.label, *log_type);
            let name = format!("sender-{index}-{type_index}");
            let body = json!({
                "name": name,
                "resourceArn": sender.resource_arn,
                "logType": log_type,
            });
            let response = logs.ok_in_region(sender.region, PUT, &body).await;
            let put_reported = reported(PUT, &body, &response);
            assert_eq!(
                put_reported.get("logType").and_then(Value::as_str),
                Some(*log_type),
                "case {case:?}: the put reports the logType it stored, got {put_reported}"
            );

            let read = logs
                .ok_in_region(sender.region, GET, &json!({"name": name}))
                .await;
            let read_reported = reported(GET, &body, &read);
            assert_eq!(
                read_reported, put_reported,
                "case {case:?}: the read-back reports the stored source, got {read_reported}"
            );
        }
    }
}

/// A `logType` the sending resource's service does NOT send is refused — with the sentence
/// that names the service/log-source mismatch when the type is a log source some other sender
/// sends, and with the sentence that lists the options that service supports when it is no log
/// source at all — and the refusal stores nothing, so the name is still free afterwards.
#[tokio::test]
async fn a_log_type_the_sending_service_does_not_send_is_refused() {
    let logs = Logs::new();
    let senders = senders();

    for (index, sender) in senders.iter().enumerate() {
        // The types to try: every type ANOTHER catalogued service sends and this one does not
        // (each a log source, so the service is what is refused), every log type of a sender
        // outside this table, and the values that name no log source at all (so the type is
        // what is refused).
        let mut from_other_senders: Vec<String> = senders
            .iter()
            .flat_map(|other| other.log_types.iter())
            .chain(LOG_TYPES_OF_OTHER_SENDERS.iter())
            .filter(|log_type| !sender.log_types.contains(*log_type))
            .map(|log_type| (*log_type).to_string())
            .collect();
        from_other_senders.sort_unstable();
        from_other_senders.dedup();

        let not_valid = invalid_log_type_message(sender.log_types);
        let rejected: Vec<(String, &str)> = from_other_senders
            .into_iter()
            .map(|log_type| (log_type, SERVICE_NOT_ALLOWED_MESSAGE))
            .chain(
                values_that_are_no_log_source(sender.log_types[0])
                    .into_iter()
                    .map(|log_type| (log_type, not_valid.as_str())),
            )
            .collect();

        for (type_index, (log_type, expected)) in rejected.iter().enumerate() {
            let case = (sender.label, log_type.as_str());
            let name = format!("refused-{index}-{type_index}");
            let (code, message) = logs
                .refusal_in_region(
                    sender.region,
                    PUT,
                    &json!({
                        "name": name,
                        "resourceArn": sender.resource_arn,
                        "logType": log_type,
                    }),
                )
                .await;
            assert_eq!(
                code, VALIDATION,
                "case {case:?}: expected {VALIDATION}, got {code} {message:?}"
            );
            assert_eq!(
                &message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );

            // The refusal stored nothing: the name holds no delivery source.
            let code = logs
                .err_in_region(sender.region, GET, &json!({"name": name}))
                .await;
            assert_eq!(
                code, NOT_FOUND,
                "case {case:?}: a refused put stores nothing, so the read expects {NOT_FOUND}, \
                 got {code}"
            );
        }
    }
}

/// A resource whose service this emulator publishes no log-type list for is no basis to
/// refuse a type: whatever the member's own constraints admit is stored and reported.
#[tokio::test]
async fn a_sender_with_no_published_list_keeps_the_log_type_it_was_given() {
    let logs = Logs::new();

    // Types no catalogued service sends, at the published bounds of `LogType`
    // (`@length` 1..255, `@pattern ^[\w]*$`) and in between.
    let kept = [
        ("one character", "L".to_string()),
        ("a type another service sends", "ACCESS_LOGS".to_string()),
        ("a type no service sends", "SOMETHING_ELSE".to_string()),
        ("the published maximum length", "L".repeat(255)),
    ];

    for (index, (case, log_type)) in kept.iter().enumerate() {
        let name = format!("uncatalogued-{index}");
        let body = json!({
            "name": name,
            "resourceArn": UNCATALOGUED_ARN,
            "logType": log_type,
        });
        let response = logs.ok(PUT, &body).await;
        let put_reported = reported(PUT, &body, &response);
        assert_eq!(
            put_reported.get("logType").and_then(Value::as_str),
            Some(log_type.as_str()),
            "case {case:?}: expected the stored logType {log_type:?}, got {put_reported}"
        );

        let read = logs.ok(GET, &json!({"name": name})).await;
        assert_eq!(
            reported(GET, &body, &read),
            put_reported,
            "case {case:?}: the read-back reports the stored source, got {read}"
        );
    }
}
