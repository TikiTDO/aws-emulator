// SPDX-License-Identifier: CC0-1.0

//! What a rule's target receives when the target asks for the event to be
//! RESHAPED, end to end over the wire through the real `MultiServiceRouter` with
//! SQS mounted beside EventBridge — so the transformation is read back where a
//! consumer reads it: off the message on the queue.
//!
//! The RULE asserted here: a target carrying `InputPath` receives the JSON text
//! of the part of the event that path selects ("the value of the JSONPath that is
//! used for extracting part of the matched event when passing it to the target",
//! `Target$InputPath`), and a target carrying an `InputTransformer` receives its
//! `InputTemplate` with every `<key>` placeholder filled from
//! `InputPathsMap` — the template being delivered as it renders, so one that is
//! a JSON object carries its placeholders "in quotes or out" and one that is a
//! JSON string carries text whose own quoting survives
//! (`InputTransformer$InputTemplate`). A placeholder standing as a JSON value
//! carries the selected value's JSON text; one carrying CHARACTERS — inside the
//! template's own string — carries what the value holds with no JSON string
//! quoting at any level, so an array of strings reads `[a,b]`. A path
//! that names no place in the event selects NOTHING, which an `InputTransformer`
//! placeholder is filled with the empty string for — distinct from a place that
//! IS there and holds `null`, which renders as the literal `null` where the
//! placeholder stands as a value and as the characters `null` inside a string.
//!
//! What a target may be HANDED is a JSON document, and a substituted value is
//! never escaped for the place it lands in, so a rendering that is not one is a
//! delivery that does not happen — that half of the rule is
//! `tests/all/transformed_input_is_json.rs`, which also owns the rows whose
//! rendering comes out invalid.
//!
//! The tables span the class rather than one recorded case: the rows differ along
//! the KIND of place a path names (a member, a nested member, an array element,
//! a slice of an array, a member the event does not carry, an index past the end
//! of an array, a slice spanning nothing, a member holding `null`), along WHERE
//! the placeholder stands (a bare JSON value, inside a JSON string), and
//! along the FORM of the template (a JSON object, a JSON array, a quoted string).

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::state::InvocationOutcome;

use crate::target_dispatch::{
    drain, emulator, events, given_queue, invocations, DETAIL_TYPE, SOURCE,
};

/// The event every row below transforms. Its `detail` carries a scalar, a nested
/// object and an array, so one event spans every kind of place a path may name.
fn detail() -> Value {
    json!({
        "orderId": "A-1",
        "amount": 7,
        "items": ["first", "second", "third"],
        "lines": [{ "sku": "s1" }, { "sku": "s2" }],
        "nothing": null,
        "customer": { "name": "Ada", "quote": "she said \"go\"" },
    })
}

/// Publish the one event these rows route, and answer its `EventId`.
async fn publish(emulator: &crate::target_dispatch::Emulator) -> String {
    let (status, body) = events(
        emulator,
        "PutEvents",
        &json!({
            "Entries": [{
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": detail().to_string(),
            }],
        }),
    )
    .await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    body["Entries"][0]["EventId"]
        .as_str()
        .unwrap_or_else(|| panic!("no EventId in {body}"))
        .to_string()
}

/// Write a rule whose single target is a queue, configured by `configuration`,
/// publish one event, and answer what the queue then holds together with what
/// the handover to that target was recorded as.
async fn handed_over(case: &str, configuration: Value) -> (Vec<String>, InvocationOutcome) {
    let emulator = emulator();
    let (url, arn) = given_queue(&emulator, "transformed").await;
    let mut target = json!({ "Id": "t1", "Arn": arn });
    for (member, value) in configuration.as_object().cloned().unwrap_or_default() {
        target[member] = value;
    }
    crate::target_dispatch::given_rule(&emulator, "transform", json!([target])).await;

    let _ = publish(&emulator).await;

    let handovers = invocations(&emulator);
    assert_eq!(
        handovers.len(),
        1,
        "case {case:?}: expected one handover to the queue target, got {handovers:?}"
    );
    let bodies = drain(&emulator, &url)
        .await
        .iter()
        .map(|message| message["Body"].as_str().unwrap_or_default().to_string())
        .collect();
    let (_, _, outcome) = handovers
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the handover was just asserted to exist"));
    (bodies, outcome)
}

