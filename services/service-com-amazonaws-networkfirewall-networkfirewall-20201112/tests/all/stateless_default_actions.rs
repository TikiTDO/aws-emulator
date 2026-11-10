// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a firewall policy's STATELESS DEFAULT ACTIONS, and for
//! the error class a create whose name is already taken answers with.
//!
//! Two rules, both about what a request may carry rather than about one request:
//!
//! * an entry of `StatelessDefaultActions` or `StatelessFragmentDefaultActions`
//!   must name a standard stateless action with its namespace (`aws:pass`,
//!   `aws:drop`, `aws:forward_to_sfe`) or a custom action the same document
//!   declares in `StatelessCustomActions`, neither list may be empty, and one
//!   list may name only one standard action; anything else is refused with HTTP
//!   400 / `InvalidRequestException` before any state changes, so the policy name
//!   stays free;
//! * a create under a name that is already taken is a defect of the REQUEST
//!   (`InvalidRequestException`), not the in-use/state conflict
//!   `InvalidOperationException` reports.
//!
//! Every test is a TABLE spanning the class each rule covers — both action
//! members, the create and the update path, standard and custom actions, and
//! every resource this service creates by name — driven over the wire through the
//! shared [`Harness`].

use serde_json::{json, Value};

use crate::harness::{
    error_code, error_message, minimal_policy_document, string_at, Harness,
    NOT_FOUND_MESSAGE as NOT_FOUND,
};

/// The standard stateless actions, from the documentation of the two members
/// that take them (`smithy_query(query="shape",shape="FirewallPolicy")`,
/// `StatelessDefaultActions` / `StatelessFragmentDefaultActions`: "You must
/// specify one of the standard actions: aws:pass, aws:drop, or
/// aws:forward_to_sfe").
const STANDARD_ACTIONS: [&str; 3] = ["aws:pass", "aws:drop", "aws:forward_to_sfe"];

/// The text a create whose name is already taken is refused with, as recorded.
const ALREADY_EXISTS: &str = "A resource with the specified name already exists";

/// The actions a described policy reports for `member`, exactly as it reports
/// them: a policy keeps the spelling the accepted request used, so a read is
/// compared by bytes.
async fn described_actions(harness: &Harness, name: &str, member: &str) -> Vec<String> {
    let described = harness
        .ok("DescribeFirewallPolicy", &json!({ "FirewallPolicyName": name }))
        .await;
    described["FirewallPolicy"][member]
        .as_array()
        .unwrap_or_else(|| panic!("expected {member} in {described:?}"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("expected {member} entries to be strings"))
                .to_string()
        })
        .collect()
}

/// A policy document with the given default actions and, optionally, one declared
/// custom action.
fn document(default_actions: &[&str], fragment_actions: &[&str], custom: Option<&str>) -> Value {
    let mut policy = json!({
        "StatelessDefaultActions": default_actions,
        "StatelessFragmentDefaultActions": fragment_actions,
    });
    if let Some(name) = custom {
        policy["StatelessCustomActions"] = json!([{
            "ActionName": name,
            "ActionDefinition": { "PublishMetricAction": {
                "Dimensions": [{ "Value": name }],
            }},
        }]);
    }
    policy
}

/// One row of the accepted table: the case, the two action lists the document
/// carries, and the custom action it declares (if any).
type AcceptedRow<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Option<&'a str>);

/// One row of a refused table: the case, the two action lists, the member whose
/// entry fails and the action it fails on.
type RefusedRow<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, &'a str, &'a str);

/// One row of the duplicate-name table: the case, the create operation and its
/// request, the read that proves the stored resource survived with its selector,
/// and the response path of that resource's ARN.
type DuplicateRow<'a> = (&'a str, &'a str, Value, &'a str, Value, Vec<&'a str>);

