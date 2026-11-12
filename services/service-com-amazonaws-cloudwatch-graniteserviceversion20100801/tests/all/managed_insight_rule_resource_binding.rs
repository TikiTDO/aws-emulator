// SPDX-License-Identifier: CC0-1.0

//! Cover for the RESOURCE BINDING a managed Contributor Insights rule carries in its
//! definition, read back the way a client reads it: through `ListManagedInsightRules` and
//! `DescribeInsightRules` after `PutManagedInsightRules` enabled the rule.
//!
//! The rule under test: **every managed rule an entry of a `PutManagedInsightRules` batch
//! enables carries, under `Resource` in its `Definition`, exactly the `ResourceARN` that entry
//! named** — whichever published template the entry named, however many other entries of the
//! same batch could not be enabled, and whatever else the same batch enabled for other
//! resources. "When you enable a managed rule, you create a Contributor Insights rule that
//! collects data from Amazon Web Services services." (`PutManagedInsightRules`) and the rule is
//! reported with "the definition of the rule, as a JSON object"
//! (`smithy_query(query="shape",shape="InsightRule")`, member `Definition`), so the one
//! resource such a rule collects from is what its definition names: a client that holds only
//! the minted rule name recovers the monitored resource from the definition it reads back.
//!
//! What `managed_insight_rule_definition.rs` pins is the definition of ONE rule enabled on its
//! own from a named template. This suite drives the loop a client actually runs, so the rows
//! differ along what the rule generalises over: which of the templates
//! `ListManagedInsightRules` publishes for the resource the entry names (its first, its last,
//! all of them), where the entry sits among entries the catalog cannot resolve (before, after,
//! between), which half of the catalog published the template (the service's own, this
//! account's), and the kind, partition, Region, account and resource-field form of the ARN.
//! Every template name a request carries is taken from the listing — "as returned by
//! `ListManagedInsightRules`" (`ManagedRule$TemplateName`) — rather than written into the row.

use crate::harness::{build_router_with_engine, ok, publish_managed_rule_template};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// What a managed rule reports as its schema: "For managed rules, this is `{"Name":
/// "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`), as the compact schema that value
/// names.
const MANAGED_SCHEMA: &str = "ServiceLogRule/1";

/// A Contributor Insights rule that is evaluating: "Indicates whether the rule is enabled or
/// disabled." (`InsightRule$State`)
const ENABLED: &str = "ENABLED";

/// One resource a row enables managed rules for.
struct Resource {
    /// The `ResourceARN` the row's entries carry for it.
    arn: &'static str,
    /// Templates the row publishes into THIS account's half of the catalog for that resource
    /// before reading the listing; empty when the service publishes the resource's templates
    /// itself (a VPC endpoint service's).
    publish: &'static [&'static str],
}

/// One entry of the row's `PutManagedInsightRules` batch, in the order the request carries it.
enum Entry {
    /// Enable, for `resources[resource]`, the template `ListManagedInsightRules` reports for it
    /// at index `template` — so the entry names a template exactly as the listing returned it.
    Enable { resource: usize, template: usize },
    /// Name, for `resources[resource]`, a template name the catalog publishes for nothing: the
    /// entry cannot be enabled and comes back among "the rules that could not be enabled"
    /// (`PutManagedInsightRulesOutput$Failures`), identified by the template it named.
    Reject {
        resource: usize,
        template: &'static str,
    },
}

/// One row: the resources it works with and the batch it submits for them.
struct Row {
    /// What the row is, for the case name every assertion carries.
    case: &'static str,
    /// The resources the row's entries name, indexed by [`Entry`].
    resources: &'static [Resource],
    /// The batch, in request order.
    entries: &'static [Entry],
}

/// A VPC endpoint service, whose managed-rule templates the SERVICE publishes.
const VPC_ENDPOINT_SERVICE: Resource = Resource {
    arn: "arn:aws:ec2:us-east-1:123456789012:vpc-endpoint-service/vpce-svc-0123456789abcdef0",
    publish: &[],
};