/// [`handed_over`] for a row whose delivery must be MADE: answers what the queue
/// holds.
async fn delivered(case: &str, configuration: Value) -> Vec<String> {
    let (bodies, outcome) = handed_over(case, configuration).await;
    assert_eq!(
        outcome,
        InvocationOutcome::Invoked,
        "case {case:?}: expected the transformed event to be handed over, got {outcome:?}"
    );
    bodies
}

/// `InputPath` hands the target the part of the event that path selects, over a
/// TABLE whose rows differ along the kind of place the path names.
#[tokio::test]
async fn an_input_path_delivers_the_part_of_the_event_it_selects() {
    // (case, the path, the JSON text the target receives)
    let rows: [(&str, &str, Value); 7] = [
        ("a top-level member", "$.source", json!(SOURCE)),
        ("a hyphenated member", "$.detail-type", json!(DETAIL_TYPE)),
        ("a nested member", "$.detail.orderId", json!("A-1")),
        ("a member two levels down", "$.detail.customer.name", json!("Ada")),
        ("a number", "$.detail.amount", json!(7)),
        ("an array element", "$.detail.items[2]", json!("third")),
        // A path naming no place selects JSON's empty value, so that is what the
        // target is handed.
        ("a member the event does not carry", "$.detail.missing", json!(null)),
    ];

    for (case, path, expected) in rows {
        let bodies = delivered(case, json!({ "InputPath": path })).await;
        assert_eq!(
            bodies.len(),
            1,
            "case {case:?}: expected exactly one delivery, got {bodies:?}"
        );
        let actual: Value = serde_json::from_str(&bodies[0]).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the delivered text is not JSON ({error}): {:?}",
                bodies[0]
            )
        });
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {path} to deliver {expected}, got {actual}"
        );
    }
}

