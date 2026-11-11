// SPDX-License-Identifier: CC0-1.0

//! The resource types CloudFormation serves ITSELF, end to end over the awsQuery wire.
//!
//! RULE under test: **a template may declare a resource type CloudFormation serves itself —
//! one whose deployment calls no owning service, so there is nothing to register or activate
//! — and such a resource is deployed inertly**: every operation that submits a template
//! accepts the body, the resource is recorded under the type and logical id the template
//! declared, it reaches the deployment's own terminal resource status, a property change
//! updates it in place, the stack's deletion removes it without a failure, and the stored
//! template still carries the properties it was submitted with. A name OUTSIDE the
//! recognised set is still refused with HTTP 400, `ValidationError` and `Template format
//! error: Unrecognized resource types: [<types>]`.
//!
//! `state::resource_types::BUILT_IN_RESOURCE_TYPES` is that set, and the rows below are
//! driven over every entry of it rather than over one spelling, so an entry added there is
//! held to the same five obligations. `unrecognized_resource_types.rs` owns the refusing half
//! of the catalogue rule across namespaces, and `appconfig_resource_types.rs` /
//! `cognito_resource_types.rs` own the admitting half for a PUBLISHED namespace; this suite
//! is the admitting half for the types that are in no published namespace at all.
//!
//! The rows differ along what the rule generalises over — how the declaration is spelled and
//! what it carries: properties present and absent, a property value of several shapes and
//! sizes, the logical id (the one the AWS CDK emits and another), a condition that holds and
//! one that does not, and both template spellings (JSON and YAML). The refusing rows are the
//! near misses of the same namespace: a plural, the wrong case, a resource name nothing
//! serves, a module suffix and another organisation's namespace.
//!
//! Such a type has no provisioner, so each resource is declared-only
//! (`unprovisioned_resources.rs` pins that nothing is called in an owning service and the
//! stack still completes) and carries the physical resource id CloudFormation's own naming
//! rules give it — which is what this suite asserts is STABLE across an update, not what its
//! characters are.

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::{
    is_published, is_recognized, BUILT_IN_RESOURCE_TYPES, CDK_METADATA,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};

use crate::list_stack_resources::{list_stack_resources, wire_summaries};
use crate::stack_lifecycle::{
    call, create_stack, describe_stack, get_template, member, outside_parameters, router,
    wait_for_the_transition, xml_text, REGION,
};
use crate::stack_resources::{element, resources_of, wire_resources};

/// The logical id the AWS CDK gives the resource it adds to every stack it synthesizes.
///
/// `pub(crate)` with [`ANALYTICS_PROPERTY`] and [`template_declaring`] because the suite that
/// carries the same rule into a nested stack's and a StackSet's template
/// (`built_in_types_in_submitted_hierarchies.rs`) declares the same resource, so the id, the
/// property and the template shape are spelled ONCE.
pub(crate) const CDK_LOGICAL_ID: &str = "CDKMetadata";

/// The name of the condition the AWS CDK guards that resource with.
const CDK_CONDITION: &str = "CDKMetadataAvailable";

/// A property value of the form the CDK encodes its version report in.
const ANALYTICS: &str = "v2:deflate64:H4sIAAAAAAAA";

/// The property that report is carried in. Source: the issue's own template and the AWS CDK
/// Developer Guide's "Version reporting" section, which is where this resource's one property
/// is published.
pub(crate) const ANALYTICS_PROPERTY: &str = "Analytics";

/// One accepted row: a template and what the deployment owes for the declared resource.
struct Accepted {
    case: &'static str,
    /// The built-in type the row declares.
    resource_type: &'static str,
    stack_name: String,
    template: String,
    /// The logical id the stack must report the resource under, or `None` where the row's
    /// condition does not hold and the stack must report no resource at all.
    expected_logical_id: Option<&'static str>,
}

