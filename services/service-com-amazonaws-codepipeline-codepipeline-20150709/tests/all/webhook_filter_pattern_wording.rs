// SPDX-License-Identifier: CC0-1.0

//! What `PutWebhook`'s `InvalidWebhookFilterPatternException` SAYS, over the wire the
//! service speaks (`awsJson1_1`, `POST /` with
//! `X-Amz-Target: CodePipeline_20150709.<Operation>`).
//!
//! The rule under test: a filter rule whose `jsonPath` is not a JsonPath expression is
//! refused with HTTP 400 `InvalidWebhookFilterPatternException` carrying the one sentence
//! that refusal states — its fixed text, followed by the offending expression exactly as
//! the request sent it, with nothing else of the request named: no member path, no list
//! index, no quoting of the value and no parser diagnostic.
//!
//! `webhooks.rs` already covers WHICH expressions are refused (the classification) and
//! `tag_removal_and_refusal_wording.rs` states the sentence on a single-filter request.
//! What this suite adds is the dimension the sentence's variable part generalises over:
//! WHERE in the `WebhookFilters` list the offending rule sits (`WebhookFilters` is
//! `@length(min: 0, max: 5)`, `smithy_query query="shape" shape="WebhookFilters"`), how
//! many admissible rules surround it, and which authentication type the definition
//! carries — none of which may reach the text.

use serde_json::{json, Value};

use service_com_amazonaws_codepipeline_codepipeline_20150709::state::{
    invalid_filter_pattern_message, AUTHENTICATION_GITHUB_HMAC, AUTHENTICATION_IP,
    AUTHENTICATION_UNAUTHENTICATED, AUTH_PROPERTY_ALLOWED_IP_RANGE, AUTH_PROPERTY_SECRET_TOKEN,
};

use crate::pipeline_lifecycle::Emulator;

/// The status `InvalidWebhookFilterPatternException` defaults to (`httpError` 400).
const REFUSAL_STATUS: u16 = 400;

/// The error `PutWebhook` models for a filter rule that is not in a valid format.
const FILTER_PATTERN_ERROR: &str = "InvalidWebhookFilterPatternException";

/// The upper bound `WebhookFilters` carries, so a row can fill the list.
const MAX_FILTERS: usize = 5;

/// A filter rule whose `jsonPath` IS a JsonPath expression, used to pad a list around the
/// one rule a row makes unparseable.
fn admissible_filter(index: usize) -> Value {
    json!({"jsonPath": format!("$.commits[{index}].id"), "matchEquals": "x"})
}

/// A `filters` list of `total` rules in which the rule at `position` carries `pattern` and
/// every other rule is admissible.
fn filters_with(pattern: &str, position: usize, total: usize) -> Value {
    Value::Array(
        (0..total)
            .map(|index| {
                if index == position {
                    json!({"jsonPath": pattern, "matchEquals": "refs/heads/main"})
                } else {
                    admissible_filter(index)
                }
            })
            .collect(),
    )
}

/// A `WebhookDefinition` carrying `filters`, under `authentication` and its
/// `authentication_configuration`.
fn definition(
    name: &str,
    authentication: &str,
    authentication_configuration: Value,
    filters: Value,
) -> Value {
    json!({
        "name": name,
        "targetPipeline": "demo",
        "targetAction": "Src",
        "filters": filters,
        "authentication": authentication,
        "authenticationConfiguration": authentication_configuration,
    })
}

/// Every webhook the account holds.
async fn listed_webhooks(emulator: &Emulator) -> Vec<Value> {
    let listed = emulator.ok("ListWebhooks", &json!({})).await;
    listed["webhooks"]
        .as_array()
        .unwrap_or_else(|| panic!("ListWebhooks reports a webhooks list, got {listed}"))
        .clone()
}

