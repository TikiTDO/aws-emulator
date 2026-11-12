// SPDX-License-Identifier: CC0-1.0

//! Cover for how the operations that PAGE Contributor Insights rules refuse the two paging
//! values a caller supplies, and for what a DELETE of a rule that is not there reports.
//!
//! Three rules are under test:
//!
//! 1. **A listing that pages Contributor Insights rules refuses a `NextToken` it did not issue
//!    with the modelled `InvalidNextToken` (HTTP 400) and ONE fixed coded sentence per FAULT,
//!    echoing nothing of the token: a value that is no opaque pagination token at all is
//!    refused as the token it is not, and a value that is one but carries no cursor this
//!    service minted is refused as a token of a form it does not support.** "Include this
//!    value, if it was returned by the previous operation, to get the next set of rules."
//!    (`DescribeInsightRulesInput$NextToken`, `ListManagedInsightRulesInput$NextToken`) — a
//!    token the service did not mint names no page of the listing, and which of the two faults
//!    it has is all a client can repair, so the sentence says that and nothing of the value. A
//!    token the listing DID issue is honoured.
//!
//! 2. **Such a listing refuses a page size below the published minimum of the member that asks
//!    for one as a bad parameter VALUE — the parameter-value class, named in full in the body
//!    and reported with its legacy query code — under the coded sentence naming that member and
//!    the bound.** `MaxResults` is "the maximum number of results to return in one operation"
//!    (`DescribeInsightRulesInput$MaxResults`, `ListManagedInsightRulesInput$MaxResults`, both
//!    targeting `InsightRuleMaxResults` range 1..500), so one result is the smallest page there
//!    is: a value under it asks for a page that could report nothing. Each published bound is
//!    served, and a value above the maximum keeps the request-shape class the model's own
//!    constraint is answered under.
//!
//! 3. **`DeleteInsightRules` reports no `Failures` entry for a name it holds no rule for.** "An
//!    array listing the rules that could not be deleted." (`DeleteInsightRulesOutput$Failures`),
//!    "This array is empty if the API operation was successful for all the rules specified in
//!    the request." (`PartialFailure`) — the state a delete asks a rule to reach is being gone,
//!    which a name no rule answers to has reached already, so the request left nothing undone.
//!    The operations that MOVE a rule's state do report such a name, which is what shows the
//!    difference is the end state asked for and not the name.
//!
//! Rows differ along what each rule generalises over: which listing is called, how the token is
//! unusable, which page size is asked for (both published bounds, the values below the minimum
//! and one past the maximum), and — for the delete — how many names a request carries, how known
//! and unknown and repeated names are mixed, and the length and characters of the name.

use crate::harness::{
    build_router, build_router_with_engine, call, call_in_query_mode, error_code, error_message,
    ok, publish_managed_rule_template,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The resource a managed-rule listing is asked about in every row below.
const RESOURCE_ARN: &str = "arn:aws:dynamodb:us-east-1:123456789012:table/Orders";

/// A rule definition in the Contributor Insights rule syntax, as `PutInsightRule` takes it.
fn definition() -> String {
    json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    })
    .to_string()
}

/// Create the rule `name`, as `PutInsightRule` stores it.
async fn given_rule(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "PutInsightRule",
        &json!({"RuleName": name, "RuleDefinition": definition()}),
    )
    .await;
}

/// An emulator holding two Contributor Insights rules and one published managed-rule template,
/// so both listings have more than one page to report.
async fn given_two_rules_and_a_template() -> MultiServiceRouter {
    let (router, engine) = build_router_with_engine();
    publish_managed_rule_template(&engine, "ServiceLogRule-DynamoDB", RESOURCE_ARN);
    given_rule(&router, "alpha").await;
    given_rule(&router, "bravo").await;
    router
}

/// One paging request, per listing: the operation and the members it takes besides the paging
/// ones under test.
const LISTINGS: [(&str, &str); 2] = [
    ("DescribeInsightRules", "{}"),
    (
        "ListManagedInsightRules",
        r#"{"ResourceARN": "arn:aws:dynamodb:us-east-1:123456789012:table/Orders"}"#,
    ),
];

