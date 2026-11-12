// SPDX-License-Identifier: CC0-1.0

//! A Lambda handler that INVALIDATES the CloudFront distribution in front of the origin it
//! just wrote, end to end over the assembled router.
//!
//! RULE: the AWS SDK a managed runtime supplies reaches CloudFront as it reaches every other
//! service, so a handler's `create_invalidation` call is MADE against the distribution it
//! names — under the function's own execution role — and the invalidation it submits is
//! CloudFront's own state from that moment on: the service reports it through
//! `GetInvalidation` and lists it through `ListInvalidations` with the `Paths` the batch
//! carried and a `Status` that is not yet `Completed`, the operation being asynchronous.
//!
//! Only a cross-service suite can pin this: the invalidation is CloudFront's record, the
//! handler and the SDK seam it calls through are Lambda's, and the origin behind the
//! distribution is Amazon S3's. The Lambda crate's own suites pin what a handler computes,
//! and the CloudFront crate's pin what an invalidation does to the edge; neither can state
//! that a RUNNING handler can submit one at all.
//!
//! The rows differ along what the rule generalises over: the shape of the batch the handler
//! submits (one exact path, several paths, a wildcard over a prefix), and which of the two
//! reads the handler itself performs after submitting it (none, `GetInvalidation`,
//! `ListInvalidations`) — every row then being read back through CloudFront's own API, which
//! is where the invalidation has to exist.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::query_wire::{form_encode, outputs_of};

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// The account every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// CloudFront's control-plane host, and the API version every route is under.
const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
const API_VERSION: &str = "2020-05-31";

/// The bucket the distribution's origin is, and the object behind the paths invalidated.
const BUCKET: &str = "edge-invalidation-origin";
const KEY: &str = "assets/app.js";

/// The `StackStatus` the setup waits for.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long work the clock owes is waited for, and how often it is looked at. Generous: the
/// deadline is not what any row measures.
const WAIT: Duration = Duration::from_secs(30);
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The role the function runs as, and the function's own name.
const ROLE: &str = "edge-invalidation-role";
const FUNCTION: &str = "edge-invalidator";

/// The handler the stack deploys: it submits the batch the event names through the AWS SDK
/// the runtime supplies, and then performs whichever read the event asked for — so one
/// invocation reports both what CloudFront answered the write and what it answered the read.
const HANDLER: &str = r#"
import boto3


def lambda_handler(event, context):
    cloudfront = boto3.client("cloudfront")
    submitted = cloudfront.create_invalidation(
        DistributionId=event["distribution"],
        InvalidationBatch={
            "Paths": {"Items": event["paths"]},
            "CallerReference": event["reference"],
        },
    )
    invalidation = submitted["Invalidation"]
    answered = {
        "id": invalidation["Id"],
        "status": invalidation["Status"],
        "paths": invalidation["InvalidationBatch"]["Paths"]["Items"],
        "location": submitted.get("Location", ""),
    }
    if event["read"] == "get":
        read = cloudfront.get_invalidation(
            DistributionId=event["distribution"], Id=invalidation["Id"]
        )
        answered["read_status"] = read["Invalidation"]["Status"]
        answered["read_paths"] = read["Invalidation"]["InvalidationBatch"]["Paths"]["Items"]
    if event["read"] == "list":
        listed = cloudfront.list_invalidations(DistributionId=event["distribution"])
        answered["listed"] = [
            entry["Id"] for entry in listed["InvalidationList"].get("Items", [])
        ]
    return answered
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own SDK call is dispatched
/// back into this same router, so the distribution it invalidates is the one this suite
/// created.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = aws_emulator_router::build_router(engine);
    // The suite exercises the services, not SigV4: each setup call carries a credential
    // scope so the router can name the target service, without a computed signature.
    router.set_request_authentication(false);
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature itself is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One response: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The text of the first `<name>…</name>` element of the body, or the empty string.
    fn element(&self, name: &str) -> String {
        crate::query_wire::opt_element(&self.body, name).unwrap_or_default()
    }
}

/// One request against a service's own host, with `signing_name` in the credential scope.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    method: &str,
    path: &str,
    content_type: &str,
    body: &str,
) -> Answer {
    let request = ::http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name))
        .header("content-type", content_type)
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
}

/// One Amazon S3 REST call, path-style as the emulator serves it.
async fn s3(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    call(
        router,
        "s3",
        &format!("s3.{REGION}.amazonaws.com"),
        method,
        path,
        "application/xml",
        body,
    )
    .await
}

