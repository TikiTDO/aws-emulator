// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule-group state mutations — `DeleteRuleGroup` and
//! `UpdateRuleGroup` — and for the cross-resource references a firewall policy
//! makes to rule groups.
//!
//! Every test drives the service the way a client does, through the shared
//! [`Harness`] in `crate::harness`: an awsJson1_0 `POST /` with
//! `X-Amz-Target: NetworkFirewall_20201112.<OperationName>`, so what is asserted
//! is the WIRE behaviour and not a handler's return value.
//!
//! The tests are tables: each row is one input in the class the rule covers, and
//! every assertion names its row and the actual value.

use serde_json::{json, Value};

use crate::harness::{
    error_code, is_uuid_form, minimal_policy_document as minimal_policy, string_at, with_tag_list,
    Harness,
};
use crate::rule_group_lifecycle::{
    create_request, expected_arn, stateful_one_rule, stateless_any, stateless_group, stateless_rule,
};

/// A stateful domain-list document over `targets`, which a `STATEFUL_DOMAIN` rule
/// group's `RulesSourceList` is built from (`Targets`, `TargetTypes` and
/// `GeneratedRulesType` are all required).
fn domain_list(targets: &[&str]) -> Value {
    json!({
        "RulesSource": {
            "RulesSourceList": {
                "Targets": targets,
                "TargetTypes": ["TLS_SNI"],
                "GeneratedRulesType": "ALLOWLIST",
            },
        },
    })
}

/// A Suricata flat-format rules string with one `pass` rule per entry of `sids`.
fn suricata(sids: &[u32]) -> String {
    sids.iter()
        .map(|sid| format!("pass tcp any any -> any any (sid:{sid};)\n"))
        .collect()
}

/// Send `CreateRuleGroup` for a rule group that must be created, and return the
/// whole response body.
async fn create_rule_group(
    harness: &Harness,
    name: &str,
    group_type: &str,
    capacity: i64,
    specification: &Value,
) -> Value {
    harness
        .ok(
            "CreateRuleGroup",
            &create_request(name, group_type, capacity, specification),
        )
        .await
}

/// One row of the delete table.
#[derive(Debug)]
struct DeleteCase {
    name: &'static str,
    group_type: &'static str,
    capacity: i64,
    /// Whether the delete names the rule group by its ARN rather than by
    /// `RuleGroupName` + `Type`; both identify the same resource.
    by_arn: bool,
}

