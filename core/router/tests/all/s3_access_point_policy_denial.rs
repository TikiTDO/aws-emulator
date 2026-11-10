// SPDX-License-Identifier: CC0-1.0

//! An ACCESS POINT POLICY that denies a role, enforced on the data path a Lambda function's
//! own calls take — across the three services that have to agree about it.
//!
//! The rule under test: **an S3 access point is a named data path onto one bucket, with a
//! policy of its own that governs the requests made THROUGH the access point and no others.
//! A request addressed to the access point reaches the objects of the bucket it fronts and is
//! authorized against that policy as well as the caller's identity-based documents and the
//! bucket's own, over the resource the access point names
//! (`<access-point-arn>/object/<key>`) — so an explicit `Deny` there refuses the read through
//! the access point with HTTP 403 `AccessDenied`, while the same key read by BUCKET NAME,
//! which that document does not govern, is served. Removing the document with
//! `DeleteAccessPointPolicy` lets the same caller read through the access point too.**
//!
//! "Access points are named network endpoints that are attached to buckets that you can use
//! to perform S3 object operations", and "access point policies … are evaluated in addition
//! to the bucket policy" (Amazon S3 User Guide, *Managing data access with Amazon S3 access
//! points*); "an explicit deny in any policy overrides any allows" (IAM User Guide, *Policy
//! evaluation logic*).
//!
//! Four crates have to agree for that to hold, so the suite lives here: S3 Control creates
//! the access point and holds its policy, S3 resolves the address and takes the decision, IAM
//! answers for the execution role the function's own SDK call is signed as, and Lambda runs
//! the handler that makes both calls. The S3 crate's own `access_point_data_plane` suite pins
//! the decision over the address forms and the policy shapes; what only this suite can reach
//! is the whole chain — the access point created through its own API, the policy put through
//! its own API, and both reads made by a RUNNING function as its execution role's session,
//! which is the identity an `aws:PrincipalArn` condition selects.
//!
//! The two arms are asserted in ONE invocation payload, which is what tells a denied access
//! point path from a data plane that serves nothing at all.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::stack_status;
use crate::query_wire::{form_encode, outputs_of};

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The account id this emulator serves, which the S3 Control calls name in
/// `x-amz-account-id` and the access point's ARN carries.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The XML namespace S3 Control's model declares.
const S3CONTROL_XMLNS: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// The `StackStatus` the handler stack must reach.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// How long this suite waits for work the clock owes — the stack's resources coming up and
/// the function reaching `Active` — and how often it looks while waiting.
const WAIT: Duration = Duration::from_secs(60);
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// The bucket the access point fronts, the key both arms read, and the bytes it holds.
const BUCKET: &str = "xsvc-access-point-denied";
const KEY: &str = "data/report.txt";
const BODY: &str = "the object behind the access point";

/// The access point, the role the function runs as and the function itself, each named here
/// because the refusal's own sentence names the role and the function: the identity a
/// function's SDK call is signed as is the session
/// `arn:aws:sts::<account>:assumed-role/<role>/<function>`.
const ACCESS_POINT: &str = "xsvc-denied-ap";
const ROLE: &str = "xsvc-access-point-reader";
const FUNCTION: &str = "xsvc-access-point-probe";

/// The code and status S3 refuses an unauthorized read with (Amazon S3 "Error responses").
const ACCESS_DENIED: &str = "AccessDenied";

/// The error code `GetAccessPointPolicy` answers for an access point carrying no policy.
const NO_SUCH_ACCESS_POINT_POLICY: &str = "NoSuchAccessPointPolicy";

/// The two labels the handler reads the one key under: the access point's address, and the
/// bucket's own name.
const ACCESS_POINT_ARM: &str = "access_point";
const BUCKET_ARM: &str = "bucket";

/// The handler both arms run: for each `label -> address` pair the event carries it reads the
/// one key through that address with the AWS SDK the runtime supplies, and answers either the
/// bytes it read or the error code and sentence S3 refused it with. One invocation therefore
/// reports BOTH outcomes, which is what makes the contrast observable at all.
const PROBE_HANDLER: &str = r#"
import boto3
from botocore.exceptions import ClientError


