// SPDX-License-Identifier: CC0-1.0

//! A stack's declared resources are CREATED in the services that own them, and what a
//! stack's declared outputs report for them.
//!
//! The rule under test: **a stack operation that deploys a template calls the owning
//! service's own create API once per resource the `Resources` section declares, so once
//! the deployment completes the resource EXISTS and is addressable by the physical
//! resource id the stack reports for it** — and a resource the owning service REFUSES is
//! not created, so the deployment fails instead of completing and says which resource and
//! why. Deleting the stack deletes those same resources again.
//!
//! The rows differ along what the rule generalises over: each resource type
//! `state::provisioning` carries a provisioner for, a resource the template NAMES and one
//! whose physical id CloudFormation generated, both template spellings, properties that
//! are plain scalars and properties built from another resource of the same template
//! (`Ref`, `Fn::GetAtt`), one resource and several, a re-deployment of the same template,
//! and both verdicts an owning service can give. Every expected value is computed here
//! from what the row declares, never read back out of the answer being checked.
//!
//! Everything goes over the wire each service speaks — awsQuery for CloudFormation and
//! IAM, REST for Amazon S3, awsJson1.1 for CodePipeline — through the one router
//! `stack_lifecycle.rs` builds, which mounts those services beside CloudFormation on one
//! engine.
//!
//! The second half of this file is the CONTAINER-PLATFORM output table, whose own rule
//! and bounds are stated where it begins.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, describe_stack, member, resource_event_reason,
    router, wait_for_the_transition, xml_text, xml_texts, ACCOUNT_ID, REGION,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary, resource_status,
};

/// The `X-Amz-Target` prefix CodePipeline's OPERATIONS.md records.
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// Deploy `template` as stack `name`, wait past the instant its create settles, and
/// answer the `StackStatus` and `StackStatusReason` the stack then reports.
async fn deploy(
    router: &MultiServiceRouter,
    name: &str,
    template: &str,
) -> (Option<String>, Option<String>) {
    // A row's template may declare an IAM role, named or not, so the deployment
    // acknowledges the template's IAM resources.
    let (status, body) = create_stack(router, name, template, &acknowledging_iam(&[])).await;
    assert_eq!(status, 200, "CreateStack({name}) answered {status}: {body}");
    settled_status(router, name).await
}

/// The status a stack reports once the operation in flight on it has settled.
async fn settled_status(
    router: &MultiServiceRouter,
    name: &str,
) -> (Option<String>, Option<String>) {
    wait_for_the_transition().await;
    let (status, body) = describe_stack(router, name).await;
    assert_eq!(
        status, 200,
        "DescribeStacks({name}) answered {status}: {body}"
    );
    (
        xml_text(&body, "StackStatus"),
        xml_text(&body, "StackStatusReason"),
    )
}

/// The physical resource id the stack reports for one logical id.
async fn physical_id(router: &MultiServiceRouter, stack: &str, logical_id: &str) -> String {
    let (status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeStackResources({stack}, {logical_id}) answered {status}: {body}"
    );
    xml_text(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {logical_id:?}: expected a physical resource id, got {body}")
    })
}

/// One Amazon S3 request, path style, the way the emulated S3 service routes it.
pub(crate) async fn s3(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
) -> (u16, http::HeaderMap, String) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/s3/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("authorization", authorization)
        .body(bytes::Bytes::copy_from_slice(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let headers = response.headers().clone();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, headers, text)
}

/// One IAM request, over the awsQuery wire IAM speaks.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
        body.push_str(&format!("&{key}={value}"));
    }
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iam/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://iam.{REGION}.amazonaws.com/"))
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, text)
}