/// A `Fn::Or` of `Fn::Equals` over `AWS::Region`, as the CDK emits its condition: true for a
/// list carrying the Region the request is signed for, false for one that does not.
fn region_condition(regions: &[&str]) -> Value {
    json!({
        "Fn::Or": regions
            .iter()
            .map(|region| json!({ "Fn::Equals": [{ "Ref": "AWS::Region" }, region] }))
            .collect::<Vec<Value>>()
    })
}

/// The stack name one row of `resource_type` runs under: the type's segments, lower-cased,
/// which is inside a stack name's own alphabet.
///
/// `pub(crate)` because the suite that carries the same rule into a nested stack's and a
/// StackSet's template (`built_in_types_in_submitted_hierarchies.rs`) names its stacks and its
/// staged objects the same way, so there is ONE spelling of it.
pub(crate) fn stack_name_for(resource_type: &str, suffix: &str) -> String {
    let spelled = resource_type.replace("::", "-").to_ascii_lowercase();
    format!("{spelled}-{suffix}")
}

/// Every accepted row for one built-in type.
fn accepted_rows(resource_type: &'static str) -> Vec<Accepted> {
    let mut rows = vec![
        Accepted {
            case: "the declaration the CDK synthesizes: its logical id and an analytics string",
            resource_type,
            stack_name: stack_name_for(resource_type, "cdk"),
            template: json!({
                "Resources": {
                    CDK_LOGICAL_ID: {
                        "Type": resource_type,
                        "Properties": { ANALYTICS_PROPERTY: ANALYTICS }
                    }
                }
            })
            .to_string(),
            expected_logical_id: Some(CDK_LOGICAL_ID),
        },
        Accepted {
            case: "no properties at all",
            resource_type,
            stack_name: stack_name_for(resource_type, "bare"),
            template: json!({ "Resources": { CDK_LOGICAL_ID: { "Type": resource_type } } })
                .to_string(),
            expected_logical_id: Some(CDK_LOGICAL_ID),
        },
        Accepted {
            case: "an empty property value",
            resource_type,
            stack_name: stack_name_for(resource_type, "empty-property"),
            template: json!({
                "Resources": {
                    CDK_LOGICAL_ID: {
                        "Type": resource_type,
                        "Properties": { ANALYTICS_PROPERTY: "" }
                    }
                }
            })
            .to_string(),
            expected_logical_id: Some(CDK_LOGICAL_ID),
        },
        Accepted {
            case: "a long property value",
            resource_type,
            stack_name: stack_name_for(resource_type, "long-property"),
            template: json!({
                "Resources": {
                    CDK_LOGICAL_ID: {
                        "Type": resource_type,
                        "Properties": { ANALYTICS_PROPERTY: format!("v2:deflate64:{}", "A".repeat(600)) }
                    }
                }
            })
            .to_string(),
            expected_logical_id: Some(CDK_LOGICAL_ID),
        },
        Accepted {
            // The rule is about the TYPE: a template may declare it under any logical id.
            case: "another logical id, beside a resource of a published type",
            resource_type,
            stack_name: stack_name_for(resource_type, "other-logical-id"),
            template: json!({
                "Resources": {
                    "Metadata": {
                        "Type": resource_type,
                        "Properties": { ANALYTICS_PROPERTY: ANALYTICS }
                    },
                    "Topic": {
                        "Type": "AWS::SNS::Topic",
                        "Properties": { "TopicName": "cdk-metadata-topic" }
                    }
                }
            })
            .to_string(),
            expected_logical_id: Some("Metadata"),
        },
        Accepted {
            case: "a condition that holds for the Region the request is signed for",
            resource_type,
            stack_name: stack_name_for(resource_type, "condition-holds"),
            template: json!({
                "Conditions": { CDK_CONDITION: region_condition(&[REGION, "eu-west-1"]) },
                "Resources": {
                    CDK_LOGICAL_ID: {
                        "Type": resource_type,
                        "Condition": CDK_CONDITION,
                        "Properties": { ANALYTICS_PROPERTY: ANALYTICS }
                    }
                }
            })
            .to_string(),
            expected_logical_id: Some(CDK_LOGICAL_ID),
        },
        Accepted {
            case: "a condition that does not hold, which leaves the resource undeployed",
            resource_type,
            stack_name: stack_name_for(resource_type, "condition-fails"),
            template: json!({
                "Conditions": { CDK_CONDITION: region_condition(&["eu-west-1", "ap-south-1"]) },
                "Resources": {
                    CDK_LOGICAL_ID: {
                        "Type": resource_type,
                        "Condition": CDK_CONDITION,
                        "Properties": { ANALYTICS_PROPERTY: ANALYTICS }
                    }
                }
            })
            .to_string(),
            expected_logical_id: None,
        },
    ];
    rows.push(Accepted {
        case: "the YAML spelling of the same declaration",
        resource_type,
        stack_name: stack_name_for(resource_type, "yaml"),
        template: format!(
            "Resources:\n  {CDK_LOGICAL_ID}:\n    Type: {resource_type}\n    Properties:\n      \
             {ANALYTICS_PROPERTY}: {ANALYTICS}\n"
        ),
        expected_logical_id: Some(CDK_LOGICAL_ID),
    });
    rows
}

