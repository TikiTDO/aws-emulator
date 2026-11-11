// SPDX-License-Identifier: CC0-1.0

//! A resource type CloudFormation serves ITSELF, declared in a template the request does not
//! carry the body of: the template of a NESTED stack and the template of a StackSet.
//!
//! RULE under test: **the recognised set of resource types is the same wherever a template is
//! submitted for deployment** — `state::resource_types::BUILT_IN_RESOURCE_TYPES` is recognised
//! in a nested stack's own template and in a StackSet's template exactly as it is in the body
//! of a top-level `CreateStack`, the resource is deployed inertly in the stack whose template
//! declared it, and a name outside the recognised set is still refused there with HTTP 400,
//! `ValidationError` and `Template format error: Unrecognized resource types: [<types>]`.
//!
//! `cdk_metadata_resource_type.rs` owns that rule for the template a request carries itself
//! (`CreateStack`, `UpdateStack`, `CreateChangeSet`, `ValidateTemplate`); this suite is the
//! same rule for the two places a template is submitted for a stack the request does NOT name:
//! the `TemplateURL` of an `AWS::CloudFormation::Stack` resource, which CloudFormation deploys
//! into a stack of its own, and a StackSet's template, which its stack instances are deployed
//! from. Both are driven over every entry of that set rather than over one spelling, so an
//! entry added there is held to the same obligations.
//!
//! The rows differ along what the rule generalises over — where in a hierarchy the declaration
//! sits and what it carries: the depth it is declared at (a child of the request's stack, and a
//! grandchild), the logical id it is declared under, properties present and absent, a property
//! value of two sizes, and whether the declaring template also deploys a resource of its own in
//! an owning service. The refusing half is driven over the near misses of the same namespace at
//! both paths: refused on the way in where the request carries the body (a StackSet's), and
//! failing the deployment that submits it where a child template carries it.

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::resource_types::BUILT_IN_RESOURCE_TYPES;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::cdk_metadata_resource_type::{
    stack_name_for, template_declaring, ANALYTICS_PROPERTY, CDK_LOGICAL_ID,
};
use crate::change_set_lifecycle::settle_the_store;
use crate::harness::{
    call, create_stack, get_template, member, router, router_with_engine, xml_blocks, xml_text,
    xml_texts,
};
use crate::nested_stacks::{physical_resource_id, staged_url, STACK_RESOURCE_TYPE};
use crate::stack_lifecycle::describe_stack;
use crate::stack_resource_provisioning::s3;
use crate::template_by_reference::{create_staging_bucket, stage};

/// The logical id every template here declares its nested stack under.
const NESTED_LOGICAL_ID: &str = "Nested";

/// A property value of the form the CDK encodes its version report in, and a longer one: the
/// report names the libraries a stack was synthesized with, so its length is not fixed.
const SHORT_ANALYTICS: &str = "v2:deflate64:abc";
const LONG_ANALYTICS: &str = "v2:deflate64:aws-cdk-lib.Stack=2.0.0,\
                              aws-cdk-lib.aws_s3.Bucket=2.0.0,\
                              aws-cdk-lib.aws_lambda.Function=2.0.0";