/// One CodePipeline request, over the awsJson1.1 wire it speaks.
async fn codepipeline(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/codepipeline/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://codepipeline.{REGION}.amazonaws.com/"))
        .header("host", format!("codepipeline.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "x-amz-target",
            format!("{CODEPIPELINE_TARGET_PREFIX}.{operation}"),
        )
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// A trust policy an `AWS::IAM::Role` declares inline, as a template object.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "codepipeline.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

// ---------------------------------------------------------------------------
// Amazon S3 buckets
// ---------------------------------------------------------------------------

/// Every bucket a stack declares EXISTS in Amazon S3 under the physical id the stack
/// reports — named by the template or generated by CloudFormation — and carries the
/// `VersioningConfiguration` the template declared for it.
#[tokio::test]
async fn declared_buckets_exist_in_s3_under_the_reported_physical_id() {
    // Rows: the template spelling, whether the bucket is named, and the versioning status
    // declared for it (`VersioningConfiguration$Status`: `Enabled` / `Suspended`).
    let rows: &[(&str, &str, String, &str, Option<&str>)] = &[
        (
            "a bucket the template names, versioning enabled (JSON)",
            "named-bucket",
            json!({"Resources": {"Artifacts": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketName": "cfn-provisioned-artifacts",
                    "VersioningConfiguration": {"Status": "Enabled"}
                }}}})
            .to_string(),
            "Artifacts",
            Some("Enabled"),
        ),
        (
            "a bucket the template names, versioning suspended (JSON)",
            "suspended-bucket",
            json!({"Resources": {"Artifacts": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketName": "cfn-suspended-artifacts",
                    "VersioningConfiguration": {"Status": "Suspended"}
                }}}})
            .to_string(),
            "Artifacts",
            Some("Suspended"),
        ),
        (
            "a bucket CloudFormation names, no versioning declared (YAML)",
            "generated-bucket",
            concat!("Resources:\n", "  Unnamed:\n", "    Type: AWS::S3::Bucket\n").to_string(),
            "Unnamed",
            None,
        ),
    ];

    for (case, stack, template, logical_id, versioning) in rows {
        let router = router();
        let (status, reason) = deploy(&router, stack, template).await;
        assert_eq!(
            status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {status:?} ({reason:?})"
        );
        let bucket = physical_id(&router, stack, logical_id).await;

        // Addressable by exactly that id: a write into it is accepted.
        let (code, headers, body) = s3(
            &router,
            "PUT",
            &format!("/{bucket}/source.zip"),
            b"artifact-bytes",
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected a write into {bucket:?} to be accepted, got {code}: {body}"
        );

        // The declared `VersioningConfiguration` reached S3 — read back through S3's own
        // `GetBucketVersioning`, and visible in the write above, which a
        // versioning-enabled bucket answers with the version id it minted.
        let (code, text) = {
            let (code, _, text) = s3(&router, "GET", &format!("/{bucket}?versioning"), b"").await;
            (code, text)
        };
        assert_eq!(
            code, 200,
            "case {case:?}: expected GetBucketVersioning({bucket:?}) to answer, got {code}: {text}"
        );
        let reported = xml_text(&text, "Status");
        assert_eq!(
            reported.as_deref(),
            *versioning,
            "case {case:?}: expected the declared versioning status, got {reported:?} in {text}"
        );
        let version_id = headers
            .get("x-amz-version-id")
            .map(|value| value.to_str().unwrap_or_default().to_string());
        assert_eq!(
            version_id.is_some(),
            *versioning == Some("Enabled"),
            "case {case:?}: only a versioning-enabled bucket answers a write with a version \
             id; got {version_id:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// IAM roles
// ---------------------------------------------------------------------------

/// A role a stack declares EXISTS in IAM, with the trust policy, the inline policies and
/// the managed policies the template declared.
#[tokio::test]
async fn a_declared_role_exists_in_iam_with_its_policies() {
    // An AWS-managed policy ARN: IAM's `AttachRolePolicy` takes one as present (the
    // account segment is `aws`, so it is AWS's resource, not this account's).
    const MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess";

    let template = json!({
        "Resources": {"Role": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": "cfn-provisioned-role",
                "Path": "/service-role/",
                "Description": "deploys the pipeline",
                "AssumeRolePolicyDocument": trust_policy(),
                "ManagedPolicyArns": [MANAGED_POLICY_ARN],
                "Policies": [{
                    "PolicyName": "artifacts",
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{"Effect": "Allow", "Action": "s3:*", "Resource": "*"}]
                    }
                }]
            }}},
        // The two identifiers the type publishes as its return values, so the stack's own
        // report of them can be compared with the records IAM holds.
        "Outputs": {
            "RoleArn": {"Value": {"Fn::GetAtt": ["Role", "Arn"]}},
            "RoleId": {"Value": {"Fn::GetAtt": ["Role", "RoleId"]}},
        },
    })
    .to_string();

    let router = router();
    let (status, reason) = deploy(&router, "role-stack", &template).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let role_name = physical_id(&router, "role-stack", "Role").await;
    assert_eq!(
        role_name, "cfn-provisioned-role",
        "expected the declared RoleName as the physical id, got {role_name:?}"
    );

    let (code, body) = iam(&router, "GetRole", &[("RoleName", &role_name)]).await;
    assert_eq!(
        code, 200,
        "expected GetRole({role_name:?}) to answer, got {code}: {body}"
    );
    // The record IAM holds for the role, kept for the identifier comparison at the end.
    let role_record = body.clone();
    let reported = xml_text(&body, "RoleName");
    assert_eq!(
        reported.as_deref(),
        Some(role_name.as_str()),
        "expected IAM to report the created role, got {reported:?} in {body}"
    );
    // The trust policy the template declared as an inline OBJECT reached IAM as text.
    let document = xml_text(&body, "AssumeRolePolicyDocument").unwrap_or_default();
    assert!(
        document.contains("codepipeline.amazonaws.com"),
        "expected the declared trust policy on the role, got {document:?}"
    );

    let (code, body) = iam(
        &router,
        "GetRolePolicy",
        &[("RoleName", &role_name), ("PolicyName", "artifacts")],
    )
    .await;
    assert_eq!(
        code, 200,
        "expected GetRolePolicy({role_name:?}, \"artifacts\") to answer, got {code}: {body}"
    );
    let reported = xml_text(&body, "PolicyName");
    assert_eq!(
        reported.as_deref(),
        Some("artifacts"),
        "expected the declared inline policy on the role, got {reported:?} in {body}"
    );

    // And the declared managed policy really is attached.
    let (code, body) = iam(
        &router,
        "ListAttachedRolePolicies",
        &[("RoleName", &role_name)],
    )
    .await;
    assert_eq!(
        code, 200,
        "expected ListAttachedRolePolicies({role_name:?}) to answer, got {code}: {body}"
    );
    let attached = xml_texts(&body, "PolicyArn");
    assert!(
        attached.iter().any(|arn| arn == MANAGED_POLICY_ARN),
        "expected the declared ManagedPolicyArns attached to the role, got {attached:?}"
    );

    // The identifiers the stack reports for the role are the ones IAM holds: `RoleId` is
    // the unique id IAM MINTS for a role, so an id the stack computed for itself would
    // have a client reading two different identities for one role.
    let (code, described) = describe_stack(&router, "role-stack").await;
    assert_eq!(code, 200, "DescribeStacks answered {code}: {described}");
    let outputs: BTreeMap<String, String> = xml_texts(&described, "OutputKey")
        .into_iter()
        .zip(xml_texts(&described, "OutputValue"))
        .collect();
    let held_id = xml_text(&role_record, "RoleId").unwrap_or_default();
    assert_eq!(
        outputs.get("RoleId"),
        Some(&held_id),
        "expected the stack to report the RoleId IAM minted ({held_id:?}), got {outputs:?}"
    );
    let held_arn = xml_text(&role_record, "Arn").unwrap_or_default();
    assert_eq!(
        outputs.get("RoleArn"),
        Some(&held_arn),
        "expected the stack to report the ARN IAM holds ({held_arn:?}), got {outputs:?}"
    );
}

// ---------------------------------------------------------------------------
// CodePipeline pipelines
// ---------------------------------------------------------------------------

/// The template the pipeline rows deploy: a bucket, a role and a pipeline that REFERS to
/// both. `Ref`/`Fn::GetAtt` are what tie them together, so this also pins that a
/// provisioned resource's properties carry the identifiers of the other resources of its
/// own template.
fn pipeline_template(pipeline_name: &str, input_artifact: &str) -> String {
    json!({"Resources": {
        "Store": {
            "Type": "AWS::S3::Bucket",
            "Properties": {
                "BucketName": "cfn-pipeline-store",
                "VersioningConfiguration": {"Status": "Enabled"}
            }
        },
        "PipelineRole": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": "cfn-pipeline-role",
                "AssumeRolePolicyDocument": trust_policy()
            }
        },
        "Flow": {
            "Type": "AWS::CodePipeline::Pipeline",
            "Properties": {
                "Name": pipeline_name,
                "RoleArn": {"Fn::GetAtt": ["PipelineRole", "Arn"]},
                "ArtifactStore": {"Type": "S3", "Location": {"Ref": "Store"}},
                "Stages": [
                    {
                        "Name": "Source",
                        "Actions": [{
                            "Name": "Fetch",
                            "ActionTypeId": {
                                "Category": "Source", "Owner": "AWS",
                                "Provider": "S3", "Version": "1"
                            },
                            "Configuration": {
                                "S3Bucket": {"Ref": "Store"},
                                "S3ObjectKey": "source.zip",
                                "PollForSourceChanges": "false"
                            },
                            "OutputArtifacts": [{"Name": "SourceOut"}],
                            "RunOrder": 1
                        }]
                    },
                    {
                        "Name": "Deploy",
                        "Actions": [{
                            "Name": "Ship",
                            "ActionTypeId": {
                                "Category": "Deploy", "Owner": "AWS",
                                "Provider": "S3", "Version": "1"
                            },
                            "Configuration": {
                                "BucketName": {"Ref": "Store"},
                                "ObjectKey": "shipped.zip",
                                "Extract": "false"
                            },
                            "InputArtifacts": [{"Name": input_artifact}],
                            "RunOrder": 1
                        }]
                    }
                ]
            }
        }
    }})
    .to_string()
}