/// `CreateStack` accepts every accepted row, the stack completes, and the resource the
/// template declared is reported by each of the three per-resource reads under the type and
/// logical id it was declared with — or by none of them, where the row's condition does not
/// hold.
#[tokio::test]
async fn a_built_in_type_is_accepted_and_deployed_inertly() {
    let router = router();
    let rows: Vec<Accepted> = BUILT_IN_RESOURCE_TYPES
        .iter()
        .flat_map(|resource_type| accepted_rows(resource_type))
        .collect();
    // Submit every row first, then wait once: the transition delay is per stack and runs
    // from that stack's own create.
    for row in &rows {
        let (status_code, body) = create_stack(&router, &row.stack_name, &row.template, &[]).await;
        assert_eq!(
            status_code, 200,
            "case {:?}: expected HTTP 200 from CreateStack for a type CloudFormation serves \
             itself, got {status_code} with body {body}",
            row.case
        );
    }
    wait_for_the_transition().await;

    for row in &rows {
        let case = row.case;
        let (status_code, body) = describe_stack(&router, &row.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStacks to report the created stack, got \
             {status_code} with body {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: expected the stack to reach {:?}, got body {body}",
            status::CREATE_COMPLETE
        );

        // DescribeStackResources: the whole-stack read.
        let (status_code, described) = resources_of(&router, &row.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStackResources to answer, got {status_code} with \
             body {described}"
        );
        let reported = wire_resources(&described);
        let Some(expected_logical_id) = row.expected_logical_id else {
            assert!(
                reported.is_empty(),
                "case {case:?}: expected no resource for a declaration whose condition does not \
                 hold, got {reported:?}"
            );
            continue;
        };
        let found = reported
            .iter()
            .find(|resource| resource.logical_id.as_deref() == Some(expected_logical_id));
        let Some(found) = found else {
            panic!(
                "case {case:?}: expected a resource reported under {expected_logical_id:?}, got \
                 {reported:?}"
            );
        };
        let declared_type = row.resource_type;
        assert_eq!(
            found.resource_type.as_deref(),
            Some(declared_type),
            "case {case:?}: expected the declared type to be reported, got {:?}",
            found.resource_type
        );
        assert_eq!(
            found.status.as_deref(),
            Some(resource_status::CREATE_COMPLETE),
            "case {case:?}: expected the resource to report {:?}, got {:?}",
            resource_status::CREATE_COMPLETE,
            found.status
        );
        let physical_id = found.physical_id.clone().unwrap_or_default();
        assert!(
            !physical_id.is_empty(),
            "case {case:?}: expected a physical resource id for the declared resource, got none \
             in {reported:?}"
        );

        // DescribeStackResource: the single-resource read of the same record.
        let (status_code, detail) = call(
            &router,
            "DescribeStackResource",
            &[
                member("StackName", &row.stack_name),
                member("LogicalResourceId", expected_logical_id),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStackResource to answer, got {status_code} with body \
             {detail}"
        );
        assert_eq!(
            element(&detail, "ResourceType").as_deref(),
            Some(declared_type),
            "case {case:?}: expected DescribeStackResource to report the declared type, got body \
             {detail}"
        );
        assert_eq!(
            element(&detail, "ResourceStatus").as_deref(),
            Some(resource_status::CREATE_COMPLETE),
            "case {case:?}: expected DescribeStackResource to report {:?}, got body {detail}",
            resource_status::CREATE_COMPLETE
        );
        assert_eq!(
            element(&detail, "PhysicalResourceId").as_deref(),
            Some(physical_id.as_str()),
            "case {case:?}: expected the two reads to report one physical id {physical_id:?}, got \
             body {detail}"
        );

        // ListStackResources: the summary read of the same record.
        let (status_code, listed) =
            list_stack_resources(&router, &[member("StackName", &row.stack_name)]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected ListStackResources to answer, got {status_code} with body \
             {listed}"
        );
        let summarised = wire_summaries(&listed);
        let summary = summarised
            .iter()
            .find(|summary| summary.logical_id.as_deref() == Some(expected_logical_id));
        let Some(summary) = summary else {
            panic!(
                "case {case:?}: expected ListStackResources to summarise \
                 {expected_logical_id:?}, got {summarised:?}"
            );
        };
        assert_eq!(
            summary.resource_type.as_deref(),
            Some(declared_type),
            "case {case:?}: expected ListStackResources to report the declared type, got {:?}",
            summary.resource_type
        );
        assert_eq!(
            summary.status.as_deref(),
            Some(resource_status::CREATE_COMPLETE),
            "case {case:?}: expected ListStackResources to report {:?}, got {:?}",
            resource_status::CREATE_COMPLETE,
            summary.status
        );

        // The stack's own event log carries the resource's completed transition.
        let (status_code, events) = call(
            &router,
            "DescribeStackEvents",
            &[member("StackName", &row.stack_name)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStackEvents to answer, got {status_code} with body \
             {events}"
        );
        assert!(
            events.contains(declared_type),
            "case {case:?}: expected an event for the declared type {declared_type:?}, got body \
             {events}"
        );

        // GetTemplate (Original) answers the submitted body, properties included.
        let (status_code, stored) = get_template(&router, &row.stack_name, Some("Original")).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected GetTemplate to answer, got {status_code} with body {stored}"
        );
        assert_eq!(
            xml_text(&stored, "TemplateBody").as_deref(),
            Some(row.template.as_str()),
            "case {case:?}: expected the stored template to be the submitted body, got body \
             {stored}"
        );
    }
}

/// One refusing row: a name of the same namespace nothing serves.
struct NearMiss {
    case: &'static str,
    suffix: &'static str,
    /// How the near miss is spelled from the built-in type it is a near miss OF.
    spell: fn(&str) -> String,
}

/// The near misses of each built-in type: accepting the set must not accept
/// `AWS::<its namespace>::<anything>`, and the case of a type name is part of it.
const NEAR_MISSES: &[NearMiss] = &[
    NearMiss {
        case: "the type, pluralised",
        suffix: "plural",
        spell: |resource_type| format!("{resource_type}s"),
    },
    NearMiss {
        case: "the type in lower case",
        suffix: "lower-case",
        spell: |resource_type| resource_type.to_ascii_lowercase(),
    },
    NearMiss {
        case: "a resource name of the same namespace nothing serves",
        suffix: "invented",
        spell: |resource_type| {
            let namespace = resource_type
                .rsplit_once("::")
                .map(|(head, _)| head.to_string())
                .unwrap_or_default();
            format!("{namespace}::Analytics")
        },
    },
    NearMiss {
        case: "the type with a module suffix",
        suffix: "module",
        spell: |resource_type| format!("{resource_type}::MODULE"),
    },
    NearMiss {
        case: "another organisation's namespace",
        suffix: "third-party",
        spell: |resource_type| {
            let resource = resource_type.rsplit("::").next().unwrap_or_default();
            let namespace = resource_type
                .split("::")
                .nth(1)
                .unwrap_or_default()
                .to_string();
            format!("MyOrg::{namespace}::{resource}")
        },
    },
];

/// A name outside the recognised set is still refused, and nothing is created for it.
#[tokio::test]
async fn a_near_miss_of_a_built_in_type_is_still_refused() {
    let router = router();
    for resource_type in BUILT_IN_RESOURCE_TYPES {
        for near_miss in NEAR_MISSES {
            let spelled = (near_miss.spell)(resource_type);
            let case = near_miss.case;
            let stack_name = stack_name_for(resource_type, near_miss.suffix);
            // The built-in type beside the unrecognised one: the refusal names only what it
            // did not recognise.
            let template = json!({
                "Resources": {
                    CDK_LOGICAL_ID: { "Type": resource_type },
                    "Unknown": { "Type": spelled }
                }
            })
            .to_string();
            let (status_code, body) = create_stack(&router, &stack_name, &template, &[]).await;
            assert_eq!(
                status_code, 400,
                "case {case:?} ({spelled:?}): expected HTTP 400 for an unrecognized resource \
                 type, got {status_code} with body {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?} ({spelled:?}): expected the wire code ValidationError, got body \
                 {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            assert_eq!(
                message,
                format!("Template format error: Unrecognized resource types: [{spelled}]"),
                "case {case:?} ({spelled:?}): expected the refusal to name only the unrecognized \
                 type, got {message:?}"
            );
            let (status_code, body) = describe_stack(&router, &stack_name).await;
            assert_eq!(
                status_code, 400,
                "case {case:?} ({spelled:?}): expected the refused stack not to exist, \
                 DescribeStacks answered {status_code} with body {body}"
            );
        }
    }
}

/// A template declaring ONE resource of `resource_type` under `logical_id`, carrying the
/// version report where `analytics` names one and no `Properties` at all where it does not,
/// plus a bucket of the template's own where `bucket` names one.
///
/// The ONE template builder of the suites that declare a built-in type: a StackSet's body and
/// the document a nested stack's `TemplateURL` addresses are the same template a `CreateStack`
/// carries inline, so no suite spells that shape itself.
pub(crate) fn template_declaring(
    resource_type: &str,
    logical_id: &str,
    analytics: Option<&str>,
    bucket: Option<&str>,
) -> String {
    let mut declaration = Map::new();
    declaration.insert("Type".to_string(), json!(resource_type));
    if let Some(analytics) = analytics {
        declaration.insert(
            "Properties".to_string(),
            json!({ ANALYTICS_PROPERTY: analytics }),
        );
    }
    let mut resources = Map::new();
    resources.insert(logical_id.to_string(), Value::Object(declaration));
    if let Some(bucket) = bucket {
        resources.insert(
            "Data".to_string(),
            json!({
                "Type": "AWS::S3::Bucket",
                "Properties": { "BucketName": bucket }
            }),
        );
    }
    json!({ "Resources": resources }).to_string()
}

/// The template a stack of one built-in type is created with, carrying `analytics`.
fn analytics_template(resource_type: &str, analytics: &str) -> String {
    template_declaring(resource_type, CDK_LOGICAL_ID, Some(analytics), None)
}

/// A property change updates the resource IN PLACE — the same physical resource id, reported
/// `UPDATE_COMPLETE` — and the stack's deletion then removes it without a failure.
#[tokio::test]
async fn a_property_change_updates_in_place_and_the_stack_deletes() {
    let router = router();
    for resource_type in BUILT_IN_RESOURCE_TYPES {
        let case = *resource_type;
        let stack_name = stack_name_for(resource_type, "update-and-delete");
        let (status_code, body) = create_stack(
            &router,
            &stack_name,
            &analytics_template(resource_type, ANALYTICS),
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to accept the template, got {status_code} with \
             body {body}"
        );
        wait_for_the_transition().await;

        let (_, described) = resources_of(&router, &stack_name).await;
        let created = wire_resources(&described);
        let before = created
            .iter()
            .find(|resource| resource.logical_id.as_deref() == Some(CDK_LOGICAL_ID))
            .and_then(|resource| resource.physical_id.clone())
            .unwrap_or_default();
        assert!(
            !before.is_empty(),
            "case {case:?}: expected a physical resource id after the create, got {created:?}"
        );

        // An `Analytics` change: the resource goes on existing, with the new value.
        let next = format!("{ANALYTICS}NEXT");
        let (status_code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", &stack_name),
                member("TemplateBody", &analytics_template(resource_type, &next)),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to accept a changed property, got {status_code} \
             with body {body}"
        );
        wait_for_the_transition().await;

        let (status_code, body) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStacks to answer after the update, got {status_code} \
             with body {body}"
        );
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::UPDATE_COMPLETE),
            "case {case:?}: expected the stack to reach {:?}, got body {body}",
            status::UPDATE_COMPLETE
        );

        let (status_code, described) = resources_of(&router, &stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStackResources to answer after the update, got \
             {status_code} with body {described}"
        );
        let updated = wire_resources(&described);
        let found = updated
            .iter()
            .find(|resource| resource.logical_id.as_deref() == Some(CDK_LOGICAL_ID));
        let Some(found) = found else {
            panic!("case {case:?}: expected the resource to survive the update, got {updated:?}");
        };
        assert_eq!(
            found.status.as_deref(),
            Some(resource_status::UPDATE_COMPLETE),
            "case {case:?}: expected the resource to report {:?} after a property change, got {:?}",
            resource_status::UPDATE_COMPLETE,
            found.status
        );
        assert_eq!(
            found.physical_id.as_deref(),
            Some(before.as_str()),
            "case {case:?}: expected an in-place update to keep the physical resource id \
             {before:?}, got {:?}",
            found.physical_id
        );
        // The stored template is the one the update submitted, with the new property value.
        let (status_code, stored) = get_template(&router, &stack_name, Some("Original")).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected GetTemplate to answer after the update, got {status_code} \
             with body {stored}"
        );
        assert_eq!(
            xml_text(&stored, "TemplateBody").as_deref(),
            Some(analytics_template(resource_type, &next).as_str()),
            "case {case:?}: expected the stored template to carry the updated property, got body \
             {stored}"
        );

        // The deletion removes it with no error, and the stack is gone.
        let (status_code, body) = call(
            &router,
            "DeleteStack",
            &[member("StackName", &stack_name)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DeleteStack to be accepted, got {status_code} with body \
             {body}"
        );
        wait_for_the_transition().await;
        let (status_code, body) = describe_stack(&router, &stack_name).await;
        assert_eq!(
            status_code, 400,
            "case {case:?}: expected the deleted stack to be gone, DescribeStacks answered \
             {status_code} with body {body}"
        );
    }
}

