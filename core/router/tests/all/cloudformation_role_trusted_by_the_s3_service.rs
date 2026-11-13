// SPDX-License-Identifier: CC0-1.0

//! A stack whose IAM role is trusted by the S3 service principal, deployed with the function
//! and the buckets that pipeline uses.
//!
//! RULE: a deployment reaches a TERMINAL stack status and the resources it declares are the
//! records their owning services hold — a role whose trust policy names several service
//! principals is created with that document, the function declared with it is created, and
//! the pipeline it wires up really runs; and a declaration an owning service REFUSES fails
//! that resource and rolls the stack back, which is terminal too. No template leaves a
//! deployment pending: a client polling the stack is answered either way.
//!
//! "The trust policy that is associated with this role" (`Role$AssumeRolePolicyDocument`) is
//! a resource-based policy whose `Principal.Service` element is one name or a LIST of them,
//! so one role can be trusted by two services at once; and Lambda refuses a function whose
//! execution role its own principal cannot assume, which is a refusal of that one resource.
//!
//! The rows differ along what the rule generalises over: which service principals the role's
//! trust policy names (the function's service beside the S3 service, and the S3 service
//! alone), which decides whether the function's own resource is created or refused — and so
//! whether the stack converges or rolls back. Either way the status the stack settles in is
//! terminal, which is what a client's poll loop needs.
//!
//! Five crates have to agree for that to hold (CloudFormation deploys the estate, IAM holds
//! the role and its trust document, Lambda creates and runs the function, STS mints the
//! session its own calls are signed as, and S3 evaluates the notification and serves the
//! handler's own calls), so the suite lives here.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{elements, form_encode, opt_element};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// How often to look while waiting for work the clock owes — a stack settling, an
/// asynchronous invocation being delivered — and how many looks to take. The spans are the
/// owning services' own, so nothing here sleeps for a fixed span expecting work to be done,
/// and the deadline is generous so a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 1_800;

/// How long one invocation is given, as the function's own `Timeout`: well past what a run
/// needs on any host.
const FUNCTION_TIMEOUT: u64 = 120;

/// The service principal a Lambda execution role has to be trusted by.
const LAMBDA_PRINCIPAL: &str = "lambda.amazonaws.com";

/// The service principal the row under test adds, and which the bucket's notification is
/// granted the invoke under.
const S3_PRINCIPAL: &str = "s3.amazonaws.com";

/// The suffix the handler writes its marker under, in the OTHER bucket, so a marker never
/// re-triggers the configuration that produced it.
const MARKER_SUFFIX: &str = ".indexed";

/// The handler the stack deploys: for each record it is handed it reads the object the event
/// named through the AWS SDK the runtime supplies and writes a marker object derived from it
/// into the stack's second bucket.
const PIPELINE_HANDLER: &str = r#"
import os

import boto3


def handler(event, context):
    s3 = boto3.client("s3")
    marked = []
    for record in event["Records"]:
        bucket = record["s3"]["bucket"]["name"]
        key = record["s3"]["object"]["key"]
        body = s3.get_object(Bucket=bucket, Key=key)["Body"].read()
        marker = key + ".indexed"
        s3.put_object(Bucket=os.environ["MARKER_BUCKET"], Key=marker, Body=body)
        marked.append(marker)
    return {"marked": marked}
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own calls are dispatched
/// back into this same router, so the bucket it writes to is the bucket this suite reads.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One awsQuery call of `service`, answered with the HTTP status and the XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
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
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One request against S3's REST surface, path-style, as its status, its headers and its
/// body.
async fn s3_call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &str,
) -> (u16, http::HeaderMap, String) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let (parts, answered) = response.into_parts();
    (
        parts.status.as_u16(),
        parts.headers,
        String::from_utf8_lossy(&answered).into_owned(),
    )
}

/// One request against S3's REST surface, path-style.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> (u16, String) {
    let (status, _, answered) = s3_call(router, method, path, body).await;
    (status, answered)
}

/// Whether a stack status is one a deployment STOPS in, so polling it again tells a client
/// nothing new.
fn is_terminal(status: &str) -> bool {
    status.ends_with("_COMPLETE") || status.ends_with("_FAILED")
}