/// RULE: a delete removes the rule group from state, so every later read of it —
/// by either identifier — takes the not-found path
/// (`ResourceNotFoundException`, "unable to locate a resource using the
/// parameters that you provided"), a second delete finds nothing to delete, and
/// the name is free again.
///
/// The rows span the rule group kinds, both ways of giving the specification
/// (`RuleGroup` and the Suricata `Rules` string) and both identifiers a delete
/// may name the rule group by.
#[tokio::test]
async fn a_deleted_rule_group_is_gone_from_every_read() {
    let cases: [(DeleteCase, Value); 4] = [
        (
            DeleteCase {
                name: "del-stateless",
                group_type: "STATELESS",
                capacity: 10,
                by_arn: false,
            },
            json!({ "RuleGroup": stateless_any() }),
        ),
        (
            DeleteCase {
                name: "del-stateful",
                group_type: "STATEFUL",
                capacity: 10,
                by_arn: true,
            },
            json!({ "RuleGroup": stateful_one_rule() }),
        ),
        (
            DeleteCase {
                name: "del-stateful-domain",
                group_type: "STATEFUL_DOMAIN",
                capacity: 20,
                by_arn: false,
            },
            json!({ "RuleGroup": domain_list(&[".a.test", "b.test"]) }),
        ),
        (
            DeleteCase {
                name: "del-from-rules-string",
                group_type: "STATEFUL",
                capacity: 10,
                by_arn: true,
            },
            json!({ "Rules": suricata(&[1, 2]) }),
        ),
    ];

    let harness = Harness::new();
    for (case, specification) in &cases {
        let created = create_rule_group(
            &harness,
            case.name,
            case.group_type,
            case.capacity,
            specification,
        )
        .await;
        let arn = expected_arn(case.name, case.group_type);
        let created_id =
            string_at(&created, &["RuleGroupResponse", "RuleGroupId"]).to_string();

        let by_name = json!({ "RuleGroupName": case.name, "Type": case.group_type });
        let by_arn = json!({ "RuleGroupArn": arn });
        let delete_request = if case.by_arn { &by_arn } else { &by_name };

        let deleted = harness.ok("DeleteRuleGroup", delete_request).await;
        let response = deleted
            .get("RuleGroupResponse")
            .unwrap_or_else(|| panic!("case {case:?}: expected RuleGroupResponse in {deleted:?}"));
        assert_eq!(
            (
                response.get("RuleGroupArn"),
                response.get("RuleGroupId"),
                response.get("Type"),
                response.get("Capacity"),
                response.get("RuleGroupStatus"),
            ),
            (
                by_arn.get("RuleGroupArn"),
                Some(&json!(created_id)),
                Some(&json!(case.group_type)),
                Some(&json!(case.capacity)),
                Some(&json!("DELETING")),
            ),
            "case {case:?}: expected the delete to report the rule group it removed in its \
             terminal status, got {deleted:?}"
        );

        // Neither identifier resolves any more: both halves of the rule group's
        // identity went with the record.
        for read in [&by_name, &by_arn] {
            let (status, body) = harness.call("DescribeRuleGroup", read).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?} read by {read:?}: expected the deleted rule group to be gone, \
                 got {status} {body:?}"
            );
        }

        // A second delete has nothing to delete.
        let (status, body) = harness.call("DeleteRuleGroup", delete_request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: expected a repeated delete to find nothing, got {status} {body:?}"
        );

        // And the name is free again: a re-create is a NEW resource, not the old
        // one resurfacing.
        let recreated = create_rule_group(
            &harness,
            case.name,
            case.group_type,
            case.capacity,
            specification,
        )
        .await;
        assert_ne!(
            string_at(&recreated, &["RuleGroupResponse", "RuleGroupId"]),
            created_id,
            "case {case:?}: expected the re-created rule group to be a new resource, got \
             {recreated:?}"
        );
    }
}

/// RULE: a rule group's identity is its name AND its type, so a delete removes
/// only the one in the half of the service the request named.
#[tokio::test]
async fn deleting_one_type_leaves_the_same_name_in_the_other_type() {
    let harness = Harness::new();
    let shared = "shared-name";
    create_rule_group(
        &harness,
        shared,
        "STATELESS",
        10,
        &json!({ "RuleGroup": stateless_any() }),
    )
    .await;
    let stateful =
        create_rule_group(&harness, shared, "STATEFUL", 10, &json!({ "Rules": suricata(&[7]) }))
            .await;

    harness
        .ok(
            "DeleteRuleGroup",
            &json!({ "RuleGroupName": shared, "Type": "STATELESS" }),
        )
        .await;

    let (status, body) = harness
        .call(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": shared, "Type": "STATELESS" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "case STATELESS: expected the deleted rule group to be gone, got {status} {body:?}"
    );

    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": shared, "Type": "STATEFUL" }),
        )
        .await;
    assert_eq!(
        described.get("RuleGroupResponse").cloned(),
        stateful.get("RuleGroupResponse").map(with_tag_list),
        "case STATEFUL: expected the other type's rule group to be untouched, got {described:?}"
    );
}