/// The rows.
const ROWS: &[Row] = &[
    Row {
        case: "the listing's first template, between two entries the catalog cannot resolve",
        resources: &[VPC_ENDPOINT_SERVICE],
        entries: &[
            Entry::Reject {
                resource: 0,
                template: "NoSuchTemplate",
            },
            Entry::Enable {
                resource: 0,
                template: 0,
            },
            Entry::Reject {
                resource: 0,
                template: "VpcEndpointService-Imaginary-v9",
            },
        ],
    },
    Row {
        case: "the listing's last template, as the only entry of the batch",
        resources: &[VPC_ENDPOINT_SERVICE],
        entries: &[Entry::Enable {
            resource: 0,
            template: 3,
        }],
    },
    Row {
        case: "every template the listing publishes for one resource, enabled in one batch",
        resources: &[VPC_ENDPOINT_SERVICE],
        entries: &[
            Entry::Enable {
                resource: 0,
                template: 0,
            },
            Entry::Enable {
                resource: 0,
                template: 1,
            },
            Entry::Enable {
                resource: 0,
                template: 2,
            },
            Entry::Enable {
                resource: 0,
                template: 3,
            },
        ],
    },
    Row {
        case: "the only resolvable entry is the last one of the batch",
        resources: &[VPC_ENDPOINT_SERVICE],
        entries: &[
            Entry::Reject {
                resource: 0,
                template: "Rst-Packets",
            },
            Entry::Reject {
                resource: 0,
                template: "VpcEndpointService-BytesByEndpointId-v0",
            },
            Entry::Enable {
                resource: 0,
                template: 1,
            },
        ],
    },
    Row {
        case: "two resources of different kinds, partitions, Regions and accounts in one batch",
        resources: &[
            Resource {
                arn: "arn:aws-cn:ec2:cn-north-1:210987654321:vpc-endpoint-service/vpce-svc-0abc1234",
                publish: &[],
            },
            Resource {
                arn: "arn:aws:dynamodb:eu-west-3:123456789012:table/Orders",
                publish: &["DynamoDB-ContributorInsights-v1"],
            },
        ],
        entries: &[
            Entry::Enable {
                resource: 0,
                template: 0,
            },
            Entry::Enable {
                resource: 1,
                template: 0,
            },
            Entry::Reject {
                resource: 1,
                template: "DynamoDB-Unpublished-v1",
            },
        ],
    },
    Row {
        case: "an account-published template for an ARN carrying neither Region nor account",
        resources: &[Resource {
            arn: "arn:aws:s3:::forged-access-logs",
            publish: &["S3-RequestsByRequester-v2"],
        }],
        entries: &[Entry::Enable {
            resource: 0,
            template: 0,
        }],
    },
    Row {
        case: "a resource whose ARN names it after a colon, with an unversioned template",
        resources: &[Resource {
            arn: "arn:aws:lambda:us-west-2:000000000000:function:checkout",
            publish: &["Lambda-Insights"],
        }],
        entries: &[Entry::Enable {
            resource: 0,
            template: 0,
        }],
    },
];

