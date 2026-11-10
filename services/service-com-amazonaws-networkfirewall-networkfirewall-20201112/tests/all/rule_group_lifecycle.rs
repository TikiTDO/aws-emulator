// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the rule-group slice: `CreateRuleGroup` and the
//! `DescribeRuleGroup` read that proves what it persisted.
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
    error_code, is_uuid_form, string_at, with_tag_list, Harness, ACCOUNT, REGION,
};

/// A stateless rule group document with a single rule that matches anything.
pub(crate) fn stateless_any() -> Value {
    json!({
        "RulesSource": {
            "StatelessRulesAndCustomActions": {
                "StatelessRules": [{
                    "Priority": 1,
                    "RuleDefinition": { "Actions": ["aws:pass"], "MatchAttributes": {} },
                }],
            },
        },
    })
}

/// A stateful rule group document with a single Suricata rule.
pub(crate) fn stateful_one_rule() -> Value {
    json!({
        "RulesSource": {
            "StatefulRules": [{
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
            }],
        },
    })
}

/// One stateless rule, built from the match settings a row wants to vary.
pub(crate) fn stateless_rule(priority: i64, action: &str, match_attributes: Value) -> Value {
    json!({
        "Priority": priority,
        "RuleDefinition": { "Actions": [action], "MatchAttributes": match_attributes },
    })
}

/// A stateless rule group document holding exactly `rules`.
pub(crate) fn stateless_group(rules: Vec<Value>) -> Value {
    json!({
        "RulesSource": {
            "StatelessRulesAndCustomActions": { "StatelessRules": rules },
        },
    })
}

/// The members of a `CreateRuleGroup` request, with the row's specification
/// members (`RuleGroup` and/or `Rules`) merged in.
pub(crate) fn create_request(name: &str, group_type: &str, capacity: i64, specification: &Value) -> Value {
    let mut request = json!({
        "RuleGroupName": name,
        "Type": group_type,
        "Capacity": capacity,
    });
    for (member, value) in specification
        .as_object()
        .expect("a specification is a JSON object")
    {
        request[member] = value.clone();
    }
    request
}

/// The ARN a rule group of this name and type must carry: the resource part is
/// `<stateless|stateful>-rulegroup/<name>`, which is what makes the ARN alone
/// enough to identify a rule group.
pub(crate) fn expected_arn(name: &str, group_type: &str) -> String {
    let kind = if group_type == "STATELESS" {
        "stateless-rulegroup"
    } else {
        "stateful-rulegroup"
    };
    format!("arn:aws:network-firewall:{REGION}:{ACCOUNT}:{kind}/{name}")
}

