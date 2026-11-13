// SPDX-License-Identifier: CC0-1.0

//! How `GetRecommendations` refuses a request whose naming of an inference target is
//! settled by the request alone: no target at all, or two.
//!
//! `campaignArn` and `recommenderArn` are alternatives and neither carries `@required`
//! (`smithy_query(query="operation",operation="GetRecommendations")`), so a request that
//! supplies neither — or both — reaches the handler. Each of those two conditions has its
//! OWN fixed refusal sentence: one that names the members involved and substitutes
//! nothing from the request or from state. Both are distinct from the sentence a request
//! that named ONE target the registry cannot serve earns — all three carry the same code
//! (`InvalidInputException`) and the same status (400), so the message is the only
//! observable that separates "you named no target" and "you named two" from "the target
//! you named is not servable".
//!
//! The suites below assert that over a TABLE of requests: the recorded request (a `userId`
//! and nothing else) is one row of it.

use serde_json::{json, Value};

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    MAX_NUM_RESULTS, MAX_NUM_RESULTS_WITH_METADATA,
};

use crate::support::{Emulator, RECOMMENDATIONS};

/// The sentence this operation answers a request that names neither alternative with.
const NO_TARGET: &str = "campaignArn or recommenderArn required.";

/// The sentence it answers a request that names both alternatives with.
const BOTH_TARGETS: &str = "Cannot provide both a campaignArn and recommenderArn";

/// The sentence a request that DID name a target the registry cannot serve earns — the
/// refusal the no-target one has to stay distinct from.
fn unresolvable(arn: &str) -> String {
    format!("{arn} does not exist or is not active yet.")
}

/// A well-formed campaign ARN no registry holds.
const UNREGISTERED_CAMPAIGN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/never-created";
/// A well-formed recommender ARN no registry holds.
const UNREGISTERED_RECOMMENDER: &str =
    "arn:aws:personalize:us-east-1:000000000000:recommender/never-created";
/// A well-formed filter ARN no registry holds. A filter is bound only AFTER the target is
/// resolved, so a row carrying it still earns the no-target refusal.
const UNREGISTERED_FILTER: &str = "arn:aws:personalize:us-east-1:000000000000:filter/never-created";

/// Requests that name neither alternative, differing in everything else they carry: the
/// other members present, their values, their count and their form. Every row must earn
/// the SAME sentence, which is what "fixed text, no substitution" means.
fn requests_naming_no_target() -> Vec<(&'static str, Value)> {
    vec![
        ("nothing at all", json!({})),
        ("a userId only", json!({ "userId": "user-1" })),
        ("a different userId only", json!({ "userId": "someone-else" })),
        ("an itemId only", json!({ "itemId": "item-7" })),
        (
            "both personalization inputs",
            json!({ "userId": "user-1", "itemId": "item-7" }),
        ),
        (
            "numResults at its documented maximum",
            json!({ "userId": "user-1", "numResults": MAX_NUM_RESULTS }),
        ),
        (
            "metadataColumns, so the smaller documented maximum applies",
            json!({
                "userId": "user-1",
                "numResults": MAX_NUM_RESULTS_WITH_METADATA,
                "metadataColumns": { "ITEMS": ["GENRE"] },
            }),
        ),
        (
            "a context map",
            json!({ "userId": "user-1", "context": { "DEVICE": "mobile" } }),
        ),
        (
            "an unregistered filterArn and its values",
            json!({
                "userId": "user-1",
                "filterArn": UNREGISTERED_FILTER,
                "filterValues": { "GENRE": "\"Comedy\"" },
            }),
        ),
        (
            "a complete promotion",
            json!({
                "userId": "user-1",
                "promotions": [{
                    "name": "promo",
                    "percentPromotedItems": 50,
                    "filterArn": UNREGISTERED_FILTER,
                }],
            }),
        ),
        (
            "every member except the two ARNs that name a target",
            json!({
                "userId": "user-1",
                "itemId": "item-3",
                "numResults": 4,
                "context": { "DEVICE": "tablet", "LOCATION": "berlin" },
                "filterArn": UNREGISTERED_FILTER,
                "filterValues": { "GENRE": "\"Drama\"" },
            }),
        ),
    ]
}

