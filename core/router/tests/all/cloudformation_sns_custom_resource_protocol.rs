// SPDX-License-Identifier: CC0-1.0

//! The CloudFormation CUSTOM RESOURCE protocol, as a provider really sees it: the request
//! published to the Amazon SNS topic a template's `ServiceToken` names, read off an Amazon
//! SQS queue subscribed to that topic, and the answer PUT back to the request's
//! `ResponseURL`.
//!
//! The rule under test: **what a custom resource IS is what its provider's answer says —
//! the `PhysicalResourceId` that answer carries is the id CloudFormation addresses the
//! resource by from then on (on `FAILED` as well as on `SUCCESS`), and the `Data` it carries
//! is what `Fn::GetAtt` on the resource resolves against — and every further request the
//! stack operation owes once that answer lands (the next resource in dependency order, the
//! cleanup `Delete` of a resource a replacement replaced, the rollback's own deletes in
//! reverse dependency order) is published by CloudFormation itself, with no further client
//! call.**
//!
//! Three crates have to agree for that to hold — CloudFormation publishes the request and
//! holds the resource until the answer lands, Amazon SNS delivers the publish to the
//! subscribed endpoint, and Amazon SQS holds it for the provider to read — so the suite
//! lives here: a crate-local suite can only ever see one side of it. The CloudFormation
//! crate's own `custom_resource_providers` suite pins what one request carries; what only
//! this suite can reach is a provider that reads its requests the way a provider does, off
//! a queue, and answers them with nothing else calling CloudFormation in between.
//!
//! The rows differ along what the rule generalises over: whether the `Data` carries the key
//! a dependent resource reads, the `Status` the answer carries (so the id is adopted either
//! way), whether an `Update`'s answer repeats the resource's id or names a new one (in-place
//! versus replacement), and how many resources one template chains with `DependsOn`.

use std::collections::BTreeMap;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{element, elements, form_encode, members_of, opt_element};

/// The Region every request here is signed for, and so the Region the stacks belong to.
const REGION: &str = "us-east-1";

/// How long this suite waits for work the emulator owes — a request being published, a
/// stack settling — and how often it looks while waiting. Generous: what is asserted is
/// that the request arrives at all, never how fast.
const WAIT: Duration = Duration::from_secs(60);
const LOOK: Duration = Duration::from_millis(25);

/// How long a read that must see NOTHING keeps looking before concluding nothing came.
/// Only a row asserting the absence of a request waits this out, so it is short enough to
/// keep the suite brisk and long enough to span several of the emulator's own transitions.
const QUIET_WINDOW: Duration = Duration::from_secs(8);

/// The statuses the rows are stated in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";

/// The emulator as it is ASSEMBLED, on one engine: the request CloudFormation publishes is
/// delivered by the SNS this router serves to the queue it serves too.
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

/// One CloudFormation call (API version 2010-05-15).
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "cloudformation", "2010-05-15", action, members).await
}

/// One Amazon SNS call (API version 2010-03-31).
async fn sns(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    query(router, "sns", "2010-03-31", action, members).await
}

/// One Amazon SQS call (`awsJson1_0`), as its status and parsed body.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(body.to_string()))
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

/// The provider a row's template names: the topic CloudFormation publishes its requests to,
/// and the queue — subscribed to that topic, with the policy that lets SNS write to it — the
/// provider reads them off.
struct Provider {
    topic_arn: String,
    queue_url: String,
    /// The requests read off the queue and not acted on yet, oldest first: one read takes
    /// every message then waiting off the queue, and a provider acts on them one at a time.
    received: Vec<Value>,
}

