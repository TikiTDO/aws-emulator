// SPDX-License-Identifier: CC0-1.0

//! A NESTED STACK — an `AWS::CloudFormation::Stack` resource — end to end over the awsQuery
//! wire.
//!
//! RULE: a stack created by an `AWS::CloudFormation::Stack` resource is a STACK of its own.
//! CloudFormation creates it under the name `<parent name>-<logical id>-<unique id>` with a
//! stack id of its own, deploys the template the resource's `TemplateURL` addresses into it,
//! and reports the parent's resource with that stack id as its `PhysicalResourceId`. So a
//! status-filtered `ListStacks` answers a summary for the child beside the parent's, the
//! child's `ParentId` and `RootId` are the parent's stack id (for a first level of nesting the
//! root is the parent), the parent still counts the nested stack as exactly one resource of
//! that type, and the child's own template really is deployed — its resources exist in the
//! services that own them.
//!
//! The rows differ along what the rule generalises over: how many nested stacks one parent
//! declares (one, and two under different logical ids), whether the child's template takes
//! PARAMETERS the resource passes down, and whether the parent declares a resource of its own
//! beside the nested stack.

use serde_json::json;

use crate::change_set_lifecycle::settle_the_store;
use crate::harness::{call, create_stack, member, router_with_engine, REGION};
use crate::stack_lifecycle::{describe_stack, xml_blocks, xml_text, xml_texts};
use crate::stack_resource_provisioning::s3;
use crate::template_by_reference::{create_staging_bucket, stage, STAGING_BUCKET};
use service_com_amazonaws_cloudformation_cloudformation::state::status;
use smithy_server_runtime::MultiServiceRouter;

/// The type a nested stack is declared as.
///
/// `pub(crate)` because more than one suite declares a nested stack: the hierarchy a built-in
/// resource type is submitted in (`built_in_types_in_submitted_hierarchies.rs`) is declared the
/// same way, so there is ONE spelling of the type and ONE way a test addresses a staged child
/// template.
pub(crate) const STACK_RESOURCE_TYPE: &str = "AWS::CloudFormation::Stack";

/// The URL one staged child template is addressed by, path-style.
pub(crate) fn staged_url(key: &str) -> String {
    format!("https://s3.{REGION}.amazonaws.com/{STAGING_BUCKET}/{key}")
}

/// A child template declaring one bucket under the name it is given.
fn child_template(bucket: &str) -> String {
    json!({
        "Resources": {
            "Data": {"Type": "AWS::S3::Bucket", "Properties": {"BucketName": bucket}}
        }
    })
    .to_string()
}

/// A child template whose bucket name comes from a PARAMETER the parent passes down.
fn parameterised_child_template() -> String {
    json!({
        "Parameters": {"BucketName": {"Type": "String"}},
        "Resources": {
            "Data": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": {"Ref": "BucketName"}}
            }
        }
    })
    .to_string()
}

/// One nested-stack declaration of a parent: its logical id, the template it points at, and
/// the parameters it passes down.
struct Child {
    logical_id: &'static str,
    key: &'static str,
    parameters: Vec<(&'static str, String)>,
    /// The bucket the child's own template creates, which is what says the child really
    /// deployed.
    bucket: String,
    /// The `Tags` the declaration itself asks the child stack to carry.
    declared_tags: &'static [(&'static str, &'static str)],
    /// The `NotificationARNs` it asks the child stack to notify.
    topics: &'static [&'static str],
    /// The `TimeoutInMinutes` it holds the child's creation to.
    timeout: Option<u32>,
    /// The tag set the child stack must end up reporting: the declaration's own plus the
    /// stack-level ones the parent propagates, under every key the declaration does not
    /// state.
    expected_tags: &'static [(&'static str, &'static str)],
}

