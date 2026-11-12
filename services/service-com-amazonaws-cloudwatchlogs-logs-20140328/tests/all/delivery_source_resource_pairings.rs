// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the pairing a delivery source represents: ONE resource sending ONE
//! log type.
//!
//! The rule under test: `DeliverySource$resourceArns` holds "the ARN of the Amazon Web
//! Services resource that sends logs and is represented by this delivery source" and
//! `DeliverySource$logType` "the type of log that the source is sending", so that pairing is
//! what a delivery addresses a sender's logs by and at most one delivery source of a Region's
//! account represents it: a `PutDeliverySource` that would represent, under another name, a
//! pairing an existing source already represents is refused 400 `ConflictException` with the
//! fixed sentence below and stores nothing, while the SAME resource sending a DIFFERENT log
//! type, another resource sending the same type, the same pairing in another Region, and a
//! put re-stating the pairing under its OWN name are all served.
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The table spans what the rule generalises over rather than the one request a report
//! happens to cite: the vendor shapes a `resourceArn` takes (regional, Region-less, wildcard),
//! senders that send two log types and senders that send three, and the pairing's two halves
//! varied one at a time.
//!
//! Source for every log type below: the `logType` member of `PutDeliverySourceRequest`
//! (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`), whose
//! catalogue names the log types each service sends.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// The operations under test, and the reads that show what each put stored.
const PUT: &str = "PutDeliverySource";
const GET: &str = "GetDeliverySource";
const DELETE: &str = "DeleteDeliverySource";
const DESCRIBE: &str = "DescribeDeliverySources";

/// The refusals the operations model for the cases below.
const CONFLICT: &str = "ConflictException";
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The whole sentence the refusal carries: entirely fixed text, naming neither the resource,
/// nor the log type, nor the source that already represents the pairing.
const RESOURCE_IN_USE_MESSAGE: &str =
    "This ResourceId has already been used in another Delivery Source in this account.";

/// A second Region, so the rule's scope — a delivery source is a regional resource — is
/// exercised rather than assumed. Distinct from [`REGION`].
const OTHER_REGION: &str = "us-east-2";

/// One sending resource and the log types its service sends, in the order the catalogue
/// states them.
struct Sender {
    /// What this row varies, named in every assertion.
    label: &'static str,
    resource_arn: &'static str,
    /// Another resource of the SAME service, so "another resource sending the same type" is a
    /// case the table holds too.
    sibling_arn: &'static str,
    /// At least two types, so the pairing's log-type half can vary.
    log_types: &'static [&'static str],
}

/// The senders the table spans: one per vendor shape a `resourceArn` takes, each sending more
/// than one log type.
fn senders() -> Vec<Sender> {
    vec![
        Sender {
            label: "a WorkMail organization (a regional ARN)",
            resource_arn:
                "arn:aws:workmail:eu-west-2:123456789012:organization/m-1234EXAMPLEabcd1234abcd",
            sibling_arn:
                "arn:aws:workmail:eu-west-2:123456789012:organization/m-5678EXAMPLEabcd1234abcd",
            // "For Amazon WorkMail, the valid values are ACCESS_CONTROL_LOGS,
            // AUTHENTICATION_LOGS, WORKMAIL_AVAILABILITY_PROVIDER_LOGS,
            // WORKMAIL_MAILBOX_ACCESS_LOGS, and WORKMAIL_PERSONAL_ACCESS_TOKEN_LOGS."
            log_types: &["ACCESS_CONTROL_LOGS", "AUTHENTICATION_LOGS"],
        },
        Sender {
            label: "a Bedrock AgentCore runtime (three log types)",
            resource_arn: "arn:aws:bedrock-agentcore:eu-west-2:123456789012:runtime/abcd1234",
            sibling_arn: "arn:aws:bedrock-agentcore:eu-west-2:123456789012:runtime/efgh5678",
            // "For Amazon Bedrock AgentCore Runtime, the valid values are APPLICATION_LOGS,
            // USAGE_LOGS, and TRACES."
            log_types: &["APPLICATION_LOGS", "TRACES", "USAGE_LOGS"],
        },
        Sender {
            label: "an Amplify app",
            resource_arn: "arn:aws:amplify:eu-west-2:123456789012:apps/d1example",
            sibling_arn: "arn:aws:amplify:eu-west-2:123456789012:apps/d2example",
            // "For Amazon Web Services Amplify, the valid values are ACCESS_LOGS and
            // WAF_LOGS."
            log_types: &["ACCESS_LOGS", "WAF_LOGS"],
        },
        Sender {
            label: "a wildcard log group ARN (one log type, a wildcard resource)",
            resource_arn: "arn:aws:logs:eu-west-2:111122223333:log-group:*",
            sibling_arn: "arn:aws:s3:::vended-logs-source",
            // "For query execution logs from CloudWatch Logs Insights, the valid value is
            // INSIGHTS_QUERY_LOGS." The sibling is an Amazon S3 bucket ("For Amazon S3, the
            // valid value is S3_SERVER_ACCESS_LOGS"), so this row's sibling case varies the
            // service as well as the resource.
            log_types: &["INSIGHTS_QUERY_LOGS"],
        },
    ]
}