/// Poll `DescribeStacks` until the stack settles in a terminal status, and answer that status
/// with the body it was last described with.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let mut described = String::new();
    for _ in 0..LOOKS {
        let (status, body) = query(
            router,
            "cloudformation",
            "2010-05-15",
            "DescribeStacks",
            &[("StackName", stack)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeStacks({stack:?}) must be answered, got {status} with {body}"
        );
        described = body;
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if is_terminal(&reported) {
            return (reported, described);
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!(
        "case {case:?}: the deployment must settle in a terminal status rather than leaving a \
         client polling, last described {described}"
    );
}

/// The trust policy one row's role carries: `principals` as its `Principal.Service` element,
/// written the way a client writes one — a single name as a string, several as a list.
fn trust_policy(principals: &[&str]) -> Value {
    let service = if principals.len() == 1 {
        Value::String(principals[0].to_string())
    } else {
        Value::Array(
            principals
                .iter()
                .map(|name| Value::String((*name).to_string()))
                .collect(),
        )
    };
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": service},
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The template one row deploys: the role with its own trust document, the function that runs
/// as it, the grant the bucket's notification needs, the landing bucket whose configuration
/// names the function, and the bucket the handler writes its markers into.
fn template(row: &Row) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "PipelineRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": trust_policy(row.principals),
                    "Policies": [{
                        "PolicyName": "read-the-object-and-write-the-marker",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": ["s3:GetObject", "s3:PutObject", "logs:*"],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "MarkerBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {"BucketName": row.marker_bucket},
            },
            "Pipeline": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": row.function,
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["PipelineRole", "Arn"]},
                    "Timeout": FUNCTION_TIMEOUT,
                    "Environment": {"Variables": {"MARKER_BUCKET": row.marker_bucket}},
                    "Code": {"ZipFile": PIPELINE_HANDLER},
                },
            },
            "InvokePermission": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "FunctionName": {"Fn::GetAtt": ["Pipeline", "Arn"]},
                    "Action": "lambda:InvokeFunction",
                    "Principal": S3_PRINCIPAL,
                    "SourceArn": format!("arn:aws:s3:::{}", row.landing_bucket),
                },
            },
            "LandingBucket": {
                "Type": "AWS::S3::Bucket",
                "DependsOn": "InvokePermission",
                "Properties": {
                    "BucketName": row.landing_bucket,
                    "NotificationConfiguration": {
                        "LambdaConfigurations": [{
                            "Event": "s3:ObjectCreated:*",
                            "Function": {"Fn::GetAtt": ["Pipeline", "Arn"]},
                        }],
                    },
                },
            },
        },
    })
    .to_string()
}

/// One row: the service principals the role's trust policy names, and the terminal status the
/// deployment must settle in.
struct Row {
    case: &'static str,
    principals: &'static [&'static str],
    stack: &'static str,
    function: &'static str,
    landing_bucket: &'static str,
    marker_bucket: &'static str,
    /// The terminal `StackStatus` the deployment must reach.
    settles_in: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a role the function's service and the S3 service are both trusted by",
        principals: &[LAMBDA_PRINCIPAL, S3_PRINCIPAL],
        stack: "xsvc-trusts-both",
        function: "xsvc-trusts-both-pipeline",
        landing_bucket: "xsvc-trusts-both-landing",
        marker_bucket: "xsvc-trusts-both-markers",
        settles_in: "CREATE_COMPLETE",
    },
    Row {
        case: "a role only the S3 service is trusted by, named as a function's execution role",
        principals: &[S3_PRINCIPAL],
        stack: "xsvc-trusts-s3-only",
        function: "xsvc-trusts-s3-only-pipeline",
        landing_bucket: "xsvc-trusts-s3-only-landing",
        marker_bucket: "xsvc-trusts-s3-only-markers",
        settles_in: "ROLLBACK_COMPLETE",
    },
];

/// The trust document IAM holds for `role`, as `GetRole` reports it — URL-encoded, the form
/// IAM reports a trust policy in.
async fn reported_trust_document(router: &MultiServiceRouter, role: &str) -> String {
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "GetRole",
        &[("RoleName", role)],
    )
    .await;
    assert_eq!(
        status, 200,
        "GetRole({role:?}) must be answered, got {status} with {body}"
    );
    let document = opt_element(&body, "AssumeRolePolicyDocument").unwrap_or_default();
    ::smithy_server_runtime::percent_decode(&document).to_string()
}

/// The physical id of the role the row's stack declared, which is the role name IAM holds it
/// under.
async fn role_name(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStackResources",
        &[("StackName", stack), ("LogicalResourceId", "PipelineRole")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeStackResources must be answered, got {status} with {body}"
    );
    opt_element(&body, "PhysicalResourceId").unwrap_or_else(|| {
        panic!("case {case:?}: the stack must report the id IAM assigned the role, got {body}")
    })
}

/// Poll `HeadObject` for `key` until the object is there, and answer whether it arrived.
async fn awaited_object(router: &MultiServiceRouter, bucket: &str, key: &str) -> bool {
    for _ in 0..LOOKS {
        let (status, _) = s3(router, "HEAD", &format!("/{bucket}/{key}"), "").await;
        if status == 200 {
            return true;
        }
        tokio::time::sleep(LOOK).await;
    }
    false
}

