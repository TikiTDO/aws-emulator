// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation stack that reports `CREATE_COMPLETE` has really created the
//! `AWS::SQS::Queue` resources its template declares — in SQS.
//!
//! The rule under test: **a stack reaches its completed status only once the service that
//! owns each declared resource has created it, and the handle the stack publishes for that
//! resource (`Ref`/`Fn::GetAtt`) resolves in that service.** For a queue the handle is its
//! url, so after the create waiter returns, `SendMessage`/`ReceiveMessage`/`DeleteMessage`
//! and `GetQueueAttributes` on the stack's own output succeed — rather than SQS answering
//! `AWS.SimpleQueueService.NonExistentQueue` for a url its own stack just published. Two
//! crates have to agree for that to hold (CloudFormation makes the call, SQS serves it),
//! so the suite lives here: a crate-local test can only ever see one side of it.
//!
//! The rows differ along what the rule generalises over: whether the template NAMES the
//! queue or leaves the name to CloudFormation, how many queues one stack declares, which
//! attributes the properties set (and whether they are written as numbers, booleans,
//! strings or an object), whether the queue is standard or FIFO, and whether one queue's
//! property refers to another queue of the same stack. Every expected value is computed
//! from what the row declares, never read back out of the answer being checked.
//!
//! The deletion half and the refusal half are rows of the same rule: deleting the stack
//! deletes the queues it created, and a queue SQS REFUSES to create leaves the stack
//! `ROLLBACK_COMPLETE` naming the resource rather than publishing a dangling url.
//!
//! The UPDATE half is the same rule for a stack that already has the queue: **a deployment
//! that re-declares a resource it created with a different configuration applies that
//! change through the owning service's own update API, and the stack reaches its completed
//! status with the resource's identity untouched.** For a queue the configuration is its
//! attributes and its tags, so the change is a `SetQueueAttributes` plus a
//! `TagQueue`/`UntagQueue` — never a replacement, and never a completed stack whose
//! change went nowhere.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    failed_action, resource_failure_summary,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and so the region the stack's queues and
/// their ARNs belong to.
const REGION: &str = "us-east-1";

/// The account id every ARN and queue url this emulator mints carries.
const ACCOUNT_ID: &str = "000000000000";

/// Long enough for the stack's clock-owed transition (`STATUS_TRANSITION_DELAY`, 500 ms)
/// to come due. A queue has no transient status of its own: SQS answers `CreateQueue` with
/// a queue that is usable at once.
const PAST_THE_STACK_SPAN: Duration = Duration::from_millis(1_000);