/// One row of the create -> describe round-trip table.
#[derive(Debug)]
struct RoundTripCase {
    name: &'static str,
    group_type: &'static str,
    capacity: i64,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// RULE: a create persists the rule group and its server-generated identity, and
/// a later read of the SAME rule group reports exactly what was stored,
/// whichever identifier the reader resolves it by.
///
/// The rows span the rule group kinds (`STATELESS`, `STATEFUL`,
/// `STATEFUL_DOMAIN`), both ways of giving the specification (`RuleGroup` and the
/// Suricata `Rules` string), and whether a description or tags are given.
#[tokio::test]
async fn a_created_rule_group_reads_back_identically_by_name_and_by_arn() {
    let cases: [(RoundTripCase, Value); 4] = [
        (
            RoundTripCase {
                name: "stateless-bare",
                group_type: "STATELESS",
                capacity: 10,
                description: None,
                tags: &[],
            },
            json!({ "RuleGroup": stateless_any() }),
        ),
        (
            RoundTripCase {
                name: "stateful-described-tagged",
                group_type: "STATEFUL",
                capacity: 50,
                description: Some("east-west inspection"),
                tags: &[("env", "prod"), ("team", "net")],
            },
            json!({ "RuleGroup": stateful_one_rule() }),
        ),
        (
            RoundTripCase {
                name: "stateful-from-suricata-string",
                group_type: "STATEFUL",
                capacity: 5,
                description: Some(""),
                tags: &[("only", "one")],
            },
            // "The call response returns a RuleGroup object that Network Firewall
            // has populated from your string"
            // (`CreateRuleGroupRequest$Rules`).
            json!({ "Rules": "pass tcp any any -> any any (sid:1;)\n" }),
        ),
        (
            RoundTripCase {
                name: "stateful-domain-list",
                group_type: "STATEFUL_DOMAIN",
                capacity: 20,
                description: Some("allowlisted domains"),
                tags: &[],
            },
            json!({
                "RuleGroup": {
                    "RulesSource": {
                        "RulesSourceList": {
                            "Targets": [".example.com", "images.test"],
                            "TargetTypes": ["TLS_SNI", "HTTP_HOST"],
                            "GeneratedRulesType": "ALLOWLIST",
                        },
                    },
                },
            }),
        ),
    ];

    let harness = Harness::new();
    for (case, specification) in &cases {
        let mut request = create_request(
            case.name,
            case.group_type,
            case.capacity,
            specification,
        );
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

        let created = harness.ok("CreateRuleGroup", &request).await;
        let created_response = created
            .get("RuleGroupResponse")
            .unwrap_or_else(|| panic!("case {case:?}: expected RuleGroupResponse in {created:?}"));

        let arn = expected_arn(case.name, case.group_type);
        assert_eq!(
            string_at(created_response, &["RuleGroupArn"]),
            arn,
            "case {case:?}: expected the ARN to carry the request's region and the type's \
             resource kind, got {created_response:?}"
        );
        assert!(
            is_uuid_form(string_at(created_response, &["RuleGroupId"])),
            "case {case:?}: expected RuleGroupId in the modelled UUID form, got \
             {created_response:?}"
        );
        assert!(
            is_uuid_form(string_at(&created, &["UpdateToken"])),
            "case {case:?}: expected UpdateToken in the modelled UUID form, got {created:?}"
        );
        assert_eq!(
            (
                created_response.get("Type"),
                created_response.get("Capacity"),
                created_response.get("RuleGroupStatus"),
                created_response.get("NumberOfAssociations"),
            ),
            (
                Some(&json!(case.group_type)),
                Some(&json!(case.capacity)),
                Some(&json!("ACTIVE")),
                Some(&json!(0)),
            ),
            "case {case:?}: expected the stored type, capacity, status and association count, \
             got {created_response:?}"
        );
        assert_eq!(
            created_response.get("Description"),
            case.description.map(|text| json!(text)).as_ref(),
            "case {case:?}: expected the description to round-trip, got {created_response:?}"
        );
        assert_eq!(
            created_response.get("Tags"),
            request.get("Tags"),
            "case {case:?}: expected the tags to round-trip, got {created_response:?}"
        );
        // Nothing asked for the analyzer, so the response carries no findings.
        assert_eq!(
            created_response.get("AnalysisResults"),
            None,
            "case {case:?}: expected no AnalysisResults without AnalyzeRuleGroup, got \
             {created_response:?}"
        );

        // Read back by NAME + TYPE and by ARN; both must report the stored record.
        for by in [
            json!({ "RuleGroupName": case.name, "Type": case.group_type }),
            json!({ "RuleGroupArn": arn }),
        ] {
            let described = harness.ok("DescribeRuleGroup", &by).await;
            assert_eq!(
                described.get("RuleGroupResponse"),
                Some(&with_tag_list(created_response)),
                "case {case:?} resolved by {by:?}: expected the describe to report the created \
                 rule group, got {described:?}"
            );
            assert_eq!(
                described.get("UpdateToken"),
                created.get("UpdateToken"),
                "case {case:?} resolved by {by:?}: expected the create's update token, got \
                 {described:?}"
            );
            let expected_document = specification.get("RuleGroup").cloned().unwrap_or_else(|| {
                // A `Rules` request is stored as the document that string defines.
                json!({ "RulesSource": { "RulesString": specification["Rules"] } })
            });
            assert_eq!(
                described.get("RuleGroup"),
                Some(&expected_document),
                "case {case:?} resolved by {by:?}: expected the rule group document to \
                 round-trip, got {described:?}"
            );
        }
    }
}

/// One row of the capacity table: a specification whose required capacity the
/// documented calculation fixes.
#[derive(Debug)]
struct CapacityCase {
    name: &'static str,
    group_type: &'static str,
    specification: Value,
    /// What `CreateRuleGroupRequest$Capacity`'s calculation requires: for a
    /// stateless rule group the sum over rules of the product of the match
    /// settings' element counts (an absent setting counts 1); for a stateful one
    /// the number of individual rules.
    required: i64,
}

/// The capacity table, shared by the tests that assert what it consumes and what
/// it refuses.
fn capacity_cases() -> Vec<CapacityCase> {
    vec![
        CapacityCase {
            name: "stateless-no-criteria",
            group_type: "STATELESS",
            specification: json!({ "RuleGroup": stateless_any() }),
            // "A rule with no criteria specified in any of its match settings
            // has a capacity requirement of 1."
            required: 1,
        },
        CapacityCase {
            name: "stateless-documented-example",
            group_type: "STATELESS",
            specification: json!({
                "RuleGroup": stateless_group(vec![stateless_rule(
                    100,
                    "aws:forward_to_sfe",
                    json!({
                        "Protocols": [6, 17],
                        "Sources": [
                            { "AddressDefinition": "10.0.0.0/24" },
                            { "AddressDefinition": "10.0.1.0/24" },
                            { "AddressDefinition": "10.0.2.0/24" },
                        ],
                        "Destinations": [{ "AddressDefinition": "20.1.0.0/24" }],
                    }),
                )]),
            }),
            // The doc's own worked example: "A rule with protocol setting
            // ["UDP","TCP"], source setting [three CIDRs], and a single
            // specification or no specification for each of the other match
            // settings has a capacity requirement of 6."
            required: 6,
        },
        CapacityCase {
            name: "stateless-ports-and-flags-plus-a-second-rule",
            group_type: "STATELESS",
            specification: json!({
                "RuleGroup": stateless_group(vec![
                    stateless_rule(
                        1,
                        "aws:pass",
                        json!({
                            "SourcePorts": [
                                { "FromPort": 1024, "ToPort": 2048 },
                                { "FromPort": 3000, "ToPort": 3000 },
                            ],
                            "DestinationPorts": [
                                { "FromPort": 80, "ToPort": 80 },
                                { "FromPort": 443, "ToPort": 443 },
                                { "FromPort": 8080, "ToPort": 8080 },
                            ],
                            "TCPFlags": [
                                { "Flags": ["SYN"], "Masks": ["SYN", "ACK"] },
                                { "Flags": ["ACK"], "Masks": ["SYN", "ACK"] },
                            ],
                        }),
                    ),
                    stateless_rule(2, "aws:drop", json!({})),
                ]),
            }),
            // 2 * 3 * 2 = 12 for the first rule, plus 1 for the second.
            required: 13,
        },
        CapacityCase {
            name: "stateless-two-sources-two-destinations",
            group_type: "STATELESS",
            specification: json!({
                "RuleGroup": stateless_group(vec![stateless_rule(
                    7,
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
                )]),
            }),
            required: 4,
        },
        CapacityCase {
            name: "stateful-two-rules",
            group_type: "STATEFUL",
            specification: json!({
                "RuleGroup": {
                    "RulesSource": {
                        "StatefulRules": [
                            stateful_one_rule()["RulesSource"]["StatefulRules"][0].clone(),
                            stateful_one_rule()["RulesSource"]["StatefulRules"][0].clone(),
                        ],
                    },
                },
            }),
            // "For a stateful rule group, the minimum capacity required is the
            // number of individual rules."
            required: 2,
        },
        CapacityCase {
            name: "stateful-suricata-string",
            group_type: "STATEFUL",
            specification: json!({
                "Rules": "pass tcp any any -> any any (sid:1;)\n\n\
                          # a comment carries no rule\n\
                          pass tcp any any -> any any (sid:2;)\n   \n\
                          pass tcp any any -> any any (sid:3;)\n",
            }),
            required: 3,
        },
        CapacityCase {
            name: "stateful-domain-list",
            group_type: "STATEFUL_DOMAIN",
            specification: json!({
                "RuleGroup": {
                    "RulesSource": {
                        "RulesSourceList": {
                            "Targets": [".a.test", ".b.test", "c.test", "d.test"],
                            "TargetTypes": ["TLS_SNI"],
                            "GeneratedRulesType": "DENYLIST",
                        },
                    },
                },
            }),
            required: 4,
        },
    ]
}

/// RULE: the capacity a rule group consumes is the documented calculation over
/// its rules — the product of a stateless rule's match-setting element counts
/// summed over its rules, and the number of individual rules for a stateful one —
/// and a dry run reports it without creating the rule group.
#[tokio::test]
async fn consumed_capacity_follows_the_documented_calculation() {
    let harness = Harness::new();
    for case in capacity_cases() {
        // "You can retrieve the capacity that would be required for a rule group
        // before you create the rule group by calling CreateRuleGroup with
        // DryRun set to TRUE."
        let mut dry_request =
            create_request(case.name, case.group_type, 30_000, &case.specification);
        dry_request["DryRun"] = json!(true);
        let dry = harness.ok("CreateRuleGroup", &dry_request).await;
        assert_eq!(
            dry["RuleGroupResponse"].get("ConsumedCapacity"),
            Some(&json!(case.required)),
            "case {case:?}: expected a dry run to report the required capacity, got {dry:?}"
        );

        let created = harness
            .ok(
                "CreateRuleGroup",
                &create_request(
                    case.name,
                    case.group_type,
                    case.required,
                    &case.specification,
                ),
            )
            .await;
        assert_eq!(
            created["RuleGroupResponse"].get("ConsumedCapacity"),
            Some(&json!(case.required)),
            "case {case:?}: expected the created rule group to consume its required capacity, \
             got {created:?}"
        );
        let described = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": case.name, "Type": case.group_type }),
            )
            .await;
        assert_eq!(
            described["RuleGroupResponse"].get("ConsumedCapacity"),
            Some(&json!(case.required)),
            "case {case:?}: expected the stored rule group to report the same consumed \
             capacity, got {described:?}"
        );
    }
}

