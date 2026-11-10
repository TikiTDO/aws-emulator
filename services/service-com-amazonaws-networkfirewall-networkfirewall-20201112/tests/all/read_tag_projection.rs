// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TAG COLLECTION a response that projects a stored
//! resource reports.
//!
//! One rule: a describe and a delete answer from the stored record, so each
//! reports that resource's tag collection as a LIST — the empty list when the
//! resource carries no tags — while a create and an update report the `Tags`
//! member as the request gave it, absent when the request gave none. A client
//! that iterates the member a read returns therefore never meets a missing
//! member.
//!
//! The tests are tables: the rows are the resource kinds this service tags and
//! the tag sets a resource passes through, and every assertion names its row and
//! the actual value. Everything is driven over the wire through the shared
//! [`Harness`].

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{minimal_policy_document, string_at, Harness};
use crate::resource_tags::{tag_map, tag_request, taggable_kinds};

/// The `Tags` member a body carries at `path`, or `None` when the member is
/// absent there.
fn tags_at<'a>(body: &'a Value, path: &[&str]) -> Option<&'a Value> {
    let mut cursor = body;
    for step in path {
        cursor = cursor.get(step)?;
    }
    Some(cursor)
}

/// The tag set a read reports at `path`, which must be a LIST and not an absent
/// member.
fn projected_tags(case: &str, body: &Value, path: &[&str]) -> BTreeMap<String, String> {
    let tags = tags_at(body, path).unwrap_or_else(|| {
        panic!("case {case}: expected a Tags list at {path:?}, got {body:?}");
    });
    assert!(
        tags.is_array(),
        "case {case}: expected Tags at {path:?} to be a list, got {tags:?}"
    );
    tag_map(tags)
}

/// One step of the tag walk: the step's name, the tag write that reaches it (a
/// `TagResource` or `UntagResource` body, or nothing for the state the resource
/// starts in), and the tags the resource then carries.
type TagStep<'a> = (&'a str, Option<Value>, Vec<(&'a str, &'a str)>);

/// RULE: a read of a resource reports its tag collection as a list — the empty
/// list when the resource carries no tags — for every resource kind this service
/// tags.
///
/// The rows are the four taggable kinds, each walked through the tag sets a
/// resource passes through: never tagged, one tag, several tags, and emptied
/// again by `UntagResource`, so the empty list is asserted both before any tag
/// write and after the writes removed them all.
#[tokio::test]
async fn a_read_reports_an_untagged_resource_s_tags_as_an_empty_list() {
    let harness = Harness::new();

    for kind in taggable_kinds(&harness, "read-projection").await {
        let steps: Vec<TagStep> = vec![
            ("never-tagged", None, vec![]),
            (
                "one-tag",
                Some(tag_request(
                    &kind.arn,
                    &[("env".to_string(), "prod".to_string())],
                )),
                vec![("env", "prod")],
            ),
            (
                "three-tags",
                Some(tag_request(
                    &kind.arn,
                    &[
                        ("team".to_string(), "net".to_string()),
                        ("zone".to_string(), "edge".to_string()),
                    ],
                )),
                vec![("env", "prod"), ("team", "net"), ("zone", "edge")],
            ),
            (
                "emptied-again",
                Some(json!({
                    "ResourceArn": kind.arn,
                    "TagKeys": ["env", "team", "zone"],
                })),
                vec![],
            ),
        ];

        for (step, write, expected) in steps {
            if let Some(request) = &write {
                let target = if request.get("TagKeys").is_some() {
                    "UntagResource"
                } else {
                    "TagResource"
                };
                harness.ok(target, request).await;
            }
            let case = format!("{}/{step}", kind.label);
            let described = harness.ok(kind.describe, &kind.describe_request).await;
            let reported = projected_tags(&case, &described, kind.tags_path);
            let wanted: BTreeMap<String, String> = expected
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            assert_eq!(
                reported, wanted,
                "case {case}: expected {} to report {wanted:?}, got {reported:?}",
                kind.describe
            );

            // A read reports the same collection twice: projecting it changes
            // nothing a later caller sees.
            let again = harness.ok(kind.describe, &kind.describe_request).await;
            let again = projected_tags(&case, &again, kind.tags_path);
            assert_eq!(
                again, reported,
                "case {case}: a read must not change what the next read reports, got {again:?}"
            );
        }
    }
}

