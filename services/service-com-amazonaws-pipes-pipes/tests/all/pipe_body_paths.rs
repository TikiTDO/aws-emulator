// SPDX-License-Identifier: CC0-1.0

//! What a JSON PATH into a polled record selects, end to end over the wire: the paths a
//! pipe's `TargetParameters.InputTemplate` and its target parameters carry.
//!
//! Pipes and SQS are mounted on the SAME engine and every call goes through
//! `MultiServiceRouter::dispatch` — the one path a socket-served request takes — because
//! what a path selected is asserted where a client sees it: in the payload the target
//! queue received.
//!
//! The RULE asserted over the table below: a path a pipe resolves against a polled record
//! reads that record's message body in the format its producer sent it in — a body
//! spelling a JSON document is the document it parses to, so `$.body.<member>` selects
//! that member of it and the render carries the member's own value, while a body spelling
//! no document is the text it carries and a path INTO it selects nothing at all. The
//! reading is the filter's reading: one pipe cannot read a record two ways. What a stage
//! handed the event itself receives is unchanged — the record envelope with the body as
//! sent.
//!
//! The rows differ along what the rule generalises over: the format of the body (object,
//! list, plain text), how deep the path reaches into it, the type of the value it selects
//! (string, number, the whole document), where the path sits in the template (bare,
//! quoted, unquoted inside an object, beside a path that reads the envelope), and whether
//! a filter reads the same body the template renders from.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    collect_at_least, collect_during, counted, emulator, given_queue, sqs, wait_for_visible,
    QUIET,
};
use crate::pipe_lifecycle::{call, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-body-paths";

/// How long a wait for work the pipe has to do is given before it is a failure. Well
/// above the data plane's poll interval and the span a pipe takes to reach RUNNING, so a
/// row fails only when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(20);

/// The attributes that together say how many records a queue HOLDS: the ones waiting on it
/// and the ones a consumer has taken and not deleted.
const HELD_ATTRIBUTES: [&str; 2] = [
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
];

/// One message a row sends to its source queue, and what the pipe must deliver for it.
struct Sent {
    /// The `MessageBody` as the producer sends it.
    body: &'static str,
    /// The payload the target must receive for it, or `None` for a record the pipe's
    /// filter drops.
    delivered: Option<&'static str>,
}

/// One row of the path table.
struct Row {
    /// The case, which is also the pipe's and the queues' name.
    case: &'static str,
    /// `TargetParameters.InputTemplate`: what the target is handed instead of the record.
    template: &'static str,
    /// The patterns `FilterCriteria.Filters` carries, when the row filters at all.
    patterns: &'static [&'static str],
    /// What the row sends, in send order.
    messages: &'static [Sent],
}