/// How many characters CloudFormation's generated-name suffix carries.
const GENERATED_SUFFIX_LEN: usize = 12;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly
/// as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service`, which is what
/// routes the request. The signature is never checked: request authentication is off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` value the way an AWS SDK does,
/// so a template body full of `{`, `"` and `&` survives the body.
fn form_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// Call one Amazon SQS operation over its awsJson1_0 wire.
async fn sqs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header("authorization", authorization("sqs"))
        .body(Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// One SQS call that must succeed.
async fn sqs_ok(router: &MultiServiceRouter, case: &str, operation: &str, request: Value) -> Value {
    let (status, parsed) = sqs(router, operation, &request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code of an SQS refusal: awsJson1_0 carries it as the body's `__type`.
fn sqs_error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Every `<tag>` text of an XML body, in document order, with the serializer's escaping
/// undone.
fn xml_texts(xml: &str, tag: &str) -> Vec<String> {
    let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
    let mut found = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        found.push(
            after[..end]
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("&apos;", "'")
                .replace("&amp;", "&"),
        );
        rest = &after[end + close.len()..];
    }
    found
}

fn xml_text(xml: &str, tag: &str) -> Option<String> {
    xml_texts(xml, tag).into_iter().next()
}

/// A `DescribeStacks` answer's `Outputs`, as the key/value map a client reads it as.
fn outputs_of(xml: &str) -> BTreeMap<String, String> {
    xml_texts(xml, "OutputKey")
        .into_iter()
        .zip(xml_texts(xml, "OutputValue"))
        .collect()
}

/// Create a stack, wait past its transition, and answer the `DescribeStacks` body.
async fn create_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// Update a stack with a new template, wait past its transition, and answer the
/// `DescribeStacks` body.
async fn update_and_settle(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    template: &str,
) -> String {
    let (status, body) = cloudformation(
        router,
        "UpdateStack",
        &[("StackName", stack_name), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let (status, body) =
        cloudformation(router, "DescribeStacks", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with body {body}"
    );
    body
}

/// Every attribute SQS reports for the queue at `url`.
async fn reported_attributes(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> BTreeMap<String, String> {
    let reported = sqs_ok(
        router,
        case,
        "GetQueueAttributes",
        json!({"QueueUrl": url, "AttributeNames": ["All"]}),
    )
    .await;
    reported
        .get("Attributes")
        .and_then(Value::as_object)
        .map(|attributes| {
            attributes
                .iter()
                .filter_map(|(name, value)| {
                    value.as_str().map(|value| (name.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tags SQS reports for the queue at `url`. A queue with no tag reports no `Tags`
/// member at all, which is the empty map here.
async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
) -> BTreeMap<String, String> {
    let reported = sqs_ok(router, case, "ListQueueTags", json!({"QueueUrl": url})).await;
    reported
        .get("Tags")
        .and_then(Value::as_object)
        .map(|tags| {
            tags.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The logical id [`one_queue_template`] declares its queue under, and so the resource
/// the stack's `Ref` and every assertion below address. Named once here, so the template
/// and the assertion cannot name two different resources.
///
/// It is NOT a tag on the queue: `CreateQueue` refuses a caller a tag key in the reserved
/// `aws:` namespace, so CloudFormation leaves its three `aws:cloudformation:*` records off
/// a queue — see the tag assertion in the update test.
const QUEUE_LOGICAL_ID: &str = "Queue";

/// The template one of the update rows deploys: one `AWS::SQS::Queue` carrying
/// `properties`, publishing its url, under [`QUEUE_LOGICAL_ID`].
fn one_queue_template(properties: &str) -> String {
    format!(
        r#"{{"Resources":{{"{QUEUE_LOGICAL_ID}":{{"Type":"AWS::SQS::Queue","Properties":{properties}}}}},
             "Outputs":{{"QueueUrl":{{"Value":{{"Ref":"{QUEUE_LOGICAL_ID}"}}}}}}}}"#
    )
}

/// One row of the in-place update table: the properties the queue is created with, the
/// properties the update declares for it, and what SQS must report once the update
/// completes.
struct UpdateRow {
    case: &'static str,
    stack_name: &'static str,
    /// The name the template declares for the queue, or `None` where CloudFormation
    /// generates it — a resource kept by its identity either way, so the update applies to
    /// the queue the create made.
    declared_name: Option<&'static str>,
    /// The `Properties` object the create declares, as template text.
    created: &'static str,
    /// The `Properties` object the update declares, as template text.
    updated: &'static str,
    /// Attributes `GetQueueAttributes` must report after the update.
    expected_attributes: &'static [(&'static str, &'static str)],
    /// The tag map `ListQueueTags` must report after the update.
    expected_tags: &'static [(&'static str, &'static str)],
}

/// The rows differ along what the rule generalises over: which attribute changes, whether
/// the update CHANGES a declared property, ADDS one the create did not declare or re-values
/// several at once, the form of the value (integer, boolean, opaque text, a JSON object),
/// whether the change is to the queue's attributes or to its tags (added, re-valued and
/// dropped), and whether the template named the queue or CloudFormation generated the name.
/// No row changes the queue's NAME, which is what makes each change an in-place one rather
/// than a replacement.
const UPDATE_ROWS: &[UpdateRow] = &[
    UpdateRow {
        case: "an integer attribute given a new value",
        stack_name: "update-integer-stack",
        declared_name: Some("update-integer-queue"),
        created: r#"{"QueueName":"update-integer-queue","VisibilityTimeout":60}"#,
        updated: r#"{"QueueName":"update-integer-queue","VisibilityTimeout":120}"#,
        expected_attributes: &[("VisibilityTimeout", "120")],
        expected_tags: &[],
    },
    UpdateRow {
        case: "an attribute of a queue whose name CloudFormation generated",
        stack_name: "update-generated-stack",
        declared_name: None,
        created: r#"{"MessageRetentionPeriod":600}"#,
        updated: r#"{"MessageRetentionPeriod":1200,"DelaySeconds":3}"#,
        expected_attributes: &[("MessageRetentionPeriod", "1200"), ("DelaySeconds", "3")],
        expected_tags: &[],
    },
    UpdateRow {
        case: "attributes the create never declared are added",
        stack_name: "update-added-stack",
        declared_name: Some("update-added-queue"),
        created: r#"{"QueueName":"update-added-queue","VisibilityTimeout":60}"#,
        updated: r#"{"QueueName":"update-added-queue","VisibilityTimeout":60,
             "DelaySeconds":10,"ReceiveMessageWaitTimeSeconds":20}"#,
        expected_attributes: &[
            ("VisibilityTimeout", "60"),
            ("DelaySeconds", "10"),
            ("ReceiveMessageWaitTimeSeconds", "20"),
        ],
        expected_tags: &[],
    },
    UpdateRow {
        case: "several attributes at once, one of them a policy written as an object",
        stack_name: "update-several-stack",
        declared_name: Some("update-several-queue"),
        created: r#"{"QueueName":"update-several-queue","VisibilityTimeout":30,
             "MessageRetentionPeriod":600,
             "Policy":{"Version":"2012-10-17","Statement":[]}}"#,
        updated: r#"{"QueueName":"update-several-queue","VisibilityTimeout":43200,
             "MessageRetentionPeriod":1209600,
             "Policy":{"Version":"2012-10-17","Statement":[{"Effect":"Allow"}]}}"#,
        expected_attributes: &[
            ("VisibilityTimeout", "43200"),
            ("MessageRetentionPeriod", "1209600"),
            // The object the update declared, as the compact JSON string the attribute is
            // documented to carry, with its members in the order the template wrote them.
            (
                "Policy",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow"}]}"#,
            ),
        ],
        expected_tags: &[],
    },
    UpdateRow {
        case: "an opaque text attribute re-pointed at another key",
        stack_name: "update-rekey-stack",
        declared_name: Some("update-rekey-queue"),
        created: r#"{"QueueName":"update-rekey-queue","SqsManagedSseEnabled":false,
             "KmsMasterKeyId":"alias/aws/sqs"}"#,
        updated: r#"{"QueueName":"update-rekey-queue","SqsManagedSseEnabled":false,
             "KmsMasterKeyId":"alias/aws/s3"}"#,
        expected_attributes: &[
            ("SqsManagedSseEnabled", "false"),
            ("KmsMasterKeyId", "alias/aws/s3"),
        ],
        expected_tags: &[],
    },
    UpdateRow {
        case: "a boolean attribute flipped, which takes the queue off its KMS key",
        stack_name: "update-boolean-stack",
        declared_name: Some("update-boolean-queue"),
        created: r#"{"QueueName":"update-boolean-queue","SqsManagedSseEnabled":false,
             "KmsMasterKeyId":"alias/aws/sqs"}"#,
        // Only one server-side encryption option is supported per queue, so enabling the
        // Amazon SQS owned key goes with clearing the KMS key the queue was on — and the
        // queue then reports the one option it is on, the cleared key id gone with it.
        updated: r#"{"QueueName":"update-boolean-queue","SqsManagedSseEnabled":true,
             "KmsMasterKeyId":""}"#,
        expected_attributes: &[("SqsManagedSseEnabled", "true")],
        expected_tags: &[],
    },
    UpdateRow {
        case: "tags only: one added, one re-valued, one dropped",
        stack_name: "update-tags-stack",
        declared_name: Some("update-tags-queue"),
        created: r#"{"QueueName":"update-tags-queue",
             "Tags":[{"Key":"team","Value":"platform"},{"Key":"env","Value":"staging"},
               {"Key":"retire","Value":"soon"}]}"#,
        updated: r#"{"QueueName":"update-tags-queue",
             "Tags":[{"Key":"team","Value":"payments"},{"Key":"env","Value":"staging"},
               {"Key":"owner","Value":"core"}]}"#,
        // No attribute was declared on either side, so the queue keeps the documented
        // CreateQueue default.
        expected_attributes: &[("VisibilityTimeout", "30")],
        expected_tags: &[("env", "staging"), ("owner", "core"), ("team", "payments")],
    },
    UpdateRow {
        case: "an attribute and the tags changed in the same update",
        stack_name: "update-both-stack",
        declared_name: Some("update-both-queue"),
        created: r#"{"QueueName":"update-both-queue","MessageRetentionPeriod":60,
             "Tags":[{"Key":"team","Value":"platform"}]}"#,
        updated: r#"{"QueueName":"update-both-queue","MessageRetentionPeriod":86400,
             "Tags":[{"Key":"team","Value":"platform"},{"Key":"tier","Value":"gold"}]}"#,
        expected_attributes: &[("MessageRetentionPeriod", "86400")],
        expected_tags: &[("team", "platform"), ("tier", "gold")],
    },
];

/// An update that changes a mutable property of an `AWS::SQS::Queue` the stack already
/// created is APPLIED to that queue and the stack reaches `UPDATE_COMPLETE`: the queue
/// keeps its identity and everything it holds (nothing is replaced), and SQS reports the
/// configuration the new template declares.
#[tokio::test]
async fn an_update_applies_changed_queue_properties_in_place() {
    let router = build_router();

    for row in UPDATE_ROWS {
        let case = row.case;
        let body = create_and_settle(
            &router,
            case,
            row.stack_name,
            &one_queue_template(row.created),
        )
        .await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the stack to complete before the update, got {body}"
        );
        let url = outputs_of(&body)
            .get("QueueUrl")
            .unwrap_or_else(|| panic!("case {case:?}: no QueueUrl output in {body}"))
            .clone();
        // The name the stack gave the queue: the declared one, or CloudFormation's
        // generated `<stack>-<logical id>-<suffix>`, which the update must keep.
        let queue_name = match row.declared_name {
            Some(declared) => {
                assert_eq!(
                    url,
                    queue_url(declared),
                    "case {case:?}: expected the declared queue's url"
                );
                declared.to_string()
            }
            None => {
                let name = url.rsplit('/').next().unwrap_or_default().to_string();
                assert!(
                    is_generated_name(&name, row.stack_name, "Queue"),
                    "case {case:?}: expected a generated <stack>-<logical id>-<suffix> name, \
                     got {name:?}"
                );
                name
            }
        };

        // What the queue is before the update: the instant it was created, and one message
        // it holds. A REPLACED queue would carry neither afterwards.
        let created_timestamp = reported_attributes(&router, case, &url)
            .await
            .get("CreatedTimestamp")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no CreatedTimestamp for {url}"));
        let payload = format!("before the update of {queue_name}");
        sqs_ok(
            &router,
            case,
            "SendMessage",
            json!({"QueueUrl": url, "MessageBody": payload}),
        )
        .await;

        let body = update_and_settle(
            &router,
            case,
            row.stack_name,
            &one_queue_template(row.updated),
        )
        .await;
        let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
        assert_eq!(
            (
                xml_text(&body, "StackStatus").as_deref(),
                reason.as_str()
            ),
            (Some("UPDATE_COMPLETE"), ""),
            "case {case:?}: expected the update to be applied in place, got {body}"
        );

        // The configuration the update declared is the configuration the queue has, and
        // every attribute it did not name is left alone.
        let attributes = reported_attributes(&router, case, &url).await;
        for (attribute, value) in row.expected_attributes {
            assert_eq!(
                attributes.get(*attribute).map(String::as_str),
                Some(*value),
                "case {case:?}: expected {attribute}={value} after the update, got {attributes:?}"
            );
        }
        let tags = reported_tags(&router, case, &url).await;
        // Exactly the tags the update declares. Amazon SQS's `CreateQueue` refuses a
        // CALLER a tag key in the reserved `aws:` namespace, so CloudFormation leaves its
        // three `aws:cloudformation:*` records off a queue rather than having the stack
        // fail over tags it adds of its own accord — the documented exception in the
        // provisioner's `applicable_tags`, which `AWS::IAM::Role` and the other
        // reserved-namespace types already take.
        let expected_tags: BTreeMap<String, String> = row
            .expected_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            tags, expected_tags,
            "case {case:?}: expected exactly the declared tag map after the update, got {tags:?}"
        );

        // The same queue, not a new one: the name still resolves to the same url, the
        // queue was created at the same instant, and the message sent before the update is
        // still there.
        let resolved = sqs_ok(
            &router,
            case,
            "GetQueueUrl",
            json!({"QueueName": queue_name}),
        )
        .await;
        assert_eq!(
            resolved.get("QueueUrl").and_then(Value::as_str),
            Some(url.as_str()),
            "case {case:?}: expected the update to keep the queue's url, got {resolved}"
        );
        assert_eq!(
            attributes.get("CreatedTimestamp").map(String::as_str),
            Some(created_timestamp.as_str()),
            "case {case:?}: expected the queue the stack created, not a replacement"
        );
        let received = sqs_ok(
            &router,
            case,
            "ReceiveMessage",
            json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
        )
        .await;
        let bodies: Vec<&str> = received
            .get("Messages")
            .and_then(Value::as_array)
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|message| message.get("Body").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            bodies,
            vec![payload.as_str()],
            "case {case:?}: expected the in-place update to keep the queue's messages, got \
             {received}"
        );
    }
}

