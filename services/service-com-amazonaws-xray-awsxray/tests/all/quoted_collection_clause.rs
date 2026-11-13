// SPDX-License-Identifier: CC0-1.0

//! What a constraint clause about a COLLECTION member quotes, end to end over
//! the wire.
//!
//! The rule: a value outside a modelled `@length` on a collection member is
//! refused in AWS's validation envelope whose `Value '<…>'` clause quotes the
//! COLLECTION the request submitted — a list as `[<element>, …]` and a map as
//! `{<key>=<value>, …}` — beside the lowerCamel member path and the maximum the
//! model declares. A collection has no scalar rendering, so those two forms are
//! the whole of it: a list's elements are quoted in the order the request sent
//! them, and a map's entries in the order this service's own refusals state one,
//! which is the reverse of the order the request wrote them.
//!
//! The rows below differ along what that rule generalises over: the FORM of the
//! collection (map, list), which operation and member carries it, how deep in
//! the body the member sits, how many entries or elements it holds, how far past
//! the bound it reaches, the text of the keys and values, and the ORDER they
//! were written in — two rows carrying the SAME entries in two different
//! document orders, so the quoted order is shown to come from the request rather
//! than from a sort over the keys.
//!
//! The bodies are written as JSON TEXT rather than built as a document, because
//! the order of a JSON object's members is exactly what these cases vary and a
//! document would hold them in an order of its own.

use serde_json::Value;

use super::xray_wire::{Answer, Xray};

/// The modelled maximum of `AttributeMap`, the map member of both the created
/// rule and the update document
/// (`smithy_query(query="shape",shape="AttributeMap,SamplingRuleUpdate")`).
const MAX_ATTRIBUTES: usize = 5;

/// The modelled maximum of `TraceIdListForRetrieval`
/// (`smithy_query(query="shape",shape="TraceIdListForRetrieval")`).
const MAX_TRACE_IDS: usize = 100;

/// The refusal message of an answer, or the empty string.
fn said(answer: &Answer) -> &str {
    answer.message().unwrap_or_default()
}

/// AWS's one-error validation envelope over a `@length` MAXIMUM clause: the
/// collection as submitted, the lowerCamel member path, and the bound the model
/// declares.
///
/// Spelled here rather than read off the service, so the test pins the text
/// instead of restating whatever the service produced.
fn too_long_clause(value: &str, member_path: &str, max: usize) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member_path}' failed to satisfy \
         constraint: Member must have length less than or equal to {max}"
    )
}

/// The entries as the request's JSON object writes them, in the order given.
fn submitted_object(entries: &[(&str, &str)]) -> String {
    let written: Vec<String> = entries
        .iter()
        .map(|(key, value)| format!("\"{key}\":\"{value}\""))
        .collect();
    format!("{{{}}}", written.join(","))
}

/// The map a request carried, as the clause about it quotes it: `{<key>=<value>,
/// …}` over the entries in the reverse of the order they were written.
fn quoted_as_submitted(entries: &[(&str, &str)]) -> String {
    let rendered: Vec<String> = entries
        .iter()
        .rev()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!("{{{}}}", rendered.join(", "))
}

/// `n` attribute entries, each inside `AttributeKey` / `AttributeValue` @length
/// 1..32, so the only constraint a row breaks is the map's own
/// (`smithy_query(query="shape",shape="AttributeMap")`).
fn attributes(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|index| (format!("key_{index}"), format!("value_{index}")))
        .collect()
}