/// An `InputTransformer` hands the target its template rendered with what its
/// paths select, over a TABLE whose rows differ along the form of the template
/// and along where each placeholder stands in it.
#[tokio::test]
async fn an_input_transformer_delivers_its_rendered_template() {
    // (case, InputPathsMap, InputTemplate, the text the target receives)
    let rows: [(&str, Value, &str, &str); 21] = [
        (
            "a bare placeholder in a JSON object carries the value's JSON text",
            json!({ "amount": "$.detail.amount" }),
            "{\"n\": <amount>}",
            "{\"n\": 7}",
        ),
        (
            "a quoted placeholder carries the value as a string",
            json!({ "amount": "$.detail.amount" }),
            "{\"n\": \"<amount>\"}",
            "{\"n\": \"7\"}",
        ),
        (
            "a string value inside a string loses its JSON quotes",
            json!({ "order": "$.detail.orderId" }),
            "{\"o\": \"<order>\"}",
            "{\"o\": \"A-1\"}",
        ),
        (
            "an array element",
            json!({ "item": "$.detail.items[0]" }),
            "{\"i\": <item>}",
            "{\"i\": \"first\"}",
        ),
        (
            // A slice names the LIST of elements it spans, which renders as the
            // JSON array of them.
            "a slice of an array",
            json!({ "some": "$.detail.items[0:2]" }),
            "{\"i\": <some>}",
            "{\"i\": [\"first\",\"second\"]}",
        ),
        (
            "an open-ended slice",
            json!({ "rest": "$.detail.items[1:]" }),
            "{\"i\": <rest>}",
            "{\"i\": [\"second\",\"third\"]}",
        ),
        (
            // The segments after a slice name a place within each element it
            // spans, so the placeholder carries the list of them.
            "a member of each element a slice spans",
            json!({ "skus": "$.detail.lines[0:2].sku" }),
            "{\"s\": <skus>}",
            "{\"s\": [\"s1\",\"s2\"]}",
        ),
        (
            "every element of an array",
            json!({ "all": "$.detail.items[*]" }),
            "{\"i\": <all>}",
            "{\"i\": [\"first\",\"second\",\"third\"]}",
        ),
        (
            "a member of every element",
            json!({ "skus": "$.detail.lines[*].sku" }),
            "{\"s\": <skus>}",
            "{\"s\": [\"s1\",\"s2\"]}",
        ),
        (
            // A path that selects no place of the event fills the placeholder
            // with the EMPTY STRING: a consumer can tell "the event had no such
            // place" from "the place was there and held null" (the rows below).
            "a path naming no place in the event",
            json!({ "missing": "$.detail.missing" }),
            "{\"m\": <missing>}",
            "{\"m\": \"\"}",
        ),
        (
            "a path naming no place, quoted",
            json!({ "missing": "$.detail.missing" }),
            "{\"m\": \"<missing>\"}",
            "{\"m\": \"\"}",
        ),
        (
            "an array index past the end of the array",
            json!({ "beyond": "$.detail.items[7]" }),
            "{\"b\": <beyond>}",
            "{\"b\": \"\"}",
        ),
        (
            "a member of an element the array does not have",
            json!({ "beyond": "$.detail.lines[7].sku" }),
            "{\"b\": <beyond>}",
            "{\"b\": \"\"}",
        ),
        (
            // A slice that spans nothing selects nothing, like a path naming no
            // place at all.
            "a slice past the end of the array",
            json!({ "none": "$.detail.items[5:7]" }),
            "{\"i\": <none>}",
            "{\"i\": \"\"}",
        ),
        (
            // A place that IS there and holds `null` renders as the literal.
            "a member holding null",
            json!({ "nul": "$.detail.nothing" }),
            "{\"n\": <nul>}",
            "{\"n\": null}",
        ),
        (
            "a member holding null, quoted",
            json!({ "nul": "$.detail.nothing" }),
            "{\"n\": \"<nul>\"}",
            "{\"n\": \"null\"}",
        ),
        (
            // A placeholder standing INSIDE the template's own string carries
            // the value's characters, where nothing of JSON's quoting of a
            // string is part of them — however deep in the value it sits.
            "a quoted placeholder carrying an array of strings",
            json!({ "all": "$.detail.items[*]" }),
            "{\"i\": \"<all>\"}",
            "{\"i\": \"[first,second,third]\"}",
        ),
        (
            "a quoted placeholder carrying a list of objects",
            json!({ "lines": "$.detail.lines" }),
            "{\"l\": \"<lines>\"}",
            "{\"l\": \"[{sku:s1},{sku:s2}]\"}",
        ),
        (
            // A bare placeholder stands as a JSON VALUE, so what it carries is
            // that value's JSON text, quotes and all.
            "a bare placeholder carrying an array of strings",
            json!({ "all": "$.detail.items[*]" }),
            "{\"i\": <all>}",
            "{\"i\": [\"first\",\"second\",\"third\"]}",
        ),
        (
            // A bare placeholder stands as a JSON VALUE, so a value carrying
            // quotes arrives as its JSON text, escaping and all — which is the
            // value's own rendering and not an escaping of the substitution.
            "a value carrying quotes as a bare placeholder",
            json!({ "quote": "$.detail.customer.quote" }),
            "{\"q\": <quote>}",
            "{\"q\": \"she said \\\"go\\\"\"}",
        ),
        (
            // A template that is a JSON STRING is text: its own quoting is
            // delivered as written, and what the placeholder carries is the
            // selected value's characters.
            "a quoted-string template keeps its quotes",
            json!({ "order": "$.detail.orderId" }),
            "\"<order>\"",
            "\"A-1\"",
        ),
    ];

    for (case, paths, template, expected) in rows {
        let bodies = delivered(
            case,
            json!({
                "InputTransformer": {
                    "InputPathsMap": paths,
                    "InputTemplate": template,
                },
            }),
        )
        .await;
        assert_eq!(
            bodies,
            vec![expected.to_string()],
            "case {case:?}: expected {template:?} to deliver {expected:?}, got {bodies:?}"
        );
    }
}