/// A property the update NO LONGER declares would have to be reset to Amazon SQS's own
/// documented default, which `SetQueueAttributes` cannot be asked for: the update is
/// refused naming that property rather than reported as applied, and the queue keeps the
/// value it has — so no stack ever reports a queue configured differently from its
/// declaration.
#[tokio::test]
async fn an_update_that_drops_a_declared_property_is_refused_rather_than_dropped() {
    let router = build_router();
    let case = "an update that stops declaring VisibilityTimeout";
    let stack_name = "update-dropped-stack";
    let queue_name = "update-dropped-queue";

    let body = create_and_settle(
        &router,
        case,
        stack_name,
        &one_queue_template(
            r#"{"QueueName":"update-dropped-queue","VisibilityTimeout":90,"DelaySeconds":5}"#,
        ),
    )
    .await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete before the update, got {body}"
    );
    let url = queue_url(queue_name);

    let body = update_and_settle(
        &router,
        case,
        stack_name,
        &one_queue_template(r#"{"QueueName":"update-dropped-queue","DelaySeconds":5}"#),
    )
    .await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_ROLLBACK_COMPLETE"),
        "case {case:?}: expected the update to be refused and rolled back rather than \
         silently dropped, got {body}"
    );
    // The STACK's reason summarises which resource the update failed on; the property it
    // will not reset is that resource's own diagnosis, on its event.
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::UPDATE, ["Queue"]),
        "case {case:?}: expected the stack reason to summarise what failed, got {reason:?}"
    );
    let (_, events) =
        cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    assert!(
        xml_texts(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported.contains("VisibilityTimeout")),
        "case {case:?}: expected the failing resource's event to name the property it will \
         not reset, got {events}"
    );

    // The queue is exactly as it was: nothing was applied by the refused update.
    let attributes = reported_attributes(&router, case, &url).await;
    assert_eq!(
        (
            attributes.get("VisibilityTimeout").map(String::as_str),
            attributes.get("DelaySeconds").map(String::as_str)
        ),
        (Some("90"), Some("5")),
        "case {case:?}: expected the queue to keep the configuration it has, got {attributes:?}"
    );
}