/// One accepted row: a template declaring a built-in type somewhere BELOW the stack the request
/// names, and what the deployment owes for it.
struct Row {
    case: &'static str,
    /// What the names of this row's stacks and staged objects are distinguished by.
    suffix: &'static str,
    /// The logical id the declaring template gives the built-in resource.
    logical_id: &'static str,
    /// The `Analytics` value its properties carry, or `None` for a declaration with no
    /// `Properties` at all.
    analytics: Option<&'static str>,
    /// Whether the declaring template also deploys a bucket of its own, which is what says the
    /// stack carrying the inert resource really deployed.
    own_bucket: bool,
    /// How many levels below the stack the request names the declaration sits: 1 is the child
    /// an `AWS::CloudFormation::Stack` of the submitted body creates, 2 the grandchild that
    /// child's own template creates in turn.
    depth: usize,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a child template declaring it as the CDK synthesizes it",
            suffix: "kid",
            logical_id: CDK_LOGICAL_ID,
            analytics: Some(SHORT_ANALYTICS),
            own_bucket: false,
            depth: 1,
        },
        Row {
            case: "a child template declaring it with no properties, beside a bucket of its own",
            suffix: "bare",
            logical_id: CDK_LOGICAL_ID,
            analytics: None,
            own_bucket: true,
            depth: 1,
        },
        Row {
            case: "a child template declaring it under another logical id, with a long report",
            suffix: "named",
            logical_id: "StackMetadata",
            analytics: Some(LONG_ANALYTICS),
            own_bucket: false,
            depth: 1,
        },
        Row {
            case: "a grandchild template, two levels below the submitted body",
            suffix: "deep",
            logical_id: CDK_LOGICAL_ID,
            analytics: Some(SHORT_ANALYTICS),
            own_bucket: true,
            depth: 2,
        },
    ]
}

/// The bucket one row's declaring template deploys.
fn bucket_of(row: &Row, resource_type: &str) -> String {
    format!("{}-data", stack_name_for(resource_type, row.suffix))
}

/// [`template_declaring`] for one row of the nested leg.
fn declaring_template(resource_type: &str, row: &Row) -> String {
    let bucket = row.own_bucket.then(|| bucket_of(row, resource_type));
    template_declaring(
        resource_type,
        row.logical_id,
        row.analytics,
        bucket.as_deref(),
    )
}

/// A template declaring the built-in type BESIDE a type outside the recognised set: a refusal
/// of it must name only the one that was not recognised.
fn template_beside(resource_type: &str, unrecognised: &str) -> String {
    json!({
        "Resources": {
            CDK_LOGICAL_ID: { "Type": resource_type },
            "Unknown": { "Type": unrecognised }
        }
    })
    .to_string()
}

/// The message a submission declaring `spelled` is refused with, as the service words it for a
/// type it does not recognise.
fn unrecognized_message(spelled: &str) -> String {
    format!("Template format error: Unrecognized resource types: [{spelled}]")
}

/// A template whose one resource is a nested stack deploying the template at `key`.
fn nesting_template(key: &str) -> String {
    json!({
        "Resources": {
            NESTED_LOGICAL_ID: {
                "Type": STACK_RESOURCE_TYPE,
                "Properties": { "TemplateURL": staged_url(key) }
            }
        }
    })
    .to_string()
}

/// How many settling reads one row's hierarchy needs.
///
/// A stack's terminal status becomes due one span of the clock after the call that started it,
/// and a settling read materialises what is due; a nested level costs two of them — one for the
/// child's own create and one for the parent resource that waits on it — over the reads the
/// submitted stack's own create and deletion take. The budget is therefore stated in levels
/// rather than as a number, and a read past the last due instant settles nothing.
fn settling_reads(depth: usize) -> usize {
    3 + 2 * depth
}

/// The stack the row's declaration ended up in: the parent's nested stack, followed down one
/// level per depth the row declares.
async fn declaring_stack(
    case: &str,
    router: &MultiServiceRouter,
    parent: &str,
    depth: usize,
) -> String {
    let mut stack = physical_resource_id(case, router, parent, NESTED_LOGICAL_ID).await;
    for _ in 1..depth {
        stack = physical_resource_id(case, router, &stack, NESTED_LOGICAL_ID).await;
    }
    stack
}