/// One CloudFront control-plane call.
async fn cloudfront(router: &MultiServiceRouter, method: &str, path: &str, body: &str) -> Answer {
    call(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        method,
        path,
        "application/xml",
        body,
    )
    .await
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
    let host = format!("{service}.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(service))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).into_owned(),
    )
}

/// One Lambda `restJson1` call, answered with the status and the body as JSON.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = match &body {
        Some(body) => Bytes::from(body.to_string()),
        None => Bytes::new(),
    };
    let host = format!("lambda.{REGION}.amazonaws.com");
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", host)
        .header("content-type", "application/json")
        .header("authorization", authorization("lambda"))
        .body(payload)
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The bucket the distribution serves, holding the object the paths name.
async fn create_origin(router: &MultiServiceRouter) {
    let created = s3(router, "PUT", &format!("/{BUCKET}"), "").await;
    assert_eq!(
        created.status, 200,
        "setup: CreateBucket must succeed, got {} with {}",
        created.status, created.body
    );
    let written = s3(router, "PUT", &format!("/{BUCKET}/{KEY}"), "the asset").await;
    assert_eq!(
        written.status, 200,
        "setup: PutObject must succeed, got {} with {}",
        written.status, written.body
    );
}

/// `cloudfront:CreateDistribution` over that bucket, answering the distribution's id.
async fn create_distribution(router: &MultiServiceRouter, caller_reference: &str) -> String {
    let body = format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>origin-bucket</Id>\
         <DomainName>{BUCKET}.s3.{REGION}.amazonaws.com</DomainName>\
         <S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>origin-bucket</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <ForwardedValues><QueryString>false</QueryString>\
         <Cookies><Forward>none</Forward></Cookies></ForwardedValues>\
         <MinTTL>0</MinTTL>\
         </DefaultCacheBehavior>\
         <Comment>the distribution a handler invalidates</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    );
    let created = cloudfront(router, "POST", &format!("/{API_VERSION}/distribution"), &body).await;
    assert_eq!(
        created.status, 201,
        "setup: CreateDistribution must succeed, got {} with {}",
        created.status, created.body
    );
    let id = created.element("Id");
    assert!(
        !id.is_empty(),
        "setup: the created distribution reports no Id: {}",
        created.body
    );
    id
}

/// The stack that declares the execution role and the function: the role trusts Lambda and
/// is granted the invalidation action on the distribution, which is what the handler's call
/// is judged against.
fn handler_stack(distribution: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "InvalidatorRole": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": ROLE,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "invalidate-the-distribution",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": [
                                    "cloudfront:CreateInvalidation",
                                    "cloudfront:GetInvalidation",
                                    "cloudfront:ListInvalidations",
                                ],
                                "Resource": format!(
                                    "arn:aws:cloudfront::{ACCOUNT}:distribution/{distribution}"
                                ),
                            }],
                        },
                    }],
                },
            },
            "Invalidator": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": {"Fn::GetAtt": ["InvalidatorRole", "Arn"]},
                    "Timeout": 30,
                    "Code": {"ZipFile": HANDLER},
                },
            },
        },
        "Outputs": {
            "FunctionName": {"Value": {"Ref": "Invalidator"}},
        },
    })
    .to_string()
}

/// What `DescribeStacks` reports for `stack`: its status and the whole body.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> (String, String) {
    let (_status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "DescribeStacks",
        &[("StackName", stack)],
    )
    .await;
    (crate::query_wire::element(&body, "StackStatus"), body)
}

/// Create the stack, wait for `CREATE_COMPLETE` and answer its outputs.
async fn deployed(
    router: &MultiServiceRouter,
    stack: &str,
    template: &str,
) -> BTreeMap<String, String> {
    let (status, body) = query(
        router,
        "cloudformation",
        "2010-05-15",
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut reported = stack_status(router, stack).await;
    for _ in 0..looks {
        if reported.0 == CREATE_COMPLETE {
            break;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
        reported = stack_status(router, stack).await;
    }
    assert_eq!(
        reported.0, CREATE_COMPLETE,
        "setup: the stack must reach {CREATE_COMPLETE}, got {:?} with {}",
        reported.0, reported.1
    );
    outputs_of(&reported.1)
}

/// Wait until Lambda reports the function `Active` — the state an invocation needs. The span
/// a created function spends coming up is that service's own, so nothing here sleeps a fixed
/// span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    let looks = WAIT.as_millis() / LOOK_INTERVAL.as_millis();
    let mut state = String::new();
    for _ in 0..looks {
        let (_status, body) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        state = body["State"].as_str().unwrap_or_default().to_string();
        if state == "Active" {
            return;
        }
        ::tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("setup: the function {name:?} must reach Active, last reported {state:?}");
}

/// One row: the batch the handler submits and the read it performs itself.
struct Row {
    case: &'static str,
    /// The paths the batch carries, which are what the invalidation has to report back.
    paths: &'static [&'static str],
    /// `""`, `"get"` or `"list"`: which read the handler makes after submitting.
    read: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "one exact object path, with no read of its own",
        paths: &["/assets/app.js"],
        read: "",
    },
    Row {
        case: "several paths, read back by the handler itself",
        paths: &["/assets/app.js", "/index.html"],
        read: "get",
    },
    Row {
        case: "a wildcard over the object's prefix, listed by the handler itself",
        paths: &["/assets/*"],
        read: "list",
    },
];