/// RULE: a rule group whose rules need more capacity than it declares is refused,
/// because "rule group capacity is fixed at creation" and the policy that
/// references it reserves exactly that much.
#[tokio::test]
async fn rules_that_need_more_than_the_declared_capacity_are_refused() {
    let harness = Harness::new();
    for case in capacity_cases() {
        let (status, body) = harness
            .call(
                "CreateRuleGroup",
                &create_request(
                    case.name,
                    case.group_type,
                    case.required - 1,
                    &case.specification,
                ),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {case:?}: expected a rule group needing {} units to be refused at {} units, \
             got {status} {body:?}",
            case.required,
            case.required - 1
        );
        let (status, body) = harness
            .call(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": case.name, "Type": case.group_type }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused create must persist nothing, got {status} {body:?}"
        );
    }
}

/// RULE: `Capacity` is a positive number of capacity units, bounded above by what
/// this emulator retains per rule group (`state::MAX_RULE_GROUP_CAPACITY`), and a
/// request outside that range is refused with the error the model gives for it.
#[tokio::test]
async fn the_declared_capacity_must_be_inside_the_allowed_range() {
    use service_com_amazonaws_networkfirewall_networkfirewall_20201112::state;

    let ceiling = i64::from(state::MAX_RULE_GROUP_CAPACITY);
    let cases: [(&str, i64, Option<&str>); 5] = [
        ("zero", 0, Some("InvalidRequestException")),
        ("negative", -1, Some("InvalidRequestException")),
        ("lowest-accepted", 1, None),
        ("at-the-ceiling", ceiling, None),
        ("past-the-ceiling", ceiling + 1, Some("LimitExceededException")),
    ];

    let harness = Harness::new();
    for (name, capacity, expected_error) in cases {
        let (status, body) = harness
            .call(
                "CreateRuleGroup",
                &create_request(name, "STATELESS", capacity, &json!({ "RuleGroup": stateless_any() })),
            )
            .await;
        match expected_error {
            Some(code) => assert_eq!(
                (status, error_code(&body).as_str()),
                (400, code),
                "case {name:?} with Capacity {capacity}: expected {code}, got {status} {body:?}"
            ),
            None => assert_eq!(
                (status, body["RuleGroupResponse"].get("Capacity")),
                (200, Some(&json!(capacity))),
                "case {name:?} with Capacity {capacity}: expected the capacity to be accepted \
                 and reported, got {status} {body:?}"
            ),
        }
    }
}