/// The `logType` the sibling resource of a row sends: its own service's first type.
fn sibling_log_type(sender: &Sender) -> &'static str {
    if sender.sibling_arn.starts_with("arn:aws:s3:") {
        "S3_SERVER_ACCESS_LOGS"
    } else {
        sender.log_types[0]
    }
}

/// One put's body.
fn body(name: &str, resource_arn: &str, log_type: &str) -> Value {
    json!({"name": name, "resourceArn": resource_arn, "logType": log_type})
}

/// The `deliverySource` a response carries.
fn reported(operation: &str, request: &Value, response: &Value) -> Value {
    response.get("deliverySource").cloned().unwrap_or_else(|| {
        panic!("{operation} {request}: the response must carry deliverySource, got {response}")
    })
}

/// The source `region`'s endpoint reports under `name`.
async fn get_in_region(logs: &Logs, region: &str, name: &str) -> Value {
    let request = json!({"name": name});
    let response = logs.ok_in_region(region, GET, &request).await;
    reported(GET, &request, &response)
}

/// The names of the sources `region`'s endpoint reports, in the order it enumerates them.
async fn source_names(logs: &Logs, region: &str) -> Vec<String> {
    let page = logs
        .ok_in_region(region, DESCRIBE, &json!({"limit": 50}))
        .await;
    page.get("deliverySources")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("{DESCRIBE}: the response must carry deliverySources, got {page}"))
        .iter()
        .map(|source| {
            source
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("a reported source carries a name, got {source}"))
                .to_string()
        })
        .collect()
}

/// A second source for a pairing an existing source already represents is refused with the
/// fixed sentence and stores nothing — whichever vendor shape the resource takes — while the
/// resource's OTHER log types, another resource of the same service, and a put under the
/// holder's own name are all served.
#[tokio::test]
async fn a_pairing_already_represented_is_refused_under_another_name() {
    let logs = Logs::new();

    for (index, sender) in senders().iter().enumerate() {
        let holder = format!("holder-{index}");
        let held_type = sender.log_types[0];
        let created = logs.ok(PUT, &body(&holder, sender.resource_arn, held_type)).await;
        let held = reported(PUT, &body(&holder, sender.resource_arn, held_type), &created);

        // The same pairing under a NEW name: refused, and the new name holds nothing.
        let case = (sender.label, held_type);
        let intruder = format!("intruder-{index}");
        let (code, message) = logs
            .refusal(PUT, &body(&intruder, sender.resource_arn, held_type))
            .await;
        assert_eq!(
            code, CONFLICT,
            "case {case:?}: expected {CONFLICT}, got {code} {message:?}"
        );
        assert_eq!(
            message, RESOURCE_IN_USE_MESSAGE,
            "case {case:?}: expected {RESOURCE_IN_USE_MESSAGE:?}, got {message:?}"
        );
        let read_code = logs.err(GET, &json!({"name": intruder})).await;
        assert_eq!(
            read_code, NOT_FOUND,
            "case {case:?}: a refused put stores nothing, so the read expects {NOT_FOUND}, got \
             {read_code}"
        );

        // The holder is untouched by the refusal.
        let read = get_in_region(&logs, REGION, &holder).await;
        assert_eq!(
            read, held,
            "case {case:?}: a refused put leaves the holder as it was, expected {held}, got {read}"
        );

        // Re-stating the pairing under its OWN name is the documented update, not a conflict.
        let again = logs.ok(PUT, &body(&holder, sender.resource_arn, held_type)).await;
        let again = reported(PUT, &body(&holder, sender.resource_arn, held_type), &again);
        assert_eq!(
            again, held,
            "case {case:?}: a put re-stating the pairing under its own name reports {held}, got \
             {again}"
        );

        // Every OTHER log type the same resource sends is a different pairing, so each is
        // created under its own name.
        for (type_index, log_type) in sender.log_types.iter().enumerate().skip(1) {
            let case = (sender.label, *log_type);
            let name = format!("other-type-{index}-{type_index}");
            let request = body(&name, sender.resource_arn, log_type);
            let response = logs.ok(PUT, &request).await;
            let put_reported = reported(PUT, &request, &response);
            assert_eq!(
                put_reported.get("logType").and_then(Value::as_str),
                Some(*log_type),
                "case {case:?}: another log type of the same resource is created, got \
                 {put_reported}"
            );
            assert_eq!(
                get_in_region(&logs, REGION, &name).await,
                put_reported,
                "case {case:?}: the read-back reports the stored source"
            );
        }

        // Another resource sending the same type is a different pairing too.
        let sibling_type = sibling_log_type(sender);
        let case = (sender.label, sibling_type);
        let sibling = format!("sibling-{index}");
        let request = body(&sibling, sender.sibling_arn, sibling_type);
        let response = logs.ok(PUT, &request).await;
        let put_reported = reported(PUT, &request, &response);
        assert_eq!(
            put_reported.get("resourceArns").and_then(Value::as_array),
            Some(&vec![json!(sender.sibling_arn)]),
            "case {case:?}: another resource of the same service is created, got {put_reported}"
        );
    }

    // Exactly the sources the served puts created are held: no refusal added one.
    let mut expected: Vec<String> = Vec::new();
    for (index, sender) in senders().iter().enumerate() {
        expected.push(format!("holder-{index}"));
        for type_index in 1..sender.log_types.len() {
            expected.push(format!("other-type-{index}-{type_index}"));
        }
        expected.push(format!("sibling-{index}"));
    }
    expected.sort();
    let held = source_names(&logs, REGION).await;
    assert_eq!(
        held, expected,
        "case the whole table: expected the account to hold {expected:?}, got {held:?}"
    );
}