/// A pipeline a stack declares EXISTS in CodePipeline, wired to the bucket and the role
/// the stack's own `Ref`/`Fn::GetAtt` resolved to.
#[tokio::test]
async fn a_declared_pipeline_exists_in_codepipeline_wired_to_the_stacks_resources() {
    let router = router();
    let template = pipeline_template("cfn-provisioned-pipeline", "SourceOut");
    let (status, reason) = deploy(&router, "pipeline-stack", &template).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let pipeline_name = physical_id(&router, "pipeline-stack", "Flow").await;
    let bucket = physical_id(&router, "pipeline-stack", "Store").await;
    let role_name = physical_id(&router, "pipeline-stack", "PipelineRole").await;

    let (code, body) = codepipeline(&router, "GetPipeline", &json!({"name": pipeline_name})).await;
    assert_eq!(
        code, 200,
        "expected GetPipeline({pipeline_name:?}) to answer, got {code}: {body}"
    );
    let pipeline = &body["pipeline"];
    assert_eq!(
        pipeline["name"].as_str(),
        Some(pipeline_name.as_str()),
        "expected the created pipeline, got {body}"
    );
    assert_eq!(
        pipeline["artifactStore"]["location"].as_str(),
        Some(bucket.as_str()),
        "expected the artifact store to be the stack's bucket {bucket:?}, got {body}"
    );
    assert_eq!(
        pipeline["artifactStore"]["type"].as_str(),
        Some("S3"),
        "expected the declared artifact store type, got {body}"
    );
    assert_eq!(
        pipeline["roleArn"].as_str(),
        Some(format!("arn:aws:iam::{ACCOUNT_ID}:role/{role_name}").as_str()),
        "expected the pipeline's roleArn to be the stack's role {role_name:?}, got {body}"
    );
    let stage_names: Vec<&str> = pipeline["stages"]
        .as_array()
        .map(|stages| {
            stages
                .iter()
                .map(|stage| stage["name"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        stage_names,
        vec!["Source", "Deploy"],
        "expected the declared stages in order, got {body}"
    );
    // The provider's own configuration keys came through verbatim, case included, with
    // the `Ref` inside one of them resolved to the stack's bucket.
    let source_configuration = &pipeline["stages"][0]["actions"][0]["configuration"];
    assert_eq!(
        source_configuration["S3Bucket"].as_str(),
        Some(bucket.as_str()),
        "expected the source action's S3Bucket to resolve to the stack's bucket, got {body}"
    );
    assert_eq!(
        source_configuration["S3ObjectKey"].as_str(),
        Some("source.zip"),
        "expected the provider's own configuration keys verbatim, got {body}"
    );
    assert_eq!(
        pipeline["stages"][0]["actions"][0]["runOrder"].as_i64(),
        Some(1),
        "expected the declared runOrder, got {body}"
    );

    // The bucket the pipeline was wired to really is a versioned bucket of this emulator,
    // which is what an S3 source action needs.
    let (code, headers, text) =
        s3(&router, "PUT", &format!("/{bucket}/source.zip"), b"zip-bytes").await;
    assert_eq!(
        code, 200,
        "expected a write into the pipeline's artifact store, got {code}: {text}"
    );
    assert!(
        headers.contains_key("x-amz-version-id"),
        "expected the versioned artifact store to answer with a version id, got {headers:?}"
    );
}

/// The template a custom-action-type row deploys: the registration alone, declared with
/// the properties the row states.
fn custom_action_type_template(properties: Value) -> String {
    json!({"Resources": {
        "Worker": {
            "Type": "AWS::CodePipeline::CustomActionType",
            "Properties": properties,
        }
    }})
    .to_string()
}

/// A custom action type a stack declares is REGISTERED in CodePipeline, so a job worker
/// polling for the action type the template declared is served rather than refused, and
/// deleting the stack takes the registration with it.
///
/// The rows differ along what this generalises over: the `Category` and `Version` the
/// declaration states (every property of the identifying triple), whether the type
/// declares configuration properties and settings, and whether it carries tags — each a
/// property `CreateCustomActionType` takes under its own member name.
#[tokio::test]
async fn a_declared_custom_action_type_is_registered_in_codepipeline() {
    // (case, Category, Provider, Version, the extra properties the row declares)
    let rows: Vec<(&str, &str, &str, &str, Value)> = vec![
        (
            "the smallest declaration",
            "Build",
            "cfn-build-worker",
            "1",
            json!({}),
        ),
        (
            "a Test category with configuration properties and settings",
            "Test",
            "cfn-test-worker",
            "2",
            json!({
                "ConfigurationProperties": [{
                    "Name": "ProjectName",
                    "Description": "the project the worker builds",
                    "Key": true,
                    "Required": true,
                    "Secret": false,
                    "Queryable": false,
                    "Type": "String",
                }],
                "Settings": {
                    "EntityUrlTemplate": "https://example.test/{Config:ProjectName}",
                    "ExecutionUrlTemplate": "https://example.test/runs",
                },
            }),
        ),
        (
            "a Deploy category carrying tags",
            "Deploy",
            "cfn-deploy-worker",
            "3",
            json!({"Tags": [{"Key": "owner", "Value": "platform"}]}),
        ),
    ];

    for (case, category, provider, version, extra) in rows {
        let router = router();
        let mut properties = json!({
            "Category": category,
            "Provider": provider,
            "Version": version,
            // `ArtifactDetails` is `@required` on both sides of the create, with
            // `maximumCount`/`minimumCount` inside it.
            "InputArtifactDetails": {"MaximumCount": 1, "MinimumCount": 0},
            "OutputArtifactDetails": {"MaximumCount": 1, "MinimumCount": 0},
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            properties[key] = value.clone();
        }
        let template = custom_action_type_template(properties);
        let (status, reason) = deploy(&router, "action-type-stack", &template).await;
        assert_eq!(
            status.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {status:?} ({reason:?})"
        );

        // The registration exists: a worker polling for exactly the action type the
        // template declared is SERVED (an empty job list until a job is due), which is
        // the fact a stack's own completion is supposed to have established.
        let polled = json!({
            "actionTypeId": {
                "category": category,
                "owner": "Custom",
                "provider": provider,
                "version": version,
            },
            "maxBatchSize": 10,
        });
        let (code, body) = codepipeline(&router, "PollForJobs", &polled).await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected the declared action type to be pollable, got {code}: \
             {body}"
        );
        assert_eq!(
            body["jobs"],
            json!([]),
            "case {case:?}: no job is due yet, so the poll is served an empty list, got {body}"
        );

        // And the same registration through the catalog, carrying the artifact counts the
        // declaration stated.
        let (code, body) = codepipeline(
            &router,
            "ListActionTypes",
            &json!({"actionOwnerFilter": "Custom"}),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected the catalog to answer, got {code}: {body}"
        );
        let listed = body["actionTypes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|action_type| {
                action_type["id"]["provider"] == json!(provider)
                    && action_type["id"]["category"] == json!(category)
                    && action_type["id"]["version"] == json!(version)
            })
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the stack's action type must be listed, got {body}")
            });
        assert_eq!(
            listed["inputArtifactDetails"],
            json!({"maximumCount": 1, "minimumCount": 0}),
            "case {case:?}: expected the declared input artifact details, got {listed}"
        );
        assert_eq!(
            listed["outputArtifactDetails"],
            json!({"maximumCount": 1, "minimumCount": 0}),
            "case {case:?}: expected the declared output artifact details, got {listed}"
        );
        if let Some(declared) = extra.get("ConfigurationProperties") {
            assert_eq!(
                listed["actionConfigurationProperties"][0]["name"],
                declared[0]["Name"],
                "case {case:?}: expected the declared configuration property, got {listed}"
            );
            assert_eq!(
                listed["actionConfigurationProperties"][0]["queryable"],
                declared[0]["Queryable"],
                "case {case:?}: expected the declared property's queryability, got {listed}"
            );
        }
        if let Some(declared) = extra.get("Settings") {
            assert_eq!(
                listed["settings"]["entityUrlTemplate"],
                declared["EntityUrlTemplate"],
                "case {case:?}: expected the declared settings, got {listed}"
            );
        }

        // Deleting the stack takes the registration with it: the account no longer holds
        // the action type, so the catalog no longer reports it. A worker still polling a
        // custom action this account DELETED is answered rather than refused (it drains),
        // which is why the poll above is not what says the registration is gone.
        let (code, body) = call(
            &router,
            "DeleteStack",
            &[member("StackName", "action-type-stack")],
        )
        .await;
        assert_eq!(code, 200, "case {case:?}: DeleteStack answered {code}: {body}");
        wait_for_the_transition().await;
        let (code, body) = codepipeline(
            &router,
            "ListActionTypes",
            &json!({"actionOwnerFilter": "Custom"}),
        )
        .await;
        assert_eq!(
            code, 200,
            "case {case:?}: expected the catalog to answer, got {code}: {body}"
        );
        let still_listed = body["actionTypes"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|action_type| action_type["id"]["provider"] == json!(provider));
        assert!(
            !still_listed,
            "case {case:?}: expected the registration to be gone with the stack, got {body}"
        );
    }
}