#[tokio::test]
async fn a_handler_submits_an_invalidation_cloudfront_then_reports() {
    let router = build_router();
    create_origin(&router).await;
    let distribution = create_distribution(&router, "edge-invalidation-reference").await;
    let outputs = deployed(
        &router,
        "xsvc-edge-invalidation-stack",
        &handler_stack(&distribution),
    )
    .await;
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| FUNCTION.to_string());
    wait_until_active(&router, &function).await;

    let mut submitted: Vec<String> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let reference = format!("handler-batch-{index}");
        let (status, answered) = lambda(
            &router,
            "POST",
            &format!("/2015-03-31/functions/{function}/invocations"),
            Some(json!({
                "distribution": distribution,
                "paths": row.paths,
                "reference": reference,
                "read": row.read,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the invocation must be served, got {status} with {answered}"
        );
        assert!(
            answered.get("errorMessage").is_none() && answered.get("errorType").is_none(),
            "case {case:?}: the handler must not fault, got {answered}"
        );
        let id = answered["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty(),
            "case {case:?}: the submitted invalidation must report an Id, got {answered}"
        );
        // The handler read the batch back out of CloudFront's own answer, so the paths it
        // reports are the ones the service holds for it.
        let reported: Vec<String> = answered["paths"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|path| path.as_str().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported,
            row.paths
                .iter()
                .map(|path| (*path).to_string())
                .collect::<Vec<String>>(),
            "case {case:?}: expected the batch's own paths, got {answered}"
        );
        // An invalidation is ASYNCHRONOUS, so the create cannot report it finished.
        assert_ne!(
            answered["status"].as_str().unwrap_or_default(),
            "Completed",
            "case {case:?}: an invalidation is asynchronous, so the submit cannot report it \
             finished: {answered}"
        );
        match row.read {
            "get" => assert_eq!(
                answered["read_paths"], answered["paths"],
                "case {case:?}: the handler's own GetInvalidation must report the same paths, \
                 got {answered}"
            ),
            "list" => assert!(
                answered["listed"]
                    .as_array()
                    .is_some_and(|listed| listed.iter().any(|entry| entry == &json!(id))),
                "case {case:?}: the handler's own ListInvalidations must carry the \
                 invalidation it just submitted, got {answered}"
            ),
            _ => assert!(
                answered.get("read_status").is_none() && answered.get("listed").is_none(),
                "case {case:?}: the handler performed no read of its own, got {answered}"
            ),
        }

        // BACKED: the invalidation is CloudFront's own state, read through CloudFront's own
        // API rather than out of what the handler said.
        let read = cloudfront(
            &router,
            "GET",
            &format!("/{API_VERSION}/distribution/{distribution}/invalidation/{id}"),
            "",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetInvalidation({id}) must answer, got {} with {}",
            read.status, read.body
        );
        for path in row.paths {
            assert!(
                read.body.contains(&format!("<Path>{path}</Path>")),
                "case {case:?}: CloudFront must hold the path {path:?} for this \
                 invalidation, got {}",
                read.body
            );
        }
        submitted.push(id);
    }

    // Every invalidation the handler submitted is listed on the distribution, which is where
    // a client that waits for one finds it.
    let listed = cloudfront(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution/{distribution}/invalidation"),
        "",
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListInvalidations must answer, got {} with {}",
        listed.status, listed.body
    );
    for id in &submitted {
        assert!(
            listed.body.contains(id),
            "the distribution must list the invalidation {id}, got {}",
            listed.body
        );
    }
}
