// SPDX-License-Identifier: CC0-1.0

//! Masking the sensitive data a log group's data protection policy names, end to end over
//! the wire.
//!
//! RULE under test, as `PutDataProtectionPolicy` states it: "Sensitive data is detected and
//! masked when it is ingested into the log group. When you set a data protection policy, log
//! events ingested into the log group before that time are not masked. By default, when a
//! user views a log event that includes masked data, the sensitive data is replaced by
//! asterisks. A user who has the logs:Unmask permission can use a GetLogEvents or
//! FilterLogEvents operation with the unmask parameter set to true to view the unmasked log
//! events."
//!
//! So the masked span is the matched term and nothing else — the rest of the message is the
//! ingested text — the policy applies only to the group it was put on, and only to the events
//! ingested while it was in force.
//!
//! Every case drives the service the way a client does, through the shared harness, so a
//! response that is not computed from the shared store cannot pass.

use serde_json::{json, Value};

use crate::harness::{base_ms, batch, group_with_stream, messages, Logs};

/// The prefix every AWS-managed data identifier ARN carries, as
/// `PutDataProtectionPolicy$policyDocument`'s `DataIdentifier` array names one.
const DATA_IDENTIFIER_PREFIX: &str = "arn:aws:dataprotection::aws:data-identifier/";

/// A policy document over the managed data identifiers `names`, in the form
/// `PutDataProtectionPolicy$policyDocument` documents: the Audit block with its
/// `FindingsDestination`, then the Deidentify block with the empty `MaskConfig`, both listing
/// the same `DataIdentifier` array.
fn policy_document(names: &[&str]) -> String {
    let identifiers: Vec<String> = names
        .iter()
        .map(|name| format!("{DATA_IDENTIFIER_PREFIX}{name}"))
        .collect();
    json!({
        "Name": "masking-suite-policy",
        "Version": "2021-06-01",
        "Statement": [
            {
                "Sid": "audit",
                "DataIdentifier": identifiers,
                "Operation": {"Audit": {"FindingsDestination": {}}},
            },
            {
                "Sid": "mask",
                "DataIdentifier": identifiers,
                "Operation": {"Deidentify": {"MaskConfig": {}}},
            },
        ],
    })
    .to_string()
}

/// One row of the masking table: the identifiers the group's policy names, the message
/// ingested, and the message a default read must report.
struct Row {
    case: &'static str,
    identifiers: &'static [&'static str],
    ingested: &'static str,
    masked: &'static str,
}

/// The rows span what the rule generalises over: which managed identifier the policy names,
/// whether the term is at the start, the middle or the end of the message, how many terms one
/// message carries, how many identifiers one policy names, and a message that carries none of
/// the named data at all.
///
/// The identifiers are the ones this emulator detects
/// (`state::data_protection::DETECTED_IDENTIFIERS`), each published by "Types of data that
/// you can mask", which `PutDataProtectionPolicy$policyDocument` links to for its
/// `DataIdentifier` array — including rows the issue did not cite (an IP address, a credit
/// card number).
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an email address in the middle of the message",
            identifiers: &["EmailAddress"],
            ingested: "signup from ada@example.com accepted",
            masked: "signup from *************** accepted",
        },
        Row {
            case: "an email address the message ends with",
            identifiers: &["EmailAddress"],
            ingested: "reset requested for grace.hopper@navy.example.org",
            masked: "reset requested for *****************************",
        },
        Row {
            case: "two email addresses in one message",
            identifiers: &["EmailAddress"],
            ingested: "ada@example.com invited bob@example.org",
            masked: "*************** invited ***************",
        },
        Row {
            case: "an IPv4 address the message starts with",
            identifiers: &["IpAddress"],
            ingested: "198.51.100.7 requested /orders",
            masked: "************ requested /orders",
        },
        Row {
            case: "a credit card number in grouped form",
            identifiers: &["CreditCardNumber"],
            ingested: "charged 4111 1111 1111 1111 for order 7",
            masked: "charged ******************* for order 7",
        },
        Row {
            case: "a policy naming two identifiers, both found",
            identifiers: &["EmailAddress", "IpAddress"],
            ingested: "ada@example.com from 203.0.113.9",
            masked: "*************** from ***********",
        },
        Row {
            case: "a message carrying none of the named data",
            identifiers: &["EmailAddress"],
            ingested: "an entirely benign message",
            masked: "an entirely benign message",
        },
    ]
}

