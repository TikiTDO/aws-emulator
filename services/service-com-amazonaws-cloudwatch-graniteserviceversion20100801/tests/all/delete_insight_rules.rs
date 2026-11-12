// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteInsightRules`, read back through `DescribeInsightRules`,
//! `ListTagsForResource` and `ListManagedInsightRules`.
//!
//! The rule under test: **a batch Contributor Insights rule operation acts on exactly the rules
//! it was named and holds — `DeleteInsightRules` removing each one with everything stored under
//! it, so no later read reports it — changes nothing about a rule it was not named, and reports
//! no `Failures` entry for a name it holds no rule for**: "Permanently deletes the
//! specified Contributor Insights rules." / "An array listing the rules that could not be
//! deleted." (`DeleteInsightRules`, `DeleteInsightRulesOutput$Failures`), empty when every named
//! rule reached the state the request asked for ("This array is empty if the API operation was
//! successful for all the rules specified in the request.", `PartialFailure`) — and a name no
//! rule answers to has reached it already. An empty `RuleNames` array asks for
//! nothing and is refused as a bad parameter value, under the modelled
//! `InvalidParameterValueException` (HTTP 400) every one of those operations declares.
//!
//! Every table row is a distinct input along what the rule generalises over (how many names one
//! request carries, whether a name is held or unknown or repeated, the state the rule holds,
//! the name's length and the characters its pattern allows, whether the rule was created by
//! `PutInsightRule` or by `PutManagedInsightRules`, and which of the batch operations is
//! called), so the recorded case is one row among many rather than the specification.

use crate::harness::{
    build_router, build_router_with_engine, call, error_code, error_message, ok,
    publish_managed_rule_template,
};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// How this service words an empty `RuleNames` array, per batch operation: the delete path
/// names the rule names it was given nothing in, and the two operations that MOVE a rule's
/// state name the set of states such a request asks to write.
const EMPTY_ARRAY_REFUSALS: [(&str, &str); 3] = [
    (
        "DeleteInsightRules",
        "EMPTY_RULE_NAMES: RuleNames cannot be an empty array.",
    ),
    (
        "EnableInsightRules",
        "EMPTY_RULE_STATES: RuleStates cannot be an empty array.",
    ),
    (
        "DisableInsightRules",
        "EMPTY_RULE_STATES: RuleStates cannot be an empty array.",
    ),
];

/// A rule definition in the Contributor Insights rule syntax.
fn definition() -> String {
    json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    })
    .to_string()
}

/// Create the rule `name` in `rule_state`, as `PutInsightRule` stores it.
async fn given_rule(router: &MultiServiceRouter, name: &str, rule_state: &str) {
    ok(
        router,
        "PutInsightRule",
        &json!({
            "RuleName": name,
            "RuleDefinition": definition(),
            "RuleState": rule_state,
        }),
    )
    .await;
}

/// The rule `DescribeInsightRules` reports under `name`, if any.
async fn described_rule(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let body = ok(router, "DescribeInsightRules", &json!({})).await;
    body.get("InsightRules")?
        .as_array()?
        .iter()
        .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(name))
        .cloned()
}