/// The url a queue named `name` is addressed at in this emulator.
fn queue_url(name: &str) -> String {
    format!(
        "{}/sqs/{REGION}/{ACCOUNT_ID}/{name}",
        smithy_server_runtime::UNSERVED_URL_BASE
    )
}

/// The ARN a queue named `name` carries.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{ACCOUNT_ID}:{name}")
}

/// What the row says about one queue the template declares.
struct DeclaredQueue {
    /// The output key whose value is the queue's url (`{"Ref": <logical id>}`).
    url_output: &'static str,
    /// The resource's logical id, which the generated name is built from.
    logical_id: &'static str,
    /// The name the template declares, or `None` where CloudFormation generates it.
    declared_name: Option<&'static str>,
    /// Attributes `GetQueueAttributes` must report for the created queue, computed from
    /// what the row's template declares.
    expected_attributes: &'static [(&'static str, &'static str)],
    /// Whether this row exercises the message plane on the queue's url.
    ///
    /// Every created queue is read back through `GetQueueUrl`/`GetQueueAttributes`; a
    /// send/receive/delete round trip is asserted wherever SQS serves one. It is not for
    /// a FIFO queue, because SQS's own `SendMessage` to a FIFO queue is still one of this
    /// emulator's 501 stubs — which is that operation's own issue, not this stack's.
    message_plane: bool,
}