/// RULE: the same read reports the member as a LIST whatever the CREATE reported
/// — the create echoes the request's own optional `Tags` member, absent when the
/// request named none, and the read that follows projects the stored record.
///
/// The rows differ in how many tags the create request carried, the empty row
/// being the one that separates "no tags" from "tags not reported", and span
/// both of the metadata structures a create mints beside the firewall's own.
#[tokio::test]
async fn a_create_echoes_the_requested_tags_and_the_read_that_follows_reports_a_list() {
    let harness = Harness::new();

    let rows: [(&str, &[(&str, &str)]); 4] = [
        ("untagged", &[]),
        ("one-tag", &[("env", "prod")]),
        ("two-tags", &[("env", "staging"), ("team", "net")]),
        ("three-tags", &[("a", "1"), ("b", "2"), ("c", "3")]),
    ];

    for (label, pairs) in rows {
        let requested: Vec<Value> = pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect();
        let wanted: BTreeMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();

        let policy_name = format!("echo-policy-{label}");
        let mut policy_request = json!({
            "FirewallPolicyName": policy_name,
            "FirewallPolicy": minimal_policy_document(),
        });
        let group_name = format!("echo-rule-group-{label}");
        let mut group_request = json!({
            "RuleGroupName": group_name,
            "Type": "STATELESS",
            "Capacity": 10,
            "RuleGroup": { "RulesSource": { "StatelessRulesAndCustomActions": {
                "StatelessRules": [{
                    "Priority": 1,
                    "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
                }],
            }}},
        });
        if !requested.is_empty() {
            policy_request["Tags"] = json!(requested);
            group_request["Tags"] = json!(requested);
        }

        let created_policy = harness.ok("CreateFirewallPolicy", &policy_request).await;
        let created_group = harness.ok("CreateRuleGroup", &group_request).await;
        let policy_arn =
            string_at(&created_policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

        let firewall_name = format!("echo-firewall-{label}");
        let mut firewall_request = json!({
            "FirewallName": firewall_name,
            "FirewallPolicyArn": policy_arn,
            "VpcId": "vpc-0123abcd",
            "SubnetMappings": [{ "SubnetId": format!("subnet-echo-{label}") }],
        });
        if !requested.is_empty() {
            firewall_request["Tags"] = json!(requested);
        }
        let created_firewall = harness.ok("CreateFirewall", &firewall_request).await;

        // (what the create minted, where its Tags sit, the read of it and its
        // request)
        let projections: [(&str, &Value, &[&str], &str, Value); 3] = [
            (
                "firewall-policy",
                &created_policy,
                &["FirewallPolicyResponse", "Tags"],
                "DescribeFirewallPolicy",
                json!({ "FirewallPolicyName": format!("echo-policy-{label}") }),
            ),
            (
                "rule-group",
                &created_group,
                &["RuleGroupResponse", "Tags"],
                "DescribeRuleGroup",
                json!({ "RuleGroupName": format!("echo-rule-group-{label}"), "Type": "STATELESS" }),
            ),
            (
                "firewall",
                &created_firewall,
                &["Firewall", "Tags"],
                "DescribeFirewall",
                json!({ "FirewallName": format!("echo-firewall-{label}") }),
            ),
        ];

        for (kind, created, path, describe, selector) in projections {
            let case = format!("{kind}/{label}");
            // The create reports the member exactly as the request gave it.
            let echoed = tags_at(created, path);
            if requested.is_empty() {
                assert_eq!(
                    echoed, None,
                    "case {case}: expected the create to carry no Tags member, got {created:?}"
                );
            } else {
                assert_eq!(
                    echoed,
                    Some(&json!(requested)),
                    "case {case}: expected the create to echo the request's Tags, got {created:?}"
                );
            }

            // The read projects the stored record, so the member is a list even
            // where the create carried none.
            let described = harness.ok(describe, &selector).await;
            let reported = projected_tags(&case, &described, path);
            assert_eq!(
                reported, wanted,
                "case {case}: expected {describe} to report {wanted:?} as a list, got \
                 {reported:?}"
            );
        }
    }
}