/// A group whose policy names an identifier serves the matched span masked and the rest of
/// the message verbatim; `unmask` recovers the ingested message; and a sibling group with no
/// policy serves the same message verbatim.
#[tokio::test]
async fn an_ingested_event_is_masked_by_the_groups_data_protection_policy() {
    for (index, row) in rows().into_iter().enumerate() {
        let logs = Logs::new();
        let case = row.case;
        let protected = format!("masking-protected-{index}");
        let control = format!("masking-control-{index}");
        let stream = "ingest";
        group_with_stream(&logs, &protected, stream, Some(1)).await;
        group_with_stream(&logs, &control, stream, Some(1)).await;
        logs.ok(
            "PutDataProtectionPolicy",
            &json!({
                "logGroupIdentifier": protected.clone(),
                "policyDocument": policy_document(row.identifiers),
            }),
        )
        .await;

        let at = base_ms();
        for group in [&protected, &control] {
            logs.ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "logEvents": batch(&[(at, row.ingested)]),
                }),
            )
            .await;
        }

        // The default read of the protected group: only the matched span is replaced.
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": protected.clone(), "logStreamName": stream}),
            )
            .await;
        let read = messages(&page);
        assert_eq!(
            read,
            vec![row.masked.to_string()],
            "case {case:?}: expected the masked message, got {read:?}"
        );

        // ...and `unmask` recovers the ingested message.
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({
                    "logGroupName": protected.clone(),
                    "logStreamName": stream,
                    "unmask": true,
                }),
            )
            .await;
        let read = messages(&page);
        assert_eq!(
            read,
            vec![row.ingested.to_string()],
            "case {case:?}: expected unmask to recover the ingested message, got {read:?}"
        );

        // `FilterLogEvents` is the other read the rule names, and answers the same two ways.
        for (unmask, expected) in [(false, row.masked), (true, row.ingested)] {
            let page = logs
                .ok(
                    "FilterLogEvents",
                    &json!({"logGroupName": protected.clone(), "unmask": unmask}),
                )
                .await;
            let read: Vec<String> = page
                .get("events")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: FilterLogEvents reports events, got {page}"))
                .iter()
                .filter_map(|event| {
                    event
                        .get("message")
                        .and_then(Value::as_str)
                        .map(ToString::to_string)
                })
                .collect();
            assert_eq!(
                read,
                vec![expected.to_string()],
                "case {case:?}: expected FilterLogEvents with unmask {unmask} to report \
                 {expected:?}, got {read:?}"
            );
        }

        // The group with NO policy is untouched: the policy "applies to only the specified
        // log group".
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": control.clone(), "logStreamName": stream}),
            )
            .await;
        let read = messages(&page);
        assert_eq!(
            read,
            vec![row.ingested.to_string()],
            "case {case:?}: expected the unprotected group to serve the message verbatim, \
             got {read:?}"
        );
    }
}

/// An ACCOUNT-level data protection policy masks the terms it names in every group it
/// applies to, and the two policies are cumulative: a group that also has one of its own
/// masks the terms of either.
#[tokio::test]
async fn an_account_level_policy_masks_the_groups_it_applies_to() {
    let logs = Logs::new();
    let stream = "ingest";
    let own = "masking-cumulative-own";
    let plain = "masking-cumulative-plain";
    group_with_stream(&logs, own, stream, Some(1)).await;
    group_with_stream(&logs, plain, stream, Some(1)).await;
    // The account-level policy names the IP address, the group-level one the email address:
    // "the two policies are cumulative. Any sensitive term specified in either the
    // account-level or the log-group-level policy is masked" (PutAccountPolicy).
    logs.ok(
        "PutAccountPolicy",
        &json!({
            "policyName": "masking-account-policy",
            "policyType": "DATA_PROTECTION_POLICY",
            "policyDocument": policy_document(&["IpAddress"]),
        }),
    )
    .await;
    logs.ok(
        "PutDataProtectionPolicy",
        &json!({
            "logGroupIdentifier": own,
            "policyDocument": policy_document(&["EmailAddress"]),
        }),
    )
    .await;

    let at = base_ms();
    let message = "ada@example.com from 203.0.113.9";
    for group in [own, plain] {
        logs.ok(
            "PutLogEvents",
            &json!({
                "logGroupName": group,
                "logStreamName": stream,
                "logEvents": batch(&[(at, message)]),
            }),
        )
        .await;
    }

    let cases: &[(&str, &str, &str)] = &[
        (
            "a group covered by the account policy alone",
            plain,
            "ada@example.com from ***********",
        ),
        (
            "a group with a policy of its own beside the account's",
            own,
            "*************** from ***********",
        ),
    ];
    for (case, group, expected) in cases {
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({"logGroupName": group, "logStreamName": stream}),
            )
            .await;
        let read = messages(&page);
        assert_eq!(
            read,
            vec![(*expected).to_string()],
            "case {case:?}: expected {expected:?}, got {read:?}"
        );
        let page = logs
            .ok(
                "GetLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "unmask": true,
                }),
            )
            .await;
        let read = messages(&page);
        assert_eq!(
            read,
            vec![message.to_string()],
            "case {case:?}: expected unmask to recover the ingested message, got {read:?}"
        );
    }
}

/// An event ingested BEFORE the policy was put is not masked, and one ingested after it was
/// deleted is not masked either: masking is decided as the event is ingested.
#[tokio::test]
async fn only_the_events_ingested_while_the_policy_was_in_force_are_masked() {
    let logs = Logs::new();
    let group = "masking-in-force";
    let stream = "ingest";
    group_with_stream(&logs, group, stream, Some(1)).await;
    let at = base_ms();
    let message = |label: &str| format!("{label} from ada@example.com");

    let put = |timestamp: i64, text: String| {
        let logs = &logs;
        async move {
            logs.ok(
                "PutLogEvents",
                &json!({
                    "logGroupName": group,
                    "logStreamName": stream,
                    "logEvents": batch(&[(timestamp, text.as_str())]),
                }),
            )
            .await;
        }
    };

    put(at, message("before")).await;
    logs.ok(
        "PutDataProtectionPolicy",
        &json!({
            "logGroupIdentifier": group,
            "policyDocument": policy_document(&["EmailAddress"]),
        }),
    )
    .await;
    put(at + 1, message("during")).await;
    logs.ok(
        "DeleteDataProtectionPolicy",
        &json!({"logGroupIdentifier": group}),
    )
    .await;
    put(at + 2, message("after")).await;

    let page = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": group, "logStreamName": stream}),
        )
        .await;
    let read = messages(&page);
    let expected = vec![
        "before from ada@example.com".to_string(),
        "during from ***************".to_string(),
        "after from ada@example.com".to_string(),
    ];
    assert_eq!(
        read, expected,
        "case \"one event before the policy, one under it and one after it was deleted\": \
         expected {expected:?}, got {read:?}"
    );
}