/// A resource the owning service REFUSES is not created, so the deployment FAILS: the
/// stack reports a failed status whose reason summarises which resource failed, that
/// service's own refusal is reported on the resource's own event, and the resource is
/// absent from the owning service.
#[tokio::test]
async fn a_refused_resource_fails_the_deployment() {
    let router = router();
    // The deploy action's `inputArtifacts` names an artifact no earlier action outputs,
    // which CodePipeline's own `CreatePipeline` refuses with `InvalidStructureException`.
    let template = pipeline_template("cfn-refused-pipeline", "NoSuchArtifact");
    let (status, reason) = deploy(&router, "refused-stack", &template).await;
    assert_eq!(
        status.as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "expected the failed create to be rolled back, got {status:?} ({reason:?})"
    );
    let reason = reason.unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Flow"]),
        "expected the stack reason to be the summary naming the refused resource, got \
         {reason:?}"
    );
    // The owning service's own refusal is the failing RESOURCE's diagnosis, reported on
    // that resource's event rather than folded into the stack's summary.
    let on_the_resource = resource_event_reason(
        &router,
        "refused-stack",
        "Flow",
        resource_status::CREATE_FAILED,
    )
    .await;
    assert!(
        on_the_resource.contains("which no earlier action outputs")
            && on_the_resource.contains("HandlerErrorCode: InvalidRequest"),
        "expected the owning service's refusal on the failed resource's event, got \
         {on_the_resource:?}"
    );

    // Nothing was created for it: the pipeline is absent from CodePipeline.
    let (code, body) = codepipeline(
        &router,
        "GetPipeline",
        &json!({"name": "cfn-refused-pipeline"}),
    )
    .await;
    assert_eq!(
        code, 400,
        "expected the refused pipeline to be absent, got {code}: {body}"
    );
}

/// Deploying the same resources again is not a refusal, and an update that ADDS a
/// resource creates that one too.
#[tokio::test]
async fn redeploying_is_not_a_refusal_and_an_update_creates_what_it_adds() {
    let one_bucket = json!({"Resources": {"Artifacts": {
        "Type": "AWS::S3::Bucket",
        "Properties": {"BucketName": "cfn-redeployed-bucket"}}}})
    .to_string();
    let two_buckets = json!({"Resources": {
        "Artifacts": {
            "Type": "AWS::S3::Bucket",
            "Properties": {"BucketName": "cfn-redeployed-bucket"}
        },
        "Added": {
            "Type": "AWS::S3::Bucket",
            "Properties": {"BucketName": "cfn-added-bucket"}
        }}})
    .to_string();

    let router = router();
    let (status, reason) = deploy(&router, "first", &one_bucket).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the first create to complete, got {status:?} ({reason:?})"
    );
    // A SECOND stack declaring the same named bucket: the bucket is already there, which
    // is the resource that deployment wanted.
    let (status, reason) = deploy(&router, "second", &one_bucket).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected a re-deployment of the same bucket to complete, got {status:?} ({reason:?})"
    );

    let (code, body) = call(
        &router,
        "UpdateStack",
        &[
            member("StackName", "first"),
            member("TemplateBody", &two_buckets),
        ],
    )
    .await;
    assert_eq!(code, 200, "UpdateStack answered {code}: {body}");
    let (status, reason) = settled_status(&router, "first").await;
    assert_eq!(
        status.as_deref(),
        Some("UPDATE_COMPLETE"),
        "expected the update to complete over a resource already there, got {status:?} \
         ({reason:?})"
    );
    let added = physical_id(&router, "first", "Added").await;
    let (code, _, text) = s3(&router, "PUT", &format!("/{added}/object"), b"bytes").await;
    assert_eq!(
        code, 200,
        "expected the bucket the update added to exist, got {code}: {text}"
    );
}