/// Every row: the deployment settles in a terminal status, the role IAM holds carries the
/// trust document the template declared, and the pipeline a converged stack deployed runs.
#[tokio::test]
async fn a_role_trusted_by_the_s3_service_deploys_and_the_stack_settles() {
    let router = build_router();

    for row in ROWS {
        let case = row.case;
        let (status, body) = query(
            &router,
            "cloudformation",
            "2010-05-15",
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &template(row)),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack must be accepted, got {status} with {body}"
        );

        let (settled, described) = settled_stack(&router, case, row.stack).await;
        let (_, events) = query(
            &router,
            "cloudformation",
            "2010-05-15",
            "DescribeStackEvents",
            &[("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            settled,
            row.settles_in,
            "case {case:?}: expected the deployment to settle in {}, got {settled} with \
             {described} and the reasons {:?}",
            row.settles_in,
            elements(&events, "ResourceStatusReason")
        );
        if settled != "CREATE_COMPLETE" {
            // A rolled-back stack keeps none of its resources, and the reason the stack
            // reports is the owning service's own refusal of the declaration.
            let reasons = elements(&events, "ResourceStatusReason");
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason.contains("cannot be assumed")),
                "case {case:?}: expected the rollback to carry the refusal of the execution \
                 role, got {reasons:?}"
            );
            continue;
        }

        // The role IAM holds carries the document the template declared, with both service
        // principals in it: a trust policy's `Principal.Service` is one name or a list.
        let role = role_name(&router, case, row.stack).await;
        let document = reported_trust_document(&router, &role).await;
        for principal in row.principals {
            assert!(
                document.contains(principal),
                "case {case:?}: expected the trust document IAM holds to name {principal:?}, \
                 got {document}"
            );
        }

        // And the pipeline the stack deployed runs: an object written into the landing bucket
        // is handed to the function, whose own SDK call writes the marker.
        let key = "items/widget-1.json";
        let (status, answered) = s3(
            &router,
            "PUT",
            &format!("/{}/{key}", row.landing_bucket),
            r#"{"id":"widget-1"}"#,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutObject must succeed, got {status} with {answered}"
        );
        let marker = format!("{key}{MARKER_SUFFIX}");
        assert!(
            awaited_object(&router, row.marker_bucket, &marker).await,
            "case {case:?}: expected the handler's marker {marker:?} in {:?}",
            row.marker_bucket
        );
    }
}

/// One row of the replication table: which service principal the role the configuration names
/// is trusted by, and whether Amazon S3 can therefore make the copy.
struct ReplicationRow {
    case: &'static str,
    /// The service principals the replication role's trust policy names.
    principals: &'static [&'static str],
    stack: &'static str,
    source_bucket: &'static str,
    destination_bucket: &'static str,
    /// Whether the version written into the source must be copied to the destination.
    replicated: bool,
}

/// The table: a replication role S3 is trusted by, and one it is not — the two sides of
/// "Amazon S3 assumes [the role] when replicating objects".
const REPLICATION_ROWS: &[ReplicationRow] = &[
    ReplicationRow {
        case: "a replication role the S3 service is trusted by",
        principals: &[S3_PRINCIPAL],
        stack: "xsvc-replicates",
        source_bucket: "xsvc-replicates-source",
        destination_bucket: "xsvc-replicates-destination",
        replicated: true,
    },
    ReplicationRow {
        case: "a replication role only another service is trusted by",
        principals: &[LAMBDA_PRINCIPAL],
        stack: "xsvc-replicates-untrusted",
        source_bucket: "xsvc-replicates-untrusted-source",
        destination_bucket: "xsvc-replicates-untrusted-destination",
        replicated: false,
    },
];

/// The prefix the declared rule narrows to, so the configuration under test covers one part
/// of the bucket and not the whole of it.
const REPLICATED_PREFIX: &str = "replicated/";

/// The rule id the declaration carries, which a read of the configuration must report back.
const RULE_ID: &str = "every-replicated-object";