/// RULE: the refusal of a filter rule that is not a JsonPath expression carries the fixed
/// sentence that refusal states followed by the offending expression as the request sent
/// it, whatever position the rule holds in the list and whatever else the definition says.
#[tokio::test]
async fn the_filter_pattern_refusal_states_the_offending_expression_as_sent() {
    // The rows differ along what the sentence's variable part generalises over: the form
    // of the expression that cannot be parsed, how many rules the list holds, where in it
    // the offending rule sits (first, middle, last, and last of a full list), and the
    // authentication the definition declares.
    let rows: Vec<(&str, &str, usize, usize, &str, Value)> = vec![
        (
            "the only rule",
            "$.[[not-a-json-path",
            0,
            1,
            AUTHENTICATION_UNAUTHENTICATED,
            json!({}),
        ),
        (
            "the second of two rules",
            "not-a-json-path",
            1,
            2,
            AUTHENTICATION_UNAUTHENTICATED,
            json!({}),
        ),
        (
            "the middle of three rules",
            "$.",
            1,
            3,
            AUTHENTICATION_UNAUTHENTICATED,
            json!({}),
        ),
        (
            "the first of three rules",
            "$.commits[0",
            0,
            3,
            AUTHENTICATION_GITHUB_HMAC,
            json!({AUTH_PROPERTY_SECRET_TOKEN: "s3cr3t-github-token"}),
        ),
        (
            "the last of a full list",
            "$..",
            MAX_FILTERS - 1,
            MAX_FILTERS,
            AUTHENTICATION_IP,
            json!({AUTH_PROPERTY_ALLOWED_IP_RANGE: "10.0.0.0/8"}),
        ),
        (
            "an expression carrying a quote of its own",
            "$['action",
            2,
            4,
            AUTHENTICATION_UNAUTHENTICATED,
            json!({}),
        ),
        (
            "an expression at the modelled 150-character maximum",
            // `JsonPath` is `@length(min: 1, max: 150)`
            // (`smithy_query query="shape" shape="JsonPath"`): a value AT the bound is
            // carried into the sentence whole, not clipped.
            "ref-is-not-an-expression-ref-is-not-an-expression-ref-is-not-an-expression-ref-is-not-an-expression-ref-is-not-an-expression-ref-is-not-an-expres",
            0,
            2,
            AUTHENTICATION_UNAUTHENTICATED,
            json!({}),
        ),
    ];

    for (case, pattern, position, total, authentication, configuration) in rows {
        assert!(
            pattern.len() <= 150,
            "case {case:?}: the row's own expression must satisfy JsonPath's 1..150 length, \
             got {} characters",
            pattern.len()
        );
        let emulator = Emulator::new();
        let request = json!({"webhook": definition(
            "hook",
            authentication,
            configuration,
            filters_with(pattern, position, total),
        )});
        let (status, code, message) = emulator.refusal("PutWebhook", &request).await;
        assert_eq!(
            (status, code.as_str()),
            (REFUSAL_STATUS, FILTER_PATTERN_ERROR),
            "case {case:?}: expected a {REFUSAL_STATUS} {FILTER_PATTERN_ERROR}, got {status} \
             {code} ({message:?})"
        );
        assert_eq!(
            message,
            format!("Request contains invalid filter pattern: {pattern}"),
            "case {case:?}: expected the sentence this refusal states over the expression \
             sent, got {message:?}"
        );
        // The same text the crate's one builder produces, so the sentence has a single
        // source rather than a copy per caller.
        assert_eq!(
            message,
            invalid_filter_pattern_message(pattern),
            "case {case:?}: the refusal and the message builder disagree, got {message:?}"
        );
        // Nothing of the request's SHAPE reaches the text: no member path, no list index,
        // no quoting the caller did not send, no parser diagnostic.
        for leaked in ["filters[", "jsonPath", "JsonPath expression", &format!("[{position}]")] {
            assert!(
                !message.contains(leaked),
                "case {case:?}: the sentence names {leaked:?}, got {message:?}"
            );
        }
        let quotes = message.matches('"').count();
        assert_eq!(
            quotes,
            pattern.matches('"').count(),
            "case {case:?}: the sentence quotes the expression it carries, got {message:?}"
        );

        // A refused definition stores nothing, so the refusal is the whole effect.
        let listed = listed_webhooks(&emulator).await;
        assert!(
            listed.is_empty(),
            "case {case:?}: a refused PutWebhook stores no webhook, got {listed:?}"
        );
    }
}

/// RULE: the same sentence is the only thing that changes with the request — a list whose
/// rules are all admissible is accepted and read back through `ListWebhooks`, so the
/// refusal above is the filter rule's own defect and not the list's size or position.
#[tokio::test]
async fn an_admissible_filter_list_is_accepted_at_every_size_the_model_allows() {
    // The rows span `WebhookFilters`' modelled 0..5 length, one row per admissible size.
    for total in 0..=MAX_FILTERS {
        let emulator = Emulator::new();
        let filters = Value::Array((0..total).map(admissible_filter).collect());
        let request = json!({"webhook": definition(
            "hook",
            AUTHENTICATION_UNAUTHENTICATED,
            json!({}),
            filters.clone(),
        )});
        let put = emulator.ok("PutWebhook", &request).await;
        let stored = &put["webhook"]["definition"]["filters"];
        let expected = if total == 0 { &json!([]) } else { &filters };
        assert_eq!(
            stored, expected,
            "case {total} filters: PutWebhook reports the list it was sent, got {put}"
        );
        let listed = listed_webhooks(&emulator).await;
        assert_eq!(
            listed.len(),
            1,
            "case {total} filters: the accepted webhook is held once, got {listed:?}"
        );
        assert_eq!(
            listed[0]["definition"]["filters"], *expected,
            "case {total} filters: ListWebhooks reports the stored list, got {listed:?}"
        );
    }
}