/// A resource whose declaration is missing a property its type REQUIRES describes no
/// resource that could be created, so the deployment fails naming that property rather
/// than completing over a resource nothing created.
#[tokio::test]
async fn a_declaration_missing_a_required_property_fails_the_deployment() {
    // `AWS::IAM::Role` requires `AssumeRolePolicyDocument`, and so does IAM's own
    // `CreateRole`.
    let template = json!({"Resources": {"Role": {
        "Type": "AWS::IAM::Role",
        "Properties": {"RoleName": "cfn-trustless-role"}}}})
    .to_string();

    let router = router();
    let (status, reason) = deploy(&router, "trustless-stack", &template).await;
    assert_eq!(
        status.as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "expected the failed create to be rolled back, got {status:?} ({reason:?})"
    );
    let reason = reason.unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Role"]),
        "expected the stack reason to be the summary naming the resource that failed, got \
         {reason:?}"
    );
    // WHICH property is missing is the resource's own diagnosis: it is reported on that
    // resource's event.
    let on_the_resource = resource_event_reason(
        &router,
        "trustless-stack",
        "Role",
        resource_status::CREATE_FAILED,
    )
    .await;
    assert!(
        on_the_resource.contains("AssumeRolePolicyDocument"),
        "expected the missing property on the failed resource's event, got \
         {on_the_resource:?}"
    );

    // Nothing was created for it: IAM holds no such role.
    let (code, body) = iam(&router, "GetRole", &[("RoleName", "cfn-trustless-role")]).await;
    assert_eq!(
        code, 404,
        "expected the role to be absent from IAM, got {code}: {body}"
    );
}

/// Deleting a stack deletes the resources it created in the services that own them.
#[tokio::test]
async fn deleting_a_stack_deletes_the_resources_it_created() {
    let template = json!({"Resources": {
        "Role": {
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": "cfn-deleted-role",
                "AssumeRolePolicyDocument": trust_policy()
            }
        }}})
    .to_string();

    let router = router();
    let (status, reason) = deploy(&router, "delete-stack", &template).await;
    assert_eq!(
        status.as_deref(),
        Some("CREATE_COMPLETE"),
        "expected the create to complete, got {status:?} ({reason:?})"
    );
    let (code, body) = iam(&router, "GetRole", &[("RoleName", "cfn-deleted-role")]).await;
    assert_eq!(code, 200, "expected the created role, got {code}: {body}");

    let (code, body) = call(
        &router,
        "DeleteStack",
        &[member("StackName", "delete-stack")],
    )
    .await;
    assert_eq!(code, 200, "DeleteStack answered {code}: {body}");
    wait_for_the_transition().await;

    let (code, body) = iam(&router, "GetRole", &[("RoleName", "cfn-deleted-role")]).await;
    assert_eq!(
        code, 404,
        "expected the role the stack created to be gone with the stack, got {code}: {body}"
    );
    let codes = xml_texts(&body, "Code");
    assert!(
        codes.iter().any(|code| code == "NoSuchEntity"),
        "expected IAM's own absent-entity code, got {codes:?} in {body}"
    );
}

// ---------------------------------------------------------------------------
// What a stack's declared CONTAINER-PLATFORM resources report, end to end over the
// awsQuery wire.
//
// The rule under test: for a stack whose template declares Amazon ECR, Amazon ECS or
// Elastic Load Balancing v2 resources, every declared output carries an `OutputValue` —
// the physical id that resource type's "Return values" documents `Ref` as returning, or
// the attribute `Fn::GetAtt` names, each in the FORM the owning service publishes it in.
// That is what the standard `{o["OutputKey"]: o["OutputValue"]}` read needs: an output
// reported with its key and NO value raises `KeyError` there, and an output whose value
// is a bare name is not something a client can address a load balancer or a target group
// by.
//
// Every case drives the real request path (form-encoded body in, XML envelope out)
// through the wire harness `stack_lifecycle.rs` builds. The table spans the class the
// rule generalises over: both template spellings (YAML with short-form intrinsics, JSON
// with long-form ones), resources the template NAMES and resources it leaves
// CloudFormation to name, each type in the catalogue, and — for the load balancer, whose
// published identifiers depend on its own properties — both schemes and two of the three
// types.
//
// The one type in these templates this emulator also CREATES in the service that owns it
// (`AWS::ECS::Cluster`, in Amazon ECS) is exercised by
// `core/router/tests/all/cloudformation_provisions_container_platform_resources.rs`,
// which mounts both services: a crate-local suite can only see CloudFormation's side of
// that call.
// ---------------------------------------------------------------------------

use crate::stack_outputs::{output_for, wire_outputs, WireOutput};
use service_com_amazonaws_cloudformation_cloudformation::state::status;

/// What one declared output must report.
enum Expected {
    /// Exactly this `OutputValue`.
    Value(String),
    /// A value of this FORM: for an identifier the OWNING SERVICE assigns — which no
    /// request fixes, so no row can spell it — the rule is the documented form.
    Form {
        described: String,
        check: Box<dyn Fn(&str) -> bool>,
    },
}