/// One row of the update table: what the rule group was created with, what the
/// update asks for, and what every later read must report.
#[derive(Debug)]
struct UpdateCase {
    name: &'static str,
    group_type: &'static str,
    capacity: i64,
    /// The create's specification members (`RuleGroup`/`Rules`, `Description`,
    /// `EncryptionConfiguration`).
    initial: Value,
    /// The update's members, beside the identifier and the update token.
    update: Value,
    /// The rule group document a later `DescribeRuleGroup` must report.
    document: Value,
    /// The description a later read must report.
    description: Option<&'static str>,
    /// The encryption settings a later read must report.
    encryption: Value,
    /// `ConsumedCapacity`, re-derived from the document the update landed.
    consumed: i64,
}

/// The update table: each row changes a different part of the rule group, with a
/// row that changes none of the rules at all.
fn update_cases() -> Vec<UpdateCase> {
    // The default key names itself in both members: see
    // `response_projections::every_resource_reports_the_key_that_protects_it_in_both_members`.
    let aws_owned = json!({ "Type": "AWS_OWNED_KMS_KEY", "KeyId": "AWS_OWNED_KMS_KEY" });
    let customer_key = json!({ "Type": "CUSTOMER_KMS", "KeyId": "alias/net-fw" });
    vec![
        UpdateCase {
            name: "rules-string-replaced",
            group_type: "STATEFUL",
            capacity: 10,
            initial: json!({ "Rules": suricata(&[1]), "Description": "v1" }),
            update: json!({ "Rules": suricata(&[2, 3, 4]), "Description": "v2" }),
            document: json!({ "RulesSource": { "RulesString": suricata(&[2, 3, 4]) } }),
            description: Some("v2"),
            encryption: aws_owned.clone(),
            consumed: 3,
        },
        UpdateCase {
            name: "document-replaced",
            group_type: "STATEFUL",
            capacity: 10,
            initial: json!({ "RuleGroup": stateful_one_rule(), "Description": "first" }),
            update: json!({
                "RuleGroup": {
                    "RulesSource": {
                        "StatefulRules": [
                            stateful_one_rule()["RulesSource"]["StatefulRules"][0].clone(),
                            stateful_one_rule()["RulesSource"]["StatefulRules"][0].clone(),
                        ],
                    },
                },
            }),
            document: json!({
                "RulesSource": {
                    "StatefulRules": [
                        stateful_one_rule()["RulesSource"]["StatefulRules"][0].clone(),
                        stateful_one_rule()["RulesSource"]["StatefulRules"][0].clone(),
                    ],
                },
            }),
            // A member the update does not carry is left as it was stored.
            description: Some("first"),
            encryption: aws_owned.clone(),
            consumed: 2,
        },
        UpdateCase {
            name: "description-only",
            group_type: "STATEFUL",
            capacity: 10,
            initial: json!({ "Rules": suricata(&[5]), "Description": "before" }),
            update: json!({ "Description": "after" }),
            document: json!({ "RulesSource": { "RulesString": suricata(&[5]) } }),
            description: Some("after"),
            encryption: aws_owned.clone(),
            consumed: 1,
        },
        UpdateCase {
            name: "stateless-document-replaced",
            group_type: "STATELESS",
            capacity: 30,
            initial: json!({ "RuleGroup": stateless_any() }),
            update: json!({
                "RuleGroup": stateless_group(vec![
                    stateless_rule(
                        1,
                        "aws:pass",
                        json!({
                            "Sources": [
                                { "AddressDefinition": "10.0.0.0/24" },
                                { "AddressDefinition": "10.0.1.0/24" },
                            ],
                            "Destinations": [
                                { "AddressDefinition": "10.1.0.0/24" },
                                { "AddressDefinition": "10.1.1.0/24" },
                            ],
                        }),
                    ),
                    stateless_rule(2, "aws:drop", json!({})),
                ]),
            }),
            document: stateless_group(vec![
                stateless_rule(
                    1,
                    "aws:pass",
                    json!({
                        "Sources": [
                            { "AddressDefinition": "10.0.0.0/24" },
                            { "AddressDefinition": "10.0.1.0/24" },
                        ],
                        "Destinations": [
                            { "AddressDefinition": "10.1.0.0/24" },
                            { "AddressDefinition": "10.1.1.0/24" },
                        ],
                    }),
                ),
                stateless_rule(2, "aws:drop", json!({})),
            ]),
            description: None,
            encryption: aws_owned.clone(),
            // 2 sources * 2 destinations for the first rule, plus 1 for the second.
            consumed: 5,
        },
        UpdateCase {
            name: "domain-list-extended",
            group_type: "STATEFUL_DOMAIN",
            capacity: 20,
            initial: json!({ "RuleGroup": domain_list(&[".a.test", "b.test"]) }),
            update: json!({
                "RuleGroup": domain_list(&[".a.test", "b.test", "c.test", ".d.test"]),
            }),
            document: domain_list(&[".a.test", "b.test", "c.test", ".d.test"]),
            description: None,
            encryption: aws_owned.clone(),
            consumed: 4,
        },
        UpdateCase {
            name: "encryption-replaced",
            group_type: "STATEFUL",
            capacity: 10,
            initial: json!({ "Rules": suricata(&[9]) }),
            update: json!({ "EncryptionConfiguration": customer_key.clone() }),
            document: json!({ "RulesSource": { "RulesString": suricata(&[9]) } }),
            description: None,
            encryption: customer_key,
            consumed: 1,
        },
        UpdateCase {
            name: "a-rules-string-replaces-a-document",
            group_type: "STATEFUL",
            capacity: 10,
            initial: json!({ "RuleGroup": stateful_one_rule() }),
            update: json!({ "Rules": suricata(&[11, 12]) }),
            // "The call response returns a RuleGroup object that Network Firewall
            // has populated from your string."
            document: json!({ "RulesSource": { "RulesString": suricata(&[11, 12]) } }),
            description: None,
            encryption: aws_owned,
            consumed: 2,
        },
    ]
}