/// RULE: a rule group is defined by exactly one `RulesSource`, which must belong
/// to the half of the service its `Type` names, and the specification comes from
/// either `RuleGroup` or `Rules` but not both — every other shape of request is a
/// bad request.
#[tokio::test]
async fn a_rule_group_document_must_match_its_type() {
    let cases: [(&str, &str, Value); 8] = [
        (
            "both-rulegroup-and-rules",
            "STATEFUL",
            json!({ "RuleGroup": stateful_one_rule(), "Rules": "pass tcp any any -> any any (sid:1;)" }),
        ),
        ("neither-rulegroup-nor-rules", "STATEFUL", json!({})),
        (
            "rules-string-for-a-stateless-group",
            "STATELESS",
            json!({ "Rules": "pass tcp any any -> any any (sid:1;)" }),
        ),
        (
            "stateful-rules-in-a-stateless-group",
            "STATELESS",
            json!({ "RuleGroup": stateful_one_rule() }),
        ),
        (
            "stateless-rules-in-a-stateful-group",
            "STATEFUL",
            json!({ "RuleGroup": stateless_any() }),
        ),
        (
            "two-rules-sources-at-once",
            "STATEFUL",
            json!({
                "RuleGroup": {
                    "RulesSource": {
                        "RulesString": "pass tcp any any -> any any (sid:1;)",
                        "StatefulRules": stateful_one_rule()["RulesSource"]["StatefulRules"].clone(),
                    },
                },
            }),
        ),
        (
            "no-rules-source-at-all",
            "STATEFUL",
            json!({ "RuleGroup": { "RulesSource": {} } }),
        ),
        (
            // `RuleGroup$RuleVariables`: "You can only use these for stateful
            // rule groups."
            "rule-variables-in-a-stateless-group",
            "STATELESS",
            json!({
                "RuleGroup": {
                    "RulesSource": stateless_any()["RulesSource"].clone(),
                    "RuleVariables": {
                        "IPSets": { "HOME_NET": { "Definition": ["10.0.0.0/16"] } },
                    },
                },
            }),
        ),
    ];

    let harness = Harness::new();
    for (name, group_type, specification) in &cases {
        let (status, body) = harness
            .call(
                "CreateRuleGroup",
                &create_request(name, group_type, 100, specification),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {name:?}: expected a bad {group_type} rule group document to be refused, got \
             {status} {body:?}"
        );
    }
}