/// RULE: a path a pipe resolves against a polled record reads the record's message body in
/// the format its producer sent it in, so a path into a body spelling a JSON document
/// selects the member it names in that document and the delivery carries that member's own
/// value; a body spelling no document is the text it carries.
#[tokio::test]
async fn a_path_into_a_record_body_selects_what_the_body_spells() {
    let rows = [
        Row {
            case: "a-scalar-member-of-a-json-body",
            template: "<$.body.id>",
            patterns: &[],
            messages: &[Sent {
                body: r#"{"type": "order", "id": "ord-7"}"#,
                delivered: Some("ord-7"),
            }],
        },
        Row {
            case: "a-number-member-of-a-json-body",
            template: "<$.body.count>",
            patterns: &[],
            messages: &[Sent {
                body: r#"{"count": 3}"#,
                delivered: Some("3"),
            }],
        },
        Row {
            case: "a-nested-member-of-a-json-body",
            template: "<$.body.order.id>",
            patterns: &[],
            messages: &[Sent {
                body: r#"{"order": {"id": "deep-1"}}"#,
                delivered: Some("deep-1"),
            }],
        },
        Row {
            case: "an-element-of-a-json-list-body",
            template: "<$.body[1].id>",
            patterns: &[],
            messages: &[Sent {
                body: r#"[{"id": "first"}, {"id": "second"}]"#,
                delivered: Some("second"),
            }],
        },
        Row {
            case: "the-whole-parsed-body",
            template: "<$.body>",
            patterns: &[],
            messages: &[Sent {
                body: r#"{"id": "whole-1"}"#,
                // The document the body spells, which is what a path into the body reads
                // — re-emitted as the JSON it is rather than as the text it travelled as.
                delivered: Some(r#"{"id": "whole-1"}"#),
            }],
        },
        Row {
            case: "a-body-path-quoted-by-the-template",
            template: "\"<$.body.id>\"",
            patterns: &[],
            messages: &[Sent {
                body: r#"{"id": "quoted-1"}"#,
                delivered: Some("quoted-1"),
            }],
        },
        Row {
            case: "body-paths-inside-an-object-template",
            template: r#"{"order": "<$.body.id>", "n": <$.body.count>, "from": "<$.eventSource>"}"#,
            patterns: &[],
            messages: &[Sent {
                body: r#"{"id": "obj-1", "count": 2}"#,
                delivered: Some(r#"{"order": "obj-1", "n": 2, "from": "aws:sqs"}"#),
            }],
        },
        Row {
            case: "a-plain-text-body-is-the-text-it-carries",
            template: "<$.body>",
            patterns: &[],
            messages: &[Sent {
                body: "a plain body, no document",
                delivered: Some("a plain body, no document"),
            }],
        },
        Row {
            // The two readings together: the filter admits on a member of the parsed body
            // and the template renders another member of that same document.
            case: "a-filter-and-a-template-over-one-body",
            template: "<$.body.id>",
            patterns: &[r#"{"body": {"type": ["match-me"]}}"#],
            messages: &[
                Sent {
                    body: r#"{"type": "no-match", "id": "dropped-1"}"#,
                    delivered: None,
                },
                Sent {
                    body: r#"{"type": "match-me", "id": "admitted-1"}"#,
                    delivered: Some("admitted-1"),
                },
            ],
        },
    ];

    for row in &rows {
        run_row(row).await;
    }
}

/// Keep the table honest: every row is driven by this one body.
async fn run_row(row: &Row) {
    let case = row.case;
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{case}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{case}-tgt")).await;

    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{case}"),
        Some(json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": source_parameters(row.patterns),
            "TargetParameters": { "InputTemplate": row.template },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );

    for message in row.messages {
        send(&router, &source_url, case, message.body).await;
    }

    let wanted: BTreeSet<String> = row
        .messages
        .iter()
        .filter_map(|message| message.delivered)
        .map(canonical)
        .collect();

    // Wait for everything the pipe must deliver, then keep watching: a record it must
    // DROP would arrive late, so the quiet window is what makes its absence an answer
    // rather than a race.
    let mut delivered = collect_at_least(&router, &target_url, wanted.len(), BUDGET).await;
    delivered.extend(collect_during(&router, &target_url, QUIET).await);
    let seen: BTreeSet<String> = delivered.iter().map(|body| canonical(body)).collect();
    assert_eq!(
        seen, wanted,
        "case {case:?}: expected the target to receive exactly {wanted:?}, got {delivered:?}"
    );
    assert_eq!(
        delivered.len(),
        wanted.len(),
        "case {case:?}: expected {} delivery(ies), got {delivered:?}",
        wanted.len()
    );

    // Every record the pipe decided about is consumed: a delivered one is deleted and a
    // dropped one is too, so nothing is left waiting on the source.
    let left = wait_for_visible(&router, &source_url, 0, BUDGET).await;
    assert_eq!(
        left, 0,
        "case {case:?}: expected every polled record consumed, {left} are still visible \
         on the source"
    );
}

/// RULE: the reading is the PATHS' alone — a stage handed the event itself receives the
/// record envelope with the message body as the producer sent it, whatever that text
/// spells — and a path that names nothing in the body is a failed delivery rather than a
/// payload carrying the path: the record stays on the source, undeleted.
#[tokio::test]
async fn the_payload_keeps_the_body_as_sent_and_an_unresolvable_path_delivers_nothing() {
    // (case, the template the pipe carries or none at all, the body sent)
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no-template-json-body", None, r#"{"id": "as-sent-1"}"#),
        ("no-template-json-list-body", None, "[1, 2, 3]"),
        ("no-template-text-body", None, "as-sent-text"),
        (
            "a-path-into-a-text-body",
            Some("<$.body.id>"),
            "no document here",
        ),
        (
            "a-path-to-an-absent-member",
            Some("<$.body.missing>"),
            r#"{"id": "present"}"#,
        ),
    ];

    for (case, template, body) in rows {
        let router = emulator();
        let (source_url, source_arn) = given_queue(&router, &format!("{case}-src")).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{case}-tgt")).await;
        let mut request = json!({
            "Source": source_arn,
            "Target": target_arn,
            "RoleArn": ROLE,
            "DesiredState": "RUNNING",
            "SourceParameters": source_parameters(&[]),
        });
        if let Some(template) = template {
            request["TargetParameters"] = json!({ "InputTemplate": template });
        }
        let created = call(
            &router,
            "POST",
            &format!("/v1/pipes/{case}"),
            Some(request),
            REGION,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe answered {} {}",
            created.status, created.body
        );
        send(&router, &source_url, case, body).await;

        match template {
            // No template: the envelope itself, carrying the body as the producer sent it
            // — the reading the paths take is theirs alone.
            None => {
                let delivered = collect_at_least(&router, &target_url, 1, BUDGET).await;
                let first = delivered
                    .first()
                    .unwrap_or_else(|| panic!("case {case:?}: nothing was delivered"));
                let envelope: Value = serde_json::from_str(first).unwrap_or_else(|error| {
                    panic!("case {case:?}: a delivered body is not JSON ({error}): {first:?}")
                });
                assert_eq!(
                    envelope["body"],
                    json!(body),
                    "case {case:?}: expected the delivered record to carry the body as \
                     sent, got {envelope}"
                );
            }
            // A path that names no one value of the event: nothing is delivered, and the
            // record is not deleted — it is still one the source holds.
            Some(_) => {
                let delivered = collect_during(&router, &target_url, QUIET).await;
                assert!(
                    delivered.is_empty(),
                    "case {case:?}: expected no delivery for a path that names nothing, \
                     got {delivered:?}"
                );
                let held = counted(&router, &source_url, &HELD_ATTRIBUTES).await;
                assert_eq!(
                    held, 1,
                    "case {case:?}: expected the record the pipe could not render to stay \
                     on the source, which holds {held}"
                );
            }
        }
    }
}

/// The `SourceParameters` a pipe is created with: one record per batch, and the patterns
/// `FilterCriteria.Filters` carries when the row filters at all.
fn source_parameters(patterns: &[&str]) -> Value {
    let mut parameters = json!({ "SqsQueueParameters": { "BatchSize": 1 } });
    if !patterns.is_empty() {
        let filters: Vec<Value> = patterns
            .iter()
            .map(|pattern| json!({ "Pattern": pattern }))
            .collect();
        parameters["FilterCriteria"] = json!({ "Filters": filters });
    }
    parameters
}

/// Send one body to the queue at `url`.
async fn send(router: &MultiServiceRouter, url: &str, case: &str, body: &str) {
    let (status, answer) = sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage of {body:?} answered {status} {answer}"
    );
}

/// One payload in a form two of them can be compared by: the JSON it spells, with every
/// object's members in one order, or the text it is when it spells no JSON. A render is
/// asserted on the VALUE it carries and not on the byte order a serializer chose for it.
///
/// `pub(crate)` so the suite that asserts the same reading of a Kinesis record's payload
/// (`tests/all/pipe_kinesis_stream_source.rs`) compares a render the same way rather than
/// copying it.
pub(crate) fn canonical(payload: &str) -> String {
    match serde_json::from_str::<Value>(payload) {
        Ok(value) => ordered(&value).to_string(),
        Err(_) => payload.to_string(),
    }
}

/// `value` with the members of every object it holds in name order.
fn ordered(value: &Value) -> Value {
    match value {
        Value::Object(members) => {
            let mut names: Vec<&String> = members.keys().collect();
            names.sort();
            let mut rebuilt = serde_json::Map::new();
            for name in names {
                let nested = members.get(name).unwrap_or(&Value::Null);
                rebuilt.insert(name.clone(), ordered(nested));
            }
            Value::Object(rebuilt)
        }
        Value::Array(items) => Value::Array(items.iter().map(ordered).collect()),
        other => other.clone(),
    }
}