/// RULE: an update writes what the request carries onto the stored rule group, so
/// the next read reports the NEW document and metadata — the documented
/// "retrieve the current RuleGroup object, update the object as needed, and then
/// provide the updated object to this call" cycle — and mints a fresh
/// `UpdateToken` that replaces the one the caller used.
#[tokio::test]
async fn an_update_is_what_the_next_read_reports() {
    let harness = Harness::new();
    for case in update_cases() {
        let created = create_rule_group(
            &harness,
            case.name,
            case.group_type,
            case.capacity,
            &case.initial,
        )
        .await;
        let created_token = string_at(&created, &["UpdateToken"]).to_string();
        let created_id = string_at(&created, &["RuleGroupResponse", "RuleGroupId"]).to_string();

        let mut request = json!({
            "RuleGroupName": case.name,
            "Type": case.group_type,
            "UpdateToken": created_token,
        });
        for (member, value) in case.update.as_object().expect("an update is an object") {
            request[member] = value.clone();
        }
        let updated = harness.ok("UpdateRuleGroup", &request).await;

        let new_token = string_at(&updated, &["UpdateToken"]).to_string();
        assert!(
            is_uuid_form(&new_token) && new_token != created_token,
            "case {}: expected a fresh well-formed UpdateToken, got {new_token:?} against \
             {created_token:?}",
            case.name
        );
        let response = updated.get("RuleGroupResponse").unwrap_or_else(|| {
            panic!(
                "case {}: expected RuleGroupResponse in {updated:?}",
                case.name
            )
        });
        assert_eq!(
            (
                response.get("RuleGroupId"),
                response.get("Type"),
                response.get("Capacity"),
                response.get("ConsumedCapacity"),
                response.get("RuleGroupStatus"),
                response.get("Description"),
                response.get("EncryptionConfiguration"),
            ),
            (
                Some(&json!(created_id)),
                Some(&json!(case.group_type)),
                // Capacity "is fixed at creation", and the update cannot change
                // the resource's identity either.
                Some(&json!(case.capacity)),
                Some(&json!(case.consumed)),
                Some(&json!("ACTIVE")),
                case.description.map(|text| json!(text)).as_ref(),
                Some(&case.encryption),
            ),
            "case {}: expected the update to report the updated rule group, got {updated:?}",
            case.name
        );

        // The read after the write, by each identifier, reports exactly what the
        // update answered.
        for read in [
            json!({ "RuleGroupName": case.name, "Type": case.group_type }),
            json!({ "RuleGroupArn": expected_arn(case.name, case.group_type) }),
        ] {
            let described = harness.ok("DescribeRuleGroup", &read).await;
            assert_eq!(
                described.get("RuleGroup"),
                Some(&case.document),
                "case {} read by {read:?}: expected the updated document, got {described:?}",
                case.name
            );
            assert_eq!(
                (
                    described.get("RuleGroupResponse").cloned(),
                    described.get("UpdateToken")
                ),
                (Some(with_tag_list(response)), Some(&json!(new_token))),
                "case {} read by {read:?}: expected the update's own response and token, got \
                 {described:?}",
                case.name
            );
        }

        // "Network Firewall uses the token to ensure that the rule group hasn't
        // changed since you last retrieved it": the token the update consumed no
        // longer unlocks the rule group, and the refused call changes nothing.
        let (status, body) = harness.call("UpdateRuleGroup", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidTokenException"),
            "case {}: expected the consumed token to be refused, got {status} {body:?}",
            case.name
        );
        let described = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": case.name, "Type": case.group_type }),
            )
            .await;
        assert_eq!(
            (described.get("RuleGroup"), described.get("UpdateToken")),
            (Some(&case.document), Some(&json!(new_token))),
            "case {}: expected a refused update to leave the rule group alone, got {described:?}",
            case.name
        );
    }
}

