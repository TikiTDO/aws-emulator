// SPDX-License-Identifier: CC0-1.0

//! What a parent stack READS OFF a nested stack: the child's own `Outputs`, end to end over
//! the awsQuery wire.
//!
//! RULE: the `Fn::GetAtt` return values of an `AWS::CloudFormation::Stack` resource are the
//! OUTPUTS of the stack CloudFormation created for it, addressed as `Outputs.<OutputKey>`.
//! They are not a set the resource TYPE publishes — they are whatever the child's own
//! template declares — so each one carries the value the CHILD resolved for that output, and
//! a parent declaration or declared output over one reports that same value. An output key
//! the child does not carry publishes nothing, so a reference to it resolves to no value at
//! all.
//!
//! `nested_stack_output_attributes.rs` holds that rule over a CreateStack deployment; what
//! this suite adds is the two things that suite does not reach: the DEPLOYMENT PATH the
//! recorded trace took — a change set over a template staged in Amazon S3 — and a parent
//! RESOURCE PROPERTY (rather than an output) reading one of the child's outputs, read back
//! out of the service that OWNS that resource.
//!
//! The rows differ along what the rule generalises over: which KIND of value the child's
//! output carries (a resource's physical name, an attribute of a resource, a second published
//! attribute, fixed template text, and two values an intrinsic function composes), how the
//! parent SPELLS the reference (the long `["Child", "Outputs.<key>"]` form and the short
//! dotted string), and whether the key the parent names is one the child declares at all.
//! Beside the table, the parent's own `AWS::SNS::Topic` carries the value the child's output
//! decided in its `DisplayName`, read back through Amazon SNS; and the whole table is
//! asserted a second time over a `CreateChangeSet(TemplateURL=…)` + `ExecuteChangeSet`
//! deployment of the same parent.
//!
//! Every expected value is the one the CHILD's own `DescribeStacks` reports for that output,
//! read back through the wire: the rule is that the two agree, so no row restates the naming
//! or ARN form of any resource type.

use serde_json::json;

use crate::change_set_lifecycle::{create_change_set, execute_change_set, settle_the_store};
use crate::harness::{call, create_stack, member, router_with_engine, xml_blocks, xml_text};
use crate::nested_stacks::{staged_url, STACK_RESOURCE_TYPE};
use crate::stack_lifecycle::{describe_stack, sns};
use crate::stack_outputs::{output_for, wire_outputs, WireOutput};
use crate::stack_resources::sns_arn;
use crate::template_by_reference::{create_staging_bucket, stage};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// How the parent names the attribute it reads: the two forms `Fn::GetAtt` takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// `{"Fn::GetAtt": ["<logical id>", "Outputs.<key>"]}`.
    LongForm,
    /// `{"Fn::GetAtt": "<logical id>.Outputs.<key>"}`.
    ShortForm,
}

impl Spelling {
    /// The declaration that reads `Outputs.<key>` off the resource `logical_id`.
    fn reference(self, logical_id: &str, key: &str) -> serde_json::Value {
        match self {
            Self::LongForm => json!({"Fn::GetAtt": [logical_id, format!("Outputs.{key}")]}),
            Self::ShortForm => json!({"Fn::GetAtt": format!("{logical_id}.Outputs.{key}")}),
        }
    }
}

/// One row: an output the parent declares over one output of the child.
struct Row {
    case: &'static str,
    /// The `OutputKey` the PARENT declares.
    parent_key: &'static str,
    /// The `OutputKey` of the CHILD it reads.
    child_key: &'static str,
    spelling: Spelling,
    /// Whether the child declares `child_key` at all: a key it does not carries no value to
    /// read, so the parent's output is reported with no `OutputValue`.
    declared_by_the_child: bool,
}

/// The logical id the parent declares its nested stack under.
const CHILD: &str = "Child";

/// The bucket the child's own template creates, which its outputs are built from.
const CHILD_BUCKET: &str = "nest-outputs-child-data";

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an output carrying a resource's physical name, read in the long form",
            parent_key: "NestedBucketName",
            child_key: "BucketName",
            spelling: Spelling::LongForm,
            declared_by_the_child: true,
        },
        Row {
            case: "an output carrying a resource's ARN, read in the short form",
            parent_key: "NestedBucketArn",
            child_key: "BucketArn",
            spelling: Spelling::ShortForm,
            declared_by_the_child: true,
        },
        Row {
            case: "an output carrying another published attribute of that resource",
            parent_key: "NestedBucketDomain",
            child_key: "BucketDomain",
            spelling: Spelling::LongForm,
            declared_by_the_child: true,
        },
        Row {
            case: "an output carrying fixed template text, read in the short form",
            parent_key: "NestedLabel",
            child_key: "Label",
            spelling: Spelling::ShortForm,
            declared_by_the_child: true,
        },
        Row {
            case: "an output an Fn::Sub composes",
            parent_key: "NestedComposed",
            child_key: "Composed",
            spelling: Spelling::LongForm,
            declared_by_the_child: true,
        },
        Row {
            case: "an output an Fn::Join composes, read in the short form",
            parent_key: "NestedJoined",
            child_key: "Joined",
            spelling: Spelling::ShortForm,
            declared_by_the_child: true,
        },
        Row {
            case: "a key the child's template declares no output under",
            parent_key: "NestedAbsent",
            child_key: "NotDeclaredAnywhere",
            spelling: Spelling::LongForm,
            declared_by_the_child: false,
        },
    ]
}