/// RULE: a dry run "checks whether the request can run successfully, but doesn't
/// actually make the requested changes" — it answers the response the real call
/// would, and leaves the name free.
#[tokio::test]
async fn a_dry_run_reports_the_rule_group_without_creating_it() {
    let harness = Harness::new();
    for group_type in ["STATELESS", "STATEFUL"] {
        let specification = if group_type == "STATELESS" {
            json!({ "RuleGroup": stateless_any() })
        } else {
            json!({ "RuleGroup": stateful_one_rule() })
        };
        let mut request = create_request("dry-run-group", group_type, 10, &specification);
        request["DryRun"] = json!(true);

        let dry = harness.ok("CreateRuleGroup", &request).await;
        assert_eq!(
            string_at(&dry, &["RuleGroupResponse", "RuleGroupArn"]),
            expected_arn("dry-run-group", group_type),
            "case {group_type}: a dry run still answers the ARN the rule group would have, got \
             {dry:?}"
        );

        let (status, body) = harness
            .call(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": "dry-run-group", "Type": group_type }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceNotFoundException"),
            "case {group_type}: a dry run must not persist the rule group, got {status} {body:?}"
        );

        // The name is still free, so the real create succeeds.
        request["DryRun"] = json!(false);
        let real = harness.ok("CreateRuleGroup", &request).await;
        assert_eq!(
            string_at(&real, &["RuleGroupResponse", "RuleGroupName"]),
            "dry-run-group",
            "case {group_type}: expected the name to still be available, got {real:?}"
        );
    }
}

