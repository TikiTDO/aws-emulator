// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the capacity a firewall policy reserves for the rule
//! groups it references, and for resource policies on a shared RULE GROUP.
//!
//! The resource-policy tests for a firewall policy, and the policy-document
//! validation every resource kind shares, live in `resource_policies`.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, minimal_policy_document, resource_policy_document, string_at, with_tag_list, Harness,
};

/// RULE: referencing a rule group from a firewall policy reserves that rule
/// group's capacity for the policy, which is what the policy's `Consumed*`
/// members report: the declared `Capacity` of a stateless or a stateful rule
/// group, and the domain name specifications a domain list rule group consumes.
///
/// `CreateRuleGroupRequest$Capacity` states the reservation ("when you reference
/// a rule group from a firewall policy, Network Firewall reserves this capacity
/// for the rule group") and `FirewallPolicyResponse$ConsumedStatefulDomainCapacity`
/// states the domain unit ("the total number of domain name specifications across
/// all domain list rule groups in the firewall policy").
#[tokio::test]
async fn a_policy_reports_the_capacity_the_rule_groups_it_references_reserve() {
    let harness = Harness::new();

    // Three rule groups, each declaring more capacity than its rules consume, so
    // a reserved total can never be confused for a consumed one.
    let stateless_arn = harness
        .shared_rule_group("reserved-stateless", "STATELESS", 20)
        .await;
    let stateful_arn = harness
        .shared_rule_group("reserved-stateful", "STATEFUL", 10)
        .await;
    // A domain list: three domain name specifications, which is what
    // `ConsumedStatefulDomainCapacity` counts.
    let domain_arn = string_at(
        &harness
            .ok(
                "CreateRuleGroup",
                &json!({
                    "RuleGroupName": "reserved-domain",
                    "Type": "STATEFUL_DOMAIN",
                    "Capacity": 90,
                    "RuleGroup": { "RulesSource": { "RulesSourceList": {
                        "Targets": [".one.example", ".two.example", "three.example"],
                        "TargetTypes": ["TLS_SNI"],
                        "GeneratedRulesType": "ALLOWLIST",
                    }}},
                }),
            )
            .await,
        &["RuleGroupResponse", "RuleGroupArn"],
    )
    .to_string();

    // Rows: which rule groups the policy references, and the units each
    // `Consumed*` member must then report.
    let cases: [(&str, Vec<&str>, [i64; 3]); 5] = [
        ("none", vec![], [0, 0, 0]),
        ("stateless-only", vec![&stateless_arn], [20, 0, 0]),
        ("stateful-only", vec![&stateful_arn], [0, 10, 0]),
        ("domain-only", vec![&domain_arn], [0, 0, 3]),
        (
            "all-three",
            vec![&stateless_arn, &stateful_arn, &domain_arn],
            [20, 10, 3],
        ),
    ];
    for (label, references, expected) in cases {
        let mut document = minimal_policy_document();
        let stateless_references: Vec<Value> = references
            .iter()
            .filter(|arn| arn.contains(":stateless-rulegroup/"))
            .enumerate()
            .map(|(index, arn)| json!({ "ResourceArn": arn, "Priority": index + 1 }))
            .collect();
        let stateful_references: Vec<Value> = references
            .iter()
            .filter(|arn| arn.contains(":stateful-rulegroup/"))
            .map(|arn| json!({ "ResourceArn": arn }))
            .collect();
        if !stateless_references.is_empty() {
            document["StatelessRuleGroupReferences"] = json!(stateless_references);
        }
        if !stateful_references.is_empty() {
            document["StatefulRuleGroupReferences"] = json!(stateful_references);
        }

        let policy_name = format!("reserving-{label}");
        let created = harness
            .ok(
                "CreateFirewallPolicy",
                &json!({ "FirewallPolicyName": policy_name, "FirewallPolicy": document }),
            )
            .await;
        let reported = [
            ("ConsumedStatelessRuleCapacity", expected[0]),
            ("ConsumedStatefulRuleCapacity", expected[1]),
            ("ConsumedStatefulDomainCapacity", expected[2]),
        ];
        for (member, units) in reported {
            assert_eq!(
                created["FirewallPolicyResponse"][member],
                json!(units),
                "case {label}: expected {member} {units}, got {created:?}"
            );
        }

        // The read reports the same totals as the create.
        let described = harness
            .ok(
                "DescribeFirewallPolicy",
                &json!({ "FirewallPolicyName": policy_name }),
            )
            .await;
        assert_eq!(
            described["FirewallPolicyResponse"],
            with_tag_list(&created["FirewallPolicyResponse"]),
            "case {label}: expected the stored response, got {described:?}"
        );
    }
}

/// RULE: the actions a resource policy may grant are the ones the shared
/// resource's TYPE permits, and the test is CONTAINMENT: a rule group — whichever
/// of the rule-group ARN forms names it — accepts any non-empty subset of the
/// rule-group set and refuses a document naming an action outside it with
/// `InvalidResourcePolicyException` (HTTP 400), attaching nothing.
///
/// The permitted set is the one the AWS `PutResourcePolicy` documentation lists
/// for a rule group resource, held in `state::RULE_GROUP_POLICY_ACTIONS`.
#[tokio::test]
async fn only_the_permitted_action_set_is_attached_to_a_shared_rule_group() {
    let harness = Harness::new();
    let permitted: Vec<&str> = state::RULE_GROUP_POLICY_ACTIONS.to_vec();

    // One rule group per ARN form, so the action set is asserted per resource
    // TYPE and not per ARN.
    let resources: [(&str, String); 2] = [
        (
            "stateful",
            harness
                .shared_rule_group("shared-stateful", "STATEFUL", 10)
                .await,
        ),
        (
            "stateless",
            harness
                .shared_rule_group("shared-stateless", "STATELESS", 10)
                .await,
        ),
    ];

    for (kind, arn) in &resources {
        let cases: [(&str, Vec<&str>, bool); 5] = [
            ("exact-set", permitted.clone(), true),
            (
                "reversed-set",
                permitted.iter().copied().rev().collect(),
                true,
            ),
            ("a-two-action-subset", permitted[..2].to_vec(), true),
            ("a-single-action-subset", vec![permitted[2]], true),
            (
                "with-the-firewall-policy-set",
                state::FIREWALL_POLICY_POLICY_ACTIONS.to_vec(),
                false,
            ),
        ];
        for (label, actions, accepted) in cases {
            let case = format!("{kind}/{label}");
            let document = resource_policy_document(&actions, arn);

            // Start each row from "nothing attached", so it is judged on its own.
            let _ = harness
                .call("DeleteResourcePolicy", &json!({ "ResourceArn": arn }))
                .await;

            let (status, body) = harness
                .call(
                    "PutResourcePolicy",
                    &json!({ "ResourceArn": arn, "Policy": document }),
                )
                .await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case}: the permitted action set must be accepted, got {status} {body:?}"
                );
                let described = harness
                    .ok("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                    .await;
                assert_eq!(
                    described["Policy"],
                    json!(document),
                    "case {case}: expected the attached document, got {described:?}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidResourcePolicyException"),
                    "case {case}: expected InvalidResourcePolicyException, got {status} {body:?}"
                );
                let (status, body) = harness
                    .call("DescribeResourcePolicy", &json!({ "ResourceArn": arn }))
                    .await;
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "ResourceNotFoundException"),
                    "case {case}: a refused policy attaches nothing, got {status} {body:?}"
                );
            }
        }
    }
}