/// A path form this emulator does not EVALUATE is a delivery that does not
/// happen: the target receives nothing and the handover says the path was not
/// understood, rather than the target being handed a payload built as if the path
/// had merely selected nothing.
///
/// The rows differ along the selector that puts the path outside the bounded
/// reader: a wildcard member, a descendant, a union, a filter, a negative index,
/// a step, and text that is no path at all — asserted for both members a target
/// reshapes with.
#[tokio::test]
async fn a_path_this_emulator_does_not_evaluate_refuses_the_delivery() {
    let rows = [
        ("a wildcard member", "$.detail.*"),
        ("a descendant", "$..orderId"),
        ("a union of indices", "$.detail.items[0,1]"),
        ("a filter", "$.detail.items[?@=='first']"),
        ("a negative index", "$.detail.items[-1]"),
        ("a slice with a step", "$.detail.items[0:2:1]"),
        ("text that is not rooted at the event", "detail.orderId"),
    ];

    for (case, path) in rows {
        for (member, configuration) in [
            ("InputPath", json!({ "InputPath": path })),
            (
                "InputTransformer",
                json!({
                    "InputTransformer": {
                        "InputPathsMap": { "p": path },
                        "InputTemplate": "{\"p\": <p>}",
                    },
                }),
            ),
        ] {
            let case = format!("{case} as {member}");
            let (bodies, outcome) = handed_over(&case, configuration).await;
            assert!(
                bodies.is_empty(),
                "case {case:?}: expected no delivery at all, got {bodies:?}"
            );
            match &outcome {
                InvocationOutcome::NotAttempted(reason) => assert!(
                    reason.contains(path),
                    "case {case:?}: expected the recorded reason to name {path}, got {reason:?}"
                ),
                other => panic!(
                    "case {case:?}: expected the handover to be recorded as not attempted, \
                     got {other:?}"
                ),
            }
        }
    }
}

/// A TEXT template renders every placeholder as the value's own characters, so a
/// sentence reads as a sentence — and a placeholder no map key and no
/// reserved name stands for is left exactly as the template wrote it, because
/// nothing says what it means.
///
/// The template is the JSON STRING such a sentence is written as, which is what
/// makes the rendering an input a target may be handed (the counterpart rows,
/// where it is not, are in `tests/all/transformed_input_is_json.rs`).
#[tokio::test]
async fn a_text_template_renders_values_as_text_and_leaves_unknown_keys_alone() {
    let rows = [
        (
            "a sentence spanning two placeholders",
            json!({ "order": "$.detail.orderId", "amount": "$.detail.amount" }),
            "\"<order> is for <amount>\"",
            "\"A-1 is for 7\"",
        ),
        (
            "the reserved placeholder naming the matched rule",
            json!({ "order": "$.detail.orderId" }),
            "\"<aws.events.rule-name>/<order>\"",
            "\"transform/A-1\"",
        ),
        (
            "an array of strings in a sentence",
            json!({ "all": "$.detail.items[*]" }),
            "\"items <all>\"",
            "\"items [first,second,third]\"",
        ),
        (
            "a list of objects in a sentence",
            json!({ "lines": "$.detail.lines" }),
            "\"lines <lines>\"",
            "\"lines [{sku:s1},{sku:s2}]\"",
        ),
        (
            "a member holding null in a sentence",
            json!({ "nul": "$.detail.nothing" }),
            "\"value <nul>\"",
            "\"value null\"",
        ),
        (
            "a path naming no place in a sentence",
            json!({ "missing": "$.detail.missing" }),
            "\"value <missing>.\"",
            "\"value .\"",
        ),
        (
            "a placeholder nothing names",
            json!({ "order": "$.detail.orderId" }),
            "\"<order> then <unknown>\"",
            "\"A-1 then <unknown>\"",
        ),
    ];

    for (case, paths, template, expected) in rows {
        let bodies = delivered(
            case,
            json!({
                "InputTransformer": {
                    "InputPathsMap": paths,
                    "InputTemplate": template,
                },
            }),
        )
        .await;
        assert_eq!(
            bodies,
            vec![expected.to_string()],
            "case {case:?}: expected {template:?} to deliver {expected:?}, got {bodies:?}"
        );
    }
}