#[tokio::test]
async fn every_rule_a_batch_enables_names_its_own_resource_in_its_definition() {
    for row in ROWS {
        // A fresh emulator per row, so the row's own batch is the only thing that enabled a
        // rule and the rules a read reports are exactly the ones it asked for.
        let (router, engine) = build_router_with_engine();
        for resource in row.resources {
            for template in resource.publish {
                publish_managed_rule_template(&engine, template, resource.arn);
            }
        }

        // The templates each entry may name, read from the listing the model points a caller at.
        let mut published: Vec<Vec<String>> = Vec::new();
        for resource in row.resources {
            published.push(published_templates(&router, resource.arn).await);
        }

        let mut batch: Vec<Value> = Vec::new();
        let mut expected_failures: Vec<String> = Vec::new();
        // The (template, resource) pairs the batch asks to have enabled, in request order.
        let mut expected_rules: Vec<(String, &'static str)> = Vec::new();
        for entry in row.entries {
            match entry {
                Entry::Enable { resource, template } => {
                    let arn = row.resources[*resource].arn;
                    let name = published[*resource].get(*template).unwrap_or_else(|| {
                        panic!(
                            "case {:?}: the listing publishes a template at index {template} \
                             for {arn}, got {:?}",
                            row.case, published[*resource]
                        )
                    });
                    batch.push(json!({"TemplateName": name, "ResourceARN": arn}));
                    expected_rules.push((name.clone(), arn));
                }
                Entry::Reject { resource, template } => {
                    let arn = row.resources[*resource].arn;
                    assert!(
                        !published[*resource].iter().any(|held| held == template),
                        "case {:?}: {template} is a name the catalog publishes for nothing, \
                         got the listing {:?}",
                        row.case,
                        published[*resource]
                    );
                    batch.push(json!({"TemplateName": template, "ResourceARN": arn}));
                    expected_failures.push((*template).to_string());
                }
            }
        }

        let put = ok(
            &router,
            "PutManagedInsightRules",
            &json!({"ManagedRules": batch}),
        )
        .await;
        // "An array that lists the rules that could not be enabled."
        // (`PutManagedInsightRulesOutput$Failures`) — every entry naming a template the catalog
        // does not publish, and nothing else.
        let failures: Vec<String> = put
            .get("Failures")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|failure| {
                failure
                    .get("FailureResource")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        assert_eq!(
            failures, expected_failures,
            "case {:?}: the unresolvable entries are reported back, got {put}",
            row.case
        );

        let described = described_rules(&router).await;
        assert_eq!(
            described.len(),
            expected_rules.len(),
            "case {:?}: expected one rule per enabled entry, got {described:?}",
            row.case
        );

        for (template, arn) in &expected_rules {
            // The rule's NAME is what the listing hands a client back for the pair: "The name of
            // the Contributor Insights rule that contains data for the specified Amazon Web
            // Services resource." (`ManagedRuleState$RuleName`)
            let description = description_of(&router, arn, template).await.unwrap_or_else(|| {
                panic!(
                    "case {:?}: the listing for {arn} describes template {template}",
                    row.case
                )
            });
            let rule_name = description
                .pointer("/RuleState/RuleName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                description.pointer("/RuleState/State").and_then(Value::as_str),
                Some(ENABLED),
                "case {:?}: the enabled rule for {template} is reported enabled, got \
                 {description}",
                row.case
            );

            let rule = described
                .iter()
                .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(&rule_name))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: DescribeInsightRules reports the rule {rule_name}, got \
                         {described:?}",
                        row.case
                    )
                });
            assert_eq!(
                (
                    rule.get("ManagedRule").and_then(Value::as_bool),
                    rule.get("Schema").and_then(Value::as_str),
                    rule.get("State").and_then(Value::as_str),
                ),
                (Some(true), Some(MANAGED_SCHEMA), Some(ENABLED)),
                "case {:?}: {rule_name} is an enabled managed rule, got {rule}",
                row.case
            );

            let definition = definition_of(rule, row.case);
            assert_eq!(
                definition.get("Resource").and_then(Value::as_str),
                Some(*arn),
                "case {:?}: the definition of {rule_name} names the resource it collects \
                 from, got {definition}",
                row.case
            );
            assert_eq!(
                definition.get("TemplateName").and_then(Value::as_str),
                Some(template.as_str()),
                "case {:?}: the definition of {rule_name} names the template it was enabled \
                 from, got {definition}",
                row.case
            );
        }

        // No rule the batch enabled is bound to a resource no entry named: the binding is per
        // rule, not one value the whole batch shares.
        for rule in &described {
            let definition = definition_of(rule, row.case);
            let bound = definition
                .get("Resource")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            assert!(
                row.resources.iter().any(|resource| resource.arn == bound),
                "case {:?}: every definition names a resource the batch enabled a rule for, \
                 got {bound:?}",
                row.case
            );
        }
    }
}

