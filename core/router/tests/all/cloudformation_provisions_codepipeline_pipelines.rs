// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::CodePipeline::Pipeline`, `AWS::S3::Bucket` and `AWS::IAM::Role` resources its
//! template declares, in the services that OWN those types.
//!
//! The rule under test: **a stack reports a declared resource as created only where the
//! service that owns the type created it, through that service's own API and with the
//! declared properties applied, so the physical resource id the stack publishes — and the
//! `Outputs` evaluated over those ids — address resources those services really hold; a
//! create an owning service refuses leaves the stack in its rolled-back status with nothing
//! behind it, and deleting the stack releases what it made.** For a pipeline the handle is
//! its NAME ("Ref returns the pipeline name"), for a bucket its name and for a role its
//! role name, so once the stack completes `GetPipeline` answers for the published pipeline
//! name, `PutObject` writes into the published bucket and `GetRole` answers for the
//! published role — rather than CodePipeline answering `PipelineNotFoundException` and
//! Amazon S3 answering `NoSuchBucket` for resources their own stack just reported complete.
//!
//! Four crates have to agree for that to hold (CloudFormation makes the calls; CodePipeline,
//! Amazon S3 and IAM serve them), so this suite lives here: a crate-local test can only ever
//! see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES each
//! resource or leaves the name to CloudFormation, the Region the stack is deployed in, the
//! ORDER the resources are written in the body (which is not the order they have to be
//! created in — the pipeline is declared before the role and bucket it refers to in the
//! second row), the sub-resource configuration a declaration carries (a bucket with a
//! `VersioningConfiguration` against one without, a role with an inline policy against one
//! without), which stages and action categories the pipeline declares, whether tags are
//! declared, and a create an owning service REFUSES.
//! Deleting a completed stack is the same rule read backwards and is the suite's last row.
//!
//! Assembling the whole router costs stack (see `boot_serve.rs`), so it is built — and the
//! requests driven — on a sized thread. The awsQuery form encoder and the XML text reader
//! are the test crate's own ([`crate::query_wire`]), not another copy.

use std::sync::Arc;
use std::time::Duration;

use aws_emulator_router::{build_router, InMemoryEngine};
use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{element, elements, form_encode, opt_element};

/// Keep this at or above `boot_serve.rs`'s stack: the same assembly runs here.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// Longer than CloudFormation's clock-owed stack transition (`STATUS_TRANSITION_DELAY`,
/// 500 ms), so a stack this suite deployed has reached its terminal status — which is when
/// its resources are reported — by the time it is read. A failed create owes TWO of those
/// spans (the create's failure, then the rollback), so the waits below are taken twice
/// where a rollback is expected.
const PAST_STACK_SPAN: Duration = Duration::from_millis(900);

/// CodePipeline's `X-Amz-Target` prefix, from that service's own model (service shape
/// `CodePipeline_20150709`).
const CODEPIPELINE_TARGET_PREFIX: &str = "CodePipeline_20150709";

/// The error code CodePipeline answers for a pipeline it does not hold.
const PIPELINE_NOT_FOUND: &str = "PipelineNotFoundException";

/// The error code Amazon S3 answers for a bucket it does not hold.
const NO_SUCH_BUCKET: &str = "NoSuchBucket";

/// The longest name CodePipeline accepts for a pipeline (`PipelineName`, `@length(min: 1,
/// max: 100)` in that service's own model), which a name CloudFormation GENERATES is held
/// to as well.
const MAX_PIPELINE_NAME: usize = 100;

/// The documented bounds on an Amazon S3 general purpose bucket name.
const MIN_BUCKET_NAME_LEN: usize = 3;
/// See [`MIN_BUCKET_NAME_LEN`].
const MAX_BUCKET_NAME_LEN: usize = 63;

