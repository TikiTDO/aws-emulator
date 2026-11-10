// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the firewall-policy + firewall lifecycle slice.
//!
//! Every test drives the service the way a client does: an awsJson1_0 `POST /`
//! with `X-Amz-Target: NetworkFirewall_20201112.<OperationName>` through the same
//! router the emulator boots, so what is asserted is the WIRE behaviour and not a
//! handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.
//!
//! The harness, the fixtures and the response readers live in `crate::harness`,
//! which every suite in this test crate drives the service through.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

use crate::harness::{
    error_code, is_uuid_form, minimal_policy_document as minimal_policy, string_at, with_tag_list,
    Harness, ACCOUNT, REGION,
};

/// One row of the create -> describe round-trip table.
#[derive(Debug)]
struct RoundTripCase {
    firewall_name: &'static str,
    vpc_id: &'static str,
    subnets: &'static [&'static str],
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
    delete_protection: bool,
    subnet_change_protection: bool,
    firewall_policy_change_protection: bool,
}

/// RULE: a create persists the resource, and a later read of the SAME resource
/// reports exactly what was stored — the server-generated id and ARN included —
/// whichever identifier the reader resolves it by.
///
/// The rows vary the members a firewall carries: subnet count, whether a
/// description is given, how many tags, and each protection flag, so the
/// assertion is about the round trip and not about one request's values.
#[tokio::test]
async fn a_created_firewall_reads_back_identically_by_name_and_by_arn() {
    let cases = [
        RoundTripCase {
            firewall_name: "single-subnet",
            vpc_id: "vpc-0123abcd",
            subnets: &["subnet-0123abcd"],
            description: None,
            tags: &[],
            delete_protection: false,
            subnet_change_protection: false,
            firewall_policy_change_protection: false,
        },
        RoundTripCase {
            firewall_name: "three-subnets-described",
            vpc_id: "vpc-beef",
            subnets: &["subnet-aaa1", "subnet-bbb2", "subnet-ccc3"],
            description: Some("edge inspection"),
            tags: &[("env", "prod")],
            delete_protection: true,
            subnet_change_protection: false,
            firewall_policy_change_protection: true,
        },
        RoundTripCase {
            firewall_name: "all-protections-many-tags",
            vpc_id: "vpc-00ff00ff",
            subnets: &["subnet-1", "subnet-22", "subnet-333", "subnet-4444"],
            description: Some(""),
            tags: &[("a", "1"), ("b", "2"), ("c", "3")],
            delete_protection: true,
            subnet_change_protection: true,
            firewall_policy_change_protection: true,
        },
    ];

    let harness = Harness::new();
    let policy = harness.create_policy("round-trip-policy").await;
    let policy_arn = string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

    for case in &cases {
        let mut request = json!({
            "FirewallName": case.firewall_name,
            "FirewallPolicyArn": policy_arn,
            "VpcId": case.vpc_id,
            "SubnetMappings": case.subnets.iter().map(|id| json!({ "SubnetId": id })).collect::<Vec<_>>(),
            "DeleteProtection": case.delete_protection,
            "SubnetChangeProtection": case.subnet_change_protection,
            "FirewallPolicyChangeProtection": case.firewall_policy_change_protection,
        });
        if let Some(description) = case.description {
            request["Description"] = json!(description);
        }
        if !case.tags.is_empty() {
            request["Tags"] = case
                .tags
                .iter()
                .map(|(key, value)| json!({ "Key": key, "Value": value }))
                .collect::<Vec<_>>()
                .into();
        }

        let created = harness.ok("CreateFirewall", &request).await;
        let created_firewall = created
            .get("Firewall")
            .unwrap_or_else(|| panic!("case {case:?}: expected Firewall in {created:?}"));

        let expected_arn = format!(
            "arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/{}",
            case.firewall_name
        );
        assert_eq!(
            string_at(created_firewall, &["FirewallArn"]),
            expected_arn,
            "case {case:?}: expected the firewall ARN to be built from the request's region, \
             got {created_firewall:?}"
        );
        let firewall_id = string_at(created_firewall, &["FirewallId"]).to_string();
        assert!(
            is_uuid_form(&firewall_id),
            "case {case:?}: expected FirewallId in the modelled UUID form, got {firewall_id:?}"
        );
        assert_eq!(
            created_firewall.get("SubnetMappings").map(Value::as_array),
            Some(request["SubnetMappings"].as_array()),
            "case {case:?}: expected SubnetMappings to round-trip, got {created_firewall:?}"
        );

        // One SyncStates entry per Availability Zone, i.e. per firewall subnet.
        let sync_states = created
            .get("FirewallStatus")
            .and_then(|status| status.get("SyncStates"))
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: expected SyncStates in {created:?}"));
        assert_eq!(
            sync_states.len(),
            case.subnets.len(),
            "case {case:?}: expected one SyncStates entry per subnet, got {sync_states:?}"
        );

        // Read back by NAME and by ARN; both must report the stored record.
        for by in [
            json!({ "FirewallName": case.firewall_name }),
            json!({ "FirewallArn": expected_arn }),
        ] {
            let described = harness.ok("DescribeFirewall", &by).await;
            let described_firewall = described
                .get("Firewall")
                .unwrap_or_else(|| panic!("case {case:?}: expected Firewall in {described:?}"));
            assert_eq!(
                described_firewall,
                &with_tag_list(created_firewall),
                "case {case:?} resolved by {by:?}: expected the describe to report the created \
                 firewall, got {described_firewall:?}"
            );
            let token = string_at(&described, &["UpdateToken"]).to_string();
            assert!(
                is_uuid_form(&token),
                "case {case:?} resolved by {by:?}: expected UpdateToken in the modelled UUID \
                 form, got {token:?}"
            );
        }
    }

    // Every firewall above associated itself with the one policy, so the policy's
    // association count is the number of firewalls that reference it.
    let described_policy = harness
        .ok(
            "DescribeFirewallPolicy",
            &json!({ "FirewallPolicyName": "round-trip-policy" }),
        )
        .await;
    assert_eq!(
        described_policy["FirewallPolicyResponse"]["NumberOfAssociations"],
        json!(cases.len()),
        "expected NumberOfAssociations to count the associated firewalls, got \
         {described_policy:?}"
    );
}

