// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS the two refusals a request can earn before any catalog is
//! read, over a table of inputs rather than one recorded request:
//!
//! 1. A value outside the modelled `@pattern` is a boundary `ValidationException`
//!    (HTTP 400) whose clause names the member path and the expression the value has to
//!    satisfy — the pattern WITHOUT the `^`/`$` anchors the model writes to make the
//!    match total, and with no echo of the rejected value. Only that class is reworded:
//!    a violated `@range` bound keeps the framework's clause, value and all.
//! 2. `GetActionRecommendations` carries no `@required` on `campaignArn`, so a request
//!    that names no campaign reaches the handler; it is `InvalidInputException`
//!    (HTTP 400) reporting the campaign ARN the request gave — `null` for an absent
//!    member.

use serde_json::json;

use service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state::{
    MAX_NUM_RESULTS, MAX_NUM_RESULTS_WITH_METADATA,
};

use crate::support::{Emulator, ACTION_RECOMMENDATIONS, PERSONALIZE_RANKING, RECOMMENDATIONS};

/// The modelled `@pattern` on `Arn`, which `campaignArn`, `recommenderArn` and
/// `filterArn` all target:
/// `smithy_query(query="operation",operation="GetRecommendations")` reports
/// `pattern: ^arn:([a-z\d-]+):personalize:.*:.*:.+$`.
const ARN_PATTERN: &str = r"^arn:([a-z\d-]+):personalize:.*:.*:.+$";

/// That same expression without its anchors — what the clause states.
const ARN_PATTERN_UNANCHORED: &str = r"arn:([a-z\d-]+):personalize:.*:.*:.+";

/// The clause this service states for a value outside `ARN_PATTERN` at `member`.
fn pattern_clause(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: Member \
         must satisfy regular expression pattern: {ARN_PATTERN_UNANCHORED}"
    )
}

#[tokio::test]
async fn a_value_outside_the_arn_pattern_names_the_member_and_the_unanchored_pattern() {
    assert!(
        ARN_PATTERN == format!("^{ARN_PATTERN_UNANCHORED}$"),
        "the unanchored expression must be the modelled pattern minus its anchors, got \
         {ARN_PATTERN_UNANCHORED:?} against {ARN_PATTERN:?}"
    );
    let emulator = Emulator::new();

    // The rows vary the VALUE that misses the pattern and the MEMBER that carries it,
    // across every operation that binds an ARN member.
    let malformed = [
        ("not an ARN at all", "not-an-arn"),
        ("an ARN of another service", "arn:aws:s3:::bucket/key"),
        (
            "an uppercase partition",
            "arn:AWS:personalize:us-east-1:000000000000:campaign/x",
        ),
        (
            "no resource part",
            "arn:aws:personalize:us-east-1:000000000000:",
        ),
        ("an ARN prefix only", "arn:aws:personalize"),
    ];

    for (form, value) in malformed {
        let rows: [(&str, &str, serde_json::Value); 5] = [
            (
                "campaignArn",
                RECOMMENDATIONS,
                json!({ "campaignArn": value, "userId": "user-1" }),
            ),
            (
                "recommenderArn",
                RECOMMENDATIONS,
                json!({ "recommenderArn": value, "userId": "user-1" }),
            ),
            (
                "campaignArn",
                PERSONALIZE_RANKING,
                json!({ "campaignArn": value, "userId": "user-1", "inputList": ["item-1"] }),
            ),
            (
                "campaignArn",
                ACTION_RECOMMENDATIONS,
                json!({ "campaignArn": value, "userId": "user-1" }),
            ),
            (
                "filterArn",
                ACTION_RECOMMENDATIONS,
                json!({
                    "campaignArn": "arn:aws:personalize:us-east-1:000000000000:campaign/actions",
                    "userId": "user-1",
                    "filterArn": value,
                }),
            ),
        ];

        for (member, route, body) in rows {
            let refusal = emulator.refusal(route, body).await;
            assert_eq!(
                (
                    refusal.status,
                    refusal.code.as_str(),
                    refusal.errortype.as_str()
                ),
                (400, "ValidationException", "ValidationException"),
                "case {route} {member} {form}: expected 400 ValidationException, got {refusal:?}"
            );
            assert_eq!(
                refusal.message,
                pattern_clause(member),
                "case {route} {member} {form}: expected the clause to name the member and the \
                 unanchored pattern and to echo no value, got {:?}",
                refusal.message
            );
        }
    }
}

#[tokio::test]
async fn a_violation_of_another_class_keeps_the_frameworks_clause() {
    let emulator = Emulator::new();

    // `numResults` is modelled `range 0..`, so a negative value is a `@range` violation
    // — a class this service does not reword, so its clause still echoes the value.
    let refusal = emulator
        .refusal(
            RECOMMENDATIONS,
            json!({
                "campaignArn": "arn:aws:personalize:us-east-1:000000000000:campaign/any",
                "userId": "user-1",
                "numResults": -1,
            }),
        )
        .await;
    assert_eq!(
        (refusal.status, refusal.code.as_str()),
        (400, "ValidationException"),
        "case negative numResults: expected 400 ValidationException, got {refusal:?}"
    );
    assert_eq!(
        refusal.message,
        "1 validation error detected: Value '-1' at 'numResults' failed to satisfy constraint: \
         Member must have value greater than or equal to 0",
        "case negative numResults: expected the framework's own clause, got {:?}",
        refusal.message
    );
}