/// RULE: a rule group's identity is its name AND its type, so one name is taken
/// only within the half of the service its type names.
#[tokio::test]
async fn a_rule_group_name_is_taken_only_within_its_type() {
    let harness = Harness::new();
    let shared = "shared-name";

    for (group_type, specification) in [
        ("STATELESS", json!({ "RuleGroup": stateless_any() })),
        ("STATEFUL", json!({ "RuleGroup": stateful_one_rule() })),
    ] {
        let created = harness
            .ok(
                "CreateRuleGroup",
                &create_request(shared, group_type, 10, &specification),
            )
            .await;
        assert_eq!(
            string_at(&created, &["RuleGroupResponse", "RuleGroupArn"]),
            expected_arn(shared, group_type),
            "case {group_type}: expected each type's rule group to carry its own ARN, got \
             {created:?}"
        );

        // The same name within the SAME type is taken.
        let (status, body) = harness
            .call(
                "CreateRuleGroup",
                &create_request(shared, group_type, 10, &specification),
            )
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidRequestException"),
            "case {group_type}: expected a repeated name to be refused, got {status} {body:?}"
        );
    }

    // And the two rule groups are distinct resources, each reachable by its type.
    let mut ids = Vec::new();
    for group_type in ["STATELESS", "STATEFUL"] {
        let described = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": shared, "Type": group_type }),
            )
            .await;
        ids.push(string_at(&described, &["RuleGroupResponse", "RuleGroupId"]).to_string());
    }
    assert!(
        ids[0] != ids[1],
        "expected the stateless and stateful rule groups to be distinct resources, both \
         reported {ids:?}"
    );
}

/// RULE: a name identifies a rule group only together with its type, so
/// `DescribeRuleGroup$Type` "is required for requests that do not include the
/// RuleGroupARN"; an ARN carries the type itself, and a name sent beside an ARN
/// must agree with it.
#[tokio::test]
async fn describing_by_name_requires_the_type() {
    let harness = Harness::new();
    harness
        .ok(
            "CreateRuleGroup",
            &create_request(
                "typed-lookup",
                "STATEFUL",
                10,
                &json!({ "RuleGroup": stateful_one_rule() }),
            ),
        )
        .await;
    let arn = expected_arn("typed-lookup", "STATEFUL");

    let cases: [(&str, Value, Option<&str>); 5] = [
        ("name-without-type", json!({ "RuleGroupName": "typed-lookup" }), Some("InvalidRequestException")),
        ("no-identifier", json!({}), Some("InvalidRequestException")),
        ("name-with-type", json!({ "RuleGroupName": "typed-lookup", "Type": "STATEFUL" }), None),
        ("arn-alone", json!({ "RuleGroupArn": arn.clone() }), None),
        (
            "name-disagreeing-with-arn",
            json!({ "RuleGroupName": "another-group", "RuleGroupArn": arn.clone() }),
            Some("InvalidRequestException"),
        ),
    ];

    for (name, request, expected_error) in &cases {
        let (status, body) = harness.call("DescribeRuleGroup", request).await;
        match expected_error {
            Some(code) => assert_eq!(
                (status, error_code(&body).as_str()),
                (400, *code),
                "case {name:?}: expected {code}, got {status} {body:?}"
            ),
            None => assert_eq!(
                (status, body["RuleGroupResponse"].get("RuleGroupArn")),
                (200, Some(&json!(arn))),
                "case {name:?}: expected the stored rule group, got {status} {body:?}"
            ),
        }
    }

    // A name with the WRONG type names a rule group that does not exist.
    let (status, body) = harness
        .call(
            "DescribeRuleGroup",
            &json!({ "RuleGroupName": "typed-lookup", "Type": "STATELESS" }),
        )
        .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ResourceNotFoundException"),
        "expected the other type's name space to be empty, got {status} {body:?}"
    );
}