/// One row: a template and the queues it declares.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    template: &'static str,
    queues: &'static [DeclaredQueue],
}

const ROWS: &[Row] = &[
    Row {
        case: "an unnamed queue with no properties, whose name CloudFormation generates",
        stack_name: "unnamed-queue-stack",
        template: r#"{"Resources":{"Queue":{"Type":"AWS::SQS::Queue"}},
             "Outputs":{"QueueUrl":{"Value":{"Ref":"Queue"}}}}"#,
        queues: &[DeclaredQueue {
            url_output: "QueueUrl",
            logical_id: "Queue",
            declared_name: None,
            // The documented CreateQueue defaults, which a template that sets nothing
            // leaves the queue with.
            expected_attributes: &[
                ("VisibilityTimeout", "30"),
                ("MessageRetentionPeriod", "345600"),
                ("DelaySeconds", "0"),
                ("MaximumMessageSize", "1048576"),
                ("ReceiveMessageWaitTimeSeconds", "0"),
            ],
            message_plane: true,
        }],
    },
    Row {
        case: "a named queue whose properties are attributes written as numbers and tags",
        stack_name: "named-queue-stack",
        template: r#"{"Resources":{"Events":{"Type":"AWS::SQS::Queue",
               "Properties":{"QueueName":"stack-events-queue","VisibilityTimeout":120,
                 "MessageRetentionPeriod":600,"DelaySeconds":5,
                 "ReceiveMessageWaitTimeSeconds":10,
                 "Tags":[{"Key":"team","Value":"platform"}]}}},
             "Outputs":{"EventsUrl":{"Value":{"Ref":"Events"}},
               "EventsArn":{"Value":{"Fn::GetAtt":["Events","Arn"]}},
               "EventsName":{"Value":{"Fn::GetAtt":["Events","QueueName"]}}}}"#,
        queues: &[DeclaredQueue {
            url_output: "EventsUrl",
            logical_id: "Events",
            declared_name: Some("stack-events-queue"),
            expected_attributes: &[
                ("VisibilityTimeout", "120"),
                ("MessageRetentionPeriod", "600"),
                ("DelaySeconds", "5"),
                ("ReceiveMessageWaitTimeSeconds", "10"),
            ],
            message_plane: true,
        }],
    },
    Row {
        case: "a FIFO queue, whose properties are booleans and a value set",
        stack_name: "fifo-queue-stack",
        template: r#"{"Resources":{"Ordered":{"Type":"AWS::SQS::Queue",
               "Properties":{"QueueName":"stack-ordered-queue.fifo","FifoQueue":true,
                 "ContentBasedDeduplication":true,"DeduplicationScope":"queue",
                 "FifoThroughputLimit":"perQueue"}}},
             "Outputs":{"OrderedUrl":{"Value":{"Ref":"Ordered"}}}}"#,
        queues: &[DeclaredQueue {
            url_output: "OrderedUrl",
            logical_id: "Ordered",
            declared_name: Some("stack-ordered-queue.fifo"),
            expected_attributes: &[
                ("FifoQueue", "true"),
                ("ContentBasedDeduplication", "true"),
                ("DeduplicationScope", "queue"),
                ("FifoThroughputLimit", "perQueue"),
            ],
            message_plane: false,
        }],
    },
    Row {
        case: "two queues, one a dead-letter target the other's RedrivePolicy object names",
        stack_name: "redrive-queue-stack",
        template: r#"{"Resources":{
               "DeadLetters":{"Type":"AWS::SQS::Queue",
                 "Properties":{"QueueName":"stack-dead-letters"}},
               "Work":{"Type":"AWS::SQS::Queue",
                 "Properties":{"QueueName":"stack-work","VisibilityTimeout":60,
                   "RedrivePolicy":{"maxReceiveCount":3,
                     "deadLetterTargetArn":{"Fn::GetAtt":["DeadLetters","Arn"]}}}}},
             "Outputs":{"WorkUrl":{"Value":{"Ref":"Work"}},
               "DeadLettersUrl":{"Value":{"Ref":"DeadLetters"}}}}"#,
        queues: &[
            DeclaredQueue {
                url_output: "WorkUrl",
                logical_id: "Work",
                declared_name: Some("stack-work"),
                expected_attributes: &[
                    ("VisibilityTimeout", "60"),
                    // The object the template declared, as the compact JSON string the
                    // attribute is documented to carry, with the intrinsic resolved to the
                    // other queue's ARN — in the rendering SQS reports a redrive policy in,
                    // which is the owning service's and not the template's member order.
                    (
                        "RedrivePolicy",
                        r#"{"deadLetterTargetArn":"arn:aws:sqs:us-east-1:000000000000:stack-dead-letters","maxReceiveCount":3}"#,
                    ),
                ],
                message_plane: true,
            },
            DeclaredQueue {
                url_output: "DeadLettersUrl",
                logical_id: "DeadLetters",
                declared_name: Some("stack-dead-letters"),
                expected_attributes: &[("VisibilityTimeout", "30")],
                message_plane: true,
            },
        ],
    },
];

/// Whether `name` is the `<stack>-<logical id>-<suffix>` name CloudFormation generates for
/// a resource the template does not name.
fn is_generated_name(name: &str, stack_name: &str, logical_id: &str) -> bool {
    let prefix = format!("{stack_name}-{logical_id}-");
    name.strip_prefix(&prefix).is_some_and(|suffix| {
        suffix.len() == GENERATED_SUFFIX_LEN
            && suffix
                .chars()
                .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
    })
}