#[tokio::test]
async fn a_request_that_violates_two_classes_at_once_states_both_in_one_envelope() {
    let emulator = Emulator::new();

    // A `@pattern` violation and a `@range` violation in ONE request: the envelope
    // counts both and joins them with `; `, the pattern clause is the one this service
    // words and the range clause is the framework's, value and all.
    let refusal = emulator
        .refusal(
            RECOMMENDATIONS,
            json!({ "campaignArn": "not-an-arn", "userId": "user-1", "numResults": -1 }),
        )
        .await;
    assert_eq!(
        (refusal.status, refusal.code.as_str()),
        (400, "ValidationException"),
        "case two violation classes: expected 400 ValidationException, got {refusal:?}"
    );
    let pattern = format!(
        "Value at 'campaignArn' failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {ARN_PATTERN_UNANCHORED}"
    );
    let range = "Value '-1' at 'numResults' failed to satisfy constraint: Member must have value \
                 greater than or equal to 0";
    // The walk's clause ORDER is the framework's, so either order satisfies the rule:
    // what this pins is the count, the joiner and both clauses.
    let either = [
        format!("2 validation errors detected: {pattern}; {range}"),
        format!("2 validation errors detected: {range}; {pattern}"),
    ];
    assert!(
        either.contains(&refusal.message),
        "case two violation classes: expected one of {either:?}, got {:?}",
        refusal.message
    );
}

#[tokio::test]
async fn a_request_that_names_no_campaign_reports_the_campaign_arn_it_gave() {
    let emulator = Emulator::new();

    // `GetActionRecommendations$campaignArn` carries no `@required`, so an absent member
    // reaches the handler and is refused there, reporting what the request gave.
    let rows = [
        (
            "campaignArn absent",
            json!({ "userId": "user-1", "numResults": 3 }),
        ),
        (
            "campaignArn absent, nothing else given either",
            json!({}),
        ),
        (
            "campaignArn absent, every other member given",
            json!({
                "userId": "user-1",
                "numResults": 2,
                "filterArn": "arn:aws:personalize:us-east-1:000000000000:filter/any",
            }),
        ),
    ];

    for (case, body) in rows {
        let refusal = emulator.refusal(ACTION_RECOMMENDATIONS, body).await;
        assert_eq!(
            (
                refusal.status,
                refusal.code.as_str(),
                refusal.errortype.as_str()
            ),
            (400, "InvalidInputException", "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {refusal:?}"
        );
        assert_eq!(
            refusal.message, "The given campaign ARN is invalid: null",
            "case {case}: expected the refusal to report the ARN the request gave, got {:?}",
            refusal.message
        );
    }
}

/// A well-formed campaign ARN no registry holds, so a request that gets past the
/// request-only checks is refused for the campaign and nothing else.
const UNREGISTERED_CAMPAIGN: &str =
    "arn:aws:personalize:us-east-1:000000000000:campaign/never-created";

#[tokio::test]
async fn an_input_list_past_its_documented_maximum_is_refused_before_the_campaign_resolves() {
    // "If you are including metadata in recommendations, the maximum is 50. Otherwise,
    // the maximum is 500." (`GetPersonalizedRankingRequest$inputList` doc.) The bound is
    // a fact about the REQUEST, so it is answered without resolving anything: at the
    // bound the request gets as far as the (unresolvable) campaign, one past it the
    // length is what is reported.
    let emulator = Emulator::new();

    let rows: [(&str, bool, usize); 4] = [
        ("without metadata, at the bound", false, MAX_NUM_RESULTS),
        ("without metadata, one past it", false, MAX_NUM_RESULTS + 1),
        (
            "with metadata, at the bound",
            true,
            MAX_NUM_RESULTS_WITH_METADATA,
        ),
        (
            "with metadata, one past it",
            true,
            MAX_NUM_RESULTS_WITH_METADATA + 1,
        ),
    ];

    for (case, with_metadata, length) in rows {
        let input_list: Vec<String> = (0..length).map(|index| format!("item-{index}")).collect();
        let max = if with_metadata {
            MAX_NUM_RESULTS_WITH_METADATA
        } else {
            MAX_NUM_RESULTS
        };
        let mut body = json!({
            "campaignArn": UNREGISTERED_CAMPAIGN,
            "userId": "user-1",
            "inputList": input_list,
        });
        if with_metadata {
            body["metadataColumns"] = json!({ "ITEMS": ["GENRE"] });
        }

        let refusal = emulator.refusal(PERSONALIZE_RANKING, body).await;
        assert_eq!(
            (refusal.status, refusal.code.as_str()),
            (400, "InvalidInputException"),
            "case {case}: expected 400 InvalidInputException, got {refusal:?}"
        );
        let expected = if length > max {
            format!("Length of 'inputList' cannot be greater than {max}")
        } else {
            format!("{UNREGISTERED_CAMPAIGN} does not exist or is not active yet.")
        };
        assert_eq!(
            refusal.message, expected,
            "case {case}: expected {expected:?}, got {:?}",
            refusal.message
        );
    }
}