/// One row: a parent stack and the nested stacks it declares.
struct Row {
    case: &'static str,
    stack: &'static str,
    children: Vec<Child>,
    /// Whether the parent declares a bucket of its OWN beside the nested stacks.
    own_bucket: Option<String>,
    /// The stack-level `Tags` the parent is created with, which CloudFormation propagates to
    /// the resources it creates — a nested stack among them.
    stack_tags: &'static [(&'static str, &'static str)],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one nested stack",
            stack: "nest-one",
            children: vec![Child {
                logical_id: "Child",
                key: "nest-one-child.json",
                parameters: Vec::new(),
                bucket: "nest-one-child-data".to_string(),
                declared_tags: &[],
                topics: &[],
                timeout: None,
                expected_tags: &[],
            }],
            own_bucket: None,
            stack_tags: &[],
        },
        Row {
            case: "two nested stacks under different logical ids, the parent tagged",
            stack: "nest-two",
            children: vec![
                Child {
                    logical_id: "Kid",
                    key: "nest-two-kid.json",
                    parameters: Vec::new(),
                    bucket: "nest-two-kid-data".to_string(),
                    // The declaration states one key the stack also states, and one of its
                    // own: the declaration is the more specific statement about this
                    // resource.
                    declared_tags: &[("tier", "silver"), ("team", "data")],
                    topics: &["arn:aws:sns:us-east-1:000000000000:nest-two-events"],
                    timeout: Some(30),
                    expected_tags: &[
                        ("tier", "silver"),
                        ("team", "data"),
                        ("owner", "platform"),
                    ],
                },
                Child {
                    logical_id: "OtherKid",
                    key: "nest-two-other.json",
                    parameters: Vec::new(),
                    bucket: "nest-two-other-data".to_string(),
                    declared_tags: &[],
                    topics: &[],
                    timeout: None,
                    // Nothing of its own, so the stack's own tags are the whole set.
                    expected_tags: &[("owner", "platform"), ("tier", "gold")],
                },
            ],
            own_bucket: None,
            stack_tags: &[("owner", "platform"), ("tier", "gold")],
        },
        Row {
            case: "a nested stack taking a parameter, beside the parent's own bucket",
            stack: "nest-parameterised",
            children: vec![Child {
                logical_id: "Child",
                key: "nest-parameterised-child.json",
                parameters: vec![("BucketName", "nest-parameterised-child-data".to_string())],
                bucket: "nest-parameterised-child-data".to_string(),
                declared_tags: &[],
                topics: &[],
                timeout: None,
                expected_tags: &[],
            }],
            own_bucket: Some("nest-parameterised-own".to_string()),
            stack_tags: &[],
        },
    ]
}

/// The request members a row's stack-level tags travel on.
fn tag_members(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            [
                member(&format!("Tags.member.{position}.Key"), key),
                member(&format!("Tags.member.{position}.Value"), value),
            ]
        })
        .collect()
}

/// The parent template one row deploys.
fn parent_template(row: &Row) -> String {
    let mut resources = serde_json::Map::new();
    for child in &row.children {
        let mut properties = serde_json::Map::new();
        properties.insert("TemplateURL".to_string(), json!(staged_url(child.key)));
        if !child.parameters.is_empty() {
            let passed: serde_json::Map<String, serde_json::Value> = child
                .parameters
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect();
            properties.insert("Parameters".to_string(), json!(passed));
        }
        if !child.declared_tags.is_empty() {
            let declared: Vec<serde_json::Value> = child
                .declared_tags
                .iter()
                .map(|(key, value)| json!({"Key": key, "Value": value}))
                .collect();
            properties.insert("Tags".to_string(), json!(declared));
        }
        if !child.topics.is_empty() {
            properties.insert("NotificationARNs".to_string(), json!(child.topics));
        }
        if let Some(timeout) = child.timeout {
            properties.insert("TimeoutInMinutes".to_string(), json!(timeout));
        }
        resources.insert(
            child.logical_id.to_string(),
            json!({"Type": STACK_RESOURCE_TYPE, "Properties": properties}),
        );
    }
    if let Some(bucket) = &row.own_bucket {
        resources.insert(
            "Own".to_string(),
            json!({"Type": "AWS::S3::Bucket", "Properties": {"BucketName": bucket}}),
        );
    }
    json!({"Resources": resources}).to_string()
}