/// The child's template: one bucket, and one output per KIND of value an output can carry.
fn child_template() -> String {
    json!({
        "Resources": {
            "Data": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": CHILD_BUCKET}
            }
        },
        "Outputs": {
            "BucketName": {"Value": {"Ref": "Data"}},
            "BucketArn": {"Value": {"Fn::GetAtt": ["Data", "Arn"]}},
            "BucketDomain": {"Value": {"Fn::GetAtt": "Data.DomainName"}},
            "Label": {"Value": "the child's own label"},
            "Composed": {"Value": {"Fn::Sub": "${Data}/under-a-prefix"}},
            "Joined": {"Value": {"Fn::Join": ["-", [{"Ref": "Data"}, "joined"]]}}
        }
    })
    .to_string()
}

/// The logical id of the parent's own `AWS::SNS::Topic`, the resource whose PROPERTIES read
/// the child's output rather than reporting it as an output of the parent.
const TOPIC: &str = "ChildBucketTopic";

/// The parent's template: the nested stack the table reads, a topic whose `DisplayName` is
/// one of the child's outputs — so the read is exercised in a resource PROPERTY as well —
/// and one declared output per row.
fn parent_template(child_key: &str, topic_name: &str) -> String {
    let mut outputs = serde_json::Map::new();
    for row in rows() {
        outputs.insert(
            row.parent_key.to_string(),
            json!({"Value": row.spelling.reference(CHILD, row.child_key)}),
        );
    }
    json!({
        "Resources": {
            CHILD: {
                "Type": STACK_RESOURCE_TYPE,
                "Properties": {"TemplateURL": staged_url(child_key)}
            },
            TOPIC: {
                "Type": "AWS::SNS::Topic",
                "Properties": {
                    "TopicName": topic_name,
                    "DisplayName": {"Fn::GetAtt": [CHILD, "Outputs.BucketName"]}
                }
            }
        },
        "Outputs": outputs
    })
    .to_string()
}

/// The `Outputs` a settled stack reports, read through `DescribeStacks`.
async fn settled_outputs(
    case: &str,
    router: &smithy_server_runtime::MultiServiceRouter,
    stack: &str,
) -> Vec<WireOutput> {
    let (code, body) = describe_stack(router, stack).await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStacks({stack}) must answer, got {code} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::CREATE_COMPLETE),
        "case {case:?}: expected {stack} complete, got {body}"
    );
    wire_outputs(&body)
}