#[tokio::test]
async fn a_completed_stack_has_created_every_queue_it_declares_in_sqs() {
    let router = build_router();

    for row in ROWS {
        let body = create_and_settle(&router, row.case, row.stack_name, row.template).await;
        assert_eq!(
            xml_text(&body, "StackStatus").as_deref(),
            Some("CREATE_COMPLETE"),
            "case {:?}: expected the stack to complete, got {body}",
            row.case
        );
        let outputs = outputs_of(&body);

        for queue in row.queues {
            let case = format!("{} [{}]", row.case, queue.logical_id);
            let url = outputs
                .get(queue.url_output)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no {} output in {body}", queue.url_output)
                })
                .clone();

            // The name the stack gave the queue: the declared one, or CloudFormation's
            // generated `<stack>-<logical id>-<suffix>`.
            let name = match queue.declared_name {
                Some(declared) => {
                    assert_eq!(
                        url,
                        queue_url(declared),
                        "case {case:?}: expected the Ref to be the declared queue's url"
                    );
                    declared.to_string()
                }
                None => {
                    let name = url.rsplit('/').next().unwrap_or_default().to_string();
                    assert_eq!(
                        url,
                        queue_url(&name),
                        "case {case:?}: expected a queue url of this account, got {url}"
                    );
                    assert!(
                        is_generated_name(&name, row.stack_name, queue.logical_id),
                        "case {case:?}: expected a generated <stack>-<logical id>-<suffix> \
                         name, got {name:?}"
                    );
                    name
                }
            };

            // SQS itself resolves the name the stack gave the queue, and answers the same
            // url: the two halves agree about which queue exists.
            let resolved = sqs_ok(
                &router,
                &case,
                "GetQueueUrl",
                json!({"QueueName": name}),
            )
            .await;
            assert_eq!(
                resolved.get("QueueUrl").and_then(Value::as_str),
                Some(url.as_str()),
                "case {case:?}: expected SQS to resolve the stack's queue name to its url, \
                 got {resolved}"
            );

            // The configuration the template declared is the configuration the queue has.
            let reported = sqs_ok(
                &router,
                &case,
                "GetQueueAttributes",
                json!({"QueueUrl": url, "AttributeNames": ["All"]}),
            )
            .await;
            let attributes = reported
                .get("Attributes")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                attributes.get("QueueArn").and_then(Value::as_str),
                Some(queue_arn(&name).as_str()),
                "case {case:?}: expected the created queue's own ARN, got {attributes}"
            );
            for (attribute, value) in queue.expected_attributes {
                assert_eq!(
                    attributes.get(*attribute).and_then(Value::as_str),
                    Some(*value),
                    "case {case:?}: expected {attribute}={value}, got {attributes}"
                );
            }

            // And the message plane works on the url the stack published: a send, a
            // receive of the same body, and a delete of what was received.
            if !queue.message_plane {
                continue;
            }
            let payload = format!("hello {}", queue.logical_id);
            let sent = sqs_ok(
                &router,
                &case,
                "SendMessage",
                json!({"QueueUrl": url, "MessageBody": payload}),
            )
            .await;
            assert!(
                sent.get("MessageId").and_then(Value::as_str).is_some(),
                "case {case:?}: expected a MessageId from SendMessage, got {sent}"
            );
            let received = sqs_ok(
                &router,
                &case,
                "ReceiveMessage",
                json!({"QueueUrl": url, "MaxNumberOfMessages": 10}),
            )
            .await;
            let messages = received
                .get("Messages")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                messages.len(),
                1,
                "case {case:?}: expected the one message that was sent, got {received}"
            );
            assert_eq!(
                messages[0].get("Body").and_then(Value::as_str),
                Some(payload.as_str()),
                "case {case:?}: expected the body that was sent, got {received}"
            );
            let handle = messages[0]
                .get("ReceiptHandle")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case:?}: no ReceiptHandle in {received}"));
            sqs_ok(
                &router,
                &case,
                "DeleteMessage",
                json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
}

#[tokio::test]
async fn deleting_the_stack_deletes_the_queues_it_created() {
    let router = build_router();
    let case = "two queues deleted with their stack";
    let stack_name = "deleted-queue-stack";
    let template = r#"{"Resources":{
           "Kept":{"Type":"AWS::SQS::Queue","Properties":{"QueueName":"deleted-stack-kept"}},
           "Other":{"Type":"AWS::SQS::Queue"}},
         "Outputs":{"KeptUrl":{"Value":{"Ref":"Kept"}},
           "OtherUrl":{"Value":{"Ref":"Other"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the stack to complete, got {body}"
    );
    let outputs = outputs_of(&body);
    let urls: Vec<String> = ["KeptUrl", "OtherUrl"]
        .iter()
        .map(|key| {
            outputs
                .get(*key)
                .unwrap_or_else(|| panic!("case {case:?}: no {key} output in {body}"))
                .clone()
        })
        .collect();
    for url in &urls {
        sqs_ok(
            &router,
            case,
            "GetQueueAttributes",
            json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
        )
        .await;
    }

    let (status, body) =
        cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with body {body}"
    );
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;

    // Every queue the stack created is gone from SQS, which is what makes the stack's
    // resources the stack's own rather than leftovers.
    for url in &urls {
        let (status, answer) = sqs(&router, "ReceiveMessage", &json!({"QueueUrl": url})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the queue at {url} to be gone, got {status} with {answer}"
        );
        assert_eq!(
            sqs_error_code(&answer),
            "QueueDoesNotExist",
            "case {case:?}: expected QueueDoesNotExist for {url}, got {answer}"
        );
    }
}