/// The other operations that submit a template for deployment are held to the same set:
/// `CreateChangeSet` (for a new stack and for an existing one) and `ValidateTemplate` accept a
/// body declaring a built-in type.
#[tokio::test]
async fn every_operation_that_submits_a_template_accepts_a_built_in_type() {
    let router = router();
    for resource_type in BUILT_IN_RESOURCE_TYPES {
        let case = *resource_type;
        let template = analytics_template(resource_type, ANALYTICS);
        let stack_name = stack_name_for(resource_type, "change-set");

        let (status_code, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", &stack_name),
                member("ChangeSetName", "a-create-change-set"),
                member("ChangeSetType", "CREATE"),
                member("TemplateBody", &template),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateChangeSet (CREATE) to accept the template, got \
             {status_code} with body {body}"
        );

        // ValidateTemplate is NOT held to the catalogue — its documented scope is the body's
        // FORM — so it answers a VERDICT for a body declaring the type: what that template
        // declares, read back off the response.
        let described = json!({
            "Description": "a CDK-synthesized stack",
            "Parameters": { "Stage": { "Type": "String", "Default": "dev" } },
            "Resources": {
                CDK_LOGICAL_ID: {
                    "Type": resource_type,
                    "Properties": { ANALYTICS_PROPERTY: ANALYTICS }
                }
            }
        })
        .to_string();
        let (status_code, body) = call(
            &router,
            "ValidateTemplate",
            &[member("TemplateBody", &described)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected ValidateTemplate to answer a verdict, got {status_code} with \
             body {body}"
        );
        assert_eq!(
            xml_text(&outside_parameters(&body), "Description").as_deref(),
            Some("a CDK-synthesized stack"),
            "case {case:?}: expected ValidateTemplate to report the template's own Description, \
             got body {body}"
        );
        assert_eq!(
            xml_text(&body, "ParameterKey").as_deref(),
            Some("Stage"),
            "case {case:?}: expected ValidateTemplate to report the declared parameter, got body \
             {body}"
        );

        // An existing stack whose update ADDS the resource: the same catalogue decides the
        // submitted body.
        let existing = stack_name_for(resource_type, "added-by-update");
        let (status_code, body) = create_stack(
            &router,
            &existing,
            r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic","Properties":{"TopicName":"cdk-added-topic"}}}}"#,
            &[],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the setup stack to be created, got {status_code} with body \
             {body}"
        );
        wait_for_the_transition().await;
        let widened = json!({
            "Resources": {
                "Topic": {
                    "Type": "AWS::SNS::Topic",
                    "Properties": { "TopicName": "cdk-added-topic" }
                },
                CDK_LOGICAL_ID: {
                    "Type": resource_type,
                    "Properties": { ANALYTICS_PROPERTY: ANALYTICS }
                }
            }
        })
        .to_string();
        let (status_code, body) = call(
            &router,
            "CreateChangeSet",
            &[
                member("StackName", &existing),
                member("ChangeSetName", "an-update-change-set"),
                member("TemplateBody", &widened),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateChangeSet on a live stack to accept the template, got \
             {status_code} with body {body}"
        );
        let (status_code, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", &existing),
                member("TemplateBody", &widened),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to accept a template that adds the resource, got \
             {status_code} with body {body}"
        );
        wait_for_the_transition().await;
        let (status_code, described) = resources_of(&router, &existing).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected DescribeStackResources to answer, got {status_code} with \
             body {described}"
        );
        let reported = wire_resources(&described);
        let added = reported
            .iter()
            .find(|resource| resource.logical_id.as_deref() == Some(CDK_LOGICAL_ID));
        let Some(added) = added else {
            panic!("case {case:?}: expected the added resource to be recorded, got {reported:?}");
        };
        assert_eq!(
            added.resource_type.as_deref(),
            Some(case),
            "case {case:?}: expected the added resource's declared type, got {:?}",
            added.resource_type
        );
    }
}

/// The set itself, as the rule depends on it: every entry is recognised, no entry is in the
/// PUBLISHED catalogue (it is in no published namespace at all), and the type the AWS CDK
/// adds is one of them.
#[test]
fn the_built_in_set_is_recognized_and_is_not_the_published_catalogue() {
    assert!(
        !BUILT_IN_RESOURCE_TYPES.is_empty(),
        "expected at least one type CloudFormation serves itself, got none"
    );
    for resource_type in BUILT_IN_RESOURCE_TYPES {
        assert!(
            is_recognized(resource_type),
            "type {resource_type:?}: expected a type CloudFormation serves itself to be \
             recognized, it is not"
        );
        assert!(
            !is_published(resource_type),
            "type {resource_type:?}: expected the published catalogue not to carry it, it does"
        );
    }
    assert!(
        BUILT_IN_RESOURCE_TYPES.contains(&CDK_METADATA),
        "expected the type the AWS CDK adds to every synthesized stack to be one of \
         {BUILT_IN_RESOURCE_TYPES:?}"
    );
}