/// RULE: the work one request may set off through nested stacks is BOUNDED, by the template's
/// own shape rather than by what the machine survives. Two bounds do it, and this case holds
/// the emulator to both:
///
/// * a child whose template reference a stack above it is already deploying is refused as the
///   cycle it is, so a template that nests ITSELF stops at the first repetition;
/// * a chain of DISTINCT references is refused past the documented maximum nesting depth of
///   five, so breadth cannot be multiplied by unbounded depth.
///
/// Either way every stack the attempt created settles, and the refusal reaches the stack that
/// asked for it.
#[tokio::test]
async fn the_nesting_one_request_can_set_off_is_bounded() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;

    // A template that nests ITSELF: every level declares another level of the same object.
    let key = "nest-self.json";
    let itself = json!({
        "Resources": {
            "Child": {
                "Type": STACK_RESOURCE_TYPE,
                "Properties": {"TemplateURL": staged_url(key)}
            }
        }
    })
    .to_string();
    stage(&router, key, &itself).await;

    let parent = "nest-self";
    let (code, body) = create_stack(&router, parent, &itself, &[]).await;
    assert_eq!(
        code, 200,
        "case \"a self-nesting template\": the parent must be accepted, got {code} {body}"
    );
    for _ in 0..8 {
        settle_the_store(&router, &engine).await;
    }
    let chain = settled_chain("a self-nesting template", &router, parent).await;
    // The parent and the ONE child it got to: the grandchild's reference is the one already
    // in flight, so it is refused rather than deployed.
    assert_eq!(
        chain.len(),
        2,
        "case \"a self-nesting template\": expected the parent and one nested stack, got \
         {chain:?}"
    );

    // ...and a chain of DISTINCT references stops at the depth limit. Seven templates, each
    // nesting the next: the first is the parent's own body, so the nested levels are six and
    // the sixth is one level past the maximum.
    let levels = 7;
    for level in (1..=levels).rev() {
        let body = if level == levels {
            json!({"Resources": {"Data": {"Type": "AWS::S3::Bucket", "Properties": {}}}})
                .to_string()
        } else {
            json!({
                "Resources": {
                    "Kid": {
                        "Type": STACK_RESOURCE_TYPE,
                        "Properties": {"TemplateURL": staged_url(&format!("depth-{}.json", level + 1))}
                    }
                }
            })
            .to_string()
        };
        stage(&router, &format!("depth-{level}.json"), &body).await;
    }
    let deep = "nest-depth";
    let root = json!({
        "Resources": {
            "Kid": {
                "Type": STACK_RESOURCE_TYPE,
                "Properties": {"TemplateURL": staged_url("depth-2.json")}
            }
        }
    })
    .to_string();
    let (code, body) = create_stack(&router, deep, &root, &[]).await;
    assert_eq!(
        code, 200,
        "case \"a chain of distinct references\": the parent must be accepted, got {code} \
         {body}"
    );
    for _ in 0..12 {
        settle_the_store(&router, &engine).await;
    }
    let chain = settled_chain("a chain of distinct references", &router, deep).await;
    // The parent and the five levels the limit allows, and no sixth.
    assert_eq!(
        chain.len(),
        6,
        "case \"a chain of distinct references\": expected the parent and five nested stacks, \
         got {chain:?}"
    );
    // The level that asked for a sixth reports the refusal as its own failure. Read by its
    // stack ID: the rollback deleted it, and a deleted stack is addressable only by that.
    let (code, body) = call(
        &router,
        "DescribeStackEvents",
        &[member("StackName", &chain[chain.len() - 1].2)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"a chain of distinct references\": the deepest stack's events must be \
         readable, got {code} {body}"
    );
    assert!(
        xml_texts(&body, "ResourceStatusReason")
            .iter()
            .any(|reason| reason.contains("limited to a depth of 5")),
        "case \"a chain of distinct references\": expected the depth refusal on the deepest \
         stack, got {body}"
    );
}