/// RULE: with `AnalyzeRuleGroup` set, a stateless rule group reports exactly the
/// two configurations `IdentifiedType` documents — a forwarding rule with no
/// return path in the group, and a forwarding rule that inspects TCP flags —
/// naming the priorities of the rules it identified; without it, nothing is
/// analyzed.
#[tokio::test]
async fn the_stateless_analyzer_reports_the_documented_findings() {
    /// A pass rule from `sources` to `destinations`, optionally inspecting TCP
    /// flags.
    fn directional(priority: i64, sources: &str, destinations: &str, tcp_flags: bool) -> Value {
        let mut attributes = json!({
            "Sources": [{ "AddressDefinition": sources }],
            "Destinations": [{ "AddressDefinition": destinations }],
        });
        if tcp_flags {
            attributes["TCPFlags"] = json!([{ "Flags": ["SYN"], "Masks": ["SYN", "ACK"] }]);
        }
        stateless_rule(priority, "aws:pass", attributes)
    }

    /// One expected finding: an `IdentifiedType` and the rule priorities it names.
    type Finding = (&'static str, Vec<&'static str>);

    // (case name, rules, expected findings)
    let cases: [(&str, Vec<Value>, Vec<Finding>); 5] = [
        (
            "symmetric-pair-is-clean",
            vec![
                directional(1, "10.1.0.0/24", "20.1.0.0/24", false),
                directional(2, "20.1.0.0/24", "10.1.0.0/24", false),
            ],
            vec![],
        ),
        (
            "match-anything-mirrors-itself",
            vec![stateless_rule(1, "aws:forward_to_sfe", json!({}))],
            vec![],
        ),
        (
            "one-way-rule-is-asymmetric",
            vec![directional(10, "10.1.0.0/24", "20.1.0.0/24", false)],
            vec![("STATELESS_RULE_FORWARDING_ASYMMETRICALLY", vec!["10"])],
        ),
        (
            "symmetric-pair-with-tcp-flags",
            vec![
                directional(1, "10.1.0.0/24", "20.1.0.0/24", true),
                directional(2, "20.1.0.0/24", "10.1.0.0/24", false),
            ],
            vec![("STATELESS_RULE_CONTAINS_TCP_FLAGS", vec!["1"])],
        ),
        (
            "asymmetric-rules-with-tcp-flags",
            vec![
                directional(5, "10.1.0.0/24", "20.1.0.0/24", true),
                directional(6, "10.2.0.0/24", "20.2.0.0/24", true),
                // A dropped rule is not forwarded traffic, so it is not analyzed.
                stateless_rule(
                    7,
                    "aws:drop",
                    json!({
                        "Sources": [{ "AddressDefinition": "10.3.0.0/24" }],
                        "Destinations": [{ "AddressDefinition": "20.3.0.0/24" }],
                        "TCPFlags": [{ "Flags": ["RST"] }],
                    }),
                ),
            ],
            vec![
                ("STATELESS_RULE_FORWARDING_ASYMMETRICALLY", vec!["5", "6"]),
                ("STATELESS_RULE_CONTAINS_TCP_FLAGS", vec!["5", "6"]),
            ],
        ),
    ];

    let harness = Harness::new();
    for (name, rules, expected) in &cases {
        let mut request = create_request(
            name,
            "STATELESS",
            30_000,
            &json!({ "RuleGroup": stateless_group(rules.clone()) }),
        );
        request["AnalyzeRuleGroup"] = json!(true);

        let created = harness.ok("CreateRuleGroup", &request).await;
        let expected_results: Vec<Value> = expected
            .iter()
            .map(|(identified_type, priorities)| {
                json!({ "IdentifiedType": identified_type, "IdentifiedRuleIds": priorities })
            })
            .collect();
        let reported: Vec<Value> = created["RuleGroupResponse"]["AnalysisResults"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: expected AnalysisResults in {created:?}"))
            .iter()
            .map(|result| {
                json!({
                    "IdentifiedType": result.get("IdentifiedType"),
                    "IdentifiedRuleIds": result.get("IdentifiedRuleIds"),
                })
            })
            .collect();
        assert_eq!(
            reported, expected_results,
            "case {name:?}: expected the documented findings, got {created:?}"
        );

        // The analyzer runs on a describe too, and only when asked.
        let described = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": name, "Type": "STATELESS", "AnalyzeRuleGroup": true }),
            )
            .await;
        assert_eq!(
            described["RuleGroupResponse"].get("AnalysisResults"),
            created["RuleGroupResponse"].get("AnalysisResults"),
            "case {name:?}: expected the describe to report the same findings, got {described:?}"
        );
        let unanalyzed = harness
            .ok(
                "DescribeRuleGroup",
                &json!({ "RuleGroupName": name, "Type": "STATELESS" }),
            )
            .await;
        assert_eq!(
            unanalyzed["RuleGroupResponse"].get("AnalysisResults"),
            None,
            "case {name:?}: expected no findings without AnalyzeRuleGroup, got {unanalyzed:?}"
        );
    }

    // The analyzer reports on STATELESS rule groups only, so a stateful one has
    // no findings rather than an empty list.
    let mut stateful = create_request(
        "analyzed-stateful",
        "STATEFUL",
        10,
        &json!({ "RuleGroup": stateful_one_rule() }),
    );
    stateful["AnalyzeRuleGroup"] = json!(true);
    let created = harness.ok("CreateRuleGroup", &stateful).await;
    assert_eq!(
        created["RuleGroupResponse"].get("AnalysisResults"),
        None,
        "expected no analysis for a stateful rule group, got {created:?}"
    );
}