/// The request a row submits: the listing's own members, plus `member` set to `value`.
fn paging_request(members: &str, member: &str, value: Value) -> Value {
    let mut request: Value = serde_json::from_str(members).expect("the listing's own members");
    request[member] = value;
    request
}

/// The `Failures` entries an answer carries, in the order it carried them.
fn failures(body: &Value) -> Vec<Value> {
    body.get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The fully qualified shape id a failure body names its class under.
fn qualified_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn a_next_token_no_listing_issued_is_refused_with_the_sentence_for_its_fault() {
    // Rows differ along WHICH of the two faults the value has, and along how it has it: a value
    // whose length no opaque token has (one symbol, and five), and values that are opaque tokens
    // carrying no cursor this service minted — text that is not the encoding, an odd number of
    // hex digits, a decoding that is not text, one with base64 padding, and one in the standard
    // alphabet rather than the URL-safe one.
    //
    // (the token, the sentence it is refused with)
    let rows: [(&str, &str); 9] = [
        ("z", "INVALID_TOKEN: Invalid token."),
        ("abcde", "INVALID_TOKEN: Invalid token."),
        (
            "sim2real-probe-cwp-not-a-minted-token",
            "INVALID_TOKEN: Invalid token.",
        ),
        (
            "not-a-valid-next-token",
            "INVALID_TOKEN: Unsupported token version.",
        ),
        (
            "not-a-real-pagination-token",
            "INVALID_TOKEN: Unsupported token version.",
        ),
        ("abc", "INVALID_TOKEN: Unsupported token version."),
        ("fffe", "INVALID_TOKEN: Unsupported token version."),
        ("YWJjZA==", "INVALID_TOKEN: Unsupported token version."),
        ("ab+/", "INVALID_TOKEN: Unsupported token version."),
    ];
    let router = given_two_rules_and_a_template().await;

    for (operation, members) in LISTINGS {
        for (token, expected) in rows {
            let case = format!("{operation} / token {token:?}");
            let (status, body) = call(
                &router,
                operation,
                &paging_request(members, "NextToken", json!(token)),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidNextToken"),
                "case {case:?}: expected HTTP 400 InvalidNextToken, got {status} with {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the sentence for the token's fault, got {message:?}"
            );
            assert!(
                !message.contains(token),
                "case {case:?}: the refusal must echo nothing of the token, got {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_next_token_a_listing_issued_is_honoured() {
    let router = given_two_rules_and_a_template().await;

    // The first page of a one-rule listing hands back the token the second page is asked with.
    let first = ok(
        &router,
        "DescribeInsightRules",
        &json!({"MaxResults": 1}),
    )
    .await;
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("expected a continuation token, got {first}"))
        .to_string();
    let second = ok(
        &router,
        "DescribeInsightRules",
        &json!({"MaxResults": 1, "NextToken": token}),
    )
    .await;
    let names: Vec<&str> = second
        .get("InsightRules")
        .and_then(Value::as_array)
        .map(|rules| {
            rules
                .iter()
                .filter_map(|rule| rule.get("Name").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["bravo"],
        "case \"the token the first page issued\": expected the next rule, got {second}"
    );
}

#[tokio::test]
async fn a_page_size_below_the_published_minimum_is_refused_as_a_parameter_value() {
    // Rows differ along how far below `InsightRuleMaxResults`'s published minimum of 1 the page
    // size is: the boundary value itself, one below it and a large negative.
    let below_the_minimum = [0, -1, -500];
    let router = given_two_rules_and_a_template().await;

    for (operation, members) in LISTINGS {
        for max_results in below_the_minimum {
            let case = format!("{operation} / MaxResults {max_results}");
            let (status, headers, body) = call_in_query_mode(
                &router,
                operation,
                &paging_request(members, "MaxResults", json!(max_results)),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                qualified_type(&body),
                "com.amazon.coral.service#InvalidParameterValueException",
                "case {case:?}: expected the parameter-value class named in full, got {body}"
            );
            assert_eq!(
                error_message(&body),
                "INVALID_MAX_RESULTS: MaxResults must be greater than 0.",
                "case {case:?}: expected the coded sentence naming the member, got {body}"
            );
            // The legacy query code a query-compatible client branches on, for the class and
            // the side of the fault.
            let reported = headers
                .get("x-amzn-query-error")
                .and_then(|value| value.to_str().ok());
            assert_eq!(
                reported,
                Some("InvalidParameterValue;Sender"),
                "case {case:?}: expected the query-error report of the class, got {reported:?}"
            );
        }
    }
}

#[tokio::test]
async fn each_published_page_size_bound_is_served_and_one_past_the_maximum_is_not() {
    // `InsightRuleMaxResults` is range 1..500, so both bounds are page sizes a listing answers;
    // one past the maximum is outside the shape the model declares.
    let router = given_two_rules_and_a_template().await;

    for (operation, members) in LISTINGS {
        for max_results in [1, 500] {
            let case = format!("{operation} / MaxResults {max_results}");
            let (status, body) = call(
                &router,
                operation,
                &paging_request(members, "MaxResults", json!(max_results)),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected a published bound to be served, got {status} {body}"
            );
        }
        let case = format!("{operation} / MaxResults 501");
        let (status, body) = call(
            &router,
            operation,
            &paging_request(members, "MaxResults", json!(501)),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the request-shape class for a value past the modelled \
             maximum, got {status} {body}"
        );
    }
}

/// One row of the delete table: the rules held before the call and the names the request
/// carries.
struct DeleteRow {
    /// What this row is, named in every assertion message.
    case: &'static str,
    /// The rule names created before the call.
    held: Vec<String>,
    /// The rule names the request carries, in request order.
    named: Vec<String>,
}

/// One row, from the borrowed names a case states.
fn delete_row(case: &'static str, held: &[&str], named: &[&str]) -> DeleteRow {
    DeleteRow {
        case,
        held: held.iter().map(|name| (*name).to_string()).collect(),
        named: named.iter().map(|name| (*name).to_string()).collect(),
    }
}

#[tokio::test]
async fn deleting_a_rule_that_is_not_there_reports_no_failure_while_moving_its_state_does() {
    // Rows span one unknown name with nothing held, an unknown name beside a held one, the same
    // unknown name twice, a one-character name, the modelled 128-character maximum, a name
    // carrying the punctuation `^[\x20-\x7E]+$` allows, and a request naming only held rules.
    let long_name = "n".repeat(128);
    let rows: Vec<DeleteRow> = vec![
        delete_row("one unknown name, nothing held", &[], &["no-such-rule"]),
        delete_row(
            "a held name between two unknown ones",
            &["held"],
            &["ghost one", "held", "ghost two"],
        ),
        delete_row("the same unknown name twice", &[], &["ghost", "ghost"]),
        delete_row("a one-character unknown name", &["held"], &["x"]),
        delete_row("a 128-character unknown name", &[], &[long_name.as_str()]),
        delete_row(
            "an unknown name with the punctuation the pattern allows",
            &[],
            &["My rule (v2) / high-cardinality"],
        ),
        delete_row("every named rule held", &["alpha"], &["alpha"]),
    ];

    for DeleteRow { case, held, named } in &rows {
        let router = build_router();
        for name in held {
            given_rule(&router, name).await;
        }

        let deleted = ok(&router, "DeleteInsightRules", &json!({"RuleNames": named})).await;
        assert_eq!(
            failures(&deleted),
            Vec::<Value>::new(),
            "case {case:?}: expected no failure entry from a delete, got {deleted}"
        );
        assert!(
            deleted.get("Failures").is_some_and(Value::is_array),
            "case {case:?}: expected the Failures array to be answered, got {deleted}"
        );

        // The same names handed to the operations that MOVE a rule's state: a name no rule
        // answers to cannot be written, and is reported per name.
        for operation in ["EnableInsightRules", "DisableInsightRules"] {
            let case = format!("{operation} / {case}");
            let moved = ok(&router, operation, &json!({"RuleNames": named})).await;
            let entries = failures(&moved);
            let reported: Vec<&str> = entries
                .iter()
                .filter_map(|entry| entry.get("FailureResource").and_then(Value::as_str))
                .collect();
            let expected: Vec<&str> = named.iter().map(String::as_str).collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected one entry per name, every rule having been deleted, \
                 got {moved}"
            );
        }
    }
}