/// Every string value a request body carries, however deeply nested — what the refusal
/// must not have copied if its text is fixed.
fn string_values(body: &Value) -> Vec<String> {
    match body {
        Value::String(value) => vec![value.clone()],
        Value::Array(entries) => entries.iter().flat_map(string_values).collect(),
        Value::Object(members) => members.values().flat_map(string_values).collect(),
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn a_request_naming_neither_alternative_earns_one_fixed_sentence() {
    let emulator = Emulator::new();

    for (case, body) in requests_naming_no_target() {
        let values = string_values(&body);
        let refusal = emulator.refusal(RECOMMENDATIONS, body).await;
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.errortype.as_str()
            ),
            (400, "InvalidInputException", "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException in both __type and \
             x-amzn-errortype, got {refusal:?}"
        );
        assert_eq!(
            refusal.message, NO_TARGET,
            "case {case}: expected {NO_TARGET:?}, got {:?}",
            refusal.message
        );
        // Fixed text means fixed END TO END: nothing the request sent may appear in it,
        // so the sentence a client matches on does not move with the request.
        for value in values {
            assert!(
                value.is_empty() || !refusal.message.contains(&value),
                "case {case}: the sentence must substitute nothing from the request, but it \
                 carries {value:?}: {:?}",
                refusal.message
            );
        }
    }
}

/// Requests that name BOTH alternatives, differing in the ARNs they pair and in what
/// else they carry. Whether either ARN would have resolved is beside the point: naming
/// two targets is settled by the request alone.
fn requests_naming_both_targets() -> Vec<(&'static str, Value)> {
    vec![
        (
            "both, neither registered",
            json!({
                "campaignArn": UNREGISTERED_CAMPAIGN,
                "recommenderArn": UNREGISTERED_RECOMMENDER,
                "userId": "user-1",
            }),
        ),
        (
            "both, no personalization input",
            json!({
                "campaignArn": UNREGISTERED_CAMPAIGN,
                "recommenderArn": UNREGISTERED_RECOMMENDER,
            }),
        ),
        (
            "both, the same ARN twice",
            json!({
                "campaignArn": UNREGISTERED_CAMPAIGN,
                "recommenderArn": UNREGISTERED_CAMPAIGN,
                "itemId": "item-2",
            }),
        ),
        (
            "both, with a filter and a context",
            json!({
                "campaignArn": UNREGISTERED_CAMPAIGN,
                "recommenderArn": UNREGISTERED_RECOMMENDER,
                "userId": "user-9",
                "numResults": 3,
                "context": { "DEVICE": "desktop" },
                "filterArn": UNREGISTERED_FILTER,
            }),
        ),
    ]
}

#[tokio::test]
async fn a_request_naming_both_alternatives_earns_its_own_fixed_sentence() {
    let emulator = Emulator::new();

    for (case, body) in requests_naming_both_targets() {
        let values = string_values(&body);
        let refusal = emulator.refusal(RECOMMENDATIONS, body).await;
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.errortype.as_str()
            ),
            (400, "InvalidInputException", "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException in both __type and \
             x-amzn-errortype, got {refusal:?}"
        );
        assert_eq!(
            refusal.message, BOTH_TARGETS,
            "case {case}: expected {BOTH_TARGETS:?}, got {:?}",
            refusal.message
        );
        for value in values {
            assert!(
                value.is_empty() || !refusal.message.contains(&value),
                "case {case}: the sentence must substitute nothing from the request, but it \
                 carries {value:?}: {:?}",
                refusal.message
            );
        }
    }
}

#[tokio::test]
async fn the_three_refusals_of_this_operation_are_pairwise_distinct() {
    // The point of the fixed texts: one code and one status, three conditions, so a
    // client discriminating them has only the message to go on.
    let sentences = [
        NO_TARGET.to_string(),
        BOTH_TARGETS.to_string(),
        unresolvable(UNREGISTERED_CAMPAIGN),
    ];
    for (left, right) in [(0, 1), (0, 2), (1, 2)] {
        assert_ne!(
            sentences[left], sentences[right],
            "case sentence {left} against {right}: the refusals must stay distinguishable, \
             got {:?} twice",
            sentences[left]
        );
    }
}

#[tokio::test]
async fn naming_an_unservable_target_earns_the_resolution_sentence_instead() {
    // The complement: the same code and status, a DIFFERENT sentence. Either alternative
    // ARN, once supplied, moves the request past the no-target arm to the registry, whose
    // refusal names the ARN the caller sent.
    let emulator = Emulator::new();

    let rows: [(&str, Value, &str); 4] = [
        (
            "campaignArn",
            json!({ "campaignArn": UNREGISTERED_CAMPAIGN, "userId": "user-1" }),
            UNREGISTERED_CAMPAIGN,
        ),
        (
            "recommenderArn",
            json!({ "recommenderArn": UNREGISTERED_RECOMMENDER, "userId": "user-1" }),
            UNREGISTERED_RECOMMENDER,
        ),
        (
            "campaignArn and nothing else",
            json!({ "campaignArn": UNREGISTERED_CAMPAIGN }),
            UNREGISTERED_CAMPAIGN,
        ),
        (
            "recommenderArn and nothing else",
            json!({ "recommenderArn": UNREGISTERED_RECOMMENDER }),
            UNREGISTERED_RECOMMENDER,
        ),
    ];

    for (case, body, arn) in rows {
        let refusal = emulator.refusal(RECOMMENDATIONS, body).await;
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {refusal:?}"
        );
        assert_eq!(
            refusal.message,
            unresolvable(arn),
            "case {case}: expected the resolution sentence naming the ARN the request sent, \
             got {:?}",
            refusal.message
        );
        for other in [NO_TARGET, BOTH_TARGETS] {
            assert_ne!(
                refusal.message, other,
                "case {case}: a request that named exactly ONE target must earn neither of the \
                 request-only sentences, got {:?}",
                refusal.message
            );
        }
    }
}