/// RULE: a create persists the policy document and its server-generated identity,
/// and `DescribeFirewallPolicy` reports both back unchanged, resolved by name or
/// ARN.
///
/// Rows vary the policy document (default actions, a stateful default action, a
/// custom action), the description and the tags.
#[tokio::test]
async fn a_created_firewall_policy_reads_back_identically() {
    let cases: [(&str, Value, Option<&str>); 3] = [
        ("bare", minimal_policy(), None),
        (
            "with-stateful-defaults",
            json!({
                "StatelessDefaultActions": ["aws:pass"],
                "StatelessFragmentDefaultActions": ["aws:drop"],
                "StatefulDefaultActions": ["aws:drop_strict"],
            }),
            Some("strict"),
        ),
        (
            "with-custom-action",
            json!({
                "StatelessDefaultActions": ["aws:forward_to_sfe", "CustomMetric"],
                "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
                "StatelessCustomActions": [{
                    "ActionName": "CustomMetric",
                    "ActionDefinition": {
                        "PublishMetricAction": { "Dimensions": [{ "Value": "sfe" }] }
                    }
                }],
            }),
            Some(""),
        ),
    ];

    let harness = Harness::new();
    for (name, document, description) in &cases {
        let mut request = json!({ "FirewallPolicyName": name, "FirewallPolicy": document });
        if let Some(description) = description {
            request["Description"] = json!(description);
        }
        let created = harness.ok("CreateFirewallPolicy", &request).await;

        let expected_arn =
            format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/{name}");
        assert_eq!(
            string_at(&created, &["FirewallPolicyResponse", "FirewallPolicyArn"]),
            expected_arn,
            "case {name}: expected the policy ARN to end in firewall-policy/{name}, got {created:?}"
        );
        let policy_id = string_at(&created, &["FirewallPolicyResponse", "FirewallPolicyId"]);
        assert!(
            is_uuid_form(policy_id),
            "case {name}: expected FirewallPolicyId in the modelled UUID form, got {policy_id:?}"
        );
        assert_eq!(
            created["FirewallPolicyResponse"]["NumberOfAssociations"],
            json!(0),
            "case {name}: a brand new policy has no associations, got {created:?}"
        );
        assert_eq!(
            created["FirewallPolicyResponse"]["FirewallPolicyStatus"],
            json!("ACTIVE"),
            "case {name}: expected an ACTIVE policy, got {created:?}"
        );

        for by in [
            json!({ "FirewallPolicyName": name }),
            json!({ "FirewallPolicyArn": expected_arn }),
        ] {
            let described = harness.ok("DescribeFirewallPolicy", &by).await;
            assert_eq!(
                described["FirewallPolicyResponse"],
                with_tag_list(&created["FirewallPolicyResponse"]),
                "case {name} resolved by {by:?}: expected the stored response, got {described:?}"
            );
            assert_eq!(
                described["FirewallPolicy"], *document,
                "case {name} resolved by {by:?}: expected the stored policy document, got \
                 {described:?}"
            );
            assert_eq!(
                described["UpdateToken"], created["UpdateToken"],
                "case {name} resolved by {by:?}: the UpdateToken changes only on mutation, got \
                 {described:?}"
            );
        }
    }
}