def lambda_handler(event, context):
    s3 = boto3.client("s3")
    answered = {}
    for label, address in event["paths"].items():
        try:
            read = s3.get_object(Bucket=address, Key=event["key"])
            answered[label] = {"body": read["Body"].read().decode("utf-8")}
        except ClientError as refused:
            answered[label] = {
                "code": refused.response["Error"]["Code"],
                "message": refused.response["Error"]["Message"],
            }
    return answered
"#;

/// The emulator as it is ASSEMBLED, on one engine: the handler's own SDK call is dispatched
/// back into this same router, so the access point it reads through is the one this suite
/// created.
fn build_router() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
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

/// One response: the status and the body as text.
struct Answer {
    status: u16,
    body: String,
}

impl Answer {
    /// The text of the first `<name>` element of the body, or the empty string, with the XML
    /// escaping the serializer applies undone — a policy document's quotes travel as
    /// `&quot;` in an XML text node.
    fn element(&self, name: &str) -> String {
        crate::query_wire::opt_element(&self.body, name)
            .unwrap_or_default()
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&")
    }
}

/// One request against the `s3` signing name — which is what both S3 and S3 Control sign as;
/// the URI path is what tells them apart.
async fn s3_wire(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: &str,
) -> Answer {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("x-amz-account-id", ACCOUNT)
        .header("content-type", "application/xml")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: String::from_utf8_lossy(response.body()).into_owned(),
    }
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
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
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
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
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

/// `PUT /<bucket>` — S3's `CreateBucket`.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let answer = s3_wire(router, "PUT", &format!("/{bucket}"), "").await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>/<key>` — S3's `PutObject`.
async fn put_object(router: &MultiServiceRouter, bucket: &str, key: &str, body: &str) {
    let answer = s3_wire(router, "PUT", &format!("/{bucket}/{key}"), body).await;
    assert_eq!(
        answer.status, 200,
        "setup: PutObject({bucket:?}, {key:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /<bucket>?policy` — S3's `PutBucketPolicy`, carrying the statement that DELEGATES
/// access control on the bucket to the access points of its own account, which is the shape
/// AWS documents for a bucket fronted by access points ("you can delegate access control to
/// access points with the `s3:DataAccessPointAccount` condition key").
async fn delegate_to_access_points(router: &MultiServiceRouter, bucket: &str) {
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"AWS": "*"},
            "Action": "*",
            "Resource": [
                format!("arn:aws:s3:::{bucket}"),
                format!("arn:aws:s3:::{bucket}/*"),
            ],
            "Condition": {"StringEquals": {"s3:DataAccessPointAccount": ACCOUNT}},
        }],
    })
    .to_string();
    let answer = s3_wire(router, "PUT", &format!("/{bucket}?policy"), &document).await;
    assert_eq!(
        answer.status, 204,
        "setup: PutBucketPolicy({bucket:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `PUT /v20180820/accesspoint/<name>` — S3 Control's `CreateAccessPoint`, answering the ARN
/// it minted.
async fn create_access_point(router: &MultiServiceRouter, name: &str, bucket: &str) -> String {
    let answer = s3_wire(
        router,
        "PUT",
        &format!("/v20180820/accesspoint/{name}"),
        &format!(
            "<CreateAccessPointRequest xmlns=\"{S3CONTROL_XMLNS}\">\
             <Bucket>{bucket}</Bucket></CreateAccessPointRequest>"
        ),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup: CreateAccessPoint({name:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
    let arn = answer.element("AccessPointArn");
    assert!(
        arn.contains(":accesspoint/"),
        "setup: CreateAccessPoint must answer the access point's ARN, got {}",
        answer.body
    );
    arn
}

/// `PUT /v20180820/accesspoint/<name>/policy` — S3 Control's `PutAccessPointPolicy`.
async fn put_access_point_policy(router: &MultiServiceRouter, name: &str, document: &str) {
    let answer = s3_wire(
        router,
        "PUT",
        &format!("/v20180820/accesspoint/{name}/policy"),
        &format!(
            "<PutAccessPointPolicyRequest xmlns=\"{S3CONTROL_XMLNS}\">\
             <Policy>{document}</Policy></PutAccessPointPolicyRequest>"
        ),
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "PutAccessPointPolicy({name:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// `GET /v20180820/accesspoint/<name>/policy` — S3 Control's `GetAccessPointPolicy`.
async fn get_access_point_policy(router: &MultiServiceRouter, name: &str) -> Answer {
    s3_wire(
        router,
        "GET",
        &format!("/v20180820/accesspoint/{name}/policy"),
        "",
    )
    .await
}

/// `DELETE /v20180820/accesspoint/<name>/policy` — S3 Control's `DeleteAccessPointPolicy`.
async fn delete_access_point_policy(router: &MultiServiceRouter, name: &str) {
    let answer = s3_wire(
        router,
        "DELETE",
        &format!("/v20180820/accesspoint/{name}/policy"),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 204,
        "DeleteAccessPointPolicy({name:?}) must succeed, got {} with {}",
        answer.status, answer.body
    );
}

/// The stack that declares the execution role and the probe function: the role trusts Lambda
/// and is granted the read on BOTH arms — the bucket's objects and the access point's — so a
/// refusal below can only come from the access point's own policy.
fn handler_stack(access_point_arn: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "ProbeRole": {
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
                        "PolicyName": "read-both-arms",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": "s3:GetObject",
                                "Resource": [
                                    format!("arn:aws:s3:::{BUCKET}/*"),
                                    format!("{access_point_arn}/object/*"),
                                ],
                            }],
                        },
                    }],
                },
            },
            "Probe": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": FUNCTION,
                    "Runtime": "python3.12",
                    "Handler": "index.lambda_handler",
                    "Role": {"Fn::GetAtt": ["ProbeRole", "Arn"]},
                    "Timeout": 30,
                    "Code": {"ZipFile": PROBE_HANDLER},
                },
            },
        },
        "Outputs": {
            "FunctionName": {"Value": {"Ref": "Probe"}},
        },
    })
    .to_string()
}