/// RULE: a default stateless action naming a standard action — in any CASE of
/// its namespaced name — or a custom action the same document declares is
/// accepted, stored, and reported back in the spelling the request used.
///
/// Rows span all three standard actions (the catalog above; `aws:pass` and
/// `aws:drop` are not the one the recorded case used), the upper-case and
/// mixed-case spellings of them, the same standard action named twice (which
/// still names one action), a declared custom action beside a standard one, and
/// a custom action in the fragment member.
#[tokio::test]
async fn a_supported_stateless_default_action_is_accepted_and_round_trips() {
    let harness = Harness::new();

    let rows: Vec<AcceptedRow> = vec![
        ("pass", vec![STANDARD_ACTIONS[0]], vec![STANDARD_ACTIONS[0]], None),
        ("drop", vec![STANDARD_ACTIONS[1]], vec![STANDARD_ACTIONS[1]], None),
        (
            "forward-to-sfe",
            vec![STANDARD_ACTIONS[2]],
            vec![STANDARD_ACTIONS[2]],
            None,
        ),
        // The spelling the corpus records a stored policy reporting back.
        ("upper-case", vec!["AWS:PASS"], vec!["AWS:FORWARD_TO_SFE"], None),
        (
            "mixed-case",
            vec!["Aws:Forward_To_Sfe"],
            vec!["aWs:dRop"],
            None,
        ),
        (
            "the-same-standard-action-twice",
            vec!["aws:pass", "AWS:PASS"],
            vec!["aws:drop"],
            None,
        ),
        (
            "standard-plus-declared-custom",
            vec!["aws:pass", "DeclaredAction"],
            vec!["aws:drop"],
            Some("DeclaredAction"),
        ),
        (
            "declared-custom-in-the-fragment-member",
            vec!["aws:forward_to_sfe"],
            vec!["aws:drop", "FragmentAction"],
            Some("FragmentAction"),
        ),
    ];

    for (case, actions, fragment_actions, custom) in &rows {
        let name = format!("accepted-{case}");
        let policy = document(actions, fragment_actions, *custom);
        let created = harness
            .ok(
                "CreateFirewallPolicy",
                &json!({ "FirewallPolicyName": name, "FirewallPolicy": policy }),
            )
            .await;
        let status = string_at(&created, &["FirewallPolicyResponse", "FirewallPolicyStatus"]);
        assert_eq!(
            status, "ACTIVE",
            "case {case}: expected the created policy to be ACTIVE, got {created:?}"
        );

        for (member, expected) in [
            ("StatelessDefaultActions", actions),
            ("StatelessFragmentDefaultActions", fragment_actions),
        ] {
            let reported = described_actions(&harness, &name, member).await;
            let expected: Vec<String> =
                expected.iter().map(|entry| (*entry).to_string()).collect();
            assert_eq!(
                reported, expected,
                "case {case}: expected the read to report {member} as the request spelled it, \
                 {expected:?}, got {reported:?}"
            );
        }
    }
}