/// The same entries as borrowed pairs, for the renderings above.
fn pairs(entries: &[(String, String)]) -> Vec<(&str, &str)> {
    entries
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// A `CreateSamplingRule` body carrying `attributes` as written: every other
/// member is a value the service accepts, so the only refusal is the map's.
fn create_with(attributes: &[(&str, &str)]) -> String {
    format!(
        r#"{{"SamplingRule":{{"RuleName":"attribute-probe","ResourceARN":"*","Priority":500,"FixedRate":0.05,"ReservoirSize":1,"ServiceName":"*","ServiceType":"*","Host":"*","HTTPMethod":"*","URLPath":"*","Version":1,"Attributes":{}}}}}"#,
        submitted_object(attributes)
    )
}

/// An `UpdateSamplingRule` body carrying `attributes` as written, for a named
/// rule.
fn update_with(attributes: &[(&str, &str)]) -> String {
    format!(
        r#"{{"SamplingRuleUpdate":{{"RuleName":"attribute-probe","Attributes":{}}}}}"#,
        submitted_object(attributes)
    )
}

#[tokio::test]
async fn a_map_over_its_length_maximum_is_refused_quoting_the_map() {
    let xray = Xray::new();

    let six = attributes(6);
    let ten = attributes(10);
    let mut six_reordered = six.clone();
    six_reordered.reverse();
    // Keys and values of the other characters an attribute admits, written in an
    // order that is neither ascending nor descending over them.
    let named: Vec<(&str, &str)> = vec![
        ("zone", "eu-west-1a"),
        ("http.method", "GET"),
        ("tier", "gold"),
        ("A", "1"),
        ("http.url", "/orders"),
        ("build", "2024.11"),
    ];

    // (case, path, body, member path, the entries as written)
    type Row<'a> = (&'a str, &'a str, String, &'a str, Vec<(&'a str, &'a str)>);
    let rows: Vec<Row> = vec![
        (
            "one attribute past the maximum, on the created rule",
            "/CreateSamplingRule",
            create_with(&pairs(&six)),
            "samplingRule.attributes",
            pairs(&six),
        ),
        (
            "the same entries written in the other order",
            "/CreateSamplingRule",
            create_with(&pairs(&six_reordered)),
            "samplingRule.attributes",
            pairs(&six_reordered),
        ),
        (
            "twice the maximum, on the created rule",
            "/CreateSamplingRule",
            create_with(&pairs(&ten)),
            "samplingRule.attributes",
            pairs(&ten),
        ),
        (
            "one attribute past the maximum, on the update document",
            "/UpdateSamplingRule",
            update_with(&pairs(&six)),
            "samplingRuleUpdate.attributes",
            pairs(&six),
        ),
        (
            "keys and values of the other characters an attribute admits",
            "/CreateSamplingRule",
            create_with(&named),
            "samplingRule.attributes",
            named.clone(),
        ),
    ];

    for (case, path, body, member_path, written) in rows {
        let refused = xray.call_raw(path, &body).await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: expected 400 ValidationException, got {refused:?}"
        );
        let expected = too_long_clause(
            &quoted_as_submitted(&written),
            member_path,
            MAX_ATTRIBUTES,
        );
        assert_eq!(
            said(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            said(&refused)
        );
    }

    // The two rows carrying the same entries in two document orders quote two
    // different maps, so the order is the REQUEST's and not one over the keys.
    assert_ne!(
        quoted_as_submitted(&pairs(&six)),
        quoted_as_submitted(&pairs(&six_reordered)),
        "the two written orders must be quoted differently"
    );
}

#[tokio::test]
async fn a_map_inside_its_length_maximum_is_accepted_and_round_trips() {
    // The bound itself is accepted, so the rows above refuse the SIZE of the map
    // and not the member: a rule created with exactly the maximum number of
    // attributes is stored and read back with them.
    let xray = Xray::new();
    let five = attributes(MAX_ATTRIBUTES);
    let created = xray
        .call_raw("/CreateSamplingRule", &create_with(&pairs(&five)))
        .await;
    assert_eq!(
        created.status, 200,
        "a map at its maximum must be accepted, got {created:?}"
    );
    let stored = created
        .get("SamplingRuleRecord")
        .get("SamplingRule")
        .and_then(|rule| rule.get("Attributes"))
        .cloned()
        .unwrap_or(Value::Null);
    let expected: Value =
        serde_json::from_str(&submitted_object(&pairs(&five))).expect("the object parses");
    assert_eq!(
        stored, expected,
        "the accepted attributes must round-trip, got {stored:?}"
    );
}

#[tokio::test]
async fn a_list_over_its_length_maximum_quotes_the_list_in_the_order_it_was_sent() {
    let xray = Xray::new();

    // The other form of the same rule, on a list whose elements have a text
    // rendering: a 35-character trace id, `1-<8 hex>-<24 hex>`.
    let ascending: Vec<String> = (1..=101)
        .map(|n: u32| format!("1-{n:08x}-{n:024x}"))
        .collect();
    let descending: Vec<String> = ascending.iter().rev().cloned().collect();

    for (case, ids) in [
        ("one id past the maximum", &ascending),
        ("the same ids in the other order", &descending),
    ] {
        let now = service_com_amazonaws_xray_awsxray::state::now_epoch_seconds();
        let written: Vec<String> = ids.iter().map(|id| format!("\"{id}\"")).collect();
        let body = format!(
            r#"{{"StartTime":{},"EndTime":{},"TraceIds":[{}]}}"#,
            now - 3_600.0,
            now,
            written.join(",")
        );
        let refused = xray.call_raw("/StartTraceRetrieval", &body).await;
        let expected = too_long_clause(&format!("[{}]", ids.join(", ")), "traceIds", MAX_TRACE_IDS);
        assert_eq!(
            said(&refused),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            said(&refused)
        );
    }
}