/// Create the stack, wait for `CREATE_COMPLETE` and answer its outputs.
async fn deployed(router: &MultiServiceRouter, stack: &str, template: &str) -> BTreeMap<String, String> {
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
        tokio::time::sleep(LOOK_INTERVAL).await;
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
        let (_, body) = lambda(
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
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("setup: the function {name:?} must reach Active, last reported {state:?}");
}

/// Invoke the probe synchronously and answer the document its handler returned: one read per
/// address, under the label the event named it by.
async fn probe(router: &MultiServiceRouter, name: &str, access_point_arn: &str) -> Value {
    let (status, payload) = lambda(
        router,
        "POST",
        &format!("/2015-03-31/functions/{name}/invocations"),
        Some(json!({
            "key": KEY,
            "paths": {
                ACCESS_POINT_ARM: access_point_arn,
                BUCKET_ARM: BUCKET,
            },
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "the invocation must be served, got {status} with {payload}"
    );
    assert!(
        payload.get("errorMessage").is_none() && payload.get("errorType").is_none(),
        "the handler must not fault, got {payload}"
    );
    payload
}

/// The sentence S3 refuses a read an access point policy explicitly denies with: who was
/// refused, the action, the resource the ACCESS POINT names for the key, and the clause
/// naming the document that decided it. Written out here rather than built from the
/// service's own helper so the wording has something to be wrong about.
fn explicit_deny_message(access_point_arn: &str) -> String {
    format!(
        "User: arn:aws:sts::{ACCOUNT}:assumed-role/{ROLE}/{FUNCTION} is not authorized to \
         perform: s3:GetObject on resource: \"{access_point_arn}/object/{KEY}\" with an \
         explicit deny in a resource-based policy"
    )
}

/// The whole chain: the access point's policy denies the function's role, so the read through
/// the access point is refused 403 `AccessDenied` in the same invocation that reads the same
/// key by bucket name — and removing the document lets both arms read it.
#[tokio::test]
async fn an_access_point_policy_denial_refuses_only_the_access_point_path() {
    let router = build_router();
    create_bucket(&router, BUCKET).await;
    put_object(&router, BUCKET, KEY, BODY).await;
    let access_point_arn = create_access_point(&router, ACCESS_POINT, BUCKET).await;
    delegate_to_access_points(&router, BUCKET).await;
    let outputs = deployed(
        &router,
        "xsvc-access-point-probe-stack",
        &handler_stack(&access_point_arn),
    )
    .await;
    let function = outputs
        .get("FunctionName")
        .cloned()
        .unwrap_or_else(|| panic!("setup: the stack must output the function's name"));
    assert_eq!(
        function, FUNCTION,
        "setup: the stack must name the function {FUNCTION:?}, got {function:?}"
    );
    wait_until_active(&router, &function).await;

    // Before any access point policy is put, BOTH arms read the object: the role's own
    // documents grant the read on each of them, so what the deny below changes is observable.
    let served = probe(&router, &function, &access_point_arn).await;
    for arm in [ACCESS_POINT_ARM, BUCKET_ARM] {
        assert_eq!(
            served[arm]["body"].as_str(),
            Some(BODY),
            "case no access point policy: the {arm} arm must read the object, got {served}"
        );
    }

    // The access point's policy DENIES the function's role, selected by the identity its
    // calls are signed as through the `aws:PrincipalArn` condition key.
    let deny = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Deny",
            "Principal": "*",
            "Action": "s3:GetObject",
            "Resource": format!("{access_point_arn}/object/*"),
            "Condition": {"ArnEquals": {
                "aws:PrincipalArn": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}"),
            }},
        }],
    })
    .to_string();
    put_access_point_policy(&router, ACCESS_POINT, &deny).await;
    let read_back = get_access_point_policy(&router, ACCESS_POINT).await;
    assert_eq!(
        (read_back.status, read_back.element("Policy")),
        (200, deny.clone()),
        "the stored document must read back byte for byte, got {} with {}",
        read_back.status,
        read_back.body
    );

    // ONE invocation, both arms: the access point path is refused and the bucket path serves
    // the bytes.
    let contrasted = probe(&router, &function, &access_point_arn).await;
    assert_eq!(
        contrasted[ACCESS_POINT_ARM]["code"].as_str(),
        Some(ACCESS_DENIED),
        "case a denying access point policy: the {ACCESS_POINT_ARM} arm must be refused \
         {ACCESS_DENIED}, got {contrasted}"
    );
    assert_eq!(
        contrasted[ACCESS_POINT_ARM]["message"].as_str(),
        Some(explicit_deny_message(&access_point_arn).as_str()),
        "case a denying access point policy: the refusal must carry the published sentence, \
         got {contrasted}"
    );
    assert_eq!(
        contrasted[BUCKET_ARM]["body"].as_str(),
        Some(BODY),
        "case a denying access point policy: the {BUCKET_ARM} arm must be untouched by it, \
         got {contrasted}"
    );

    // And the document's REMOVAL is live: the same invocation reads the bytes through the
    // access point again, and the policy read answers that there is none.
    delete_access_point_policy(&router, ACCESS_POINT).await;
    let repaired = probe(&router, &function, &access_point_arn).await;
    for arm in [ACCESS_POINT_ARM, BUCKET_ARM] {
        assert_eq!(
            repaired[arm]["body"].as_str(),
            Some(BODY),
            "case the access point policy removed: the {arm} arm must read the object, got \
             {repaired}"
        );
    }
    let absent = get_access_point_policy(&router, ACCESS_POINT).await;
    assert_eq!(
        (absent.status, absent.element("Code").as_str()),
        (404, NO_SUCH_ACCESS_POINT_POLICY),
        "the removed document must be reported absent, got {} with {}",
        absent.status,
        absent.body
    );
}