/// One row: a template, and what each of its outputs must report.
struct Case {
    case: &'static str,
    stack_name: &'static str,
    template: String,
    parameters: Vec<(String, String)>,
    expected: Vec<(&'static str, Expected)>,
}

fn params(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .enumerate()
        .flat_map(|(index, (key, value))| {
            let position = index + 1;
            vec![
                member(&format!("Parameters.member.{position}.ParameterKey"), key),
                member(
                    &format!("Parameters.member.{position}.ParameterValue"),
                    value,
                ),
            ]
        })
        .collect()
}

fn value(key: &'static str, value: String) -> (&'static str, Expected) {
    (key, Expected::Value(value))
}

fn form(
    key: &'static str,
    described: impl Into<String>,
    check: impl Fn(&str) -> bool + 'static,
) -> (&'static str, Expected) {
    (
        key,
        Expected::Form {
            described: described.into(),
            check: Box::new(check),
        },
    )
}

/// `arn:aws:ecr:<region>:<account>:repository/<name>`, the ARN
/// `Fn::GetAtt <repository>.Arn` reports.
fn repository_arn(name: &str) -> String {
    format!("arn:aws:ecr:{REGION}:{ACCOUNT_ID}:repository/{name}")
}

/// `<registry endpoint>/<name>` — the URI `Fn::GetAtt <repository>.RepositoryUri` reports
/// and a container image reference is built on, as Amazon ECR announces it here: a
/// `repositoryUri` is an address a client pushes and pulls at, so it names the endpoint
/// THIS emulator serves the registry at, not the
/// `aws_account_id.dkr.ecr.region.amazonaws.com` host AWS publishes, which nothing here
/// answers on. The stack's value is the one the repository's owning service reported, so
/// this reads it from the same place that service does.
fn repository_uri(name: &str) -> String {
    format!("{}/{name}", announced_registry_endpoint())
}

/// The `host[:port]` Amazon ECR announces its registry at for a stack's in-process create:
/// that call is addressed to ECR's own service endpoint host, which no registry client can
/// use, so the address announced is the one this emulator is served at — the base of an
/// emulator reached in process for a router that was given no serving address.
fn announced_registry_endpoint() -> &'static str {
    ::service_common::data_plane_addressing::url_authority(
        ::smithy_server_runtime::UNSERVED_URL_BASE,
    )
}

/// `arn:aws:ecs:<region>:<account>:task-definition/<family>:1` — what `Ref` of an
/// `AWS::ECS::TaskDefinition` returns for the FIRST revision of a family ("the first task
/// definition that you registered to a family is given a revision of 1").
fn task_definition_arn(family: &str) -> String {
    format!("arn:aws:ecs:{REGION}:{ACCOUNT_ID}:task-definition/{family}:1")
}

/// The network the load balancers of these rows are placed in: a VPC and two subnets of it,
/// in two Availability Zones, since Elastic Load Balancing resolves a placement against
/// Amazon EC2 and an Application Load Balancer needs subnets in at least two zones.
const NETWORK: &str = concat!(
    "  Vpc:\n",
    "    Type: AWS::EC2::VPC\n",
    "    Properties:\n",
    "      CidrBlock: 10.3.0.0/16\n",
    "  SubnetA:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.3.1.0/24\n",
    "      AvailabilityZone: us-east-1a\n",
    "  SubnetB:\n",
    "    Type: AWS::EC2::Subnet\n",
    "    Properties:\n",
    "      VpcId: !Ref Vpc\n",
    "      CidrBlock: 10.3.2.0/24\n",
    "      AvailabilityZone: us-east-1b\n",
);

/// Whether `value` is the 16 lowercase hexadecimal characters Elastic Load Balancing
/// assigns a load balancer or a target group as its id
/// (`.../app/my-load-balancer/50dc6c495c0c9188`).
fn is_elb_id(value: &str) -> bool {
    value.len() == 16
        && value
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

/// Whether `value` is the ARN of the target group named `name`:
/// `arn:aws:elasticloadbalancing:<region>:<account>:targetgroup/<name>/<id>`.
fn is_target_group_arn(value: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!(
            "arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:targetgroup/{name}/"
        ))
        .is_some_and(is_elb_id)
}

/// Whether `value` is the ARN of the `type_code` load balancer named `name`:
/// `arn:aws:elasticloadbalancing:<region>:<account>:loadbalancer/<type>/<name>/<id>`.
fn is_load_balancer_arn(value: &str, type_code: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!(
            "arn:aws:elasticloadbalancing:{REGION}:{ACCOUNT_ID}:loadbalancer/{type_code}/{name}/"
        ))
        .is_some_and(is_elb_id)
}

/// Whether `value` is an address a client on this machine can DIAL: the `host:port` of a
/// listener this emulator opened for the load balancer, which is what an APPLICATION load
/// balancer publishes as its `DNSName` — "the public DNS name of the load balancer" is the
/// address a client reaches it at, and this emulator serves that family's traffic itself.
fn is_published_address(value: &str) -> bool {
    value.parse::<std::net::SocketAddr>().is_ok()
}

/// Whether `value` is the host a NETWORK load balancer named `name` publishes: that family's
/// guide puts the `elb` label ahead of the Region and the id beside the name is the very
/// hexadecimal one its ARN carries
/// (`my-network-load-balancer-1234567890abcdef.elb.us-west-2.amazonaws.com`), with the scheme
/// stated nowhere in it. The emulator serves no traffic for that family, so it publishes the
/// name rather than an address.
fn is_network_dns_name(value: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!("{name}-"))
        .and_then(|rest| rest.strip_suffix(&format!(".elb.{REGION}.amazonaws.com")))
        .is_some_and(is_elb_id)
}

/// Whether `value` is the `<type>/<name>/<id>` path a load balancer's
/// `LoadBalancerFullName` reports — the resource part of its own ARN.
fn is_load_balancer_full_name(value: &str, type_code: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!("{type_code}/{name}/"))
        .is_some_and(is_elb_id)
}