/// RULE: `DryRun` asks the service to check the request instead of running it, so
/// nothing is persisted and the name stays free.
#[tokio::test]
async fn a_dry_run_create_persists_nothing() {
    let harness = Harness::new();
    for name in ["dry-one", "dry-two"] {
        let dry = harness
            .ok(
                "CreateFirewallPolicy",
                &json!({
                    "FirewallPolicyName": name,
                    "FirewallPolicy": minimal_policy(),
                    "DryRun": true,
                }),
            )
            .await;
        assert!(
            is_uuid_form(string_at(
                &dry,
                &["FirewallPolicyResponse", "FirewallPolicyId"]
            )),
            "case {name}: a dry run still answers a well-formed response, got {dry:?}"
        );

        let (status, body) = harness
            .call(
                "DescribeFirewallPolicy",
                &json!({ "FirewallPolicyName": name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {name}: a dry run must not persist the policy, got {status} {body:?}"
        );

        // The name is still free, so the real create succeeds.
        let real = harness.create_policy(name).await;
        assert_eq!(
            string_at(&real, &["FirewallPolicyResponse", "FirewallPolicyName"]),
            name,
            "case {name}: expected the name to still be available, got {real:?}"
        );
    }

    let listed = harness
        .ok("ListFirewallPolicies", &json!({ "MaxResults": 100 }))
        .await;
    assert_eq!(
        listed["FirewallPolicies"].as_array().map(Vec::len),
        Some(2),
        "expected only the two really-created policies, got {listed:?}"
    );
}

/// RULE: creating a resource whose name is already taken is refused, and the
/// stored resource is left exactly as it was.
#[tokio::test]
async fn a_duplicate_name_is_refused_and_does_not_overwrite() {
    let harness = Harness::new();
    let first = harness.create_policy("taken").await;
    let first_arn = string_at(&first, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    let first_id = string_at(&first, &["FirewallPolicyResponse", "FirewallPolicyId"]).to_string();

    let (status, body) = harness
        .call(
            "CreateFirewallPolicy",
            &json!({ "FirewallPolicyName": "taken", "FirewallPolicy": minimal_policy() }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case policy: expected a duplicate name to be refused, got {status} {body:?}"
    );

    let described = harness
        .ok(
            "DescribeFirewallPolicy",
            &json!({ "FirewallPolicyName": "taken" }),
        )
        .await;
    assert_eq!(
        (
            string_at(&described, &["FirewallPolicyResponse", "FirewallPolicyArn"]),
            string_at(&described, &["FirewallPolicyResponse", "FirewallPolicyId"]),
        ),
        (first_arn.as_str(), first_id.as_str()),
        "case policy: the refused create must not replace the stored policy, got {described:?}"
    );

    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": "taken-fw",
                "FirewallPolicyArn": first_arn,
                "VpcId": "vpc-1",
                "SubnetMappings": [{ "SubnetId": "subnet-1" }],
            }),
        )
        .await;
    let (status, body) = harness
        .call(
            "CreateFirewall",
            &json!({
                "FirewallName": "taken-fw",
                "FirewallPolicyArn": first_arn,
                "VpcId": "vpc-2",
                "SubnetMappings": [{ "SubnetId": "subnet-2" }],
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case firewall: expected a duplicate name to be refused, got {status} {body:?}"
    );
    let described = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "taken-fw" }))
        .await;
    assert_eq!(
        described["Firewall"]["VpcId"],
        json!("vpc-1"),
        "case firewall: the refused create must not replace the stored firewall, got {described:?}"
    );
}

