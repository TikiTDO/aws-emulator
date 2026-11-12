// SPDX-License-Identifier: CC0-1.0

//! The management-event fan-out of a GLOBAL service and of a REGIONAL one, reaching
//! EventBridge rules in the Region each event belongs to.
//!
//! RULE: while a trail of the account is logging management events of a call's read/write
//! class, every control-plane call the account serves is ALSO published onto the DEFAULT
//! event bus — with `source` `aws.<service>`, `detail-type` `AWS API Call via CloudTrail`
//! and the CloudTrail record of the call as `detail` — in the Region the record is filed
//! in: the partition's leader Region for a GLOBAL service (IAM), and the Region the call was
//! served in for a regional one (Amazon SQS). A rule on that Region's default bus whose
//! pattern matches the envelope, or reaches into the record, delivers to its targets; a rule
//! on any other Region's bus sees none of those events.
//!
//! Only a cross-service suite can pin this: the events are published by the services that
//! SERVED the calls (IAM over awsQuery, Amazon SQS over awsJson1_0), the delivery decision
//! belongs to CloudTrail (awsJson1_1), the match and the hand-over belong to EventBridge
//! (awsJson1_1) and what arrived is read from Amazon SQS — four services over ONE engine, as
//! `core/router/src/lib.rs` mounts them.
//!
//! The rows span what the rule generalises over rather than the one call that motivated it:
//! which service served the call (one global, one regional), which Region the rule lives in
//! (the leader Region, another Region), the event NAME a pattern pins (two calls of one
//! service, each reaching only its own rule), the read/write class the trail's selector
//! admits (a reporting call under a WriteOnly selector reaches nothing) and the class of
//! call itself (the per-message data plane is no management event, so it publishes nothing).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The partition's leader Region: where a GLOBAL service's management events are filed, and
/// where the trail of this suite is created.
const LEADER_REGION: &str = "us-east-1";

/// A second Region, which is where the regional service's calls below are served — the
/// "comparison Region" a rule on it sees its own events in and no global service's.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated resource belongs to (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// IAM's API version, which every awsQuery form body of it carries.
const IAM_API_VERSION: &str = "2010-05-08";

/// The `detail-type` CloudTrail's management-event fan-out carries
/// (`service_common::api_call_events::API_CALL_DETAIL_TYPE`).
const API_CALL_DETAIL_TYPE: &str = "AWS API Call via CloudTrail";

/// The `source` an event published for an IAM call carries.
const IAM_EVENT_SOURCE: &str = "aws.iam";

/// The `source` an event published for an Amazon SQS call carries.
const SQS_EVENT_SOURCE: &str = "aws.sqs";

/// The input transformation every target below delivers through — the shape a client wires
/// an audit consumer with: the rule that matched, and the whole event beside it. The two
/// placeholders are EventBridge's own reserved fills, `<aws.events.rule-name>` and
/// `<aws.events.event.json>`.
const INPUT_TEMPLATE: &str = r#"{"rule": "<aws.events.rule-name>", "event": <aws.events.event.json>}"#;

/// A trust policy for the roles below, written so it carries nothing an
/// `application/x-www-form-urlencoded` body treats specially.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"sts:AssumeRole"}]}"#
);

/// How long a delivery is waited for. Generous: the suite waits for the CONDITION (the
/// envelope arriving on the queue), and this is only the point at which waiting longer
/// cannot help.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How long between two looks at a queue while waiting for one.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The services this fan-out runs through, over ONE shared engine.
///
/// Amazon S3 is mounted beside them because a trail's destination bucket is resolved in the
/// account's object store: the trail is created the way a client creates one, against a
/// bucket that really exists.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is not the subject here, so requests are unauthenticated; the credential
    // scope is still carried, because it is what names the Region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service` and `region`, which is
/// what routes the request and what names the Region it was signed for. The signature is
/// never checked here.
fn authorization(service: &str, region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One `X-Amz-Target` RPC call in `region`, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    region: &str,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{region}.amazonaws.com"))
        .header("authorization", authorization(signing_name, region))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) in `region` that must answer
/// 200.
async fn events(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = rpc(router, region, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation} in {region}, \
         got {status} with {parsed}"
    );
    parsed
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) in `region` that must answer
/// 200.
async fn sqs(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) = rpc(router, region, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Amazon SQS {operation} in {region}, \
         got {status} with {parsed}"
    );
    parsed
}