/// The template one replication row deploys: the role the configuration names, the versioned
/// destination bucket, and the versioned source bucket whose `ReplicationConfiguration`
/// names both.
fn replication_template(row: &ReplicationRow) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ReplicationRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": trust_policy(row.principals),
                    "Policies": [{
                        "PolicyName": "replicate-the-objects",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "s3:GetReplicationConfiguration",
                                    "s3:ListBucket",
                                    "s3:GetObjectVersionForReplication",
                                    "s3:ReplicateObject",
                                    "s3:ReplicateDelete",
                                ],
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "DestinationBucket": {
                "Type": "AWS::S3::Bucket",
                "Properties": {
                    "BucketName": row.destination_bucket,
                    "VersioningConfiguration": {"Status": "Enabled"},
                },
            },
            "SourceBucket": {
                "Type": "AWS::S3::Bucket",
                "DependsOn": ["DestinationBucket", "ReplicationRole"],
                "Properties": {
                    "BucketName": row.source_bucket,
                    "VersioningConfiguration": {"Status": "Enabled"},
                    "ReplicationConfiguration": {
                        "Role": {"Fn::GetAtt": ["ReplicationRole", "Arn"]},
                        "Rules": [{
                            "Id": RULE_ID,
                            "Status": "Enabled",
                            "Priority": 1,
                            "DeleteMarkerReplication": {"Status": "Disabled"},
                            "Filter": {"Prefix": REPLICATED_PREFIX},
                            "Destination": {
                                "Bucket": format!("arn:aws:s3:::{}", row.destination_bucket),
                                "StorageClass": "STANDARD",
                            },
                        }],
                    },
                },
            },
        },
    })
    .to_string()
}

/// The replication status Amazon S3 reports for the version at `key` of `bucket`, polled
/// until it is TERMINAL — a write's replication is asynchronous, so `PENDING` is the answer
/// the clock still owes an outcome for.
async fn settled_replication_status(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
) -> String {
    let mut reported = String::new();
    for _ in 0..LOOKS {
        let (_, headers, _) = s3_call(router, "HEAD", &format!("/{bucket}/{key}"), "").await;
        reported = headers
            .get("x-amz-replication-status")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        if reported != "PENDING" {
            return reported;
        }
        tokio::time::sleep(LOOK).await;
    }
    reported
}

/// Every row: the configuration the template declared is the one Amazon S3 holds for the
/// bucket, and the copy is made exactly when S3 is trusted by the role it names.
#[tokio::test]
async fn a_declared_replication_configuration_replicates_under_the_role_it_names() {
    let router = build_router();

    for row in REPLICATION_ROWS {
        let case = row.case;
        let (status, body) = query(
            &router,
            "cloudformation",
            "2010-05-15",
            "CreateStack",
            &[
                ("StackName", row.stack),
                ("TemplateBody", &replication_template(row)),
                ("Capabilities.member.1", "CAPABILITY_IAM"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateStack must be accepted, got {status} with {body}"
        );
        let (settled, described) = settled_stack(&router, case, row.stack).await;
        let (_, events) = query(
            &router,
            "cloudformation",
            "2010-05-15",
            "DescribeStackEvents",
            &[("StackName", row.stack)],
        )
        .await;
        assert_eq!(
            settled, "CREATE_COMPLETE",
            "case {case:?}: the deployment must converge, got {settled} with {described} and \
             the reasons {:?}",
            elements(&events, "ResourceStatusReason")
        );

        // The configuration the bucket HOLDS is the one the template declared: the property
        // reached `PutBucketReplication`, which is the only call that installs one.
        let (status, reported) = s3(
            &router,
            "GET",
            &format!("/{}?replication", row.source_bucket),
            "",
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetBucketReplication must report the declared configuration, got \
             {status} with {reported}"
        );
        for expected in [
            RULE_ID,
            "Enabled",
            REPLICATED_PREFIX,
            &format!("arn:aws:s3:::{}", row.destination_bucket),
        ] {
            assert!(
                reported.contains(expected),
                "case {case:?}: expected the reported configuration to carry {expected:?}, got \
                 {reported}"
            );
        }

        // And the write is copied exactly when Amazon S3 can assume the role the
        // configuration names.
        let key = format!("{REPLICATED_PREFIX}hello.txt");
        let document = "the payload the rule replicates";
        let (status, answered) = s3(
            &router,
            "PUT",
            &format!("/{}/{key}", row.source_bucket),
            document,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutObject into the source must succeed, got {status} with {answered}"
        );
        let settled = settled_replication_status(&router, row.source_bucket, &key).await;
        assert_eq!(
            settled,
            if row.replicated { "COMPLETED" } else { "FAILED" },
            "case {case:?}: expected the source version's replication to settle, got \
             {settled:?}"
        );
        let (status, replica) = s3(
            &router,
            "GET",
            &format!("/{}/{key}", row.destination_bucket),
            "",
        )
        .await;
        if row.replicated {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the replica at {key:?} in the destination, got \
                 {status} with {replica}"
            );
            assert_eq!(
                replica, document,
                "case {case:?}: expected the replica to carry the source's own bytes, got \
                 {replica:?}"
            );
        } else {
            assert_ne!(
                status, 200,
                "case {case:?}: a role S3 is not trusted by replicates nothing, got {status} \
                 with {replica}"
            );
        }
    }
}