/// Whether `value` is the `targetgroup/<name>/<id>` path a target group's
/// `TargetGroupFullName` reports.
fn is_target_group_full_name(value: &str, name: &str) -> bool {
    value
        .strip_prefix(&format!("targetgroup/{name}/"))
        .is_some_and(is_elb_id)
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            // The issue's own stack, in the YAML spelling with short-form intrinsics: the
            // registry, the task definition, the target group and the internet-facing
            // application load balancer an ECS service runs behind, every resource NAMED
            // by the template through a parameter, and the three outputs the issue's
            // deploy helper reads.
            case: "yaml, a named ecr + ecs + elbv2 application stack",
            stack_name: "shop-platform",
            template: [
                concat!(
                    "Parameters:\n",
                    "  AppName:\n",
                    "    Type: String\n",
                    "Resources:\n",
                ),
                NETWORK,
                concat!(
                "  AppRepository:\n",
                "    Type: AWS::ECR::Repository\n",
                "    Properties:\n",
                "      RepositoryName: !Sub \"${AppName}-images\"\n",
                "      ImageTagMutability: MUTABLE\n",
                "  TargetGroup:\n",
                "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
                "    Properties:\n",
                "      Name: !Sub \"${AppName}-tg\"\n",
                "      Port: 80\n",
                "      Protocol: HTTP\n",
                "      TargetType: ip\n",
                "      VpcId: !Ref Vpc\n",
                "  LoadBalancer:\n",
                "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
                "    Properties:\n",
                "      Name: !Sub \"${AppName}-alb\"\n",
                "      Scheme: internet-facing\n",
                "      Type: application\n",
                "      Subnets:\n",
                "        - !Ref SubnetA\n",
                "        - !Ref SubnetB\n",
                "  TaskDefinition:\n",
                "    Type: AWS::ECS::TaskDefinition\n",
                "    Properties:\n",
                "      Family: !Sub \"${AppName}-task\"\n",
                "      Cpu: \"256\"\n",
                "      Memory: \"512\"\n",
                "      NetworkMode: awsvpc\n",
                "      ContainerDefinitions:\n",
                "        - Name: app\n",
                "          Image: !Sub \"${AppRepository.RepositoryUri}:v1\"\n",
                "          Essential: true\n",
                "Outputs:\n",
                "  RepositoryUri:\n",
                "    Value: !GetAtt AppRepository.RepositoryUri\n",
                "  RepositoryArn:\n",
                "    Value: !GetAtt AppRepository.Arn\n",
                "  RepositoryName:\n",
                "    Value: !Ref AppRepository\n",
                "  TaskDefinitionArn:\n",
                "    Value: !Ref TaskDefinition\n",
                "  TargetGroupArn:\n",
                "    Value: !Ref TargetGroup\n",
                "  TargetGroupFullName:\n",
                "    Value: !GetAtt TargetGroup.TargetGroupFullName\n",
                "  LoadBalancerArn:\n",
                "    Value: !Ref LoadBalancer\n",
                "  LoadBalancerDnsName:\n",
                "    Value: !GetAtt LoadBalancer.DNSName\n",
                "  LoadBalancerFullName:\n",
                "    Value: !GetAtt LoadBalancer.LoadBalancerFullName\n",
                "  ContainerImage:\n",
                "    Value: !Sub \"${AppRepository.RepositoryUri}:v1\"\n",
                ),
            ]
            .concat(),
            parameters: params(&[("AppName", "shop")]),
            expected: vec![
                value("RepositoryUri", repository_uri("shop-images")),
                value("RepositoryArn", repository_arn("shop-images")),
                value("RepositoryName", "shop-images".to_string()),
                value("TaskDefinitionArn", task_definition_arn("shop-task")),
                form(
                    "TargetGroupArn",
                    "the target group's own ARN, targetgroup/shop-tg/<id>",
                    |value| is_target_group_arn(value, "shop-tg"),
                ),
                form(
                    "TargetGroupFullName",
                    "targetgroup/shop-tg/<id>",
                    |value| is_target_group_full_name(value, "shop-tg"),
                ),
                form(
                    "LoadBalancerArn",
                    "the load balancer's own ARN, loadbalancer/app/shop-alb/<id>",
                    |value| is_load_balancer_arn(value, "app", "shop-alb"),
                ),
                form(
                    "LoadBalancerDnsName",
                    "the host:port of the listener the emulator opened for the load balancer",
                    is_published_address,
                ),
                form("LoadBalancerFullName", "app/shop-alb/<id>", |value| {
                    is_load_balancer_full_name(value, "app", "shop-alb")
                }),
                value(
                    "ContainerImage",
                    format!("{}:v1", repository_uri("shop-images")),
                ),
            ],
        },
        Case {
            // The same class with the variables flipped: the JSON spelling with long-form
            // intrinsics, an INTERNAL NETWORK load balancer (whose published ARN segment
            // is `net`), and no resource named by the template — so each takes
            // CloudFormation's generated `<stack>-<logical id>-<unique id>` name, which is
            // what the identifiers are then built on.
            case: "json, an internal network load balancer and unnamed resources",
            stack_name: "warehouse",
            template: r#"{"Resources":{
                 "Vpc":{"Type":"AWS::EC2::VPC","Properties":{"CidrBlock":"10.4.0.0/16"}},
                 "SubnetA":{"Type":"AWS::EC2::Subnet","Properties":{
                   "VpcId":{"Ref":"Vpc"},"CidrBlock":"10.4.1.0/24",
                   "AvailabilityZone":"us-east-1a"}},
                 "Balancer":{"Type":"AWS::ElasticLoadBalancingV2::LoadBalancer","Properties":{
                   "Name":"warehouse-internal",
                   "Scheme":"internal","Type":"network",
                   "Subnets":[{"Ref":"SubnetA"}]}},
                 "Group":{"Type":"AWS::ElasticLoadBalancingV2::TargetGroup","Properties":{
                   "Name":"warehouse-tg","Port":8080,"Protocol":"TCP","TargetType":"ip",
                   "VpcId":{"Ref":"Vpc"}}},
                 "Registry":{"Type":"AWS::ECR::Repository"},
                 "Task":{"Type":"AWS::ECS::TaskDefinition","Properties":{
                   "Cpu":"512","Memory":"1024","NetworkMode":"awsvpc",
                   "ContainerDefinitions":[{"Name":"web","Image":"public.ecr.aws/nginx:1",
                     "Essential":true}]}}},
               "Outputs":{
                 "LoadBalancerArn":{"Value":{"Ref":"Balancer"}},
                 "LoadBalancerDnsName":{"Value":{"Fn::GetAtt":["Balancer","DNSName"]}},
                 "LoadBalancerName":{"Value":{"Fn::GetAtt":["Balancer","LoadBalancerName"]}},
                 "TargetGroupArn":{"Value":{"Ref":"Group"}},
                 "TargetGroupName":{"Value":{"Fn::GetAtt":["Group","TargetGroupName"]}},
                 "RepositoryUri":{"Value":{"Fn::GetAtt":["Registry","RepositoryUri"]}},
                 "TaskDefinitionArn":{"Value":{"Fn::GetAtt":["Task","TaskDefinitionArn"]}}}}"#
                .to_string(),
            parameters: vec![],
            expected: vec![
                form(
                    "LoadBalancerArn",
                    "loadbalancer/net/warehouse-internal/<id>",
                    |value| is_load_balancer_arn(value, "net", "warehouse-internal"),
                ),
                form(
                    "LoadBalancerDnsName",
                    "warehouse-internal-<id>.elb.us-east-1.amazonaws.com",
                    |value| is_network_dns_name(value, "warehouse-internal"),
                ),
                value("LoadBalancerName", "warehouse-internal".to_string()),
                form(
                    "TargetGroupArn",
                    "targetgroup/warehouse-tg/<id>",
                    |value| is_target_group_arn(value, "warehouse-tg"),
                ),
                value("TargetGroupName", "warehouse-tg".to_string()),
                // Neither the registry nor the task definition is named by the template,
                // so both are built on CloudFormation's generated physical name — a
                // repository's lower-cased ("Repository names … can only contain lower
                // case letters"), a task definition's as its family.
                form(
                    "RepositoryUri",
                    "<the registry endpoint this emulator serves>/warehouse-registry-<id>",
                    |value| {
                        value
                            .strip_prefix(&format!(
                                "{}/warehouse-registry-",
                                announced_registry_endpoint()
                            ))
                            .is_some_and(|id| {
                                !id.is_empty() && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
                            })
                    },
                ),
                form(
                    "TaskDefinitionArn",
                    "arn:aws:ecs:us-east-1:<account>:task-definition/warehouse-Task-<id>:1",
                    |value| {
                        value
                            .strip_prefix(&format!(
                                "arn:aws:ecs:{REGION}:{ACCOUNT_ID}:task-definition/warehouse-Task-"
                            ))
                            .is_some_and(|rest| rest.ends_with(":1"))
                    },
                ),
            ],
        },
    ]
}