/// The `Failures` entries an answer carries, in the order it carried them.
fn failures(body: &Value) -> Vec<Value> {
    body.get("Failures")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The ARN of the Contributor Insights rule `name`: "The ARN format of a Contributor Insights
/// rule is arn:aws:cloudwatch:Region:account-id:insight-rule/insight-rule-name"
/// (`ListTagsForResourceInput$ResourceARN`).
fn rule_arn(name: &str) -> String {
    format!("arn:aws:cloudwatch:us-east-1:000000000000:insight-rule/{name}")
}

/// One row of the delete table: the rules that exist before the call, each with the state it
/// was created in, and the rule names the request carries.
struct Row {
    /// What this row is, named in every assertion message.
    case: String,
    /// The rules created before the call, as (name, created state).
    held: Vec<(String, String)>,
    /// The rule names the request carries, in request order.
    named: Vec<String>,
}

/// One table row, from the borrowed names a case states.
fn row(case: &str, held: &[(&str, &str)], named: &[&str]) -> Row {
    Row {
        case: case.to_string(),
        held: held
            .iter()
            .map(|(name, rule_state)| ((*name).to_string(), (*rule_state).to_string()))
            .collect(),
        named: named.iter().map(|name| (*name).to_string()).collect(),
    }
}

#[tokio::test]
async fn every_named_rule_this_service_holds_is_deleted_and_every_other_rule_is_left_alone() {
    // Each row is one request: the rules that exist beforehand (name, state) and the names the
    // request carries. Rows span a single name, several names, every held name, a name no rule
    // answers to, a mixture of held and unknown names, the same name twice, a disabled rule, a
    // one-character name, a 128-character name and a name carrying the spaces and punctuation
    // `^[\x20-\x7E]+$` allows.
    let long_name = "n".repeat(128);
    let odd_name = "My rule (v2) / high-cardinality";
    let rows: Vec<Row> = vec![
        row("one held rule", &[("one", "ENABLED")], &["one"]),
        row("one held DISABLED rule", &[("off", "DISABLED")], &["off"]),
        row(
            "three held rules, two of them named",
            &[
                ("first", "ENABLED"),
                ("second", "DISABLED"),
                ("third", "ENABLED"),
            ],
            &["first", "third"],
        ),
        row(
            "every held rule named",
            &[("alpha", "DISABLED"), ("bravo", "ENABLED")],
            &["alpha", "bravo"],
        ),
        row(
            "one name no rule answers to",
            &[("held", "ENABLED")],
            &["no-such-rule"],
        ),
        row(
            "a held name and two unknown ones",
            &[("held", "DISABLED")],
            &["unknown one", "held", "unknown two"],
        ),
        row(
            "the same held name twice",
            &[("twice", "ENABLED"), ("kept", "ENABLED")],
            &["twice", "twice"],
        ),
        row(
            "the same unknown name twice",
            &[("held", "ENABLED")],
            &["ghost", "ghost"],
        ),
        row("a one-character name", &[("r", "ENABLED")], &["r"]),
        row(
            "a 128-character name",
            &[(long_name.as_str(), "DISABLED")],
            &[long_name.as_str()],
        ),
        row(
            "a name with the spaces and punctuation the pattern allows",
            &[(odd_name, "ENABLED"), ("other", "ENABLED")],
            &[odd_name],
        ),
        row("no rule configured at all", &[], &["nothing here"]),
    ];

    for Row { case, held, named } in &rows {
        let router = build_router();
        for (name, rule_state) in held {
            given_rule(&router, name, rule_state).await;
        }
        // What every rule looked like before the call, so a rule the request did not name can
        // be asserted unchanged member by member rather than by its state alone.
        let mut before: Vec<(String, Option<Value>)> = Vec::new();
        for (name, _) in held {
            before.push((name.clone(), described_rule(&router, name).await));
        }

        let answered = ok(&router, "DeleteInsightRules", &json!({"RuleNames": named})).await;

        // A named rule this service held is gone; a rule the request did not name is reported
        // exactly as it was.
        for (name, snapshot) in &before {
            let actual = described_rule(&router, name).await;
            let expected = if named.contains(name) {
                None
            } else {
                snapshot.clone()
            };
            assert_eq!(
                actual, expected,
                "case {case:?}: expected rule {name:?} to read back as {expected:?}, got \
                 {actual:?}"
            );
        }

        // No failure entry at all: every rule this service holds is one it deletes, and a name
        // it holds no rule for already names nothing to delete.
        let reported = failures(&answered);
        assert_eq!(
            reported,
            Vec::<Value>::new(),
            "case {case:?}: expected an empty Failures array, got {answered}"
        );
        assert!(
            answered.get("Failures").is_some_and(Value::is_array),
            "case {case:?}: expected the Failures array to be answered, got {answered}"
        );

        // The deletion is permanent and the delete is idempotent: naming the same rules again
        // finds none of them, changes nothing, and still reports no failure.
        let again = ok(&router, "DeleteInsightRules", &json!({"RuleNames": named})).await;
        assert_eq!(
            failures(&again),
            Vec::<Value>::new(),
            "case {case:?}: a second delete of the same names reports no failure, got {again}"
        );
        for name in named {
            let actual = described_rule(&router, name).await;
            assert_eq!(
                actual, None,
                "case {case:?}: rule {name:?} must stay deleted, got {actual:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_rule_takes_its_tags_with_it_and_its_name_can_be_created_again() {
    // Rows differ in what the deleted rule was created with: its state, whether it carries
    // tags, and how many.
    let rows: Vec<(&str, Value)> = vec![
        (
            "the required members only",
            json!({"RuleName": "plain", "RuleDefinition": definition()}),
        ),
        (
            "a disabled rule with one tag",
            json!({
                "RuleName": "tagged once",
                "RuleDefinition": definition(),
                "RuleState": "DISABLED",
                "Tags": [{"Key": "team", "Value": "observability"}],
            }),
        ),
        (
            "an enabled rule with two tags, evaluated on transformed logs",
            json!({
                "RuleName": "tagged twice",
                "RuleDefinition": definition(),
                "RuleState": "ENABLED",
                "ApplyOnTransformedLogs": true,
                "Tags": [
                    {"Key": "team", "Value": "observability"},
                    {"Key": "env", "Value": "prod"},
                ],
            }),
        ),
    ];

    for (case, request) in rows {
        let router = build_router();
        ok(&router, "PutInsightRule", &request).await;
        let name = request["RuleName"].as_str().unwrap_or_default();

        let answered = ok(&router, "DeleteInsightRules", &json!({"RuleNames": [name]})).await;
        assert_eq!(
            failures(&answered),
            Vec::<Value>::new(),
            "case {case:?}: a request naming only held rules reports no failure, got {answered}"
        );

        // The rule's tags were stored with the rule, so the resource they hung on is gone too.
        let (status, body) = call(
            &router,
            "ListTagsForResource",
            &json!({"ResourceARN": rule_arn(name)}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: a deleted rule is no resource to list tags for, got {status} {body}"
        );

        // The name is free again, and the rule created under it carries nothing from the one
        // that was deleted: neither its state, nor its tags.
        ok(
            &router,
            "PutInsightRule",
            &json!({"RuleName": name, "RuleDefinition": definition()}),
        )
        .await;
        let recreated = described_rule(&router, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: expected the recreated rule back"));
        assert_eq!(
            recreated.get("State").and_then(Value::as_str),
            Some("ENABLED"),
            "case {case:?}: a rule created with no RuleState is ENABLED, got {recreated}"
        );
        let tags = ok(
            &router,
            "ListTagsForResource",
            &json!({"ResourceARN": rule_arn(name)}),
        )
        .await;
        assert_eq!(
            tags.get("Tags").cloned().unwrap_or_else(|| json!([])),
            json!([]),
            "case {case:?}: the recreated rule carries no tag of the deleted one, got {tags}"
        );
    }
}

#[tokio::test]
async fn a_managed_rule_is_deleted_by_the_same_operation() {
    // "The rules can be enabled, disabled, and deleted using EnableInsightRules,
    // DisableInsightRules, and DeleteInsightRules." (`PutManagedInsightRules`) — so a managed
    // rule is deleted the same way as one a caller wrote, whatever resource it was enabled for.
    let resources = [
        (
            "a DynamoDB table",
            "arn:aws:dynamodb:us-east-1:123456789012:table/Orders",
        ),
        (
            "an S3 bucket with neither Region nor account",
            "arn:aws:s3:::access-logs",
        ),
    ];

    for (case, resource_arn) in resources {
        let (router, engine) = build_router_with_engine();
        publish_managed_rule_template(&engine, "ServiceLogRule-DynamoDB", resource_arn);
        ok(
            &router,
            "PutManagedInsightRules",
            &json!({
                "ManagedRules": [{
                    "TemplateName": "ServiceLogRule-DynamoDB",
                    "ResourceARN": resource_arn,
                }],
            }),
        )
        .await;
        let listed = ok(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource_arn}),
        )
        .await;
        let name = listed["ManagedRules"][0]["RuleState"]["RuleName"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a managed rule, got {listed}"))
            .to_string();

        let answered = ok(&router, "DeleteInsightRules", &json!({"RuleNames": [&name]})).await;
        assert_eq!(
            failures(&answered),
            Vec::<Value>::new(),
            "case {case:?}: a managed rule is one this service holds, got {answered}"
        );
        let actual = described_rule(&router, &name).await;
        assert_eq!(
            actual, None,
            "case {case:?}: the managed rule must be gone, got {actual:?}"
        );

        // The template it was enabled from is still published, now with no rule enabled from
        // it: "If present, it contains information about the Contributor Insights rule that
        // contains information about the related Amazon Web Services resource."
        // (`ManagedRuleDescription$RuleState`)
        let listed = ok(
            &router,
            "ListManagedInsightRules",
            &json!({"ResourceARN": resource_arn}),
        )
        .await;
        assert_eq!(
            listed["ManagedRules"][0]["TemplateName"].as_str(),
            Some("ServiceLogRule-DynamoDB"),
            "case {case:?}: the template stays published, got {listed}"
        );
        let rule_state = listed["ManagedRules"][0].get("RuleState");
        assert_eq!(
            rule_state.and_then(|state| state.get("RuleName")),
            None,
            "case {case:?}: no rule is enabled from the template any more, got {listed}"
        );
    }
}

#[tokio::test]
async fn an_empty_rule_names_array_is_refused_and_changes_nothing() {
    for (operation, expected_message) in EMPTY_ARRAY_REFUSALS {
        let router = build_router();
        given_rule(&router, "untouched", "ENABLED").await;
        let before = described_rule(&router, "untouched").await;

        let (status, body) = call(&router, operation, &json!({"RuleNames": []})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {operation:?}: expected a 400 InvalidParameterValueException, got {status} \
             {body}"
        );
        assert_eq!(
            error_message(&body),
            expected_message,
            "case {operation:?}: expected the empty-array wording, got {body}"
        );
        let actual = described_rule(&router, "untouched").await;
        assert_eq!(
            actual, before,
            "case {operation:?}: a refused request changes no rule, got {actual:?}"
        );
    }
}