/// Create that provider: topic, queue, the queue policy SNS's delivery needs, and the
/// subscription that carries one to the other.
async fn provider(router: &MultiServiceRouter, name: &str) -> Provider {
    let (status, body) = sns(router, "CreateTopic", &[("Name", name)]).await;
    assert_eq!(
        status, 200,
        "CreateTopic({name:?}) must be accepted, got {status} with {body}"
    );
    let topic_arn = element(&body, "TopicArn");

    let (status, answered) = sqs(router, "CreateQueue", &json!({ "QueueName": name })).await;
    assert_eq!(
        status, 200,
        "CreateQueue({name:?}) must be accepted, got {status} with {answered}"
    );
    let queue_url = answered["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateQueue({name:?}) must answer a QueueUrl: {answered}"))
        .to_string();

    let (status, answered) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetQueueAttributes({name:?}) must be answered, got {status} with {answered}"
    );
    let queue_arn = answered["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("the queue must report its ARN: {answered}"))
        .to_string();

    // The grant Amazon SNS needs to write this topic's notifications to the queue, scoped to
    // the publishing topic exactly as the documented queue policy scopes it.
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "sns.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": queue_arn,
            "Condition": { "ArnEquals": { "aws:SourceArn": topic_arn } },
        }],
    })
    .to_string();
    let (status, answered) = sqs(
        router,
        "SetQueueAttributes",
        &json!({ "QueueUrl": queue_url, "Attributes": { "Policy": policy } }),
    )
    .await;
    assert_eq!(
        status, 200,
        "SetQueueAttributes({name:?}) must be accepted, got {status} with {answered}"
    );

    let (status, body) = sns(
        router,
        "Subscribe",
        &[
            ("TopicArn", &topic_arn),
            ("Protocol", "sqs"),
            ("Endpoint", &queue_arn),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "Subscribe({name:?}) must be accepted, got {status} with {body}"
    );

    Provider {
        topic_arn,
        queue_url,
        received: Vec::new(),
    }
}

/// Take every provider request waiting on the queue now off it, oldest first, onto what the
/// provider has received — the documents the notifications carry.
async fn receive(router: &MultiServiceRouter, provider: &mut Provider) {
    loop {
        let (status, answered) = sqs(
            router,
            "ReceiveMessage",
            &json!({ "QueueUrl": provider.queue_url, "MaxNumberOfMessages": 10 }),
        )
        .await;
        assert_eq!(
            status, 200,
            "ReceiveMessage must be answered, got {status} with {answered}"
        );
        let messages = answered["Messages"].as_array().cloned().unwrap_or_default();
        if messages.is_empty() {
            return;
        }
        for message in messages {
            if let Some(document) = published_request(&message) {
                provider.received.push(document);
            }
            if let Some(receipt) = message["ReceiptHandle"].as_str() {
                let (status, answered) = sqs(
                    router,
                    "DeleteMessage",
                    &json!({ "QueueUrl": provider.queue_url, "ReceiptHandle": receipt }),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "DeleteMessage must be answered, got {status} with {answered}"
                );
            }
        }
    }
}

/// The provider request one queue message carries: the notification envelope's `Message` is
/// the document CloudFormation published, and a raw delivery carries that document itself.
fn published_request(message: &Value) -> Option<Value> {
    let body: Value = serde_json::from_str(message["Body"].as_str()?).ok()?;
    match body.get("Message").and_then(Value::as_str) {
        Some(published) => serde_json::from_str(published).ok(),
        None => Some(body),
    }
}