/// RULE: an update this service cannot run is refused with the error its model
/// gives for it, and the stored rule group is left exactly as the previous write
/// left it.
#[tokio::test]
async fn an_update_that_cannot_run_changes_nothing() {
    // A token in the modelled form that this service never minted, so it reaches
    // the handler's own staleness check.
    const FOREIGN_TOKEN: &str = "00000000-0000-4000-8000-000000000000";

    let harness = Harness::new();
    let stateful = create_rule_group(
        &harness,
        "guarded-stateful",
        "STATEFUL",
        2,
        &json!({ "Rules": suricata(&[1]), "Description": "before" }),
    )
    .await;
    let stateless = create_rule_group(
        &harness,
        "guarded-stateless",
        "STATELESS",
        10,
        &json!({ "RuleGroup": stateless_any() }),
    )
    .await;
    let stateful_token = string_at(&stateful, &["UpdateToken"]).to_string();
    let stateless_token = string_at(&stateless, &["UpdateToken"]).to_string();

    let cases: [(&str, Value, &str); 8] = [
        (
            "both-rulegroup-and-rules",
            json!({
                "RuleGroupName": "guarded-stateful",
                "Type": "STATEFUL",
                "UpdateToken": stateful_token,
                "RuleGroup": stateful_one_rule(),
                "Rules": suricata(&[2]),
            }),
            "InvalidRequestException",
        ),
        (
            "a-rules-string-cannot-define-a-stateless-group",
            json!({
                "RuleGroupName": "guarded-stateless",
                "Type": "STATELESS",
                "UpdateToken": stateless_token,
                "Rules": suricata(&[2]),
            }),
            "InvalidRequestException",
        ),
        (
            "a-document-of-the-wrong-half",
            json!({
                "RuleGroupName": "guarded-stateful",
                "Type": "STATEFUL",
                "UpdateToken": stateful_token,
                "RuleGroup": stateless_any(),
            }),
            "InvalidRequestException",
        ),
        (
            // "You can't change or exceed this capacity when you update the rule
            // group": three rules do not fit a capacity of two.
            "past-the-fixed-capacity",
            json!({
                "RuleGroupName": "guarded-stateful",
                "Type": "STATEFUL",
                "UpdateToken": stateful_token,
                "Rules": suricata(&[2, 3, 4]),
            }),
            "InvalidRequestException",
        ),
        (
            "a-token-this-service-never-minted",
            json!({
                "RuleGroupName": "guarded-stateful",
                "Type": "STATEFUL",
                "UpdateToken": FOREIGN_TOKEN,
                "Description": "sneaked in",
            }),
            "InvalidTokenException",
        ),
        (
            "a-rule-group-that-does-not-exist",
            json!({
                "RuleGroupName": "no-such-group",
                "Type": "STATEFUL",
                "UpdateToken": FOREIGN_TOKEN,
                "Description": "nowhere",
            }),
            "ResourceNotFoundException",
        ),
        (
            // `Type` "is required for requests that do not include the
            // RuleGroupARN".
            "a-name-without-a-type",
            json!({
                "RuleGroupName": "guarded-stateful",
                "UpdateToken": stateful_token,
                "Description": "typeless",
            }),
            "InvalidRequestException",
        ),
        (
            "no-identifier-at-all",
            json!({ "UpdateToken": stateful_token, "Description": "nameless" }),
            "InvalidRequestException",
        ),
    ];

    for (name, request, expected_code) in &cases {
        let (status, body) = harness.call("UpdateRuleGroup", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected_code),
            "case {name}: expected 400 {expected_code}, got {status} {body:?}"
        );
    }

    // Every refusal above left both rule groups as the create wrote them.
    for (name, group_type, created) in [
        ("guarded-stateful", "STATEFUL", &stateful),
        ("guarded-stateless", "STATELESS", &stateless),
    ] {
        let described = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": name, "Type": group_type }),
            )
            .await;
        assert_eq!(
            (
                described.get("RuleGroupResponse").cloned(),
                described.get("UpdateToken")
            ),
            (
                created.get("RuleGroupResponse").map(with_tag_list),
                created.get("UpdateToken")
            ),
            "case {name}: expected the refused updates to change nothing, got {described:?}"
        );
    }
}