/// RULE: a nested stack's template may declare a built-in type, and the resource is deployed
/// inertly in the CHILD stack — reported under the declared logical id and type at the
/// deployment's terminal resource status, with the properties it was submitted with still in
/// the stored template — whatever depth the declaration sits at.
#[tokio::test]
async fn a_built_in_type_in_a_nested_template_deploys_with_the_stack_that_declares_it() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    for resource_type in BUILT_IN_RESOURCE_TYPES {
        for row in rows() {
            let case = row.case;
            let parent = stack_name_for(resource_type, row.suffix);
            // The leaf template is staged, and one more object per level of nesting above it.
            let leaf_key = format!("{parent}-leaf.json");
            stage(&router, &leaf_key, &declaring_template(resource_type, &row)).await;
            let mut key = leaf_key.clone();
            for level in 1..row.depth {
                let above = format!("{parent}-level-{level}.json");
                stage(&router, &above, &nesting_template(&key)).await;
                key = above;
            }

            let submitted = nesting_template(&key);
            let (status_code, body) = create_stack(&router, &parent, &submitted, &[]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected the submitted body to be accepted, got {status_code} \
                 {body}"
            );
            // Each level of nesting is one span of the clock ([`settling_reads`]).
            for _ in 0..settling_reads(row.depth) {
                settle_the_store(&router, &engine).await;
            }
            let (status_code, body) = describe_stack(&router, &parent).await;
            assert_eq!(
                xml_text(&body, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {case:?}: expected the submitted stack complete, got {status_code} {body}"
            );

            let declaring = declaring_stack(case, &router, &parent, row.depth).await;
            let (status_code, body) = describe_stack(&router, &declaring).await;
            assert_eq!(
                xml_text(&body, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {case:?}: expected the declaring stack complete, got {status_code} {body}"
            );

            // The declaring stack reports the resource under the type and logical id it was
            // declared with, at the deployment's terminal resource status.
            let (status_code, body) = call(
                &router,
                "DescribeStackResources",
                &[member("StackName", &declaring)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: the declaring stack's resources must be readable, got \
                 {status_code} {body}"
            );
            let reported: Vec<(String, String, String)> = xml_blocks(&body, "member")
                .iter()
                .map(|resource| {
                    (
                        xml_text(resource, "LogicalResourceId").unwrap_or_default(),
                        xml_text(resource, "ResourceType").unwrap_or_default(),
                        xml_text(resource, "ResourceStatus").unwrap_or_default(),
                    )
                })
                .collect();
            assert!(
                reported.contains(&(
                    row.logical_id.to_string(),
                    (*resource_type).to_string(),
                    resource_status::CREATE_COMPLETE.to_string()
                )),
                "case {case:?}: expected {} reported as {resource_type} at {}, got {reported:?}",
                row.logical_id,
                resource_status::CREATE_COMPLETE
            );
            // The inert resource carries a physical resource id of its own, and READING it
            // does not change it: the same id comes back from a second read.
            let physical = physical_resource_id(case, &router, &declaring, row.logical_id).await;
            assert!(
                !physical.is_empty(),
                "case {case:?}: expected {} to carry a physical resource id, got an empty one",
                row.logical_id
            );
            let again = physical_resource_id(case, &router, &declaring, row.logical_id).await;
            assert_eq!(
                physical, again,
                "case {case:?}: expected the physical resource id of {} to be stable across \
                 reads, got {physical:?} then {again:?}",
                row.logical_id
            );

            // The template the declaring stack stored still carries the declaration, with the
            // properties it was submitted with.
            let (status_code, body) = get_template(&router, &declaring, None).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: the declaring stack's template must be readable, got \
                 {status_code} {body}"
            );
            let stored: Value = serde_json::from_str(
                &xml_text(&body, "TemplateBody").unwrap_or_default(),
            )
            .unwrap_or_else(|error| {
                panic!("case {case:?}: expected a JSON template body, got {body} ({error})")
            });
            let declaration = &stored["Resources"][row.logical_id];
            assert_eq!(
                declaration["Type"].as_str(),
                Some(*resource_type),
                "case {case:?}: expected the stored template to carry the declaration, got \
                 {stored}"
            );
            // The properties come back exactly as submitted: the report where the row
            // carried one, and no `Properties` member at all where it carried none.
            match row.analytics {
                Some(analytics) => assert_eq!(
                    declaration["Properties"][ANALYTICS_PROPERTY].as_str(),
                    Some(analytics),
                    "case {case:?}: expected the stored template to carry the submitted \
                     properties, got {stored}"
                ),
                None => assert!(
                    declaration.get("Properties").is_none(),
                    "case {case:?}: expected the stored template to carry no properties for a \
                     declaration submitted without any, got {stored}"
                ),
            }

            // ...and the stack really deployed: where the row declares a bucket of its own, it
            // exists in Amazon S3 beside the inert resource.
            if row.own_bucket {
                let bucket = bucket_of(&row, resource_type);
                let head = s3(&router, "HEAD", &format!("/{bucket}"), b"").await;
                assert_eq!(
                    head.status, 200,
                    "case {case:?}: expected the bucket {bucket} of the declaring stack to \
                     exist, got {}",
                    head.status
                );
            }

            // Deleting the stack the request named takes the whole hierarchy with it, the inert
            // resource included, with no failure.
            let (status_code, body) =
                call(&router, "DeleteStack", &[member("StackName", &parent)]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: the deletion must be accepted, got {status_code} {body}"
            );
            for _ in 0..settling_reads(row.depth) {
                settle_the_store(&router, &engine).await;
            }
            let (status_code, body) = describe_stack(&router, &declaring).await;
            assert_eq!(
                xml_text(&body, "StackStatus").as_deref(),
                Some(status::DELETE_COMPLETE),
                "case {case:?}: expected the declaring stack deleted, got {status_code} {body}"
            );
        }
    }
}

/// How a near miss of a built-in type is spelled, and the case it stands for. The entries are
/// the near misses of the same namespace, which is what makes the refusal a question about the
/// NAME rather than about the request.
struct NearMiss {
    case: &'static str,
    /// What the names of this row's stacks and staged objects are distinguished by.
    suffix: &'static str,
    spell: fn(&str) -> String,
}

const NEAR_MISSES: &[NearMiss] = &[
    NearMiss {
        case: "the type pluralised",
        suffix: "plural",
        spell: |resource_type| format!("{resource_type}s"),
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
];

/// RULE, for the other place a template is submitted for stacks the request does not name: a
/// StackSet's template may declare a built-in type — `CreateStackSet` and `UpdateStackSet`
/// accept it and the StackSet stores the body as submitted — and a name outside the recognised
/// set is refused there with the same wording.
#[tokio::test]
async fn a_stack_sets_template_may_declare_a_built_in_type() {
    let router = router();
    for resource_type in BUILT_IN_RESOURCE_TYPES {
        let case = *resource_type;
        let stack_set = stack_name_for(resource_type, "set");
        let created =
            template_declaring(resource_type, CDK_LOGICAL_ID, Some(SHORT_ANALYTICS), None);
        let (status_code, body) = call(
            &router,
            "CreateStackSet",
            &[
                member("StackSetName", &stack_set),
                member("TemplateBody", &created),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: CreateStackSet must accept a built-in type, got {status_code} {body}"
        );

        // The StackSet stores the body as submitted, which is what its instances deploy.
        let (status_code, body) = call(
            &router,
            "DescribeStackSet",
            &[member("StackSetName", &stack_set)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeStackSet must answer, got {status_code} {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(created.as_str()),
            "case {case:?}: expected the StackSet to store the submitted body, got {body}"
        );

        // A changed report goes through the same catalogue on the way in.
        let updated =
            template_declaring(resource_type, CDK_LOGICAL_ID, Some(LONG_ANALYTICS), None);
        let (status_code, body) = call(
            &router,
            "UpdateStackSet",
            &[
                member("StackSetName", &stack_set),
                member("TemplateBody", &updated),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: UpdateStackSet must accept a built-in type, got {status_code} {body}"
        );

        // ...and the StackSet goes on holding what the update submitted.
        let (status_code, body) = call(
            &router,
            "DescribeStackSet",
            &[member("StackSetName", &stack_set)],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: DescribeStackSet must answer after the update, got {status_code} \
             {body}"
        );
        assert_eq!(
            xml_text(&body, "TemplateBody").as_deref(),
            Some(updated.as_str()),
            "case {case:?}: expected the StackSet to store the updated body, got {body}"
        );

        // ...and a name outside the recognised set is still refused, named on its own.
        for near_miss in NEAR_MISSES {
            let spelled = (near_miss.spell)(resource_type);
            let case = near_miss.case;
            let template = template_beside(resource_type, &spelled);
            let (status_code, body) = call(
                &router,
                "CreateStackSet",
                &[
                    member("StackSetName", &stack_name_for(resource_type, "refused")),
                    member("TemplateBody", &template),
                ],
            )
            .await;
            assert_eq!(
                status_code, 400,
                "case {case:?} ({spelled:?}): expected HTTP 400 for an unrecognized resource \
                 type, got {status_code} {body}"
            );
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("ValidationError"),
                "case {case:?} ({spelled:?}): expected the wire code ValidationError, got {body}"
            );
            let message = xml_text(&body, "Message").unwrap_or_default();
            assert_eq!(
                message,
                unrecognized_message(&spelled),
                "case {case:?} ({spelled:?}): expected the refusal to name only the unrecognized \
                 type, got {message:?}"
            );
        }
    }
}

/// RULE, the REFUSING half at the nested path: a child template declaring a type outside the
/// recognised set is refused by the deployment that submits it, so the parent's resource for
/// that nested stack fails carrying the refusal, and the stack the request named does not reach
/// `CREATE_COMPLETE` — the built-in type declared beside it buys the unrecognised one nothing.
#[tokio::test]
async fn an_unrecognised_type_in_a_nested_template_fails_the_parents_resource() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    for resource_type in BUILT_IN_RESOURCE_TYPES {
        for near_miss in NEAR_MISSES {
            let spelled = (near_miss.spell)(resource_type);
            let case = near_miss.case;
            let parent = stack_name_for(resource_type, near_miss.suffix);
            let key = format!("{parent}-child.json");
            stage(&router, &key, &template_beside(resource_type, &spelled)).await;

            // The submitted body itself declares nothing unrecognised, so it is accepted and
            // the refusal is the CHILD deployment's.
            let (status_code, body) =
                create_stack(&router, &parent, &nesting_template(&key), &[]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?} ({spelled:?}): expected the submitted body to be accepted, got \
                 {status_code} {body}"
            );
            let stack_id = xml_text(&body, "StackId").unwrap_or_default();
            for _ in 0..settling_reads(1) {
                settle_the_store(&router, &engine).await;
            }

            // Read by its stack id: a rollback may have taken the name out of the live set.
            let (status_code, body) = describe_stack(&router, &stack_id).await;
            assert_eq!(
                status_code, 200,
                "case {case:?} ({spelled:?}): the stack must be readable by its id, got \
                 {status_code} {body}"
            );
            let reported = xml_text(&body, "StackStatus").unwrap_or_default();
            assert_ne!(
                reported,
                status::CREATE_COMPLETE,
                "case {case:?} ({spelled:?}): expected the deployment not to complete, got \
                 {status_code} {body}"
            );

            // ...and the refusal reached the stack that asked for the child.
            let (status_code, events) = call(
                &router,
                "DescribeStackEvents",
                &[member("StackName", &stack_id)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?} ({spelled:?}): the stack's events must be readable, got \
                 {status_code} {events}"
            );
            let expected = unrecognized_message(&spelled);
            assert!(
                xml_texts(&events, "ResourceStatusReason")
                    .iter()
                    .any(|reason| reason.contains(&expected)),
                "case {case:?} ({spelled:?}): expected a resource failure carrying \
                 {expected:?}, got {events}"
            );
        }
    }
}