/// A CloudTrail call (`awsJson1_1`, `CloudTrail_20131101.<Operation>`) that must answer 200.
async fn cloudtrail(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(
        router,
        LEADER_REGION,
        "cloudtrail",
        "CloudTrail_20131101",
        "1.1",
        operation,
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudTrail {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One IAM awsQuery form POST, as its status and body text.
async fn iam(router: &MultiServiceRouter, body: String) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{LEADER_REGION}.amazonaws.com"))
        .header("authorization", authorization("iam", LEADER_REGION))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(Bytes::from(format!("Version={IAM_API_VERSION}&{body}")))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (
        status,
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// `PUT /<bucket>` plus `PUT /<bucket>?policy` — the bucket the trail delivers its log files
/// into, carrying the bucket policy that lets CloudTrail write into it.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}"))
        .header("host", format!("s3.{LEADER_REGION}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization("s3", LEADER_REGION))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );

    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AWSCloudTrailWrite",
            "Effect": "Allow",
            "Principal": { "Service": "cloudtrail.amazonaws.com" },
            "Action": "s3:PutObject",
            "Resource": format!("arn:aws:s3:::{bucket}/*"),
            "Condition": {
                "StringEquals": { "s3:x-amz-acl": "bucket-owner-full-control" },
            },
        }],
    })
    .to_string();
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}?policy"))
        .header("host", format!("s3.{LEADER_REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("s3", LEADER_REGION))
        .body(Bytes::from(policy))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        (200..300).contains(&response.status().as_u16()),
        "setup: PutBucketPolicy({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// Create a queue in `region` and hand back its url and ARN. Nothing may send to it yet:
/// the grant is written once the rule that delivers to it exists ([`admitting_rule`]),
/// because it names that rule's ARN.
async fn create_queue(router: &MultiServiceRouter, region: &str, name: &str) -> (String, String) {
    let created = sqs(router, name, region, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: expected a QueueUrl for {name:?}, got {created}"))
        .to_string();
    let arn = format!("arn:aws:sqs:{region}:{ACCOUNT}:{name}");
    (url, arn)
}

/// Let `rule_arn` — and only it — send to the queue at `url`: the resource policy a client
/// writes for an EventBridge target, naming the service principal the delivery is made as
/// and scoping it to the rule that makes it.
async fn admitting_rule(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    queue_arn: &str,
    rule_arn: &str,
) {
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": rule_arn } },
        }],
    });
    sqs(
        router,
        case,
        region,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
}

/// Receive up to ten messages from `url`, draining them, as their parsed bodies.
async fn drained(router: &MultiServiceRouter, case: &str, region: &str, url: &str) -> Vec<Value> {
    let answer = sqs(
        router,
        case,
        region,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    answer["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let body = message["Body"].as_str().unwrap_or_default();
                    serde_json::from_str(body).unwrap_or_else(|error| {
                        panic!("case {case:?}: a delivered envelope is JSON ({error}): {body}")
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Look at `url` until it has handed back at least `wanted` envelopes, or
/// [`DELIVERY_DEADLINE`] has passed — whichever comes first. The CONDITION is what is
/// waited for; the deadline only bounds a wait that will never be satisfied.
async fn delivered(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    url: &str,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let mut seen = Vec::new();
    loop {
        seen.extend(drained(router, case, region, url).await);
        if seen.len() >= wanted || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One rule this suite installs: the Region its bus lives in, the pattern it matches with,
/// and which call's record — if any — must reach it.
struct Row {
    case: &'static str,
    /// The Region the rule is created in, which is the bus it watches.
    region: &'static str,
    /// The rule's `EventPattern`.
    pattern: Value,
    /// The `source` and `eventName` of the record this row expects, or `None` for a row
    /// that must stay empty.
    expects: Option<(&'static str, &'static str)>,
}

/// The rules, each on its own queue so what arrived at one is never read as another's.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a global service's call, pinned to one event name, in the leader Region",
            region: LEADER_REGION,
            pattern: json!({
                "source": [IAM_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": {
                    "eventSource": ["iam.amazonaws.com"],
                    "eventName": ["CreateRole"],
                },
            }),
            expects: Some((IAM_EVENT_SOURCE, "CreateRole")),
        },
        Row {
            case: "the same service's other event name, on its own rule",
            region: LEADER_REGION,
            pattern: json!({
                "source": [IAM_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": {
                    "eventSource": ["iam.amazonaws.com"],
                    "eventName": ["DeleteRole"],
                },
            }),
            expects: Some((IAM_EVENT_SOURCE, "DeleteRole")),
        },
        Row {
            case: "a regional service's call, in the Region it was served in",
            region: OTHER_REGION,
            pattern: json!({
                "source": [SQS_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["TagQueue"] },
            }),
            expects: Some((SQS_EVENT_SOURCE, "TagQueue")),
        },
        Row {
            case: "a global service's calls are not published outside the leader Region",
            region: OTHER_REGION,
            pattern: json!({
                "source": [IAM_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["CreateRole", "DeleteRole"] },
            }),
            expects: None,
        },
        Row {
            case: "a regional service's call is not published in another Region",
            region: LEADER_REGION,
            pattern: json!({
                "source": [SQS_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["TagQueue"] },
            }),
            expects: None,
        },
        Row {
            case: "a reporting call is outside a WriteOnly selector",
            region: OTHER_REGION,
            pattern: json!({
                "source": [SQS_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["ListQueueTags"] },
            }),
            expects: None,
        },
        Row {
            case: "the message data plane is no management event",
            region: OTHER_REGION,
            pattern: json!({
                "source": [SQS_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["SendMessage", "ReceiveMessage", "DeleteMessage"] },
            }),
            expects: None,
        },
    ]
}

#[tokio::test]
async fn api_call_events_reach_the_rules_of_the_region_they_belong_to() {
    let router = router();

    // A multi-Region trail of the account, logging the WRITE management events of every
    // Region and the partition's global services with them.
    create_bucket(&router, "api-call-event-region-trail").await;
    cloudtrail(
        &router,
        "setup",
        "CreateTrail",
        &json!({
            "Name": "api-call-events-by-region",
            "S3BucketName": "api-call-event-region-trail",
            "IsMultiRegionTrail": true,
            "IncludeGlobalServiceEvents": true,
        }),
    )
    .await;
    cloudtrail(
        &router,
        "setup",
        "PutEventSelectors",
        &json!({
            "TrailName": "api-call-events-by-region",
            "EventSelectors": [{ "ReadWriteType": "WriteOnly", "IncludeManagementEvents": true }],
        }),
    )
    .await;
    cloudtrail(
        &router,
        "setup",
        "StartLogging",
        &json!({ "Name": "api-call-events-by-region" }),
    )
    .await;
    let status = cloudtrail(
        &router,
        "setup",
        "GetTrailStatus",
        &json!({ "Name": "api-call-events-by-region" }),
    )
    .await;
    assert_eq!(
        status["IsLogging"],
        json!(true),
        "setup: the trail must be logging, got {status}"
    );

    // One rule per row, each on its own queue, all on the DEFAULT bus of the row's Region —
    // the only bus an AWS-published event reaches. Each target carries the input
    // transformation a client wires an audit consumer with: the rule that matched, and the
    // whole event beside it.
    let rows = rows();
    let mut queues = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("api-call-event-sink-{index}");
        let (url, queue_arn) = create_queue(&router, row.region, &name).await;
        let rule = format!("api-call-event-rule-{index}");
        let created = events(
            &router,
            row.case,
            row.region,
            "PutRule",
            &json!({ "Name": rule, "EventPattern": row.pattern.to_string() }),
        )
        .await;
        let rule_arn = created["RuleArn"]
            .as_str()
            .unwrap_or_else(|| panic!("setup: expected a RuleArn for {rule:?}, got {created}"))
            .to_string();
        let put = events(
            &router,
            row.case,
            row.region,
            "PutTargets",
            &json!({
                "Rule": rule,
                "Targets": [{
                    "Id": "sink",
                    "Arn": queue_arn,
                    "InputTransformer": {
                        "InputTemplate": INPUT_TEMPLATE,
                    },
                }],
            }),
        )
        .await;
        assert_eq!(
            put["FailedEntryCount"],
            json!(0),
            "case {:?}: the target must be installed, got {put}",
            row.case
        );
        admitting_rule(
            &router,
            row.case,
            row.region,
            &url,
            &queue_arn,
            &rule_arn,
        )
        .await;
        queues.push((url, rule));
    }

    // A queue in the comparison Region for the regional calls below to act on, so what the
    // rules see is the record of a call made to a queue of their own Region.
    let (subject_url, _) = create_queue(&router, OTHER_REGION, "api-call-event-subject").await;

    // The calls whose records the rows above are about: a global service's two writes, a
    // regional service's write, a regional READ (outside the trail's WriteOnly selector)
    // and the per-message data plane (no management event at all).
    let role = "ApiCallEventRole";
    let (status, body) = iam(
        &router,
        format!("Action=CreateRole&RoleName={role}&AssumeRolePolicyDocument={TRUST_POLICY}"),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole must succeed, got {status}: {body}"
    );
    let role_id = xml_element(&body, "RoleId")
        .unwrap_or_else(|| panic!("setup: expected a <RoleId> in {body}"));
    let (status, body) = iam(&router, format!("Action=DeleteRole&RoleName={role}")).await;
    assert_eq!(
        status, 200,
        "setup: DeleteRole must succeed, got {status}: {body}"
    );
    sqs(
        &router,
        "setup",
        OTHER_REGION,
        "TagQueue",
        &json!({ "QueueUrl": subject_url, "Tags": { "owner": "api-call-events" } }),
    )
    .await;
    sqs(
        &router,
        "setup",
        OTHER_REGION,
        "ListQueueTags",
        &json!({ "QueueUrl": subject_url }),
    )
    .await;
    sqs(
        &router,
        "setup",
        OTHER_REGION,
        "SendMessage",
        &json!({ "QueueUrl": subject_url, "MessageBody": "a message is not a management event" }),
    )
    .await;
    let received = sqs(
        &router,
        "setup",
        OTHER_REGION,
        "ReceiveMessage",
        &json!({ "QueueUrl": subject_url, "MaxNumberOfMessages": 1 }),
    )
    .await;
    let handle = received["Messages"][0]["ReceiptHandle"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: expected a received message, got {received}"))
        .to_string();
    sqs(
        &router,
        "setup",
        OTHER_REGION,
        "DeleteMessage",
        &json!({ "QueueUrl": subject_url, "ReceiptHandle": handle }),
    )
    .await;

    // The rows expecting a delivery are read first: once each of them has its envelope, the
    // whole fan-out has run, so a row that must stay empty is read after it would have
    // arrived rather than before.
    let mut arrived: Vec<Vec<Value>> = Vec::new();
    for (row, (url, _)) in rows.iter().zip(queues.iter()) {
        let wanted = usize::from(row.expects.is_some());
        arrived.push(delivered(&router, row.case, row.region, url, wanted).await);
    }

    for ((row, (_, rule)), delivery) in rows.iter().zip(queues.iter()).zip(arrived.iter()) {
        let case = row.case;
        let Some((source, event_name)) = row.expects else {
            assert!(
                delivery.is_empty(),
                "case {case:?}: expected no delivery on the {} bus, got {delivery:?}",
                row.region
            );
            continue;
        };
        assert_eq!(
            delivery.len(),
            1,
            "case {case:?}: expected exactly one delivered message, got {delivery:?}"
        );
        // The input transformation's own two fills: the rule that matched, and the whole
        // event as JSON beside it.
        assert_eq!(
            delivery[0]["rule"],
            json!(rule),
            "case {case:?}: expected the matched rule {rule:?}, got {}",
            delivery[0]
        );
        let envelope = &delivery[0]["event"];
        assert_eq!(
            envelope["source"],
            json!(source),
            "case {case:?}: expected source {source:?}, got {envelope}"
        );
        assert_eq!(
            envelope["detail-type"],
            json!(API_CALL_DETAIL_TYPE),
            "case {case:?}: expected detail-type {API_CALL_DETAIL_TYPE:?}, got {envelope}"
        );
        assert_eq!(
            envelope["account"],
            json!(ACCOUNT),
            "case {case:?}: expected account {ACCOUNT:?}, got {envelope}"
        );
        assert_eq!(
            envelope["region"],
            json!(row.region),
            "case {case:?}: expected region {:?}, got {envelope}",
            row.region
        );
        let detail = &envelope["detail"];
        assert_eq!(
            detail["eventName"],
            json!(event_name),
            "case {case:?}: expected detail.eventName {event_name:?}, got {detail}"
        );
        assert_eq!(
            detail["awsRegion"],
            json!(row.region),
            "case {case:?}: expected detail.awsRegion {:?}, got {detail}",
            row.region
        );
        assert_eq!(
            detail["readOnly"],
            json!(false),
            "case {case:?}: a write must be recorded readOnly=false, got {detail}"
        );
        assert_eq!(
            detail["eventType"],
            json!("AwsApiCall"),
            "case {case:?}: expected an API-call record, got {detail}"
        );
    }

    // What the record of each served call carries, from the one envelope that reached each
    // positive row: the parameters the call was dispatched with, and — for the create — the
    // id its own response answered.
    let created = &arrived[0][0]["event"]["detail"];
    assert_eq!(
        created["eventSource"],
        json!("iam.amazonaws.com"),
        "the create's record must name the serving service, got {created}"
    );
    assert_eq!(
        created["requestParameters"]["roleName"],
        json!(role),
        "the create's record must carry the role it was dispatched with, got {created}"
    );
    assert_eq!(
        created["responseElements"]["role"]["roleId"],
        json!(role_id),
        "the create's record must carry the id its response answered ({role_id}), got {created}"
    );
    let deleted = &arrived[1][0]["event"]["detail"];
    assert_eq!(
        deleted["requestParameters"]["roleName"],
        json!(role),
        "the delete's record must carry the role it was dispatched with, got {deleted}"
    );
    let tagged = &arrived[2][0]["event"]["detail"];
    assert_eq!(
        tagged["eventSource"],
        json!("sqs.amazonaws.com"),
        "the tag write's record must name the serving service, got {tagged}"
    );
    assert_eq!(
        tagged["requestParameters"]["queueUrl"],
        json!(subject_url),
        "the tag write's record must carry the queue it was dispatched with, got {tagged}"
    );
}