/// Every stack of one parent's chain, as `(StackName, StackStatus, StackId)` in creation
/// order, with each one asserted SETTLED and named within the documented maximum.
async fn settled_chain(
    case: &str,
    router: &MultiServiceRouter,
    parent: &str,
) -> Vec<(String, String, String)> {
    let (code, body) = call(router, "ListStacks", &[]).await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListStacks must answer, got {code} {body}"
    );
    let chain: Vec<(String, String, String)> = xml_blocks(&body, "member")
        .iter()
        .map(|summary| {
            (
                xml_text(summary, "StackName").unwrap_or_default(),
                xml_text(summary, "StackStatus").unwrap_or_default(),
                xml_text(summary, "StackId").unwrap_or_default(),
            )
        })
        .filter(|(name, ..)| name.starts_with(parent))
        .collect();
    for (name, reported, _) in &chain {
        assert!(
            name.len() <= 128,
            "case {case:?}: expected every name within the documented maximum, got {name:?}"
        );
        assert!(
            !reported.ends_with("_IN_PROGRESS"),
            "case {case:?}: expected every stack of the chain to have settled, got {chain:?}"
        );
    }
    chain
}

/// The `PhysicalResourceId` the parent reports for one of its resources.
pub(crate) async fn physical_resource_id(
    case: &str,
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (code, body) = call(
        router,
        "DescribeStackResource",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeStackResource({stack}, {logical_id}) must answer, got {code} \
         {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_default()
}

/// Whether `candidate` is a stack ARN of this Region naming a stack whose name starts with
/// `name_prefix`: `arn:<partition>:cloudformation:<region>:<account>:stack/<name>/<uuid>`.
///
/// The ARN's own form is what is asserted — the account is 12 digits and the id a 36-character
/// uuid — rather than the values one run produced, since neither is fixed by the request.
fn is_stack_arn(candidate: &str, name_prefix: &str) -> bool {
    let Some(rest) = candidate.strip_prefix("arn:") else {
        return false;
    };
    let segments: Vec<&str> = rest.splitn(5, ':').collect();
    if segments.len() != 5 {
        return false;
    }
    let (partition, service, region, account, resource) =
        (segments[0], segments[1], segments[2], segments[3], segments[4]);
    let Some(stack) = resource.strip_prefix("stack/") else {
        return false;
    };
    let Some((name, unique)) = stack.split_once('/') else {
        return false;
    };
    partition.starts_with("aws")
        && service == "cloudformation"
        && region == REGION
        && account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && name.starts_with(name_prefix)
        && unique.len() == 36
        && unique.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// Every `StackSummary` a status-filtered `ListStacks` reports, as
/// `(StackName, StackId, ParentId, RootId)`.
async fn listed_stacks(
    case: &str,
    router: &MultiServiceRouter,
) -> Vec<(String, String, Option<String>, Option<String>)> {
    let (code, body) = call(
        router,
        "ListStacks",
        &[
            member("StackStatusFilter.member.1", status::CREATE_COMPLETE),
            member("StackStatusFilter.member.2", status::UPDATE_COMPLETE),
        ],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: ListStacks must answer, got {code} {body}"
    );
    let summaries = match xml_blocks(&body, "StackSummaries").into_iter().next() {
        Some(block) => xml_blocks(&block, "member"),
        None => Vec::new(),
    };
    summaries
        .iter()
        .map(|summary| {
            (
                xml_text(summary, "StackName").unwrap_or_default(),
                xml_text(summary, "StackId").unwrap_or_default(),
                xml_text(summary, "ParentId"),
                xml_text(summary, "RootId"),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_nested_stack_is_a_stack_of_its_own() {
    nested_stacks_are_stacks_of_their_own().await;
}

async fn nested_stacks_are_stacks_of_their_own() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    for row in rows() {
        for child in &row.children {
            let body = if child.parameters.is_empty() {
                child_template(&child.bucket)
            } else {
                parameterised_child_template()
            };
            stage(&router, child.key, &body).await;
        }
    }

    for row in rows() {
        let case = row.case;
        let template = parent_template(&row);
        let (code, body) =
            create_stack(&router, row.stack, &template, &tag_members(row.stack_tags)).await;
        assert_eq!(
            code, 200,
            "case {case:?}: the parent must be created, got {code} {body}"
        );
        // Each level of nesting is one span of the clock: the parent's resource is held open
        // until the child reports itself settled, so the store is settled until it is.
        for _ in 0..3 {
            settle_the_store(&router, &engine).await;
        }

        let (code, body) = describe_stack(&router, row.stack).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {case:?}: expected the parent complete, got {code} {body}"
        );
        let parent_id = xml_text(&body, "StackId").unwrap_or_default();
        assert_eq!(
            (
                xml_text(&body, "ParentId"),
                xml_text(&body, "RootId")
            ),
            (None, None),
            "case {case:?}: a top-level stack is nested in nothing, got {body}"
        );

        let listed = listed_stacks(case, &router).await;
        // Exactly one stack per nested declaration, beside the parent itself: a parent
        // provisions the stacks its template declares and no others.
        let from_this_parent: Vec<&String> = listed
            .iter()
            .map(|(name, ..)| name)
            .filter(|name| name.starts_with(row.stack))
            .collect();
        assert_eq!(
            from_this_parent.len(),
            row.children.len() + 1,
            "case {case:?}: expected the parent and one stack per nested declaration, got \
             {from_this_parent:?}"
        );
        for child in &row.children {
            // The child is a stack of its own, named for the parent and the logical id that
            // declared it.
            let prefix = format!("{}-{}-", row.stack, child.logical_id);
            let summary = listed
                .iter()
                .find(|(name, ..)| name.starts_with(&prefix))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: expected a summary for a stack named {prefix}…, got \
                         {listed:?}"
                    )
                });
            let (name, stack_id, reported_parent, reported_root) = summary;
            assert!(
                is_stack_arn(stack_id, &prefix),
                "case {case:?}: expected the child {name} to carry a stack ARN, got \
                 {stack_id:?}"
            );
            assert_eq!(
                (reported_parent.as_deref(), reported_root.as_deref()),
                (Some(parent_id.as_str()), Some(parent_id.as_str())),
                "case {case:?}: expected the child {name} to report its parent as both parent \
                 and root"
            );

            // The parent's own resource for it carries that same stack id, which is what
            // lets a caller follow the resource to the child stack.
            let reported =
                physical_resource_id(case, &router, row.stack, child.logical_id).await;
            assert_eq!(
                &reported, stack_id,
                "case {case:?}: expected the resource {} to report the child's stack id",
                child.logical_id
            );
            let (code, body) = describe_stack(&router, &reported).await;
            assert_eq!(
                xml_text(&body, "StackStatus").as_deref(),
                Some(status::CREATE_COMPLETE),
                "case {case:?}: the child must be readable by the id the parent reports, got \
                 {code} {body}"
            );
            // The child carries what its declaration asked for: the tag set (its own tags,
            // plus the stack-level ones the parent propagates under every key it does not
            // state) and the topics it notifies.
            let reported_tags: Vec<(String, String)> =
                match xml_blocks(&body, "Tags").into_iter().next() {
                    Some(block) => xml_texts(&block, "Key")
                        .into_iter()
                        .zip(xml_texts(&block, "Value"))
                        .collect(),
                    None => Vec::new(),
                };
            let mut reported_tags = reported_tags;
            reported_tags.sort();
            let mut expected_tags: Vec<(String, String)> = child
                .expected_tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect();
            expected_tags.sort();
            assert_eq!(
                reported_tags, expected_tags,
                "case {case:?}: expected the child {name} to carry the declared and \
                 propagated tags, got {body}"
            );
            let reported_topics: Vec<String> =
                match xml_blocks(&body, "NotificationARNs").into_iter().next() {
                    Some(block) => xml_texts(&block, "member"),
                    None => Vec::new(),
                };
            assert_eq!(
                reported_topics,
                child.topics.iter().map(|arn| (*arn).to_string()).collect::<Vec<String>>(),
                "case {case:?}: expected the child {name} to notify the declared topics, got \
                 {body}"
            );

            // ...and the child's own template really deployed: its bucket exists in Amazon
            // S3, which is the only thing that makes the child a stack rather than a record.
            let head = s3(&router, "HEAD", &format!("/{}", child.bucket), b"").await;
            assert_eq!(
                head.status, 200,
                "case {case:?}: expected the child's bucket {} to exist, got {}",
                child.bucket, head.status
            );
        }

        // The parent counts each nested stack as exactly one resource of that type.
        let (code, body) = call(
            &router,
            "DescribeStackResources",
            &[member("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: the parent's resources must be readable, got {code} {body}"
        );
        let nested: Vec<String> = xml_blocks(&body, "member")
            .iter()
            .filter(|resource| {
                xml_text(resource, "ResourceType").as_deref() == Some(STACK_RESOURCE_TYPE)
            })
            .map(|resource| xml_text(resource, "LogicalResourceId").unwrap_or_default())
            .collect();
        let expected: Vec<String> = row
            .children
            .iter()
            .map(|child| child.logical_id.to_string())
            .collect();
        assert_eq!(
            nested, expected,
            "case {case:?}: expected one resource per nested stack, got {nested:?}"
        );
    }
}

/// RULE: deleting the parent deletes the stacks it provisioned — a nested stack is the
/// parent's resource, so the parent's teardown is its deletion, and the resources of the child
/// go with it.
#[tokio::test]
async fn deleting_the_parent_deletes_the_nested_stack() {
    the_parents_deletion_deletes_the_nested_stack().await;
}

async fn the_parents_deletion_deletes_the_nested_stack() {
    let (router, engine) = router_with_engine();
    create_staging_bucket(&router).await;
    stage(&router, "nest-teardown-child.json", &child_template("nest-teardown-data")).await;
    let parent = "nest-teardown";
    let template = json!({
        "Resources": {
            "Child": {
                "Type": STACK_RESOURCE_TYPE,
                "Properties": {"TemplateURL": staged_url("nest-teardown-child.json")}
            }
        }
    })
    .to_string();
    let (code, body) = create_stack(&router, parent, &template, &[]).await;
    assert_eq!(
        code, 200,
        "case \"the parent of a deletion\": must be created, got {code} {body}"
    );
    for _ in 0..3 {
        settle_the_store(&router, &engine).await;
    }
    let child_id = physical_resource_id("the parent of a deletion", &router, parent, "Child").await;
    assert!(
        is_stack_arn(&child_id, &format!("{parent}-Child-")),
        "case \"the parent of a deletion\": expected a child stack ARN, got {child_id:?}"
    );

    let (code, body) = call(&router, "DeleteStack", &[member("StackName", parent)]).await;
    assert_eq!(
        code, 200,
        "case \"the parent of a deletion\": the delete must be accepted, got {code} {body}"
    );
    for _ in 0..3 {
        settle_the_store(&router, &engine).await;
    }

    // The child is deleted with the parent: a deleted stack is readable by its id alone, and
    // it reports the deletion completed.
    let (code, body) = describe_stack(&router, &child_id).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::DELETE_COMPLETE),
        "case \"the parent of a deletion\": expected the child deleted, got {code} {body}"
    );
    // ...and so is the bucket the child's own template created.
    let head = s3(&router, "HEAD", "/nest-teardown-data", b"").await;
    assert_ne!(
        head.status, 200,
        "case \"the parent of a deletion\": expected the child's bucket gone, got {}",
        head.status
    );
    // The parent itself is gone from the live set, which `ListStacks` still reports as
    // deleted.
    let (code, body) = call(
        &router,
        "ListStacks",
        &[member("StackStatusFilter.member.1", status::DELETE_COMPLETE)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"the parent of a deletion\": ListStacks must answer, got {code} {body}"
    );
    let deleted = xml_texts(&body, "StackName");
    assert!(
        deleted.iter().any(|name| name == parent)
            && deleted.iter().any(|name| name.starts_with(&format!("{parent}-Child-"))),
        "case \"the parent of a deletion\": expected both stacks reported deleted, got \
         {deleted:?}"
    );
}