#[tokio::test]
async fn a_queue_sqs_refuses_to_create_fails_the_stack() {
    let router = build_router();
    let case = "a visibility timeout outside the documented range";
    let stack_name = "refused-queue-stack";
    // "Valid values: An integer from 0 to 43,200 (12 hours)" — a value SQS's own
    // CreateQueue refuses, which is what the stack must report rather than completing.
    let template = r#"{"Resources":{"Queue":{"Type":"AWS::SQS::Queue",
           "Properties":{"QueueName":"refused-stack-queue","VisibilityTimeout":99999}}},
         "Outputs":{"QueueUrl":{"Value":{"Ref":"Queue"}}}}"#;

    let body = create_and_settle(&router, case, stack_name, template).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("ROLLBACK_COMPLETE"),
        "case {case:?}: expected the failed create to be rolled back rather than publish a \
         queue url nothing answers to, got {body}"
    );
    // The STACK's reason summarises which resource failed; SQS's own refusal is reported
    // on that resource's event.
    let reason = xml_text(&body, "StackStatusReason").unwrap_or_default();
    assert_eq!(
        reason,
        resource_failure_summary(failed_action::CREATE, ["Queue"]),
        "case {case:?}: expected the stack reason to summarise what failed, got {reason:?}"
    );
    let (_, events) =
        cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    assert!(
        xml_texts(&events, "ResourceStatusReason")
            .iter()
            .any(|reported| reported.contains("VisibilityTimeout")),
        "case {case:?}: expected the failing resource's event to carry SQS's own refusal, \
         got {events}"
    );

    // And nothing was created: the name the template asked for resolves to no queue.
    let (status, answer) = sqs(
        &router,
        "GetQueueUrl",
        &json!({"QueueName": "refused-stack-queue"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected no queue of that name, got {status} with {answer}"
    );
    assert_eq!(
        sqs_error_code(&answer),
        "QueueDoesNotExist",
        "case {case:?}: expected QueueDoesNotExist, got {answer}"
    );
}

/// The owner URL survives reference resolution, an applied update's rollback, and teardown.
#[tokio::test]
async fn owner_queue_url_survives_refs_rollback_and_delete() {
    let router = build_router();
    let case = "owner-assigned queue URL lifecycle";
    let stack_name = "owner-url-stack";
    let queue_name = "owner-url-queue";
    let original = json!({
        "Resources": {
            "Queue": {"Type": "AWS::SQS::Queue", "Properties": {
                "QueueName": queue_name, "VisibilityTimeout": 90
            }},
            "Policy": {"Type": "AWS::SQS::QueuePolicy", "Properties": {
                "Queues": [{"Ref": "Queue"}],
                "PolicyDocument": {
                    "Version": "2012-10-17",
                    "Id": {"Fn::Join": ["", ["before:", {"Ref": "Queue"}, ":", {"Fn::GetAtt": ["Queue", "QueueUrl"]}]]},
                    "Statement": [{"Effect": "Allow", "Principal": "*", "Action": "sqs:SendMessage", "Resource": {"Fn::GetAtt": ["Queue", "Arn"]}}]
                }
            }}
        },
        "Outputs": {
            "RefUrl": {"Value": {"Ref": "Queue"}},
            "AttUrl": {"Value": {"Fn::GetAtt": ["Queue", "QueueUrl"]}}
        }
    });
    let body = create_and_settle(&router, case, stack_name, &original.to_string()).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "{body}"
    );
    let owner = sqs_ok(
        &router,
        case,
        "GetQueueUrl",
        json!({"QueueName": queue_name}),
    )
    .await;
    let url = owner["QueueUrl"]
        .as_str()
        .expect("owner QueueUrl")
        .to_string();
    assert!(
        url.contains(&format!("/sqs/{REGION}/{ACCOUNT_ID}/{queue_name}")),
        "{url}"
    );
    let expected_outputs = BTreeMap::from([
        ("RefUrl".to_string(), url.clone()),
        ("AttUrl".to_string(), url.clone()),
    ]);
    assert_eq!(outputs_of(&body), expected_outputs);
    let attributes = reported_attributes(&router, case, &url).await;
    let policy: Value =
        serde_json::from_str(attributes.get("Policy").expect("created policy")).unwrap();
    assert_eq!(policy["Id"], json!(format!("before:{url}:{url}")));
    // Retained data distinguishes an in-place rollback from delete/recreate under the same name.
    sqs_ok(
        &router,
        case,
        "SendMessage",
        json!({"QueueUrl": url, "MessageBody": "survive-rollback"}),
    )
    .await;

    let mut changed = original.clone();
    changed["Resources"]["Queue"]["Properties"]["VisibilityTimeout"] = json!(120);
    changed["Resources"]["Policy"]["Properties"]["PolicyDocument"]["Id"]["Fn::Join"][1][0] =
        json!("after:");
    // Existing suite's known owning-service refusal, ordered AFTER both real updates.
    changed["Resources"]["Refused"] = json!({
        "Type": "AWS::SQS::Queue", "DependsOn": "Policy",
        "Properties": {"QueueName": "owner-url-refused", "VisibilityTimeout": 99999}
    });
    let body = update_and_settle(&router, case, stack_name, &changed.to_string()).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_ROLLBACK_COMPLETE"),
        "{body}"
    );
    let (status, events) =
        cloudformation(&router, "DescribeStackEvents", &[("StackName", stack_name)]).await;
    assert_eq!(status, 200, "{events}");
    let members = xml_texts(&events, "member");
    for logical_id in ["Queue", "Policy"] {
        assert!(
            members.iter().any(|event| {
                xml_text(event, "LogicalResourceId").as_deref() == Some(logical_id)
                    && xml_text(event, "ResourceStatus").as_deref() == Some("UPDATE_COMPLETE")
            }),
            "{logical_id} must have updated before the later refusal: {events}"
        );
    }
    assert!(
        members.iter().any(|event| {
            xml_text(event, "LogicalResourceId").as_deref() == Some("Refused")
                && xml_text(event, "ResourceStatusReason")
                    .is_some_and(|reason| reason.contains("VisibilityTimeout"))
        }),
        "the intended owning-service refusal must be witnessed: {events}"
    );
    assert_eq!(outputs_of(&body), expected_outputs);
    let owner = sqs_ok(
        &router,
        case,
        "GetQueueUrl",
        json!({"QueueName": queue_name}),
    )
    .await;
    assert_eq!(owner["QueueUrl"], json!(url));
    let attributes = reported_attributes(&router, case, &url).await;
    assert_eq!(
        attributes.get("VisibilityTimeout").map(String::as_str),
        Some("90")
    );
    let policy: Value =
        serde_json::from_str(attributes.get("Policy").expect("restored policy")).unwrap();
    assert_eq!(policy["Id"], json!(format!("before:{url}:{url}")));
    let received = sqs_ok(
        &router,
        case,
        "ReceiveMessage",
        json!({"QueueUrl": url, "MaxNumberOfMessages": 1}),
    )
    .await;
    assert_eq!(received["Messages"][0]["Body"], json!("survive-rollback"));

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(status, 200, "{body}");
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    for name in [queue_name, "owner-url-refused"] {
        let (status, body) = sqs(&router, "GetQueueUrl", &json!({"QueueName": name})).await;
        assert_eq!(status, 400, "{body}");
        assert_eq!(sqs_error_code(&body), "QueueDoesNotExist", "{body}");
    }
    let (status, body) = sqs(
        &router,
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["All"]}),
    )
    .await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(sqs_error_code(&body), "QueueDoesNotExist", "{body}");
}