/// Every declared output reports its resolved value.
#[tokio::test]
async fn a_container_platform_stacks_outputs_all_carry_their_resolved_value() {
    let router = router();
    for case in cases() {
        let (status, body) =
            create_stack(&router, case.stack_name, &case.template, &case.parameters).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected CreateStack to answer 200, got {status} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let (status, xml) = describe_stack(&router, case.stack_name).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected DescribeStacks to answer 200, got {status} {xml}",
            case.case
        );
        assert_eq!(
            xml_text(&xml, "StackStatus").as_deref(),
            Some(status::CREATE_COMPLETE),
            "case {:?}: expected the stack to have completed; status reason {:?}",
            case.case,
            xml_text(&xml, "StackStatusReason")
        );

        let outputs = wire_outputs(&xml);
        assert_eq!(
            outputs.len(),
            case.expected.len(),
            "case {:?}: expected one output per declared output, got {outputs:?}",
            case.case
        );
        for (key, expected) in &case.expected {
            let reported = reported_value(case.case, &outputs, key);
            match expected {
                Expected::Value(wanted) => assert_eq!(
                    &reported, wanted,
                    "case {:?}: output {key:?}",
                    case.case
                ),
                Expected::Form { described, check } => assert!(
                    check(&reported),
                    "case {:?}: expected output {key:?} to be {described}, got {reported:?}",
                    case.case
                ),
            }
        }
    }
}

/// The `OutputValue` the stack reports for `key` — which must be there: an output
/// reported with its key and no value is what the issue's
/// `{o["OutputKey"]: o["OutputValue"]}` read raises `KeyError` on.
fn reported_value(case: &str, outputs: &[WireOutput], key: &str) -> String {
    let output = output_for(outputs, key)
        .unwrap_or_else(|| panic!("case {case:?}: no output named {key:?}, got {outputs:?}"));
    let value = output.value.clone().unwrap_or_else(|| {
        panic!(
            "case {case:?}: output {key:?} carries no OutputValue at all, got {output:?} \
             (the standard {{o[\"OutputKey\"]: o[\"OutputValue\"]}} read raises KeyError here)"
        )
    });
    assert!(
        !value.is_empty(),
        "case {case:?}: output {key:?} carries an empty OutputValue"
    );
    assert!(
        !value.contains("${"),
        "case {case:?}: output {key:?} still carries an unresolved expression: {value:?}"
    );
    value
}

/// Two resources of one stack never carry the same assigned id, and the same resource
/// reports the same one on every read — the two invariants an identifier the OWNING
/// SERVICE assigns has to keep for a client to address the resource by it.
#[tokio::test]
async fn an_assigned_identifier_is_unique_per_resource_and_stable() {
    let router = router();
    let case = "two load balancers and two target groups in one stack";
    let template = [
        "Resources:\n",
        NETWORK,
        concat!(
        "  FirstBalancer:\n",
        "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
        "    Properties:\n",
        "      Name: shared-name\n",
        "      Subnets:\n",
        "        - !Ref SubnetA\n",
        "        - !Ref SubnetB\n",
        "  SecondBalancer:\n",
        "    Type: AWS::ElasticLoadBalancingV2::LoadBalancer\n",
        "    Properties:\n",
        "      Name: second-alb\n",
        "      Subnets:\n",
        "        - !Ref SubnetA\n",
        "        - !Ref SubnetB\n",
        "  FirstGroup:\n",
        // The same NAME as the first load balancer: Elastic Load Balancing keeps the two
        // name spaces apart, and the ids it assigns them are still distinct.
        "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
        "    Properties:\n",
        "      Name: shared-name\n",
        "      Port: 80\n",
        "      Protocol: HTTP\n",
        "      VpcId: !Ref Vpc\n",
        "  SecondGroup:\n",
        "    Type: AWS::ElasticLoadBalancingV2::TargetGroup\n",
        "    Properties:\n",
        "      Name: second-tg\n",
        "      Port: 80\n",
        "      Protocol: HTTP\n",
        "      VpcId: !Ref Vpc\n",
        "Outputs:\n",
        "  FirstBalancer:\n",
        "    Value: !Ref FirstBalancer\n",
        "  SecondBalancer:\n",
        "    Value: !Ref SecondBalancer\n",
        "  FirstGroup:\n",
        "    Value: !Ref FirstGroup\n",
        "  SecondGroup:\n",
        "    Value: !Ref SecondGroup\n",
        ),
    ]
    .concat();
    let (status, body) = create_stack(&router, "identity-stack", &template, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStack to answer 200, got {status} {body}"
    );
    wait_for_the_transition().await;

    let (_, first_read) = describe_stack(&router, "identity-stack").await;
    let reported: Vec<String> = ["FirstBalancer", "SecondBalancer", "FirstGroup", "SecondGroup"]
        .iter()
        .map(|key| reported_value(case, &wire_outputs(&first_read), key))
        .collect();
    let mut unique = reported.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        reported.len(),
        "case {case:?}: expected four distinct identifiers, got {reported:?}"
    );
    // The two that share a NAME still carry different assigned ids: the ARN's last
    // segment is that id, and the two name spaces are distinct.
    let balancer_id = reported[0].rsplit('/').next().unwrap_or_default();
    let group_id = reported[2].rsplit('/').next().unwrap_or_default();
    assert_ne!(
        balancer_id, group_id,
        "case {case:?}: expected the load balancer and the target group named \
         \"shared-name\" to carry different assigned ids, got {reported:?}"
    );

    let (_, second_read) = describe_stack(&router, "identity-stack").await;
    let again: Vec<String> = ["FirstBalancer", "SecondBalancer", "FirstGroup", "SecondGroup"]
        .iter()
        .map(|key| reported_value(case, &wire_outputs(&second_read), key))
        .collect();
    assert_eq!(
        again, reported,
        "case {case:?}: expected the same identifiers on a second read"
    );
}