/// RULE: a default stateless action outside that vocabulary is refused with HTTP
/// 400 / `InvalidRequestException`, whose message names the member, brackets the
/// offending action verbatim and gives the member as context — and nothing is
/// stored, so the policy name stays free.
///
/// Rows span both action members, an action of the STATEFUL vocabulary, an
/// action that exists nowhere, a custom action no document declared, and a custom
/// action declared in a DIFFERENT policy's document; each is sent with
/// `DryRun` false and true, because a dry run checks "whether the request can run
/// successfully".
#[tokio::test]
async fn an_unsupported_stateless_default_action_is_refused_and_leaves_the_name_free() {
    let harness = Harness::new();
    // A policy that DOES declare `ElsewhereAction`, so a later request naming it
    // without declaring it is refused for the document it sent, not for the
    // vocabulary of the service.
    harness
        .ok(
            "CreateFirewallPolicy",
            &json!({
                "FirewallPolicyName": "declares-elsewhere-action",
                "FirewallPolicy": document(
                    &["aws:pass", "ElsewhereAction"],
                    &["aws:drop"],
                    Some("ElsewhereAction"),
                ),
            }),
        )
        .await;

    let rows: Vec<RefusedRow> = vec![
        (
            "stateful-forwarding-action",
            vec!["aws:forward_to_stateful"],
            vec!["aws:forward_to_sfe"],
            "StatelessDefaultActions",
            "aws:forward_to_stateful",
        ),
        (
            "an-action-of-the-stateful-vocabulary",
            vec!["aws:drop_strict"],
            vec!["aws:forward_to_sfe"],
            "StatelessDefaultActions",
            "aws:drop_strict",
        ),
        (
            "an-action-that-exists-nowhere",
            vec!["aws:teleport"],
            vec!["aws:pass"],
            "StatelessDefaultActions",
            "aws:teleport",
        ),
        (
            "an-undeclared-custom-action",
            vec!["aws:pass", "UndeclaredAction"],
            vec!["aws:pass"],
            "StatelessDefaultActions",
            "UndeclaredAction",
        ),
        (
            "a-custom-action-another-policy-declares",
            vec!["aws:pass", "ElsewhereAction"],
            vec!["aws:pass"],
            "StatelessDefaultActions",
            "ElsewhereAction",
        ),
        (
            "the-fragment-member",
            vec!["aws:forward_to_sfe"],
            vec!["aws:forward_to_stateful"],
            "StatelessFragmentDefaultActions",
            "aws:forward_to_stateful",
        ),
        (
            "an-undeclared-custom-action-in-the-fragment-member",
            vec!["aws:pass", "DeclaredHere"],
            vec!["NotDeclaredHere"],
            "StatelessFragmentDefaultActions",
            "NotDeclaredHere",
        ),
        // The namespace is part of a standard action's name, so the token on its
        // own, and the token under another namespace, name no action.
        (
            "a-standard-action-without-its-namespace",
            vec!["forward_to_sfe"],
            vec!["aws:drop"],
            "StatelessDefaultActions",
            "forward_to_sfe",
        ),
        (
            "an-upper-case-token-without-its-namespace",
            vec!["PASS"],
            vec!["aws:drop"],
            "StatelessDefaultActions",
            "PASS",
        ),
        (
            "a-standard-token-under-another-namespace",
            vec!["amazon:pass"],
            vec!["aws:drop"],
            "StatelessDefaultActions",
            "amazon:pass",
        ),
        (
            "a-token-without-its-namespace-in-the-fragment-member",
            vec!["aws:pass"],
            vec!["drop"],
            "StatelessFragmentDefaultActions",
            "drop",
        ),
    ];

    for (case, actions, fragment_actions, member, offending) in &rows {
        let custom = actions
            .iter()
            .chain(fragment_actions.iter())
            .any(|entry| *entry == "DeclaredHere")
            .then_some("DeclaredHere");
        let policy = document(actions, fragment_actions, custom);
        for dry_run in [false, true] {
            let name = format!("refused-{case}-{dry_run}");
            let (status, body) = harness
                .call(
                    "CreateFirewallPolicy",
                    &json!({
                        "FirewallPolicyName": name,
                        "FirewallPolicy": policy.clone(),
                        "DryRun": dry_run,
                    }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidRequestException"),
                "case {case} (DryRun={dry_run}): expected 400 InvalidRequestException, \
                 got {status} {body:?}"
            );
            let message = error_message(&body);
            let expected =
                format!("{member} is invalid, parameter: [{offending}], context: {member}");
            assert_eq!(
                message, expected,
                "case {case} (DryRun={dry_run}): expected {expected:?}, got {message:?}"
            );

            // The refused create consumed nothing: the name is still free, so a
            // read of it misses.
            let (status, body) = harness
                .call(
                    "DescribeFirewallPolicy",
                    &json!({ "FirewallPolicyName": name }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case} (DryRun={dry_run}): expected the refused name to stay free, \
                 got {status} {body:?}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, NOT_FOUND,
                "case {case} (DryRun={dry_run}): expected {NOT_FOUND:?}, got {message:?}"
            );
        }
    }
}

/// RULE: the same vocabulary holds on the UPDATE path, where a refusal leaves the
/// stored policy and its token exactly as the last accepted write left them.
///
/// Rows span both action members and an undeclared custom action; a valid update
/// then proves the path the refusals took is the document check and not the
/// operation.
#[tokio::test]
async fn a_refused_update_leaves_the_stored_policy_unchanged() {
    let harness = Harness::new();
    let created = harness
        .ok(
            "CreateFirewallPolicy",
            &json!({
                "FirewallPolicyName": "updated-policy",
                "FirewallPolicy": document(&["aws:pass"], &["aws:drop"], None),
            }),
        )
        .await;
    let token = string_at(&created, &["UpdateToken"]).to_string();

    let rows: Vec<RefusedRow> = vec![
        (
            "the-default-member",
            vec!["aws:forward_to_stateful"],
            vec!["aws:drop"],
            "StatelessDefaultActions",
            "aws:forward_to_stateful",
        ),
        (
            "the-fragment-member",
            vec!["aws:pass"],
            vec!["aws:alert"],
            "StatelessFragmentDefaultActions",
            "aws:alert",
        ),
        (
            "an-undeclared-custom-action",
            vec!["aws:pass", "NeverDeclared"],
            vec!["aws:drop"],
            "StatelessDefaultActions",
            "NeverDeclared",
        ),
    ];

    for (case, actions, fragment_actions, member, offending) in &rows {
        let (status, body) = harness
            .call(
                "UpdateFirewallPolicy",
                &json!({
                    "FirewallPolicyName": "updated-policy",
                    "UpdateToken": token.clone(),
                    "FirewallPolicy": document(actions, fragment_actions, None),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {case}: expected 400 InvalidRequestException, got {status} {body:?}"
        );
        let message = error_message(&body);
        let expected = format!("{member} is invalid, parameter: [{offending}], context: {member}");
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );

        let described = harness
            .ok(
                "DescribeFirewallPolicy",
                &json!({ "FirewallPolicyName": "updated-policy" }),
            )
            .await;
        assert_eq!(
            string_at(&described, &["UpdateToken"]),
            token,
            "case {case}: a refused update must not rotate the token, got {described:?}"
        );
        for (member, expected) in [
            ("StatelessDefaultActions", "aws:pass"),
            ("StatelessFragmentDefaultActions", "aws:drop"),
        ] {
            let reported = described_actions(&harness, "updated-policy", member).await;
            assert_eq!(
                reported,
                vec![expected.to_string()],
                "case {case}: a refused update must leave {member} as it was, got {reported:?}"
            );
        }
    }

    // The accepted update goes through the same check and changes the document.
    harness
        .ok(
            "UpdateFirewallPolicy",
            &json!({
                "FirewallPolicyName": "updated-policy",
                "UpdateToken": token.clone(),
                "FirewallPolicy": document(
                    &["aws:drop", "AcceptedAction"],
                    &["aws:pass"],
                    Some("AcceptedAction"),
                ),
            }),
        )
        .await;
    let reported = described_actions(&harness, "updated-policy", "StatelessDefaultActions").await;
    assert_eq!(
        reported,
        vec!["aws:drop".to_string(), "AcceptedAction".to_string()],
        "case accepted update: expected the new actions to be stored, got {reported:?}"
    );
}

/// RULE: a stateless default-action list specifies ONE standard action, so a
/// list naming two different standard actions and a list naming none at all are
/// both refused with HTTP 400 / `InvalidRequestException`: the first naming
/// every standard action it carries in request order, the second naming only the
/// member it looked at — and in both cases nothing is stored.
///
/// Rows span both action members, every pair of the three standard actions, a
/// list of all three, a pair spelled in different cases, a pair beside a
/// declared custom action, and the empty list in each member.
#[tokio::test]
async fn a_list_must_specify_exactly_one_standard_action() {
    let harness = Harness::new();

    // (case, default actions, fragment actions, the message the refusal carries)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>, String)> = vec![
        (
            "pass-and-drop",
            vec!["aws:pass", "aws:drop"],
            vec!["aws:drop"],
            exist_together("StatelessDefaultActions", "aws:pass, aws:drop"),
        ),
        (
            "forward-and-pass",
            vec!["aws:forward_to_sfe", "aws:pass"],
            vec!["aws:drop"],
            exist_together(
                "StatelessDefaultActions",
                "aws:forward_to_sfe, aws:pass",
            ),
        ),
        (
            "drop-and-forward",
            vec!["aws:drop", "aws:forward_to_sfe"],
            vec!["aws:drop"],
            exist_together("StatelessDefaultActions", "aws:drop, aws:forward_to_sfe"),
        ),
        (
            "all-three",
            vec!["aws:pass", "aws:drop", "aws:forward_to_sfe"],
            vec!["aws:pass"],
            exist_together(
                "StatelessDefaultActions",
                "aws:pass, aws:drop, aws:forward_to_sfe",
            ),
        ),
        (
            "two-in-different-cases",
            vec!["AWS:PASS", "aws:drop"],
            vec!["aws:pass"],
            exist_together("StatelessDefaultActions", "AWS:PASS, aws:drop"),
        ),
        (
            "two-beside-a-declared-custom-action",
            vec!["aws:pass", "DeclaredHere", "aws:drop"],
            vec!["aws:pass"],
            exist_together("StatelessDefaultActions", "aws:pass, aws:drop"),
        ),
        (
            "two-in-the-fragment-member",
            vec!["aws:pass"],
            vec!["aws:drop", "aws:pass"],
            exist_together(
                "StatelessFragmentDefaultActions",
                "aws:drop, aws:pass",
            ),
        ),
        (
            "an-empty-default-member",
            vec![],
            vec!["aws:drop"],
            null_or_empty("StatelessDefaultActions"),
        ),
        (
            "an-empty-fragment-member",
            vec!["aws:pass"],
            vec![],
            null_or_empty("StatelessFragmentDefaultActions"),
        ),
        (
            "both-members-empty",
            vec![],
            vec![],
            null_or_empty("StatelessDefaultActions"),
        ),
    ];

    for (case, actions, fragment_actions, expected) in &rows {
        let custom = actions
            .iter()
            .chain(fragment_actions.iter())
            .any(|entry| *entry == "DeclaredHere")
            .then_some("DeclaredHere");
        let name = format!("one-standard-{case}");
        let (status, body) = harness
            .call(
                "CreateFirewallPolicy",
                &json!({
                    "FirewallPolicyName": name,
                    "FirewallPolicy": document(actions, fragment_actions, custom),
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {case}: expected 400 InvalidRequestException, got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            &message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );

        // Nothing was stored, so the name a refused create used is still free.
        let (status, body) = harness
            .call(
                "DescribeFirewallPolicy",
                &json!({ "FirewallPolicyName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case}: expected the refused name to stay free, got {status} {body:?}"
        );
    }
}

/// The message a list naming more than one standard action is refused with.
fn exist_together(member: &str, actions: &str) -> String {
    format!("{member} cannot exist together, parameter: [{actions}], context: {member}")
}

/// The message a list that names no action at all is refused with.
fn null_or_empty(member: &str) -> String {
    format!("{member} cannot be null or empty, context: {member}")
}

/// RULE: a create under a name that is already taken is refused with
/// `InvalidRequestException` — the class for "a problem with your request" — and
/// not with `InvalidOperationException`, which reports an in-use/state conflict;
/// the resource that holds the name survives untouched.
///
/// Rows span every resource this service creates by name: a firewall policy, a
/// firewall, and both halves of the rule-group namespace.
#[tokio::test]
async fn a_name_already_taken_is_refused_as_an_invalid_request() {
    let harness = Harness::new();
    let policy_arn = harness.shared_policy("dup-class-policy-for-firewall").await;

    let stateful_group = json!({ "RulesSource": { "StatefulRules": [{
        "Action": "PASS",
        "Header": {
            "Protocol": "TCP",
            "Source": "ANY",
            "SourcePort": "ANY",
            "Direction": "ANY",
            "Destination": "ANY",
            "DestinationPort": "ANY",
        },
        "RuleOptions": [{ "Keyword": "sid", "Settings": ["1"] }],
    }]}});
    let stateless_group = json!({ "RulesSource": { "StatelessRulesAndCustomActions": {
        "StatelessRules": [{
            "Priority": 1,
            "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
        }],
    }}});

    let rows: Vec<DuplicateRow> = vec![
        (
            "firewall policy",
            "CreateFirewallPolicy",
            json!({
                "FirewallPolicyName": "dup-class-policy",
                "FirewallPolicy": minimal_policy_document(),
            }),
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyName": "dup-class-policy" }),
            vec!["FirewallPolicyResponse", "FirewallPolicyArn"],
        ),
        (
            "firewall",
            "CreateFirewall",
            json!({
                "FirewallName": "dup-class-firewall",
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-dup-class" }],
            }),
            "DescribeFirewall",
            json!({ "FirewallName": "dup-class-firewall" }),
            vec!["Firewall", "FirewallArn"],
        ),
        (
            "stateful rule group",
            "CreateRuleGroup",
            json!({
                "RuleGroupName": "dup-class-stateful",
                "Type": "STATEFUL",
                "Capacity": 10,
                "RuleGroup": stateful_group,
            }),
            "DescribeRuleGroup",
            json!({ "RuleGroupName": "dup-class-stateful", "Type": "STATEFUL" }),
            vec!["RuleGroupResponse", "RuleGroupArn"],
        ),
        (
            "stateless rule group",
            "CreateRuleGroup",
            json!({
                "RuleGroupName": "dup-class-stateless",
                "Type": "STATELESS",
                "Capacity": 10,
                "RuleGroup": stateless_group,
            }),
            "DescribeRuleGroup",
            json!({ "RuleGroupName": "dup-class-stateless", "Type": "STATELESS" }),
            vec!["RuleGroupResponse", "RuleGroupArn"],
        ),
    ];

    for (case, target, request, reader, selector, arn_path) in &rows {
        let created = harness.ok(target, request).await;
        let first_arn = string_at(&created, arn_path).to_string();

        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {case}: expected the repeat create to answer 400 InvalidRequestException, \
             got {status} {body:?}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, ALREADY_EXISTS,
            "case {case}: expected {ALREADY_EXISTS:?}, got {message:?}"
        );

        // The resource holding the name is the one the FIRST create minted.
        let described = harness.ok(reader, selector).await;
        let reported = string_at(&described, arn_path);
        assert_eq!(
            reported, first_arn,
            "case {case}: expected the stored resource to keep the ARN of the first create \
             ({first_arn}), got {reported}"
        );
    }
}