/// The emulator this suite drives: the whole router over one engine, and the runtime its
/// requests are blocked on.
struct Emulator {
    router: MultiServiceRouter,
    runtime: tokio::runtime::Runtime,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
        let mut router = build_router(engine);
        router.set_request_authentication(false);
        Self {
            router,
            runtime: tokio::runtime::Builder::new_current_thread()
                // The time driver, because the emulator's services run CLOCKS: a service
                // whose state owes a transition spawns a background task that waits on a
                // timer, and a timer on a runtime built without this panics the task — and,
                // on a current-thread runtime, that panic unwinds out of whichever
                // `block_on` happened to poll it. Enabling it is what lets those clocks run
                // here, and it changes nothing this suite asserts.
                .enable_time()
                .build()
                .expect("a current-thread runtime"),
        }
    }

    /// Drive one request and answer its status, headers and body.
    fn send(
        &self,
        signing_name: &str,
        region: &str,
        method: &str,
        uri: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, http::HeaderMap, Vec<u8>) {
        let host = format!("{signing_name}.{region}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{uri}"))
            .header("host", host)
            .header("authorization", authorization(signing_name, region));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.runtime.block_on(self.router.dispatch(request));
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        (status, headers, response.into_body().to_vec())
    }

    /// One awsQuery action of `signing_name`, with the members form-encoded.
    fn query(
        &self,
        signing_name: &str,
        region: &str,
        version: &str,
        action: &str,
        members: &[(&str, &str)],
    ) -> (u16, String) {
        let mut body = format!("Action={action}&Version={version}");
        for (key, value) in members {
            body.push('&');
            body.push_str(&form_encode(key));
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let (status, _, payload) = self.send(
            signing_name,
            region,
            "POST",
            "/",
            &[("content-type", "application/x-www-form-urlencoded")],
            body.into_bytes(),
        );
        (status, String::from_utf8_lossy(&payload).into_owned())
    }

    /// One CloudFormation action (awsQuery, API version 2010-05-15).
    fn cloudformation(&self, region: &str, action: &str, members: &[(&str, &str)]) -> (u16, String) {
        self.query("cloudformation", region, "2010-05-15", action, members)
    }

    /// One IAM action (awsQuery, API version 2010-05-08). IAM is global, so every call is
    /// signed for the Region the stack under test used.
    fn iam(&self, region: &str, action: &str, members: &[(&str, &str)]) -> (u16, String) {
        self.query("iam", region, "2010-05-08", action, members)
    }

    /// One awsJson1_1 CodePipeline operation.
    fn codepipeline(&self, region: &str, operation: &str, body: Value) -> (u16, Value) {
        let (status, _, payload) = self.send(
            "codepipeline",
            region,
            "POST",
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                (
                    "x-amz-target",
                    &format!("{CODEPIPELINE_TARGET_PREFIX}.{operation}"),
                ),
            ],
            serde_json::to_vec(&body).expect("the request body serializes"),
        );
        let parsed = if payload.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&payload).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One S3 request against a bucket or object path.
    fn s3(
        &self,
        region: &str,
        method: &str,
        uri: &str,
        body: &[u8],
    ) -> (u16, http::HeaderMap, Vec<u8>) {
        self.send("s3", region, method, uri, &[], body.to_vec())
    }
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name` in `region`,
/// which is what routes the request and what each handler reads its Region from. The
/// signature is never checked: this router runs with request authentication off.
fn authorization(signing_name: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=deadbeef"
    )
}

/// The stack's resources as `(LogicalResourceId, PhysicalResourceId)` pairs, read member by
/// member so each logical id stays paired with its own physical id.
fn resources_of(xml: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find("<member") {
        let after_tag = &rest[open + "<member".len()..];
        let Some(tag_end) = after_tag.find('>') else {
            break;
        };
        let after_open = &after_tag[tag_end + 1..];
        let Some(close) = after_open.find("</member>") else {
            break;
        };
        let entry = &after_open[..close];
        let logical = elements(entry, "LogicalResourceId").into_iter().next();
        let physical = elements(entry, "PhysicalResourceId").into_iter().next();
        if let (Some(logical), Some(physical)) = (logical, physical) {
            found.push((logical, physical));
        }
        rest = &after_open[close + "</member>".len()..];
    }
    found
}

/// One stack's outputs as `(OutputKey, OutputValue)` pairs.
fn outputs_of(xml: &str) -> Vec<(String, String)> {
    elements(xml, "OutputKey")
        .into_iter()
        .zip(elements(xml, "OutputValue"))
        .collect()
}

/// The physical resource id of one logical id, or a panic naming what was reported.
fn physical_id(case: &str, resources: &[(String, String)], logical_id: &str) -> String {
    resources
        .iter()
        .find(|(logical, _)| logical == logical_id)
        .map(|(_, physical)| physical.clone())
        .unwrap_or_else(|| {
            panic!("case {case:?}: the stack must report {logical_id}, got {resources:?}")
        })
}

/// One output's value, or a panic naming what was reported.
fn output(case: &str, outputs: &[(String, String)], key: &str) -> String {
    outputs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
        .unwrap_or_else(|| panic!("case {case:?}: the stack must report {key}, got {outputs:?}"))
}

/// Whether `name` satisfies the documented Amazon S3 general purpose bucket naming rules
/// this suite can check: 3 to 63 characters, only lowercase letters, numbers, dots and
/// hyphens, beginning and ending with a letter or a number.
fn is_legal_bucket_name(name: &str) -> bool {
    let length = (MIN_BUCKET_NAME_LEN..=MAX_BUCKET_NAME_LEN).contains(&name.len());
    let charset = name.chars().all(|character| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || character == '.'
            || character == '-'
    });
    let bounded = name
        .chars()
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && name
            .chars()
            .next_back()
            .is_some_and(|last| last.is_ascii_alphanumeric());
    length && charset && bounded
}

/// A stack the template names every resource of: a versioned artifact bucket, a pipeline
/// role, and a pipeline whose source stage reads that bucket and whose second stage is a
/// manual approval. Every resource carries a declared tag, and the `Outputs` are evaluated
/// over the identifiers the owning services assigned.
const NAMED_TEMPLATE: &str = r#"{
 "Resources": {
  "ArtifactBucket": {
   "Type": "AWS::S3::Bucket",
   "Properties": {
    "BucketName": "cfn-codepipeline-named-artifacts",
    "VersioningConfiguration": { "Status": "Enabled" },
    "Tags": [ { "Key": "suite", "Value": "cfn-codepipeline" } ]
   }
  },
  "PipelineRole": {
   "Type": "AWS::IAM::Role",
   "Properties": {
    "RoleName": "cfn-codepipeline-named-role",
    "AssumeRolePolicyDocument": {
     "Version": "2012-10-17",
     "Statement": [
      {
       "Effect": "Allow",
       "Principal": { "Service": "codepipeline.amazonaws.com" },
       "Action": "sts:AssumeRole"
      }
     ]
    },
    "Policies": [
     {
      "PolicyName": "artifact-access",
      "PolicyDocument": {
       "Version": "2012-10-17",
       "Statement": [
        {
         "Effect": "Allow",
         "Action": [ "s3:GetObject", "s3:PutObject" ],
         "Resource": "*"
        }
       ]
      }
     }
    ]
   }
  },
  "Pipeline": {
   "Type": "AWS::CodePipeline::Pipeline",
   "Properties": {
    "Name": "cfn-codepipeline-named",
    "RoleArn": { "Fn::GetAtt": [ "PipelineRole", "Arn" ] },
    "ArtifactStore": { "Type": "S3", "Location": { "Ref": "ArtifactBucket" } },
    "Stages": [
     {
      "Name": "Source",
      "Actions": [
       {
        "Name": "SourceAction",
        "ActionTypeId": {
         "Category": "Source", "Owner": "AWS", "Provider": "S3", "Version": "1"
        },
        "Configuration": {
         "S3Bucket": { "Ref": "ArtifactBucket" },
         "S3ObjectKey": "source.zip",
         "PollForSourceChanges": "false"
        },
        "OutputArtifacts": [ { "Name": "SourceArtifact" } ],
        "RunOrder": 1
       }
      ]
     },
     {
      "Name": "Approve",
      "Actions": [
       {
        "Name": "ApproveAction",
        "ActionTypeId": {
         "Category": "Approval", "Owner": "AWS", "Provider": "Manual", "Version": "1"
        },
        "RunOrder": 1
       }
      ]
     }
    ],
    "Tags": [ { "Key": "suite", "Value": "cfn-codepipeline" } ]
   }
  }
 },
 "Outputs": {
  "PipelineName": { "Value": { "Ref": "Pipeline" } },
  "BucketName": { "Value": { "Ref": "ArtifactBucket" } },
  "RoleName": { "Value": { "Ref": "PipelineRole" } },
  "RoleArn": { "Value": { "Fn::GetAtt": [ "PipelineRole", "Arn" ] } }
 }
}"#;

/// A stack the template names NOTHING in, deployed in another Region, written with the
/// pipeline FIRST — before the role and the bucket it refers to — so the order the
/// resources are created in is the dependency order rather than the order the body lists
/// them. Its bucket declares no `VersioningConfiguration` and its second stage is a deploy
/// back into the same bucket.
const GENERATED_TEMPLATE: &str = r#"{
 "Resources": {
  "Pipeline": {
   "Type": "AWS::CodePipeline::Pipeline",
   "Properties": {
    "RoleArn": { "Fn::GetAtt": [ "PipelineRole", "Arn" ] },
    "ArtifactStore": { "Type": "S3", "Location": { "Ref": "ArtifactBucket" } },
    "Stages": [
     {
      "Name": "Source",
      "Actions": [
       {
        "Name": "SourceAction",
        "ActionTypeId": {
         "Category": "Source", "Owner": "AWS", "Provider": "S3", "Version": "1"
        },
        "Configuration": {
         "S3Bucket": { "Ref": "ArtifactBucket" },
         "S3ObjectKey": "source.zip"
        },
        "OutputArtifacts": [ { "Name": "SourceArtifact" } ]
       }
      ]
     },
     {
      "Name": "Deploy",
      "Actions": [
       {
        "Name": "DeployAction",
        "ActionTypeId": {
         "Category": "Deploy", "Owner": "AWS", "Provider": "S3", "Version": "1"
        },
        "Configuration": {
         "BucketName": { "Ref": "ArtifactBucket" },
         "ObjectKey": "deployed.zip",
         "Extract": "false"
        },
        "InputArtifacts": [ { "Name": "SourceArtifact" } ]
       }
      ]
     }
    ]
   }
  },
  "PipelineRole": {
   "Type": "AWS::IAM::Role",
   "Properties": {
    "AssumeRolePolicyDocument": {
     "Version": "2012-10-17",
     "Statement": [
      {
       "Effect": "Allow",
       "Principal": { "Service": "codepipeline.amazonaws.com" },
       "Action": "sts:AssumeRole"
      }
     ]
    }
   }
  },
  "ArtifactBucket": { "Type": "AWS::S3::Bucket" }
 },
 "Outputs": {
  "PipelineName": { "Value": { "Ref": "Pipeline" } },
  "BucketName": { "Value": { "Ref": "ArtifactBucket" } },
  "RoleName": { "Value": { "Ref": "PipelineRole" } },
  "RoleArn": { "Value": { "Fn::GetAtt": [ "PipelineRole", "Arn" ] } }
 }
}"#;

/// A stack whose pipeline names a role NOTHING in this account holds, which CodePipeline
/// refuses. The bucket beside it is created before the pipeline is attempted, so this row
/// also states what a refusal owes: the stack reaches its rolled-back status and nothing
/// the create had made is left behind.
const REFUSED_TEMPLATE: &str = r#"{
 "Resources": {
  "ArtifactBucket": {
   "Type": "AWS::S3::Bucket",
   "Properties": { "BucketName": "cfn-codepipeline-refused-artifacts" }
  },
  "Pipeline": {
   "Type": "AWS::CodePipeline::Pipeline",
   "Properties": {
    "Name": "cfn-codepipeline-refused",
    "RoleArn": "arn:aws:iam::000000000000:role/cfn-codepipeline-role-nothing-holds",
    "ArtifactStore": { "Type": "S3", "Location": { "Ref": "ArtifactBucket" } },
    "Stages": [
     {
      "Name": "Source",
      "Actions": [
       {
        "Name": "SourceAction",
        "ActionTypeId": {
         "Category": "Source", "Owner": "AWS", "Provider": "S3", "Version": "1"
        },
        "Configuration": {
         "S3Bucket": { "Ref": "ArtifactBucket" },
         "S3ObjectKey": "source.zip"
        },
        "OutputArtifacts": [ { "Name": "SourceArtifact" } ]
       }
      ]
     },
     {
      "Name": "Approve",
      "Actions": [
       {
        "Name": "ApproveAction",
        "ActionTypeId": {
         "Category": "Approval", "Owner": "AWS", "Provider": "Manual", "Version": "1"
        }
       }
      ]
     }
    ]
   }
  }
 }
}"#;

/// One row of the table: a stack to deploy, and what the resources it declares must be.
struct Case {
    /// The label every assertion of this row names.
    label: &'static str,
    region: &'static str,
    stack: &'static str,
    template: &'static str,
    /// The pipeline name the template asks for, or `None` where CloudFormation generates
    /// it.
    pipeline_name: Option<&'static str>,
    /// The bucket name the template asks for, or `None` where CloudFormation generates it.
    bucket_name: Option<&'static str>,
    /// The role name the template asks for, or `None` where CloudFormation generates it.
    role_name: Option<&'static str>,
    /// Whether the bucket's declaration carries `VersioningConfiguration: Enabled`.
    versioned: bool,
    /// The stage names the pipeline declares, in order.
    stages: &'static [&'static str],
    /// The capability the deployment has to acknowledge: a template declaring an IAM
    /// resource needs `CAPABILITY_IAM`, and one that NAMES it needs
    /// `CAPABILITY_NAMED_IAM`.
    capability: &'static str,
    /// The inline policy the role's declaration carries, where it carries one.
    inline_policy: Option<&'static str>,
}

/// The rows, spanning what the rule generalises over: named against generated identifiers,
/// the Region, the order the body lists the resources in, the sub-resource configuration a
/// bucket declares, and which stages the pipeline carries.
const CASES: &[Case] = &[
    Case {
        label: "named resources, versioned bucket, approval stage, us-east-1",
        region: "us-east-1",
        stack: "cfn-codepipeline-Named",
        template: NAMED_TEMPLATE,
        pipeline_name: Some("cfn-codepipeline-named"),
        bucket_name: Some("cfn-codepipeline-named-artifacts"),
        role_name: Some("cfn-codepipeline-named-role"),
        versioned: true,
        stages: &["Source", "Approve"],
        capability: "CAPABILITY_NAMED_IAM",
        inline_policy: Some("artifact-access"),
    },
    Case {
        label: "generated names, unversioned bucket, deploy stage, eu-west-1",
        region: "eu-west-1",
        stack: "cfn-codepipeline-Generated",
        template: GENERATED_TEMPLATE,
        pipeline_name: None,
        bucket_name: None,
        role_name: None,
        versioned: false,
        stages: &["Source", "Deploy"],
        capability: "CAPABILITY_IAM",
        inline_policy: None,
    },
];

#[test]
fn a_cfn_declared_pipeline_bucket_and_role_exist_in_the_services_that_own_them() {
    std::thread::Builder::new()
        .name("cfn-codepipeline-provisioning".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let emulator = Emulator::new();
            // What each row's stack published, kept so the deletion row below can address
            // the resources a stack with GENERATED names made.
            let mut deployed: Vec<(&Case, String, String)> = Vec::new();

            for case in CASES {
                let label = case.label;
                let region = case.region;

                let (status, body) = emulator.cloudformation(
                    region,
                    "CreateStack",
                    &[
                        ("StackName", case.stack),
                        ("TemplateBody", case.template),
                        ("Capabilities.member.1", case.capability),
                    ],
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: CreateStack must succeed, got {status} with body {body}"
                );
                std::thread::sleep(PAST_STACK_SPAN);

                let (status, described) =
                    emulator.cloudformation(region, "DescribeStacks", &[("StackName", case.stack)]);
                assert_eq!(
                    status, 200,
                    "case {label:?}: DescribeStacks must succeed, got {status} with body \
                     {described}"
                );
                let stack_status = element(&described, "StackStatus");
                assert_eq!(
                    stack_status, "CREATE_COMPLETE",
                    "case {label:?}: the stack must report CREATE_COMPLETE, got \
                     {stack_status:?} with reason {:?}",
                    opt_element(&described, "StackStatusReason")
                );

                let (status, listed) = emulator.cloudformation(
                    region,
                    "DescribeStackResources",
                    &[("StackName", case.stack)],
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: DescribeStackResources must succeed, got {status} with \
                     body {listed}"
                );
                let resources = resources_of(&listed);
                let pipeline = physical_id(label, &resources, "Pipeline");
                let bucket = physical_id(label, &resources, "ArtifactBucket");
                let role = physical_id(label, &resources, "PipelineRole");

                // The published identifiers are the ones the template asked for, and where
                // it asked for none they are identifiers the owning service admits.
                match case.pipeline_name {
                    Some(expected) => assert_eq!(
                        pipeline, expected,
                        "case {label:?}: a pipeline the template names keeps that name, got \
                         {pipeline:?}"
                    ),
                    None => assert!(
                        !pipeline.is_empty() && pipeline.len() <= MAX_PIPELINE_NAME,
                        "case {label:?}: a generated pipeline name must be one CodePipeline \
                         accepts, got {pipeline:?}"
                    ),
                }
                match case.bucket_name {
                    Some(expected) => assert_eq!(
                        bucket, expected,
                        "case {label:?}: a bucket the template names keeps that name, got \
                         {bucket:?}"
                    ),
                    None => assert!(
                        is_legal_bucket_name(&bucket),
                        "case {label:?}: a generated bucket name must satisfy S3's naming \
                         rules, got {bucket:?}"
                    ),
                }
                if let Some(expected) = case.role_name {
                    assert_eq!(
                        role, expected,
                        "case {label:?}: a role the template names keeps that name, got {role:?}"
                    );
                }

                // IAM holds the role the stack published, and its ARN is the one the stack
                // reports as the role's `Arn` attribute.
                let (status, described_role) =
                    emulator.iam(region, "GetRole", &[("RoleName", role.as_str())]);
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetRole on the stack's role must answer 200, got {status} \
                     with body {described_role}"
                );
                let role_arn = element(&described_role, "Arn");
                // An inline policy the declaration carries is a `PutRolePolicy` of its own
                // on the created role, so IAM lists it under the name the template gave it.
                let (status, policies) =
                    emulator.iam(region, "ListRolePolicies", &[("RoleName", role.as_str())]);
                assert_eq!(
                    status, 200,
                    "case {label:?}: ListRolePolicies must answer 200, got {status} with body \
                     {policies}"
                );
                let listed = elements(&policies, "member");
                let expected: Vec<String> = case
                    .inline_policy
                    .map(|name| vec![name.to_string()])
                    .unwrap_or_default();
                assert_eq!(
                    listed, expected,
                    "case {label:?}: the role must carry exactly the inline policies the \
                     template declared, got body {policies}"
                );

                // CodePipeline holds the pipeline the stack published, with the declaration
                // the template carried: the stages it declares, the artifact store pointing
                // at the bucket this stack created, and the role ARN IAM really minted.
                let (status, answer) = emulator.codepipeline(
                    region,
                    "GetPipeline",
                    serde_json::json!({ "name": pipeline }),
                );
                assert_eq!(
                    status, 200,
                    "case {label:?}: GetPipeline on the stack's pipeline must answer 200, got \
                     {status} with {answer}"
                );
                let declaration = &answer["pipeline"];
                assert_eq!(
                    declaration["name"].as_str(),
                    Some(pipeline.as_str()),
                    "case {label:?}: the pipeline must carry the name the stack published, got \
                     {declaration}"
                );
                assert_eq!(
                    declaration["roleArn"].as_str(),
                    Some(role_arn.as_str()),
                    "case {label:?}: the pipeline's role must be the role IAM minted for this \
                     stack, got {declaration}"
                );
                assert_eq!(
                    declaration["artifactStore"]["location"].as_str(),
                    Some(bucket.as_str()),
                    "case {label:?}: the pipeline's artifact store must name the bucket this \
                     stack created, got {declaration}"
                );
                let stages: Vec<&str> = declaration["stages"]
                    .as_array()
                    .map(|stages| {
                        stages
                            .iter()
                            .filter_map(|stage| stage["name"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    stages, case.stages,
                    "case {label:?}: the pipeline must carry the stages the template declared, \
                     got {declaration}"
                );

                // Amazon S3 holds the bucket the stack published, with the versioning the
                // declaration asked for, and an object written into it reads back.
                let (status, _, body) = emulator.s3(region, "HEAD", &format!("/{bucket}"), b"");
                assert_eq!(
                    status,
                    200,
                    "case {label:?}: HeadBucket on the stack's bucket must answer 200, got \
                     {status} with body {:?}",
                    String::from_utf8_lossy(&body)
                );
                let (status, _, versioning) =
                    emulator.s3(region, "GET", &format!("/{bucket}?versioning"), b"");
                assert_eq!(
                    status,
                    200,
                    "case {label:?}: GetBucketVersioning must answer 200, got {status} with \
                     body {:?}",
                    String::from_utf8_lossy(&versioning)
                );
                let versioning = String::from_utf8_lossy(&versioning).into_owned();
                let reported = opt_element(&versioning, "Status");
                if case.versioned {
                    assert_eq!(
                        reported.as_deref(),
                        Some("Enabled"),
                        "case {label:?}: the declared VersioningConfiguration must have \
                         reached S3, got {versioning}"
                    );
                } else {
                    assert_eq!(
                        reported, None,
                        "case {label:?}: a bucket declaring no VersioningConfiguration must \
                         report no versioning status, got {versioning}"
                    );
                }
                let (status, headers, body) = emulator.s3(
                    region,
                    "PUT",
                    &format!("/{bucket}/source.zip"),
                    b"a source bundle",
                );
                assert_eq!(
                    status,
                    200,
                    "case {label:?}: PutObject into the stack's bucket must answer 200, got \
                     {status} with body {:?}",
                    String::from_utf8_lossy(&body)
                );
                let version = headers
                    .get("x-amz-version-id")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string);
                assert_eq!(
                    version.is_some(),
                    case.versioned,
                    "case {label:?}: a write into a versioned bucket is the only one that \
                     reports a version id, got {version:?}"
                );
                let (status, _, read) =
                    emulator.s3(region, "GET", &format!("/{bucket}/source.zip"), b"");
                assert_eq!(
                    (status, read.as_slice()),
                    (200, b"a source bundle".as_slice()),
                    "case {label:?}: the object must read back, got {status} with body {:?}",
                    String::from_utf8_lossy(&read)
                );

                // The stack's outputs were evaluated over those same identifiers.
                let outputs = outputs_of(&described);
                assert_eq!(
                    output(label, &outputs, "PipelineName"),
                    pipeline,
                    "case {label:?}: Ref of the pipeline must be the pipeline's name, got \
                     {outputs:?}"
                );
                assert_eq!(
                    output(label, &outputs, "BucketName"),
                    bucket,
                    "case {label:?}: Ref of the bucket must be the bucket's name, got \
                     {outputs:?}"
                );
                assert_eq!(
                    output(label, &outputs, "RoleName"),
                    role,
                    "case {label:?}: Ref of the role must be the role's name, got {outputs:?}"
                );
                assert_eq!(
                    output(label, &outputs, "RoleArn"),
                    role_arn,
                    "case {label:?}: the role's Arn attribute must be the ARN IAM minted, got \
                     {outputs:?}"
                );

                deployed.push((case, pipeline, bucket));
            }

            // A create an owning service REFUSES is not reported as created: the stack
            // rolls back, and neither the pipeline nor the bucket the template declared is
            // left behind.
            let label = "a pipeline whose role nothing holds";
            let region = "us-east-1";
            let (status, body) = emulator.cloudformation(
                region,
                "CreateStack",
                &[
                    ("StackName", "cfn-codepipeline-Refused"),
                    ("TemplateBody", REFUSED_TEMPLATE),
                ],
            );
            assert_eq!(
                status, 200,
                "case {label:?}: CreateStack must be accepted, got {status} with body {body}"
            );
            // Twice the clock-owed span: the create's failure falls due, then the rollback.
            std::thread::sleep(PAST_STACK_SPAN);
            std::thread::sleep(PAST_STACK_SPAN);
            let (status, described) = emulator.cloudformation(
                region,
                "DescribeStacks",
                &[("StackName", "cfn-codepipeline-Refused")],
            );
            assert_eq!(
                status, 200,
                "case {label:?}: DescribeStacks must succeed, got {status} with body {described}"
            );
            let stack_status = element(&described, "StackStatus");
            assert_eq!(
                stack_status, "ROLLBACK_COMPLETE",
                "case {label:?}: a refused create must leave the stack rolled back, got \
                 {stack_status:?} with reason {:?}",
                opt_element(&described, "StackStatusReason")
            );
            let (status, answer) = emulator.codepipeline(
                region,
                "GetPipeline",
                serde_json::json!({ "name": "cfn-codepipeline-refused" }),
            );
            assert_eq!(
                status, 400,
                "case {label:?}: the refused pipeline must not exist, got {status} with {answer}"
            );
            assert!(
                answer.to_string().contains(PIPELINE_NOT_FOUND),
                "case {label:?}: the refusal must be {PIPELINE_NOT_FOUND}, got {answer}"
            );
            let (status, _, body) = emulator.s3(
                region,
                "HEAD",
                "/cfn-codepipeline-refused-artifacts",
                b"",
            );
            assert_eq!(
                status,
                404,
                "case {label:?}: the rollback must have released the bucket the create made, \
                 got {status} with body {:?}",
                String::from_utf8_lossy(&body)
            );

            // Deleting a completed stack releases what it created in the owning services:
            // the pipeline is gone from CodePipeline and the bucket from Amazon S3. Driven
            // over the row whose bucket declares no versioning, and the bucket is EMPTIED
            // first: Amazon S3 refuses to delete a bucket that still holds objects
            // (`BucketNotEmpty`), so a stack whose bucket is not empty cannot complete its
            // deletion on AWS either — emptying it is this row's prerequisite, not part of
            // the rule under test.
            let (case, pipeline, bucket) = deployed
                .iter()
                .find(|(case, _, _)| !case.versioned)
                .expect("one row's bucket declares no versioning");
            let label = "deleting a completed stack";
            let region = case.region;
            let (status, _, listing) =
                emulator.s3(region, "GET", &format!("/{bucket}?list-type=2"), b"");
            let listing = String::from_utf8_lossy(&listing).into_owned();
            assert_eq!(
                status, 200,
                "case {label:?}: ListObjectsV2 must answer 200, got {status} with body {listing}"
            );
            for key in elements(&listing, "Key") {
                let (status, _, body) =
                    emulator.s3(region, "DELETE", &format!("/{bucket}/{key}"), b"");
                assert!(
                    (200..300).contains(&status),
                    "case {label:?}: emptying the bucket must delete {key:?}, got {status} \
                     with body {:?}",
                    String::from_utf8_lossy(&body)
                );
            }
            let (status, body) =
                emulator.cloudformation(region, "DeleteStack", &[("StackName", case.stack)]);
            assert_eq!(
                status, 200,
                "case {label:?}: DeleteStack must succeed, got {status} with body {body}"
            );
            std::thread::sleep(PAST_STACK_SPAN);
            let (status, answer) = emulator.codepipeline(
                region,
                "GetPipeline",
                serde_json::json!({ "name": pipeline }),
            );
            assert_eq!(
                status, 400,
                "case {label:?}: the deleted stack's pipeline must be gone, got {status} with \
                 {answer}"
            );
            assert!(
                answer.to_string().contains(PIPELINE_NOT_FOUND),
                "case {label:?}: the refusal must be {PIPELINE_NOT_FOUND}, got {answer}"
            );
            let (status, _, body) =
                emulator.s3(region, "PUT", &format!("/{bucket}/after-delete.zip"), b"after");
            let body = String::from_utf8_lossy(&body).into_owned();
            assert_eq!(
                status, 404,
                "case {label:?}: the deleted stack's bucket must be gone, got {status} with \
                 body {body}"
            );
            assert!(
                body.contains(NO_SUCH_BUCKET),
                "case {label:?}: the refusal must be {NO_SUCH_BUCKET}, got {body}"
            );
        })
        .expect("the suite's thread starts")
        .join()
        .expect("the suite's thread runs to completion");
}