/// RULE: a dry run "checks whether the request can run successfully, but doesn't
/// actually make the requested changes" — it answers the response the real update
/// would, and the stored rule group is untouched until the real call runs.
#[tokio::test]
async fn a_dry_run_update_reports_the_change_without_making_it() {
    let harness = Harness::new();
    let created = create_rule_group(
        &harness,
        "dry-run-update",
        "STATEFUL",
        10,
        &json!({ "Rules": suricata(&[1]), "Description": "before" }),
    )
    .await;
    let token = string_at(&created, &["UpdateToken"]).to_string();

    let mut request = json!({
        "RuleGroupName": "dry-run-update",
        "Type": "STATEFUL",
        "UpdateToken": token,
        "Rules": suricata(&[2, 3]),
        "Description": "after",
        "DryRun": true,
    });
    let dry = harness.ok("UpdateRuleGroup", &request).await;
    assert_eq!(
        (
            dry["RuleGroupResponse"].get("Description"),
            dry["RuleGroupResponse"].get("ConsumedCapacity"),
        ),
        (Some(&json!("after")), Some(&json!(2))),
        "case dry-run: expected the response the real update would answer, got {dry:?}"
    );

    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "dry-run-update", "Type": "STATEFUL" }),
        )
        .await;
    assert_eq!(
        (
            described.get("RuleGroup"),
            described.get("UpdateToken"),
            described["RuleGroupResponse"].get("Description"),
        ),
        (
            Some(&json!({ "RulesSource": { "RulesString": suricata(&[1]) } })),
            Some(&json!(token)),
            Some(&json!("before")),
        ),
        "case dry-run: expected a dry run to change nothing, got {described:?}"
    );

    // The token the dry run did not consume still unlocks the real update.
    request["DryRun"] = json!(false);
    harness.ok("UpdateRuleGroup", &request).await;
    let described = harness
        .ok(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "dry-run-update", "Type": "STATEFUL" }),
        )
        .await;
    assert_eq!(
        (
            described.get("RuleGroup"),
            described["RuleGroupResponse"].get("Description"),
        ),
        (
            Some(&json!({ "RulesSource": { "RulesString": suricata(&[2, 3]) } })),
            Some(&json!("after")),
        ),
        "case real-update: expected the change to land, got {described:?}"
    );
}