/// RULE: an update writes through to the stored resource and re-mints its
/// `UpdateToken`; a read never changes it.
///
/// Rows span the `Description` value class: a word, the empty string, the longest
/// value the modelled `Description` length (0..512) admits, and omitting the
/// member, which clears it.
#[tokio::test]
async fn updating_the_description_writes_through_and_rotates_the_token() {
    let long = "d".repeat(512);
    let cases: [Option<&str>; 4] = [Some("edited"), Some(""), Some(long.as_str()), None];

    let harness = Harness::new();
    let policy = harness.create_policy("update-policy").await;
    let policy_arn = string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": "update-fw",
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
                "Description": "original",
            }),
        )
        .await;

    for case in &cases {
        let before = harness
            .ok("DescribeFirewall", &json!({ "FirewallName": "update-fw" }))
            .await;
        let stale_token = string_at(&before, &["UpdateToken"]).to_string();

        // A read does not rotate the token.
        let again = harness
            .ok("DescribeFirewall", &json!({ "FirewallName": "update-fw" }))
            .await;
        assert_eq!(
            string_at(&again, &["UpdateToken"]),
            stale_token,
            "case {case:?}: a read must not change the UpdateToken, got {again:?}"
        );

        let mut request = json!({ "FirewallName": "update-fw", "UpdateToken": stale_token });
        if let Some(description) = case {
            request["Description"] = json!(description);
        }
        let updated = harness.ok("UpdateFirewallDescription", &request).await;
        let new_token = string_at(&updated, &["UpdateToken"]).to_string();
        assert!(
            is_uuid_form(&new_token) && new_token != stale_token,
            "case {case:?}: expected a fresh UpdateToken, got {new_token:?} (was {stale_token:?})"
        );

        let after = harness
            .ok("DescribeFirewall", &json!({ "FirewallName": "update-fw" }))
            .await;
        assert_eq!(
            after["Firewall"]["Description"],
            case.map_or(Value::Null, |description| json!(description)),
            "case {case:?}: expected the describe to show the new description, got {after:?}"
        );
        assert_eq!(
            string_at(&after, &["UpdateToken"]),
            new_token,
            "case {case:?}: expected the describe to report the new token, got {after:?}"
        );

        // Replaying the token that was current BEFORE the update is refused.
        let (status, body) = harness
            .call(
                "UpdateFirewallDescription",
                &json!({
                    "FirewallName": "update-fw",
                    "UpdateToken": stale_token,
                    "Description": "replayed",
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidTokenException"),
            "case {case:?}: expected a stale UpdateToken to be refused, got {status} {body:?}"
        );
    }
}

/// RULE: a paginated listing returns at most `MaxResults` items, a `NextToken`
/// exactly when more remain, and resuming from that token yields the rest with no
/// overlap and no omission.
///
/// Rows span the page size against the item count, including the published
/// `PaginationMaxResults` bounds (`range 1..100`, from
/// `smithy_query(query="shape",shape="PaginationMaxResults")`).
#[tokio::test]
async fn listing_pages_exactly_and_resumes_from_its_token() {
    let cases: [(usize, i64); 5] = [(5, 1), (5, 2), (5, 5), (5, 100), (1, 1)];

    for (total, page_size) in cases {
        let harness = Harness::new();
        let policy = harness.create_policy("page-policy").await;
        let policy_arn =
            string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
        let mut expected: Vec<String> = Vec::new();
        for index in 0..total {
            let name = format!("fw-{index:02}");
            harness
                .ok(
                    "CreateFirewall",
                    &json!({
                        "FirewallName": name,
                        "FirewallPolicyArn": policy_arn,
                        "VpcId": "vpc-0123abcd",
                        "SubnetMappings": [{ "SubnetId": format!("subnet-{index:04x}") }],
                    }),
                )
                .await;
            expected.push(name);
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &next_token {
                request["NextToken"] = json!(token);
            }
            let listed = harness.ok("ListFirewalls", &request).await;
            let page: Vec<String> = listed["Firewalls"]
                .as_array()
                .unwrap_or_else(|| panic!("case ({total}, {page_size}): expected Firewalls in {listed:?}"))
                .iter()
                .map(|entry| string_at(entry, &["FirewallName"]).to_string())
                .collect();
            assert!(
                page.len() <= usize::try_from(page_size).unwrap(),
                "case ({total}, {page_size}): a page may not exceed MaxResults, got {page:?}"
            );
            for name in &page {
                assert!(
                    !seen.contains(name),
                    "case ({total}, {page_size}): {name} appeared on two pages, seen {seen:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            next_token = listed
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let remaining = expected.len() - seen.len();
            assert_eq!(
                next_token.is_some(),
                remaining > 0,
                "case ({total}, {page_size}): NextToken must be present exactly when items \
                 remain; {remaining} remain and the token is {next_token:?}"
            );
            if next_token.is_none() {
                break;
            }
            assert!(
                pages <= total + 1,
                "case ({total}, {page_size}): pagination did not terminate, seen {seen:?}"
            );
        }

        seen.sort();
        assert_eq!(
            seen, expected,
            "case ({total}, {page_size}): expected every firewall exactly once"
        );
    }
}

/// RULE: `MaxResults` is a modelled range (`1..100`); a value outside it is
/// refused at the boundary rather than clamped.
#[tokio::test]
async fn a_max_results_outside_the_modelled_range_is_refused() {
    let harness = Harness::new();
    harness.create_policy("bounds-policy").await;

    for (max_results, inside) in [(1, true), (100, true), (0, false), (101, false)] {
        let (status, body) = harness
            .call("ListFirewalls", &json!({ "MaxResults": max_results }))
            .await;
        if inside {
            assert_eq!(
                status, 200,
                "case MaxResults={max_results}: a published bound is accepted, got {status} \
                 {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case MaxResults={max_results}: a value past the bound is refused, got {status} \
                 {body:?}"
            );
        }
    }
}

/// RULE: `VpcIds` narrows the listing to firewalls in those VPCs; omitting it (or
/// sending it empty) narrows nothing.
#[tokio::test]
async fn listing_filters_by_vpc_ids() {
    let placements = [
        ("fw-a", "vpc-00a"),
        ("fw-b", "vpc-00b"),
        ("fw-c", "vpc-00b"),
        ("fw-d", "vpc-00c"),
    ];

    let harness = Harness::new();
    let policy = harness.create_policy("filter-policy").await;
    let policy_arn = string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    for (index, (name, vpc_id)) in placements.iter().enumerate() {
        harness
            .ok(
                "CreateFirewall",
                &json!({
                    "FirewallName": name,
                    "FirewallPolicyArn": policy_arn,
                    "VpcId": vpc_id,
                    "SubnetMappings": [{ "SubnetId": format!("subnet-{index:04x}") }],
                }),
            )
            .await;
    }

    // `None` omits `VpcIds` entirely; `Some(..)` sends the member, including the
    // explicitly EMPTY list, which selects nothing to filter on and so narrows
    // nothing.
    let cases: [(Option<Vec<&str>>, Vec<&str>); 6] = [
        (None, vec!["fw-a", "fw-b", "fw-c", "fw-d"]),
        (Some(vec![]), vec!["fw-a", "fw-b", "fw-c", "fw-d"]),
        (Some(vec!["vpc-00a"]), vec!["fw-a"]),
        (Some(vec!["vpc-00b"]), vec!["fw-b", "fw-c"]),
        (Some(vec!["vpc-00a", "vpc-00c"]), vec!["fw-a", "fw-d"]),
        (Some(vec!["vpc-0999"]), vec![]),
    ];

    for (vpc_ids, expected) in &cases {
        let mut request = json!({ "MaxResults": 100 });
        if let Some(vpc_ids) = vpc_ids {
            request["VpcIds"] = json!(vpc_ids);
        }
        let listed = harness.ok("ListFirewalls", &request).await;
        let mut names: Vec<String> = listed["Firewalls"]
            .as_array()
            .unwrap_or_else(|| panic!("case {vpc_ids:?}: expected Firewalls in {listed:?}"))
            .iter()
            .map(|entry| string_at(entry, &["FirewallName"]).to_string())
            .collect();
        names.sort();
        assert_eq!(
            names, *expected,
            "case {vpc_ids:?}: expected only firewalls in those VPCs, got {listed:?}"
        );
    }
}

/// RULE: a delete removes the resource from state, so the next reader misses it
/// and the listing no longer carries it; and a resource still in use, or
/// protected against deletion, is refused.
#[tokio::test]
async fn a_delete_is_observable_by_the_next_reader() {
    let harness = Harness::new();
    let free_policy = harness.create_policy("delete-policy").await;
    let free_policy_arn =
        string_at(&free_policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    let held_policy = harness.create_policy("guard-policy").await;
    let held_policy_arn =
        string_at(&held_policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

    // Two firewalls on two policies: one plain, one delete-protected.
    for (name, policy_arn, delete_protection) in [
        ("plain-fw", &free_policy_arn, false),
        ("protected-fw", &held_policy_arn, true),
    ] {
        harness
            .ok(
                "CreateFirewall",
                &json!({
                    "FirewallName": name,
                    "FirewallPolicyArn": policy_arn,
                    "VpcId": "vpc-0123abcd",
                    "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
                    "DeleteProtection": delete_protection,
                }),
            )
            .await;
    }

    // A policy with associations cannot be deleted.
    for policy_name in ["delete-policy", "guard-policy"] {
        let (status, body) = harness
            .call(
                "DeleteFirewallPolicy",
                &json!({ "FirewallPolicyName": policy_name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidOperationException"),
            "case in-use policy {policy_name}: expected the delete to be refused, got {status} \
             {body:?}"
        );
    }

    // A delete-protected firewall cannot be deleted, and survives the attempt.
    let (status, body) = harness
        .call("DeleteFirewall", &json!({ "FirewallName": "protected-fw" }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "case protected firewall: expected the delete to be refused, got {status} {body:?}"
    );
    let survivor = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "protected-fw" }))
        .await;
    assert_eq!(
        survivor["Firewall"]["DeleteProtection"],
        json!(true),
        "case protected firewall: expected it to still be there, got {survivor:?}"
    );

    // Deleting the plain firewall releases its policy association.
    let deleted = harness
        .ok("DeleteFirewall", &json!({ "FirewallName": "plain-fw" }))
        .await;
    assert_eq!(
        deleted["FirewallStatus"]["Status"],
        json!("DELETING"),
        "case plain firewall: expected a DELETING status, got {deleted:?}"
    );
    let (status, body) = harness
        .call("DescribeFirewall", &json!({ "FirewallName": "plain-fw" }))
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case plain firewall: expected the next read to miss it, got {status} {body:?}"
    );
    let listed = harness
        .ok("ListFirewalls", &json!({ "MaxResults": 100 }))
        .await;
    let names: Vec<String> = listed["Firewalls"]
        .as_array()
        .expect("Firewalls")
        .iter()
        .map(|entry| string_at(entry, &["FirewallName"]).to_string())
        .collect();
    assert_eq!(
        names,
        vec!["protected-fw".to_string()],
        "case plain firewall: expected it to be gone from the listing, got {listed:?}"
    );

    // Its policy is now free, so the policy delete succeeds and is observable.
    let deleted_policy = harness
        .ok(
            "DeleteFirewallPolicy",
            &json!({ "FirewallPolicyName": "delete-policy" }),
        )
        .await;
    assert_eq!(
        deleted_policy["FirewallPolicyResponse"]["FirewallPolicyStatus"],
        json!("DELETING"),
        "case freed policy: expected a DELETING status, got {deleted_policy:?}"
    );
    let (status, body) = harness
        .call(
            "DescribeFirewallPolicy",
            &json!({ "FirewallPolicyName": "delete-policy" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case freed policy: expected the next read to miss it, got {status} {body:?}"
    );
    let listed_policies = harness
        .ok("ListFirewallPolicies", &json!({ "MaxResults": 100 }))
        .await;
    let policy_names: Vec<String> = listed_policies["FirewallPolicies"]
        .as_array()
        .expect("FirewallPolicies")
        .iter()
        .map(|entry| string_at(entry, &["Name"]).to_string())
        .collect();
    assert_eq!(
        policy_names,
        vec!["guard-policy".to_string()],
        "case freed policy: expected it to be gone from the listing, got {listed_policies:?}"
    );

    // The still-held policy is still refused: its firewall is still there.
    let (status, body) = harness
        .call(
            "DeleteFirewallPolicy",
            &json!({ "FirewallPolicyName": "guard-policy" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidOperationException"),
        "case held policy: expected the delete to stay refused, got {status} {body:?}"
    );
}

/// RULE: a firewall AWS returns in a non-terminal status reaches its terminal one
/// when the clock makes that transition due — not when something reads it.
#[tokio::test]
async fn a_new_firewall_provisions_then_becomes_ready() {
    let harness = Harness::new();
    let policy = harness.create_policy("ready-policy").await;
    let policy_arn = string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

    let created = harness
        .ok(
            "CreateFirewall",
            &json!({
                "FirewallName": "ready-fw",
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-aaa1" }, { "SubnetId": "subnet-bbb2" }],
            }),
        )
        .await;
    assert_eq!(
        created["FirewallStatus"]["Status"],
        json!("PROVISIONING"),
        "expected a brand new firewall to be PROVISIONING, got {created:?}"
    );
    assert_eq!(
        created["FirewallStatus"]["ConfigurationSyncStateSummary"],
        json!("IN_SYNC"),
        "expected the configuration to be IN_SYNC, got {created:?}"
    );

    // A caller acting straight after the create still sees the transient status.
    let immediate = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "ready-fw" }))
        .await;
    assert_eq!(
        immediate["FirewallStatus"]["Status"],
        json!("PROVISIONING"),
        "the provisioning span is never zero, got {immediate:?}"
    );

    tokio::time::sleep(state::FIREWALL_PROVISIONING_SPAN + Duration::from_millis(200)).await;

    let settled = harness
        .ok("DescribeFirewall", &json!({ "FirewallName": "ready-fw" }))
        .await;
    assert_eq!(
        settled["FirewallStatus"]["Status"],
        json!("READY"),
        "expected the firewall to be READY once the span elapsed, got {settled:?}"
    );
    for (zone, sync_state) in settled["FirewallStatus"]["SyncStates"]
        .as_object()
        .expect("SyncStates")
    {
        assert_eq!(
            sync_state["Attachment"]["Status"],
            json!("READY"),
            "case zone {zone}: expected the endpoint to be READY, got {sync_state:?}"
        );
    }
}

/// RULE: a request that references a resource that does not exist, or that names
/// no resource at all, is refused with the modelled client error rather than
/// succeeding.
#[tokio::test]
async fn references_to_absent_resources_are_refused() {
    let harness = Harness::new();
    let policy = harness.create_policy("ref-policy").await;
    let policy_arn = string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

    let cases: [(&str, Value, &str); 8] = [
        (
            "CreateFirewall",
            json!({
                "FirewallName": "never-made",
                "FirewallPolicyArn":
                    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall-policy/absent"),
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
            }),
            "InvalidRequestException",
        ),
        (
            "CreateFirewall",
            json!({
                "FirewallName": "no-vpc",
                "FirewallPolicyArn": policy_arn,
                "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
            }),
            "InvalidRequestException",
        ),
        (
            "CreateFirewall",
            json!({
                "FirewallName": "repeated-subnet",
                "FirewallPolicyArn": policy_arn,
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-dup" }, { "SubnetId": "subnet-dup" }],
            }),
            "InvalidRequestException",
        ),
        (
            "DescribeFirewall",
            json!({ "FirewallName": "absent" }),
            "ResourceNotFoundException",
        ),
        (
            "DescribeFirewall",
            json!({ "FirewallArn":
                format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/absent") }),
            "ResourceNotFoundException",
        ),
        (
            "DescribeFirewall",
            json!({}),
            "InvalidRequestException",
        ),
        (
            "UpdateFirewallDescription",
            json!({ "FirewallName": "absent", "Description": "x" }),
            "ResourceNotFoundException",
        ),
        (
            "DeleteFirewall",
            json!({ "FirewallName": "absent" }),
            "ResourceNotFoundException",
        ),
    ];

    for (target, request, expected_code) in &cases {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected_code),
            "case {target} {request:?}: expected 400 {expected_code}, got {status} {body:?}"
        );
    }

    // None of the refusals created anything.
    let listed = harness
        .ok("ListFirewalls", &json!({ "MaxResults": 100 }))
        .await;
    assert_eq!(
        listed["Firewalls"].as_array().map(Vec::len),
        Some(0),
        "a refused create must store nothing, got {listed:?}"
    );
}

/// Builds one of the retained policy document's unbounded lists with `count`
/// entries.
type ListBuilder = fn(usize) -> Value;

/// RULE: a request that would make this emulator retain an unbounded amount of
/// state is refused with the modelled `LimitExceededException`, and stores
/// nothing.
///
/// Rows sit ON the bound and one PAST it, for each list of the retained
/// `FirewallPolicy` document the model leaves without a length constraint
/// (`smithy_query(query="shape",shape="FirewallPolicy")`).
#[tokio::test]
async fn a_policy_document_past_the_retained_size_bound_is_refused() {
    let bound = state::MAX_POLICY_LIST_ENTRIES;
    let builders: [(&str, ListBuilder); 3] = [
        ("StatelessDefaultActions", |count| {
            // Every entry has to name an action the service supports, so the
            // length is reached by repeating one standard action rather than by
            // inventing distinct ones (the list declares no `@uniqueItems`).
            (0..count)
                .map(|_| json!("aws:pass"))
                .collect::<Vec<Value>>()
                .into()
        }),
        ("StatelessRuleGroupReferences", |count| {
            (0..count)
                .map(|index| {
                    json!({
                        "ResourceArn": format!(
                            "arn:aws:network-firewall:{REGION}:{ACCOUNT}:stateless-rulegroup/rg{index}"
                        ),
                        // `Priority` is a modelled 1..65535 integer.
                        "Priority": index + 1,
                    })
                })
                .collect::<Vec<Value>>()
                .into()
        }),
        ("StatelessCustomActions", |count| {
            (0..count)
                .map(|index| {
                    json!({
                        // `ActionName` is a modelled ^[a-zA-Z0-9]+$ string.
                        "ActionName": format!("Action{index}"),
                        "ActionDefinition": {
                            "PublishMetricAction": { "Dimensions": [{ "Value": "sfe" }] }
                        },
                    })
                })
                .collect::<Vec<Value>>()
                .into()
        }),
    ];

    for (member, build) in builders {
        for (count, accepted) in [(bound, true), (bound + 1, false)] {
            let harness = Harness::new();
            // A rule-group reference must resolve to a stored rule group, so the
            // rule groups the reference rows name are created first: this test is
            // about the size bound, not about resolution.
            if member == "StatelessRuleGroupReferences" {
                for index in 0..count {
                    harness
                        .ok(
                            "CreateRuleGroup",
                            &json!({
                                "RuleGroupName": format!("rg{index}"),
                                "Type": "STATELESS",
                                "Capacity": 1,
                                "RuleGroup": {
                                    "RulesSource": {
                                        "StatelessRulesAndCustomActions": {
                                            "StatelessRules": [{
                                                "Priority": 1,
                                                "RuleDefinition": {
                                                    "Actions": ["aws:pass"],
                                                    "MatchAttributes": {},
                                                },
                                            }],
                                        },
                                    },
                                },
                            }),
                        )
                        .await;
                }
            }
            let mut document = minimal_policy();
            document[member] = build(count);
            let (status, body) = harness
                .call(
                    "CreateFirewallPolicy",
                    &json!({ "FirewallPolicyName": "bounded", "FirewallPolicy": document }),
                )
                .await;
            if accepted {
                assert_eq!(
                    status, 200,
                    "case {member} x{count}: a document on the bound is accepted, got {status} \
                     {body:?}"
                );
                continue;
            }
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "LimitExceededException"),
                "case {member} x{count}: a document past the bound is refused, got {status} \
                 {body:?}"
            );
            let (status, body) = harness
                .call(
                    "DescribeFirewallPolicy",
                    &json!({ "FirewallPolicyName": "bounded" }),
                )
                .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {member} x{count}: a refused create stores nothing, got {status} {body:?}"
            );
        }
    }
}

/// RULE: a server-generated id is unique across mints and always satisfies the
/// modelled form, whatever the request that triggered it.
///
/// This is the invariant `ResourceId` (`length 36`, pattern
/// `^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`) and `UpdateToken` state, so
/// it is asserted over a run of mints rather than one.
#[tokio::test]
async fn server_generated_ids_are_unique_and_well_formed() {
    const MINTS: usize = 50;

    let harness = Harness::new();
    let mut seen: Vec<String> = Vec::new();
    for index in 0..MINTS {
        let name = format!("id-{index:03}");
        let created = harness.create_policy(&name).await;
        for (member, value) in [
            (
                "FirewallPolicyId",
                string_at(&created, &["FirewallPolicyResponse", "FirewallPolicyId"]).to_string(),
            ),
            ("UpdateToken", string_at(&created, &["UpdateToken"]).to_string()),
        ] {
            assert!(
                is_uuid_form(&value),
                "case {name}/{member}: expected the modelled 36-character UUID form, got \
                 {value:?}"
            );
            assert!(
                !seen.contains(&value),
                "case {name}/{member}: {value:?} was already minted; ids must be unique"
            );
            seen.push(value);
        }
    }
    assert_eq!(
        seen.len(),
        MINTS * 2,
        "expected one id and one token per create, got {}",
        seen.len()
    );
}

/// RULE: an identifier pair that names two different resources is a bad request,
/// a `NextToken` this service did not mint is a bad request, and every paginated
/// listing honours the same page rule.
#[tokio::test]
async fn conflicting_identifiers_and_foreign_tokens_are_refused() {
    let harness = Harness::new();
    let policy_a = harness.create_policy("policy-a").await;
    let policy_b = harness.create_policy("policy-b").await;
    let arn_a = string_at(&policy_a, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();
    let arn_b = string_at(&policy_b, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

    for (name, subnet) in [("fw-a", "subnet-a1"), ("fw-b", "subnet-b2")] {
        harness
            .ok(
                "CreateFirewall",
                &json!({
                    "FirewallName": name,
                    "FirewallPolicyArn": arn_a,
                    "VpcId": "vpc-0123abcd",
                    "SubnetMappings": [{ "SubnetId": subnet }],
                }),
            )
            .await;
    }
    let firewall_arn_b = format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:firewall/fw-b");

    // The name and the ARN name different resources, on every operation that
    // accepts either identifier.
    let mismatches: [(&str, Value); 4] = [
        (
            "DescribeFirewall",
            json!({ "FirewallName": "fw-a", "FirewallArn": firewall_arn_b }),
        ),
        (
            "DeleteFirewall",
            json!({ "FirewallName": "fw-a", "FirewallArn": firewall_arn_b }),
        ),
        (
            "DescribeFirewallPolicy",
            json!({ "FirewallPolicyName": "policy-a", "FirewallPolicyArn": arn_b }),
        ),
        (
            "DeleteFirewallPolicy",
            json!({ "FirewallPolicyName": "policy-a", "FirewallPolicyArn": arn_b }),
        ),
    ];
    for (target, request) in &mismatches {
        let (status, body) = harness.call(target, request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {target} {request:?}: expected 400 InvalidRequestException, got {status} \
             {body:?}"
        );
    }

    // A token this service did not mint, on each paginated listing. It satisfies
    // the modelled `PaginationToken` character class, so it reaches the handler.
    for target in ["ListFirewalls", "ListFirewallPolicies"] {
        let (status, body) = harness
            .call(target, &json!({ "NextToken": "zzz", "MaxResults": 2 }))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {target}: expected a foreign NextToken to be refused, got {status} {body:?}"
        );
    }

    // A cursor one listing minted is not a cursor of the other: each listing
    // stamps its own kind on the token, so replaying it across listings is the
    // same bad request as a token this service never minted.
    let firewall_page = harness
        .ok("ListFirewalls", &json!({ "MaxResults": 1 }))
        .await;
    let firewall_token = firewall_page
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case firewall page 1: expected a NextToken in {firewall_page:?}"))
        .to_string();
    let (status, body) = harness
        .call(
            "ListFirewallPolicies",
            &json!({ "NextToken": firewall_token, "MaxResults": 2 }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case cross-listing NextToken: expected a firewall cursor to be refused by the policy \
         listing, got {status} {body:?}"
    );

    // And the policy listing pages and resumes like the firewall listing does.
    let first = harness
        .ok("ListFirewallPolicies", &json!({ "MaxResults": 1 }))
        .await;
    let mut names: Vec<String> = first["FirewallPolicies"]
        .as_array()
        .expect("FirewallPolicies")
        .iter()
        .map(|entry| string_at(entry, &["Name"]).to_string())
        .collect();
    assert_eq!(
        names.len(),
        1,
        "case policy page 1: expected one entry, got {first:?}"
    );
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case policy page 1: expected a NextToken in {first:?}"))
        .to_string();
    let second = harness
        .ok(
            "ListFirewallPolicies",
            &json!({ "MaxResults": 1, "NextToken": token }),
        )
        .await;
    names.extend(
        second["FirewallPolicies"]
            .as_array()
            .expect("FirewallPolicies")
            .iter()
            .map(|entry| string_at(entry, &["Name"]).to_string()),
    );
    assert!(
        second.get("NextToken").is_none(),
        "case policy page 2: nothing remains, so there must be no NextToken: {second:?}"
    );
    names.sort();
    assert_eq!(
        names,
        vec!["policy-a".to_string(), "policy-b".to_string()],
        "expected every policy exactly once across the two pages"
    );
}

/// RULE: an operation this emulator has not implemented answers HTTP 501, never
/// an empty success — a transit gateway-attached firewall is a different resource
/// with its own lifecycle, and none of its operations is implemented.
#[tokio::test]
async fn an_unimplemented_firewall_mode_answers_501() {
    let harness = Harness::new();
    let policy = harness.create_policy("mode-policy").await;
    let policy_arn = string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]).to_string();

    let cases = [
        json!({
            "FirewallName": "tgw-fw",
            "FirewallPolicyArn": policy_arn,
            "TransitGatewayId": "tgw-0123abcd",
            "AvailabilityZoneMappings": [{ "AvailabilityZone": "eu-west-1a" }],
        }),
        // `AvailabilityZoneChangeProtection` only governs the Availability Zone
        // configuration of a transit gateway-attached firewall, so a request that
        // sets it is asking for that unimplemented mode.
        json!({
            "FirewallName": "az-guard-fw",
            "FirewallPolicyArn": policy_arn,
            "VpcId": "vpc-0123abcd",
            "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
            "AvailabilityZoneChangeProtection": true,
        }),
        json!({
            "FirewallName": "proxy-fw",
            "FirewallPolicyArn": policy_arn,
            "NoSourcePreservation": true,
            "NatGatewayMappings": [{ "NatGatewayId": "nat-0123abcd" }],
            "VpcEndpoint": {
                "VpcId": "vpc-0123abcd",
                "SubnetMappings": [{ "SubnetId": "subnet-0123abcd" }],
            },
        }),
    ];

    for request in &cases {
        let (status, body) = harness.call("CreateFirewall", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (501, "NotImplemented"),
            "case {request:?}: expected a 501 NotImplemented, got {status} {body:?}"
        );
    }

    let listed = harness
        .ok("ListFirewalls", &json!({ "MaxResults": 100 }))
        .await;
    assert_eq!(
        listed["Firewalls"].as_array().map(Vec::len),
        Some(0),
        "a 501 must store nothing, got {listed:?}"
    );
}