/// Carrying an owner-assigned URL must not turn a FIFO variant change into an
/// in-place update. Unnamed variants have distinct generated names and real queues.
#[tokio::test]
async fn owner_queue_url_preserves_fifo_replacement() {
    let router = build_router();
    let case = "owner URL FIFO replacement";
    let stack_name = "owner-url-fifo-stack";
    let original = json!({
        "Resources": {"Queue": {"Type": "AWS::SQS::Queue"}},
        "Outputs": {
            "RefUrl": {"Value": {"Ref": "Queue"}},
            "AttUrl": {"Value": {"Fn::GetAtt": ["Queue", "QueueUrl"]}}
        }
    });
    let body = create_and_settle(&router, case, stack_name, &original.to_string()).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("CREATE_COMPLETE"),
        "{body}"
    );
    let before = outputs_of(&body);
    let before_url = before.get("RefUrl").expect("original Ref").clone();
    assert_eq!(before.get("AttUrl"), Some(&before_url));
    assert!(!before_url.ends_with(".fifo"), "{before_url}");
    reported_attributes(&router, case, &before_url).await;

    let mut fifo = original.clone();
    fifo["Resources"]["Queue"]["Properties"] = json!({"FifoQueue": true});
    let body = update_and_settle(&router, case, stack_name, &fifo.to_string()).await;
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some("UPDATE_COMPLETE"),
        "{body}"
    );
    let after = outputs_of(&body);
    let after_url = after.get("RefUrl").expect("replacement Ref").clone();
    assert_eq!(after.get("AttUrl"), Some(&after_url));
    assert_ne!(after_url, before_url);
    assert!(after_url.ends_with(".fifo"), "{after_url}");
    let attributes = reported_attributes(&router, case, &after_url).await;
    assert_eq!(
        attributes.get("FifoQueue").map(String::as_str),
        Some("true")
    );
    let old_name = before_url.rsplit('/').next().unwrap();
    let (status, body) = sqs(&router, "GetQueueUrl", &json!({"QueueName": old_name})).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(sqs_error_code(&body), "QueueDoesNotExist", "{body}");

    let (status, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack_name)]).await;
    assert_eq!(status, 200, "{body}");
    tokio::time::sleep(PAST_THE_STACK_SPAN).await;
    let new_name = after_url.rsplit('/').next().unwrap();
    let (status, body) = sqs(&router, "GetQueueUrl", &json!({"QueueName": new_name})).await;
    assert_eq!(status, 400, "{body}");
    assert_eq!(sqs_error_code(&body), "QueueDoesNotExist", "{body}");
}