/// The pairing is held by a source, not by the resource: once the holder is DELETED the
/// pairing is free, and the very put that was refused is then served.
#[tokio::test]
async fn deleting_the_holder_frees_the_pairing() {
    let logs = Logs::new();

    for (index, sender) in senders().iter().enumerate() {
        let case = sender.label;
        let holder = format!("holder-{index}");
        let successor = format!("successor-{index}");
        let log_type = sender.log_types[0];

        logs.ok(PUT, &body(&holder, sender.resource_arn, log_type))
            .await;
        let code = logs
            .err(PUT, &body(&successor, sender.resource_arn, log_type))
            .await;
        assert_eq!(
            code, CONFLICT,
            "case {case:?}: while the holder stands the pairing is taken, got {code}"
        );

        logs.ok(DELETE, &json!({"name": holder})).await;

        let request = body(&successor, sender.resource_arn, log_type);
        let response = logs.ok(PUT, &request).await;
        let put_reported = reported(PUT, &request, &response);
        assert_eq!(
            put_reported.get("name").and_then(Value::as_str),
            Some(successor.as_str()),
            "case {case:?}: with the holder deleted the pairing is free, got {put_reported}"
        );
        assert_eq!(
            get_in_region(&logs, REGION, &successor).await,
            put_reported,
            "case {case:?}: the read-back reports the successor"
        );
    }
}

/// A delivery source is a REGIONAL resource, so the same pairing is representable once per
/// Region: the put another Region's endpoint refuses is served by this one, and each endpoint
/// reports only its own source.
#[tokio::test]
async fn one_pairing_is_representable_in_each_region() {
    let logs = Logs::new();

    for (index, sender) in senders().iter().enumerate() {
        let case = sender.label;
        let log_type = sender.log_types[0];
        let home = format!("home-{index}");
        let away = format!("away-{index}");

        logs.ok_in_region(REGION, PUT, &body(&home, sender.resource_arn, log_type))
            .await;

        // The same pairing, the same account, another Region: served.
        let request = body(&away, sender.resource_arn, log_type);
        let response = logs.ok_in_region(OTHER_REGION, PUT, &request).await;
        let put_reported = reported(PUT, &request, &response);
        let expected_arn = ::engine::emulator_arn(
            "logs",
            OTHER_REGION,
            &format!("delivery-source:{away}"),
        );
        assert_eq!(
            put_reported.get("arn").and_then(Value::as_str),
            Some(expected_arn.as_str()),
            "case {case:?}: a source belongs to the Region its put was made in, got \
             {put_reported}"
        );

        // And a second source for the pairing WITHIN that Region is refused there too.
        let code = logs
            .err_in_region(
                OTHER_REGION,
                PUT,
                &body(&format!("away-again-{index}"), sender.resource_arn, log_type),
            )
            .await;
        assert_eq!(
            code, CONFLICT,
            "case {case:?}: the rule holds in every Region, got {code}"
        );
    }

    // Each endpoint reports its own sources only.
    for (region, prefix) in [(REGION, "home-"), (OTHER_REGION, "away-")] {
        let held = source_names(&logs, region).await;
        assert!(
            held.len() == senders().len() && held.iter().all(|name| name.starts_with(prefix)),
            "case {region}: expected the {prefix}* sources alone, got {held:?}"
        );
    }
}