/// One row of the policy-reference table: the rule-group ARNs each reference list
/// of the policy document names, and whether the create is accepted.
#[derive(Debug)]
struct ReferenceCase {
    name: &'static str,
    stateless: Vec<String>,
    stateful: Vec<String>,
    accepted: bool,
}

/// RULE: a rule-group ARN a firewall policy references must resolve to a stored
/// rule group of the kind its reference list is for — a
/// `StatelessRuleGroupReference$ResourceArn` is "the ARN of the stateless rule
/// group" and a `StatefulRuleGroupReference$ResourceArn` "the ARN of the stateful
/// rule group" — and one that does not is refused with
/// `InvalidRequestException`, whose documented examples include a request that
/// "references an ARN that is malformed, or corresponds to a resource that isn't
/// valid in the context of the request". Nothing is created for a refused
/// request.
///
/// The rows span an empty document, each reference list alone, both at once, a
/// reference to a rule group that was never created, one to a rule group that was
/// created and then deleted, each kind crossed with the other's list, and a list
/// that mixes a resolvable reference with an unresolvable one.
#[tokio::test]
async fn a_policy_reference_must_name_an_existing_rule_group_of_that_kind() {
    let harness = Harness::new();
    create_rule_group(
        &harness,
        "ref-stateless",
        "STATELESS",
        10,
        &json!({ "RuleGroup": stateless_any() }),
    )
    .await;
    create_rule_group(
        &harness,
        "ref-stateful",
        "STATEFUL",
        10,
        &json!({ "Rules": suricata(&[1]) }),
    )
    .await;
    create_rule_group(
        &harness,
        "ref-domain",
        "STATEFUL_DOMAIN",
        20,
        &json!({ "RuleGroup": domain_list(&[".a.test"]) }),
    )
    .await;
    // A rule group that existed and was deleted is as unresolvable as one that
    // never existed.
    create_rule_group(
        &harness,
        "ref-deleted",
        "STATEFUL",
        10,
        &json!({ "Rules": suricata(&[2]) }),
    )
    .await;
    harness
        .ok(
            "DeleteRuleGroup",
            &json!({ "RuleGroupName": "ref-deleted", "Type": "STATEFUL" }),
        )
        .await;

    let stateless_arn = expected_arn("ref-stateless", "STATELESS");
    let stateful_arn = expected_arn("ref-stateful", "STATEFUL");
    let domain_arn = expected_arn("ref-domain", "STATEFUL_DOMAIN");
    let deleted_arn = expected_arn("ref-deleted", "STATEFUL");
    let never_stateful_arn = expected_arn("ref-missing", "STATEFUL");
    let never_stateless_arn = expected_arn("ref-missing", "STATELESS");

    let cases = [
        ReferenceCase {
            name: "no-references",
            stateless: vec![],
            stateful: vec![],
            accepted: true,
        },
        ReferenceCase {
            name: "a-stateless-group-in-the-stateless-list",
            stateless: vec![stateless_arn.clone()],
            stateful: vec![],
            accepted: true,
        },
        ReferenceCase {
            name: "a-stateful-group-in-the-stateful-list",
            stateless: vec![],
            stateful: vec![stateful_arn.clone()],
            accepted: true,
        },
        ReferenceCase {
            name: "a-domain-group-is-stateful",
            stateless: vec![],
            stateful: vec![domain_arn.clone()],
            accepted: true,
        },
        ReferenceCase {
            name: "both-lists-at-once",
            stateless: vec![stateless_arn.clone()],
            stateful: vec![stateful_arn.clone(), domain_arn],
            accepted: true,
        },
        ReferenceCase {
            name: "a-stateful-group-that-was-never-created",
            stateless: vec![],
            stateful: vec![never_stateful_arn],
            accepted: false,
        },
        ReferenceCase {
            name: "a-stateless-group-that-was-never-created",
            stateless: vec![never_stateless_arn],
            stateful: vec![],
            accepted: false,
        },
        ReferenceCase {
            name: "a-deleted-group",
            stateless: vec![],
            stateful: vec![deleted_arn],
            accepted: false,
        },
        ReferenceCase {
            name: "a-stateless-group-in-the-stateful-list",
            stateless: vec![],
            stateful: vec![stateless_arn.clone()],
            accepted: false,
        },
        ReferenceCase {
            name: "a-stateful-group-in-the-stateless-list",
            stateless: vec![stateful_arn.clone()],
            stateful: vec![],
            accepted: false,
        },
        ReferenceCase {
            name: "one-resolvable-and-one-not",
            stateless: vec![stateless_arn],
            stateful: vec![stateful_arn, expected_arn("ref-absent", "STATEFUL")],
            accepted: false,
        },
    ];

    for case in &cases {
        let mut document = minimal_policy();
        if !case.stateless.is_empty() {
            // `StatelessRuleGroupReference$Priority` is a required 1..65535
            // integer.
            document["StatelessRuleGroupReferences"] = case
                .stateless
                .iter()
                .enumerate()
                .map(|(index, arn)| json!({ "ResourceArn": arn, "Priority": index + 1 }))
                .collect::<Vec<Value>>()
                .into();
        }
        if !case.stateful.is_empty() {
            // `StatefulRuleGroupReference$Priority` is optional, so the rows
            // leave it off the first entry and set it on the rest.
            document["StatefulRuleGroupReferences"] = case
                .stateful
                .iter()
                .enumerate()
                .map(|(index, arn)| {
                    if index == 0 {
                        json!({ "ResourceArn": arn })
                    } else {
                        json!({ "ResourceArn": arn, "Priority": index * 100 })
                    }
                })
                .collect::<Vec<Value>>()
                .into();
        }

        let policy_name = format!("policy-{}", case.name);
        let request =
            json!({ "FirewallPolicyName": policy_name, "FirewallPolicy": document.clone() });
        let (status, body) = harness.call("CreateFirewallPolicy", &request).await;

        if case.accepted {
            assert_eq!(
                status, 200,
                "case {}: expected resolvable references to be accepted, got {status} {body:?}",
                case.name
            );
            let described = harness
                .ok(
                    "DescribeFirewallPolicy",
                    &json!({ "FirewallPolicyName": policy_name }),
                )
                .await;
            assert_eq!(
                described.get("FirewallPolicy"),
                Some(&document),
                "case {}: expected the policy document to round-trip, got {described:?}",
                case.name
            );
            continue;
        }

        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {}: expected an unresolvable reference to be refused, got {status} {body:?}",
            case.name
        );
        let (status, body) = harness
            .call(
                "DescribeFirewallPolicy",
                &json!({ "FirewallPolicyName": policy_name }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {}: expected a refused create to store nothing, got {status} {body:?}",
            case.name
        );
    }

    // A dry run is how a caller pre-flights an ARN, so it is refused too: "check
    // whether the request can run successfully" includes resolving the reference.
    let mut document = minimal_policy();
    document["StatefulRuleGroupReferences"] =
        json!([{ "ResourceArn": expected_arn("ref-dry", "STATEFUL") }]);
    let (status, body) = harness
        .call(
            "CreateFirewallPolicy",
            &json!({
                "FirewallPolicyName": "policy-dry-run",
                "FirewallPolicy": document,
                "DryRun": true,
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidRequestException"),
        "case dry-run: expected the pre-flight to refuse an unresolvable reference, got {status} \
         {body:?}"
    );
}