#[tokio::test]
async fn deleting_one_managed_rule_leaves_the_other_bindings_alone() {
    // "Permanently deletes the specified Contributor Insights rules." (`DeleteInsightRules`) —
    // the rules it does not name keep the resource binding they were enabled with, so a client
    // that deletes one resource's managed rule still recovers the others' resources.
    let resources = [
        "arn:aws:ec2:us-east-1:123456789012:vpc-endpoint-service/vpce-svc-0123456789abcdef0",
        "arn:aws:ec2:us-east-1:123456789012:vpc-endpoint-service/vpce-svc-0fedcba9876543210",
        "arn:aws-us-gov:ec2:us-gov-west-1:210987654321:vpc-endpoint-service/vpce-svc-0a1b2c3d",
    ];
    let (router, _engine) = build_router_with_engine();

    let mut batch: Vec<Value> = Vec::new();
    let mut pairs: Vec<(String, &str)> = Vec::new();
    for arn in &resources {
        let template = published_templates(&router, arn)
            .await
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case \"setup\": the listing publishes a template for {arn}"));
        batch.push(json!({"TemplateName": template, "ResourceARN": arn}));
        pairs.push((template, arn));
    }
    let put = ok(
        &router,
        "PutManagedInsightRules",
        &json!({"ManagedRules": batch}),
    )
    .await;
    assert_eq!(
        put.get("Failures").and_then(Value::as_array),
        Some(&Vec::<Value>::new()),
        "case \"setup\": enabling a published template per resource reports no failure, got {put}"
    );

    // Each resource's rule is identified by the NAME the listing reports for it, because the
    // three were enabled from templates of the same kind of resource and so from the same
    // template name: the rule name is what distinguishes them.
    let mut enabled: Vec<(String, &str)> = Vec::new();
    for (template, arn) in &pairs {
        let rule_name = description_of(&router, arn, template)
            .await
            .and_then(|description| {
                description
                    .pointer("/RuleState/RuleName")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| panic!("case \"setup\": {arn} has a rule enabled from {template}"));
        enabled.push((rule_name, arn));
    }

    let (dropped, _) = enabled.remove(0);
    ok(
        &router,
        "DeleteInsightRules",
        &json!({"RuleNames": [&dropped]}),
    )
    .await;

    let described = described_rules(&router).await;
    assert_eq!(
        described.len(),
        enabled.len(),
        "case \"after one deletion\": the rules not named are kept, got {described:?}"
    );
    for (rule_name, arn) in &enabled {
        let rule = described
            .iter()
            .find(|rule| rule.get("Name").and_then(Value::as_str) == Some(rule_name.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case \"after one deletion\": the rule {rule_name} for {arn} is still \
                     reported, got {described:?}"
                )
            });
        let definition = definition_of(rule, "after one deletion");
        assert_eq!(
            definition.get("Resource").and_then(Value::as_str),
            Some(*arn),
            "case \"after one deletion\": the surviving rule still names its own resource, got \
             {definition}"
        );
    }
}

/// The template names `ListManagedInsightRules` publishes for `resource_arn`, in the order it
/// reports them.
async fn published_templates(router: &MultiServiceRouter, resource_arn: &str) -> Vec<String> {
    let body = ok(
        router,
        "ListManagedInsightRules",
        &json!({"ResourceARN": resource_arn}),
    )
    .await;
    body.get("ManagedRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|description| {
            description
                .get("TemplateName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

/// The description `ListManagedInsightRules` reports for `template_name` under `resource_arn`.
async fn description_of(
    router: &MultiServiceRouter,
    resource_arn: &str,
    template_name: &str,
) -> Option<Value> {
    let body = ok(
        router,
        "ListManagedInsightRules",
        &json!({"ResourceARN": resource_arn}),
    )
    .await;
    body.get("ManagedRules")?
        .as_array()?
        .iter()
        .find(|description| {
            description.get("TemplateName").and_then(Value::as_str) == Some(template_name)
        })
        .cloned()
}

/// Every rule `DescribeInsightRules` reports.
async fn described_rules(router: &MultiServiceRouter) -> Vec<Value> {
    ok(router, "DescribeInsightRules", &json!({}))
        .await
        .get("InsightRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A reported rule's `Definition`, parsed: "The definition of the rule, as a JSON object."
/// (`InsightRule$Definition`)
fn definition_of(rule: &Value, case: &str) -> Value {
    let text = rule
        .get("Definition")
        .and_then(Value::as_str)
        .unwrap_or_default();
    serde_json::from_str(text).unwrap_or_else(|error| {
        panic!("case {case:?}: a rule's definition is a JSON object, got {text:?} ({error})")
    })
}