/// RULE: a resource created without an `EncryptionConfiguration` reports the
/// Amazon Web Services owned key — "your data is encrypted by default with an
/// Amazon Web Services owned key" — naming that key in both members, and one
/// created with a configuration reports exactly what was given.
///
/// `response_projections` covers this rule over the whole class of requested
/// configurations and every path that reports one; this row keeps the
/// lifecycle suite's own create assertions honest.
#[tokio::test]
async fn encryption_configuration_defaults_to_the_aws_owned_key() {
    let customer_key = json!({ "Type": "CUSTOMER_KMS", "KeyId": "alias/net-fw" });
    let aws_owned = json!({ "Type": "AWS_OWNED_KMS_KEY", "KeyId": "AWS_OWNED_KMS_KEY" });

    let harness = Harness::new();
    for (suffix, requested) in [("default", None), ("explicit", Some(&customer_key))] {
        let expected = requested.unwrap_or(&aws_owned);

        // A rule group.
        let mut request = create_request(
            &format!("rule-group-{suffix}"),
            "STATELESS",
            10,
            &json!({ "RuleGroup": stateless_any() }),
        );
        if let Some(configuration) = requested {
            request["EncryptionConfiguration"] = configuration.clone();
        }
        let created = harness.ok("CreateRuleGroup", &request).await;
        assert_eq!(
            created["RuleGroupResponse"].get("EncryptionConfiguration"),
            Some(expected),
            "case {suffix:?}: expected the rule group's encryption settings, got {created:?}"
        );

        // A firewall policy.
        let policy_name = format!("policy-{suffix}");
        let mut policy_request = json!({
            "FirewallPolicyName": policy_name,
            "FirewallPolicy": {
                "StatelessDefaultActions": ["aws:forward_to_sfe"],
                "StatelessFragmentDefaultActions": ["aws:forward_to_sfe"],
            },
        });
        if let Some(configuration) = requested {
            policy_request["EncryptionConfiguration"] = configuration.clone();
        }
        let policy = harness.ok("CreateFirewallPolicy", &policy_request).await;
        assert_eq!(
            policy["FirewallPolicyResponse"].get("EncryptionConfiguration"),
            Some(expected),
            "case {suffix:?}: expected the policy's encryption settings, got {policy:?}"
        );

        // A firewall.
        let mut firewall_request = json!({
            "FirewallName": format!("firewall-{suffix}"),
            "FirewallPolicyArn": string_at(&policy, &["FirewallPolicyResponse", "FirewallPolicyArn"]),
            "VpcId": "vpc-0123abcd",
            "SubnetMappings": [{ "SubnetId": format!("subnet-{suffix}") }],
        });
        if let Some(configuration) = requested {
            firewall_request["EncryptionConfiguration"] = configuration.clone();
        }
        let firewall = harness.ok("CreateFirewall", &firewall_request).await;
        assert_eq!(
            firewall["Firewall"].get("EncryptionConfiguration"),
            Some(expected),
            "case {suffix:?}: expected the firewall's encryption settings, got {firewall:?}"
        );
    }
}