/// The next provider request, waited for with NOTHING calling CloudFormation: the request is
/// CloudFormation's own to publish once the previous answer landed, so a provider that only
/// reads its queue has to receive it.
async fn next_request(
    router: &MultiServiceRouter,
    provider: &mut Provider,
    case: &str,
    expected: &str,
) -> Value {
    let looks = WAIT.as_millis() / LOOK.as_millis();
    for _ in 0..looks {
        if !provider.received.is_empty() {
            return provider.received.remove(0);
        }
        receive(router, provider).await;
        if !provider.received.is_empty() {
            return provider.received.remove(0);
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("case {case:?}: {expected} was never published to the provider");
}

/// Every request that reaches the provider inside the quiet window — for a row asserting
/// that none is owed.
async fn requests_inside_the_quiet_window(
    router: &MultiServiceRouter,
    provider: &mut Provider,
) -> Vec<Value> {
    let looks = QUIET_WINDOW.as_millis() / LOOK.as_millis();
    for _ in 0..looks {
        receive(router, provider).await;
        tokio::time::sleep(LOOK).await;
    }
    std::mem::take(&mut provider.received)
}

/// PUT one answer to the `ResponseURL` the request carried — the plain, unsigned write a
/// provider makes, to the address this emulator published for that one request.
async fn answer(router: &MultiServiceRouter, request: &Value, document: &Value) -> (u16, String) {
    answer_bytes(router, request, document.to_string()).await
}

/// [`answer`] for a row that decides the document's BYTES itself (the response-size rule).
async fn answer_bytes(router: &MultiServiceRouter, request: &Value, body: String) -> (u16, String) {
    let url = request["ResponseURL"]
        .as_str()
        .unwrap_or_else(|| panic!("every provider request must carry a ResponseURL: {request}"));
    let host = url
        .split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default()
        .to_string();
    let http_request = http::Request::builder()
        .method("PUT")
        .uri(url)
        .header("host", host)
        .header("content-type", "")
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// Answer one request with a `SUCCESS` carrying `physical_resource_id` and `data`, asserting
/// the write was accepted.
async fn answered_success(
    router: &MultiServiceRouter,
    case: &str,
    request: &Value,
    physical_resource_id: &str,
    data: &Value,
) {
    let mut document = json!({
        "Status": "SUCCESS",
        "PhysicalResourceId": physical_resource_id,
    });
    if !data.is_null() {
        document["Data"] = data.clone();
    }
    let (status, body) = answer(router, request, &document).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the provider's answer must be accepted, got {status} with {body}"
    );
}

/// The `ResourceProperties` one request carried, as the map a provider reads.
fn properties_of(request: &Value) -> BTreeMap<String, String> {
    request["ResourceProperties"]
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(name, value)| {
                    (
                        name.clone(),
                        value.as_str().map(str::to_string).unwrap_or_else(|| value.to_string()),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `StackStatus` one stack reports now.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> String {
    let (status, body) = cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
    assert_eq!(
        status, 200,
        "DescribeStacks({stack:?}) must be answered, got {status} with {body}"
    );
    opt_element(&body, "StackStatus").unwrap_or_default()
}

/// Poll the stack until it reports `wanted`, answering every provider request that arrives
/// meanwhile with a `SUCCESS` under the id the request already names — a provider that
/// carries out whatever it is sent, which is what lets a rollback conclude.
async fn settled_with_a_responsive_provider(
    router: &MultiServiceRouter,
    provider: &mut Provider,
    case: &str,
    stack: &str,
    wanted: &str,
) -> String {
    let looks = WAIT.as_millis() / LOOK.as_millis();
    let mut reported = String::new();
    for _ in 0..looks {
        reported = stack_status(router, stack).await;
        if reported == wanted {
            return reported;
        }
        receive(router, provider).await;
        for request in std::mem::take(&mut provider.received) {
            let id = request["PhysicalResourceId"]
                .as_str()
                .unwrap_or("a-provider-minted-id")
                .to_string();
            answered_success(router, case, &request, &id, &Value::Null).await;
        }
        tokio::time::sleep(LOOK).await;
    }
    reported
}

/// The `ResourceStatusReason` the stack reports for one resource's `status` event, polled
/// for: a stack's events become readable as its own clock reaches them.
async fn reported_reason(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
    status: &str,
) -> String {
    let looks = WAIT.as_millis() / LOOK.as_millis();
    for _ in 0..looks {
        let (code, body) =
            cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
        assert_eq!(
            code, 200,
            "DescribeStackEvents({stack:?}) must be answered, got {code} with {body}"
        );
        let found = members_of(&body).into_iter().find_map(|member| {
            let logical = elements(member, "LogicalResourceId").into_iter().next()?;
            let reported = elements(member, "ResourceStatus").into_iter().next()?;
            if logical != logical_id || reported != status {
                return None;
            }
            elements(member, "ResourceStatusReason").into_iter().next()
        });
        if let Some(reason) = found {
            return reason;
        }
        tokio::time::sleep(LOOK).await;
    }
    String::new()
}

/// Create one stack, asserting the call was accepted.
async fn create_stack(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateStack({stack:?}) must be accepted, got {status} with {body}"
    );
}

/// The value the provider of the first rows publishes under the key it returns.
const RETURNED_KEY: &str = "Present";
const RETURNED_VALUE: &str = "https://provisioned.example.invalid/endpoint";

/// `Fn::GetAtt` on a custom resource resolves against the `Data` its provider answered with:
/// the dependent resource's own request carries the value the provider published, and a key
/// the `Data` lacks fails that dependent resource instead of silently dropping the property.
#[tokio::test]
async fn a_dependent_resources_request_carries_the_providers_data() {
    // (case, the stack, the key the consumer reads, whether the Data carries it)
    let rows: &[(&str, &str, &str, bool)] = &[
        (
            "a key the provider's Data carries",
            "cr-getatt-present",
            RETURNED_KEY,
            true,
        ),
        (
            "a key the provider's Data lacks",
            "cr-getatt-absent",
            "Absent",
            false,
        ),
    ];
    for (case, stack, key, carried) in rows {
        let router = build_router();
        let mut provider = provider(&router, stack).await;
        let template = json!({
            "Resources": {
                "Source": {
                    "Type": "Custom::Source",
                    "Properties": { "ServiceToken": provider.topic_arn },
                },
                "Sink": {
                    "Type": "Custom::Sink",
                    "Properties": {
                        "ServiceToken": provider.topic_arn,
                        "Value": { "Fn::GetAtt": ["Source", key] },
                    },
                },
            },
        })
        .to_string();
        create_stack(&router, case, stack, &template).await;

        let first = next_request(&router, &mut provider, case, "the Source's Create").await;
        assert_eq!(
            first["LogicalResourceId"].as_str(),
            Some("Source"),
            "case {case:?}: the resource the other one reads is asked first, got {first}"
        );
        assert_eq!(
            first["RequestType"].as_str(),
            Some("Create"),
            "case {case:?}: expected a Create, got {first}"
        );
        answered_success(
            &router,
            case,
            &first,
            "source-instance",
            &json!({ RETURNED_KEY: RETURNED_VALUE }),
        )
        .await;

        if !carried {
            // The consuming resource reads an attribute the answer does not carry, so it
            // cannot be created at all: it fails with CloudFormation's attribute error and
            // the stack rolls back.
            let settled = settled_with_a_responsive_provider(
                &router,
                &mut provider,
                case,
                stack,
                ROLLBACK_COMPLETE,
            )
            .await;
            assert_eq!(
                settled, ROLLBACK_COMPLETE,
                "case {case:?}: an attribute the Data lacks must roll the stack back, got \
                 {settled:?}"
            );
            let reason = reported_reason(&router, stack, "Sink", "CREATE_FAILED").await;
            // A custom resource publishes whatever its provider's answer carried, so a key
            // that answer left out is a fault of the VENDOR's response: the diagnostic names
            // the attribute read and the response object it was read from, which is the
            // stack, the resource and the request the answer answered — all three off the
            // request the provider itself received.
            let expected = format!(
                "CustomResource attribute error: Vendor response doesn't contain {key} \
                 attribute in object {}|{}|{}",
                first["StackId"].as_str().unwrap_or_default(),
                first["LogicalResourceId"].as_str().unwrap_or_default(),
                first["RequestId"].as_str().unwrap_or_default(),
            );
            assert_eq!(
                reason, expected,
                "case {case:?}: expected the vendor-response attribute error {expected:?}, got \
                 {reason:?}"
            );
            continue;
        }

        let second = next_request(&router, &mut provider, case, "the Sink's Create").await;
        assert_eq!(
            second["LogicalResourceId"].as_str(),
            Some("Sink"),
            "case {case:?}: the dependent resource is asked next, got {second}"
        );
        let expected: BTreeMap<String, String> = [
            ("ServiceToken".to_string(), provider.topic_arn.clone()),
            ("Value".to_string(), RETURNED_VALUE.to_string()),
        ]
        .into_iter()
        .collect();
        let carried_properties = properties_of(&second);
        assert_eq!(
            carried_properties, expected,
            "case {case:?}: expected the consumer to be asked with the provider's own value, \
             got {carried_properties:?}"
        );
        answered_success(&router, case, &second, "sink-instance", &Value::Null).await;
        let settled = settled_with_a_responsive_provider(
            &router,
            &mut provider,
            case,
            stack,
            CREATE_COMPLETE,
        )
        .await;
        assert_eq!(
            settled, CREATE_COMPLETE,
            "case {case:?}: both resources answered, so the stack must complete, got {settled:?}"
        );
    }
}

/// The `PhysicalResourceId` an answer carries is the id CloudFormation addresses the resource
/// by afterwards — on a `FAILED` answer as much as on a `SUCCESS` — so the `Delete` the
/// rollback publishes names the id the provider itself chose.
#[tokio::test]
async fn the_id_a_failed_answer_carries_is_the_id_the_rollback_deletes() {
    // (case, the stack, the id the provider answers the Create with)
    let rows: &[(&str, &str, &str)] = &[
        (
            "an id of the provider's own naming",
            "cr-failed-pid",
            "provider-chosen-failed-id",
        ),
        (
            "an id that looks nothing like a generated one",
            "cr-failed-pid-opaque",
            "7b3f/instance:42",
        ),
    ];
    for (case, stack, failed_id) in rows {
        let router = build_router();
        let mut provider = provider(&router, stack).await;
        let template = json!({
            "Resources": {
                "Thing": {
                    "Type": "Custom::Thing",
                    "Properties": { "ServiceToken": provider.topic_arn, "Marker": "v1" },
                },
            },
        })
        .to_string();
        create_stack(&router, case, stack, &template).await;

        let created = next_request(&router, &mut provider, case, "the Create").await;
        let (status, body) = answer(
            &router,
            &created,
            &json!({
                "Status": "FAILED",
                "Reason": "the provider could not carry the create out",
                "PhysicalResourceId": failed_id,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the provider's answer must be accepted, got {status} with {body}"
        );

        let deleted = next_request(&router, &mut provider, case, "the rollback's Delete").await;
        assert_eq!(
            deleted["RequestType"].as_str(),
            Some("Delete"),
            "case {case:?}: the rollback deletes the failed resource, got {deleted}"
        );
        assert_eq!(
            deleted["PhysicalResourceId"].as_str(),
            Some(*failed_id),
            "case {case:?}: expected the id the provider returned with FAILED, got {deleted}"
        );
        answered_success(&router, case, &deleted, failed_id, &Value::Null).await;
        let settled = settled_with_a_responsive_provider(
            &router,
            &mut provider,
            case,
            stack,
            ROLLBACK_COMPLETE,
        )
        .await;
        assert_eq!(
            settled, ROLLBACK_COMPLETE,
            "case {case:?}: a failed create rolls the stack back, got {settled:?}"
        );
    }
}

/// An `Update` answered with a NEW `PhysicalResourceId` is a replacement: CloudFormation
/// publishes a `Delete` for the OLD id carrying the OLD properties. An answer repeating the
/// id is an in-place update and owes no `Delete` at all.
#[tokio::test]
async fn an_update_answered_with_a_new_id_owes_a_cleanup_delete() {
    // (case, the stack, the id the Update is answered with, whether that replaces)
    let rows: &[(&str, &str, &str, bool)] = &[
        (
            "an answer naming a new physical resource",
            "cr-update-replaced",
            "thing-second",
            true,
        ),
        (
            "an answer repeating the resource's id",
            "cr-update-in-place",
            "thing-first",
            false,
        ),
    ];
    for (case, stack, updated_id, replaces) in rows {
        let router = build_router();
        let mut provider = provider(&router, stack).await;
        let token = provider.topic_arn.clone();
        let template = |marker: &str| {
            json!({
                "Resources": {
                    "Thing": {
                        "Type": "Custom::Versioned",
                        "Properties": {
                            "ServiceToken": token,
                            "Name": marker,
                        },
                    },
                },
            })
            .to_string()
        };
        create_stack(&router, case, stack, &template("n1")).await;
        let created = next_request(&router, &mut provider, case, "the Create").await;
        answered_success(&router, case, &created, "thing-first", &Value::Null).await;
        let settled = settled_with_a_responsive_provider(
            &router,
            &mut provider,
            case,
            stack,
            CREATE_COMPLETE,
        )
        .await;
        assert_eq!(
            settled, CREATE_COMPLETE,
            "case {case:?}: the created resource must complete the stack, got {settled:?}"
        );

        let (status, body) = cloudformation(
            &router,
            "UpdateStack",
            &[("StackName", stack), ("TemplateBody", &template("n2"))],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateStack must be accepted, got {status} with {body}"
        );
        let update = next_request(&router, &mut provider, case, "the Update").await;
        assert_eq!(
            update["RequestType"].as_str(),
            Some("Update"),
            "case {case:?}: expected an Update, got {update}"
        );
        assert_eq!(
            update["PhysicalResourceId"].as_str(),
            Some("thing-first"),
            "case {case:?}: the Update names the resource that exists, got {update}"
        );
        assert_eq!(
            update["OldResourceProperties"]["Name"].as_str(),
            Some("n1"),
            "case {case:?}: the Update carries the properties last deployed, got {update}"
        );
        answered_success(&router, case, &update, updated_id, &Value::Null).await;

        if !replaces {
            let unexpected = requests_inside_the_quiet_window(&router, &mut provider).await;
            assert!(
                unexpected.is_empty(),
                "case {case:?}: an in-place update owes no further request, got {unexpected:?}"
            );
            let settled = settled_with_a_responsive_provider(
                &router,
                &mut provider,
                case,
                stack,
                UPDATE_COMPLETE,
            )
            .await;
            assert_eq!(
                settled, UPDATE_COMPLETE,
                "case {case:?}: the in-place update must complete, got {settled:?}"
            );
            continue;
        }

        let cleanup = next_request(&router, &mut provider, case, "the cleanup Delete").await;
        assert_eq!(
            cleanup["RequestType"].as_str(),
            Some("Delete"),
            "case {case:?}: a replacement owes the cleanup of what it replaced, got {cleanup}"
        );
        assert_eq!(
            cleanup["PhysicalResourceId"].as_str(),
            Some("thing-first"),
            "case {case:?}: the cleanup addresses the id that was replaced, got {cleanup}"
        );
        assert_eq!(
            cleanup["ResourceProperties"]["Name"].as_str(),
            Some("n1"),
            "case {case:?}: the cleanup carries the OLD properties, got {cleanup}"
        );
        answered_success(&router, case, &cleanup, "thing-first", &Value::Null).await;
        let settled = settled_with_a_responsive_provider(
            &router,
            &mut provider,
            case,
            stack,
            UPDATE_COMPLETE,
        )
        .await;
        assert_eq!(
            settled, UPDATE_COMPLETE,
            "case {case:?}: the replacement must complete the update, got {settled:?}"
        );
    }
}

/// A response document of exactly [`MAX_RESPONSE_BODY_BYTES`] bytes completes its resource —
/// and the resource that DEPENDS on it is asked next, in dependency order — while one byte
/// more is a document CloudFormation does not read: that resource fails with the
/// response-too-long reason, and the rollback deletes it before the resource it depended on.
#[tokio::test]
async fn a_chained_resource_is_asked_after_the_one_it_depends_on_answers() {
    let case = "two custom resources chained with DependsOn";
    let stack = "cr-chained-sizes";
    let router = build_router();
    let mut provider = provider(&router, stack).await;
    let template = json!({
        "Resources": {
            "Fits": {
                "Type": "Custom::Sized",
                "Properties": { "ServiceToken": provider.topic_arn },
            },
            "Overflows": {
                "Type": "Custom::Sized",
                "DependsOn": "Fits",
                "Properties": { "ServiceToken": provider.topic_arn },
            },
        },
    })
    .to_string();
    create_stack(&router, case, stack, &template).await;

    let first = next_request(&router, &mut provider, case, "the Create of Fits").await;
    assert_eq!(
        first["LogicalResourceId"].as_str(),
        Some("Fits"),
        "case {case:?}: the resource the other depends on is asked first, got {first}"
    );
    let (status, body) = answer_bytes(&router, &first, padded_answer("fits-instance", 4096)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a document at the bound must be accepted, got {status} with {body}"
    );

    // The chained resource's own request: published by CloudFormation once the first answer
    // landed, with nothing else calling it.
    let second = next_request(&router, &mut provider, case, "the Create of Overflows").await;
    assert_eq!(
        second["LogicalResourceId"].as_str(),
        Some("Overflows"),
        "case {case:?}: the dependent resource is asked next, got {second}"
    );
    let (status, body) =
        answer_bytes(&router, &second, padded_answer("overflows-instance", 4097)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the write is accepted whatever the document's size, got {status} with \
         {body}"
    );

    // The rollback deletes the failed resource BEFORE the one it depended on: a resource is
    // not taken down while something that depends on it is still there.
    let deleted = next_request(&router, &mut provider, case, "the Delete of Overflows").await;
    assert_eq!(
        (
            deleted["RequestType"].as_str(),
            deleted["LogicalResourceId"].as_str(),
        ),
        (Some("Delete"), Some("Overflows")),
        "case {case:?}: the dependent resource is deleted first, got {deleted}"
    );
    // ...and nothing is asked about the resource it depended on while that `Delete` is
    // unanswered: the dependent is still there until its provider says it is gone.
    let early = requests_inside_the_quiet_window(&router, &mut provider).await;
    assert!(
        early.is_empty(),
        "case {case:?}: the rollback asked to delete the resource Overflows depends on before \
         Overflows was deleted: {early:?}"
    );
    answered_success(&router, case, &deleted, "overflows-instance", &Value::Null).await;
    let then = next_request(&router, &mut provider, case, "the Delete of Fits").await;
    assert_eq!(
        (
            then["RequestType"].as_str(),
            then["LogicalResourceId"].as_str(),
        ),
        (Some("Delete"), Some("Fits")),
        "case {case:?}: the resource it depended on is deleted after it, got {then}"
    );
    answered_success(&router, case, &then, "fits-instance", &Value::Null).await;

    let settled = settled_with_a_responsive_provider(
        &router,
        &mut provider,
        case,
        stack,
        ROLLBACK_COMPLETE,
    )
    .await;
    assert_eq!(
        settled, ROLLBACK_COMPLETE,
        "case {case:?}: a document past the bound rolls the stack back, got {settled:?}"
    );
    let reason = reported_reason(&router, stack, "Overflows", "CREATE_FAILED").await;
    assert!(
        reason.contains("Response object is too long."),
        "case {case:?}: expected the response-too-long reason, got {reason:?}"
    );
}

/// One `SUCCESS` answer whose whole document is exactly `bytes` long: the padding is a `Data`
/// value, which is where a provider's own payload rides.
fn padded_answer(physical_resource_id: &str, bytes: usize) -> String {
    let mut document = json!({
        "Status": "SUCCESS",
        "PhysicalResourceId": physical_resource_id,
        "Data": { "Pad": "" },
    });
    let bare = document.to_string().len();
    let padding = bytes.saturating_sub(bare);
    document["Data"]["Pad"] = Value::String("p".repeat(padding));
    let body = document.to_string();
    assert_eq!(
        body.len(),
        bytes,
        "the padded answer must be exactly {bytes} bytes, got {}",
        body.len()
    );
    body
}