/// The child stack's id, which the parent reports as its nested-stack resource's
/// `PhysicalResourceId`.
async fn child_stack_id(
    case: &str,
    router: &smithy_server_runtime::MultiServiceRouter,
    parent: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", parent),
            member("LogicalResourceId", CHILD),
        ],
    )
    .await;
    assert_eq!(
        code,
        200,
        "case {case:?}: the parent's nested-stack resource must be readable, got {code} \
         {body}; the parent's events were {}",
        stack_events(router, parent).await
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// The parent's own status and its newest events, for the diagnostic a read that should have
/// succeeded carries: a deployment that did not come up says why in its events.
async fn stack_events(
    router: &smithy_server_runtime::MultiServiceRouter,
    stack: &str,
) -> String {
    let (_, described) = describe_stack(router, stack).await;
    let (_, body) = call(router, "DescribeStackEvents", &[member("StackName", stack)]).await;
    let newest: Vec<String> = xml_blocks(&body, "member")
        .iter()
        .take(6)
        .map(|event| {
            format!(
                "{:?} {:?} {:?}",
                xml_text(event, "LogicalResourceId"),
                xml_text(event, "ResourceStatus"),
                xml_text(event, "ResourceStatusReason")
            )
        })
        .collect();
    format!(
        "status {:?}, newest events {newest:?}",
        xml_text(&described, "StackStatus")
    )
}

/// Assert the whole table against one deployed parent: each declared output carries the
/// value the CHILD reports for the output it reads, and a key the child does not declare
/// carries none.
async fn assert_the_table(
    case: &str,
    router: &smithy_server_runtime::MultiServiceRouter,
    parent: &str,
    topic_name: &str,
) {
    let child_id = child_stack_id(case, router, parent).await;
    let child_outputs = settled_outputs(case, router, &child_id).await;
    let parent_outputs = settled_outputs(case, router, parent).await;

    for row in rows() {
        let label = format!("{case} / {}", row.case);
        let reported = output_for(&parent_outputs, row.parent_key).unwrap_or_else(|| {
            panic!(
                "case {label:?}: expected the parent to report an output {:?}, got \
                 {parent_outputs:?}",
                row.parent_key
            )
        });
        if !row.declared_by_the_child {
            assert_eq!(
                output_for(&child_outputs, row.child_key),
                None,
                "case {label:?}: this row's premise is that the child declares no such \
                 output, got {child_outputs:?}"
            );
            assert_eq!(
                reported.value, None,
                "case {label:?}: expected no OutputValue for an output nothing publishes, \
                 got {:?}",
                reported.value
            );
            continue;
        }
        let published = output_for(&child_outputs, row.child_key)
            .and_then(|output| output.value.clone())
            .unwrap_or_else(|| {
                panic!(
                    "case {label:?}: the child must report a value for {:?}, got \
                     {child_outputs:?}",
                    row.child_key
                )
            });
        assert!(
            !published.is_empty(),
            "case {label:?}: the child's own output must carry a value, got {published:?}"
        );
        assert_eq!(
            reported.value.as_deref(),
            Some(published.as_str()),
            "case {label:?}: expected the parent's OutputValue to be the child's, got {:?}",
            reported.value
        );
    }

    // ...and the same read inside a resource PROPERTY: the topic the parent declared really
    // carries the value the child's output decided, which is only possible once that value
    // resolved — read back through Amazon SNS's own wire.
    let child_bucket = output_for(&child_outputs, "BucketName")
        .and_then(|output| output.value.clone())
        .unwrap_or_default();
    let arn = sns_arn(topic_name);
    let (code, attributes) = sns(router, "GetTopicAttributes", &[("TopicArn", &arn)]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: GetTopicAttributes({arn}) must answer, got {code} {attributes}"
    );
    let display_name = xml_blocks(&attributes, "entry")
        .iter()
        .find(|entry| xml_text(entry, "key").as_deref() == Some("DisplayName"))
        .and_then(|entry| xml_text(entry, "value"));
    assert_eq!(
        display_name.as_deref(),
        Some(child_bucket.as_str()),
        "case {case:?}: expected the topic's DisplayName to carry the child's output value, \
         got {display_name:?}"
    );
}

/// RULE: a nested stack's `Fn::GetAtt` return values are the child stack's own outputs, read
/// as `Outputs.<OutputKey>` — so every declared output of the parent over one carries the
/// value the child resolved for it.
#[tokio::test]
async fn a_parent_reads_the_nested_stacks_outputs() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    let child_key = "nest-outputs-child.json";
    stage(&router, child_key, &child_template()).await;

    let parent = "nest-outputs";
    let topic_name = "nest-outputs-child-bucket";
    let (code, body) =
        create_stack(&router, parent, &parent_template(child_key, topic_name), &[]).await;
    assert_eq!(code, 200, "the parent must be created, got {code} {body}");
    // Each level of nesting is one span of the clock, and the parent's own topic is only
    // created by the pass that runs once the child's wait releases.
    for _ in 0..8 {
        settle_the_store(&router, &engine).await;
    }

    assert_the_table("a parent created directly", &router, parent, topic_name).await;
}

/// RULE: the same reads hold however the template reached the stack. A change set over a
/// template staged in Amazon S3 deploys the very same document, so the parent it executes
/// reports the same resolved outputs — which is the path the recorded trace took.
#[tokio::test]
async fn a_change_set_over_a_staged_template_resolves_them_too() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    let child_key = "nest-outputs-changeset-child.json";
    stage(&router, child_key, &child_template()).await;
    let parent_key = "nest-outputs-changeset-parent.json";
    let topic_name = "nest-outputs-changeset-child-bucket";
    stage(&router, parent_key, &parent_template(child_key, topic_name)).await;

    let parent = "nest-outputs-changeset";
    let (code, body) = create_change_set(
        &router,
        parent,
        "add-the-nested-stack",
        &[
            member("ChangeSetType", "CREATE"),
            member("TemplateURL", &staged_url(parent_key)),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "the change set must be created, got {code} {body}"
    );
    settle_the_store(&router, &engine).await;

    let (code, body) = execute_change_set(&router, "add-the-nested-stack", Some(parent)).await;
    assert_eq!(
        code, 200,
        "the change set must be executed, got {code} {body}"
    );
    for _ in 0..8 {
        settle_the_store(&router, &engine).await;
    }

    assert_the_table(
        "a parent deployed by a change set",
        &router,
        parent,
        topic_name,
    )
    .await;
}
