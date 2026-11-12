// SPDX-License-Identifier: CC0-1.0

//! A stack's `NotificationARNs` reach the queue subscribed to the topic they name — through
//! the WHOLE mounted service set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **a stack whose `NotificationARNs` name Amazon SNS topics publishes
//! ONE notification per `StackEvent` to each of them, whichever operation caused the event —
//! the execution of a change set included — as a body of `Key='value'` lines; a stack
//! carrying no topics publishes nothing, so clearing the list stops the stream and naming a
//! topic again resumes it.**
//!
//! Three services have to agree for that to hold — CloudFormation publishes, SNS fans out,
//! SQS holds what arrived — and the publish CloudFormation makes is an in-process call
//! addressed the way a client's own `sns`-scoped call is, so WHICH mounted model claims it is
//! only decidable with every service mounted.
//!
//! The rows differ along what the rule generalises over: the OPERATION that causes the events
//! (a `CreateStack`, and the execution of a change set), and whether the topics were given to
//! that operation, inherited from the stack or cleared by it.

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    authorization, given_topic_with_subscriber, router, Grant,
};
use crate::query_wire::{block_of, element, elements, form_encode, members_of, outputs_of};

/// The Region every request here is signed for, and so the Region the stack lives in.
const REGION: &str = "us-west-2";

/// How many times a leg polls the stack before giving up, and how long it waits between
/// polls: a stack's transition is clock-owed, and a READ is what makes it due.
const POLLS: usize = 60;
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// The template every row deploys: one wait-condition handle (a resource no service of this
/// emulator has to be asked about) and an output that reads the stack's own topics through
/// the `AWS::NotificationARNs` pseudo parameter.
const TEMPLATE: &str = r#"{"Resources":{"Marker":{"Type":"AWS::CloudFormation::WaitConditionHandle"}},
    "Outputs":{"NotificationArns":{"Value":{"Fn::Join":[",",{"Ref":"AWS::NotificationARNs"}]}}}}"#;

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
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/cloudformation/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// One CloudFormation call that must answer 200.
async fn cfn_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    members: &[(&str, &str)],
) -> String {
    let (status, body) = cloudformation(router, action, members).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudFormation {action}, got {status} with {body}"
    );
    body
}

/// One Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`), which must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sqs.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("AmazonSQS.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/sqs/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// Take every notification waiting on the queue at `url`, as the body its publisher sent:
/// SNS delivers to a queue inside its own JSON envelope, whose `Message` is that body.
///
/// Deleted as it is handed over, so a later leg reads only what arrived since.
async fn drain_bodies(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<String> {
    let received = sqs(
        router,
        case,
        "ReceiveMessage",
        &json!({"QueueUrl": url, "MaxNumberOfMessages": 10, "WaitTimeSeconds": 0}),
    )
    .await;
    let messages = received["Messages"].as_array().cloned().unwrap_or_default();
    let mut bodies = Vec::with_capacity(messages.len());
    for message in &messages {
        let body = message["Body"].as_str().unwrap_or_default();
        let envelope: Value = serde_json::from_str(body).unwrap_or(Value::Null);
        bodies.push(
            envelope["Message"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| body.to_string()),
        );
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            sqs(
                router,
                case,
                "DeleteMessage",
                &json!({"QueueUrl": url, "ReceiptHandle": handle}),
            )
            .await;
        }
    }
    bodies
}

/// Poll the stack until it leaves every `*_IN_PROGRESS` status, and answer the status it
/// settled in with the body that reported it.
async fn settled(router: &MultiServiceRouter, case: &str, stack: &str) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..POLLS {
        let body = cfn_ok(router, case, "DescribeStacks", &[("StackName", stack)]).await;
        let status = element(&body, "StackStatus");
        last = (status, body);
        if !last.0.ends_with("_IN_PROGRESS") {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {stack:?} never left an in-progress status; last was {:?} with {}",
        last.0, last.1
    );
}

/// Every `EventId` the stack's event stream reports.
async fn event_ids(router: &MultiServiceRouter, case: &str, stack: &str) -> Vec<String> {
    let body = cfn_ok(router, case, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&body, "EventId")
}

/// The `Key='value'` body of one notification as the FIELDS a subscriber reads it as, in the
/// order the body states them — which is part of the contract: the body is parsed
/// positionally as much as by key.
fn notification_fields(body: &str) -> Vec<(String, String)> {
    body.lines()
        .filter_map(|line| line.split_once("='"))
        .map(|(key, value)| {
            (
                key.to_string(),
                value.trim_end_matches('\'').to_string(),
            )
        })
        .collect()
}

/// The value one notification body states for `key`, or `None` for a body carrying no such
/// line at all.
fn field_of(fields: &[(String, String)], key: &str) -> Option<String> {
    fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

/// Drain the queue until the events the stack reports have all arrived, or the polls run out,
/// answering every notification body that did arrive.
///
/// A READ of the stack is what makes its clock-owed events — and the notifications they owe —
/// due, so each poll asks CloudFormation again before draining.
async fn delivered_notifications(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    url: &str,
) -> Vec<std::collections::BTreeMap<String, String>> {
    delivered_fields(router, case, stack, url)
        .await
        .into_iter()
        .map(|fields| fields.into_iter().collect())
        .collect()
}

/// [`delivered_notifications`] keeping each body's own LINE ORDER: the one poll-and-drain
/// loop both readers of the queue are built on.
async fn delivered_fields(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    url: &str,
) -> Vec<Vec<(String, String)>> {
    let mut arrived: Vec<Vec<(String, String)>> = Vec::new();
    for _ in 0..POLLS {
        let wanted = event_ids(router, case, stack).await;
        for body in drain_bodies(router, case, url).await {
            arrived.push(notification_fields(&body));
        }
        let delivered: Vec<String> = arrived
            .iter()
            .filter_map(|fields| field_of(fields, "EventId"))
            .collect();
        if wanted.iter().all(|id| delivered.contains(id)) {
            return arrived;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    arrived
}

/// The `NotificationARNs` `DescribeStacks` reports for one stack.
async fn reported_topics(router: &MultiServiceRouter, case: &str, stack: &str) -> Vec<String> {
    let body = cfn_ok(router, case, "DescribeStacks", &[("StackName", stack)]).await;
    assert!(
        !members_of(&body).is_empty(),
        "case {case:?}: DescribeStacks reported no stack: {body}"
    );
    block_of(&body, "NotificationARNs")
        .map(|block| elements(block, "member"))
        .unwrap_or_default()
}
/// Every event of a stack created with `NotificationARNs` is published to the topic they
/// name, and the stack reports the topics it was given.
#[tokio::test]
async fn every_event_of_a_stack_reaches_the_topics_its_notification_arns_name() {
    let case = "a CreateStack carrying one topic";
    let router = router();
    let (topic, url) =
        given_topic_with_subscriber(&router, case, REGION, "stack-events", Grant::ScopedToTopic)
            .await;
    let stack = "notified-create";
    cfn_ok(
        &router,
        case,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", TEMPLATE),
            ("NotificationARNs.member.1", &topic),
        ],
    )
    .await;
    let (status, body) = settled(&router, case, stack).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {case:?}: the stack must complete, got {status} with {body}"
    );
    // The stack's own topics are what the pseudo parameter resolves to, so the output reads
    // them back.
    assert_eq!(
        outputs_of(&body).get("NotificationArns"),
        Some(&topic),
        "case {case:?}: the stack's output must carry its topics, got {body}"
    );
    assert_eq!(
        reported_topics(&router, case, stack).await,
        vec![topic.clone()],
        "case {case:?}: DescribeStacks must report the topics the create named"
    );

    let wanted = event_ids(&router, case, stack).await;
    assert!(
        !wanted.is_empty(),
        "case {case:?}: the stack must report events of its own creation"
    );
    let delivered = delivered_notifications(&router, case, stack, &url).await;
    for id in &wanted {
        let found = delivered
            .iter()
            .find(|lines| lines.get("EventId") == Some(id));
        let Some(lines) = found else {
            panic!(
                "case {case:?}: event {id} was never delivered; the queue had {delivered:?}"
            );
        };
        // The lines EVERY body carries, so a subscriber reads the same set for every event...
        for key in [
            "StackId",
            "Timestamp",
            "EventId",
            "LogicalResourceId",
            "Namespace",
            "ResourceProperties",
            "ResourceStatus",
            "ResourceStatusReason",
            "DetailedStatus",
            "ResourceType",
            "StackName",
            "ClientRequestToken",
        ] {
            assert!(
                lines.contains_key(key),
                "case {case:?}: the notification of {id} must carry {key}, got {lines:?}"
            );
        }
        // ...and the two whose line depends on the event itself: `PrincipalId` on an event
        // about the stack, `PhysicalResourceId` on one that names a physical resource — both
        // asserted per event, against what the event reports, by
        // [`every_notification_body_carries_the_whole_field_set_in_order`].
        if lines.get("ResourceType").map(String::as_str) == Some(STACK_RESOURCE_TYPE) {
            for key in ["PrincipalId", "PhysicalResourceId"] {
                assert!(
                    lines.contains_key(key),
                    "case {case:?}: the notification of the stack's own event {id} must carry \
                     {key}, got {lines:?}"
                );
            }
        }
        assert_eq!(
            lines.get("StackName"),
            Some(&stack.to_string()),
            "case {case:?}: the notification of {id} must name the stack, got {lines:?}"
        );
    }
}

/// The topics of a CHANGE SET: the execution publishes every event of its own to them, a
/// change set that names none keeps the stack's, and one naming an EMPTY list clears them —
/// which stops the stream until a later change set names the topic again.
#[tokio::test]
async fn a_change_sets_notification_arns_govern_the_streams_of_its_execution() {
    let case = "a change set that creates the stack, then clears and restores its topics";
    let router = router();
    let (topic, url) =
        given_topic_with_subscriber(&router, case, REGION, "change-set-events", Grant::Unscoped)
            .await;
    let stack = "notified-change-set";

    // The baseline: a CREATE change set naming the topic. Every event of its execution is
    // delivered.
    execute(&router, case, stack, "create-set", "CREATE", Some(&[topic.as_str()])).await;
    assert_eq!(
        reported_topics(&router, case, stack).await,
        vec![topic.clone()],
        "case {case:?}: the executed change set's topics must be the stack's"
    );
    let baseline = event_ids(&router, case, stack).await;
    let delivered = delivered_notifications(&router, case, stack, &url).await;
    let seen: Vec<String> = delivered
        .iter()
        .filter_map(|lines| lines.get("EventId").cloned())
        .collect();
    for id in &baseline {
        assert!(
            seen.contains(id),
            "case {case:?}: event {id} of the executed change set was never delivered; the \
             queue had {seen:?}"
        );
    }

    // An UPDATE change set naming an EMPTY list clears the stack's topics, and nothing of
    // that execution is delivered.
    execute(&router, case, stack, "clear-set", "UPDATE", Some(&[])).await;
    assert!(
        reported_topics(&router, case, stack).await.is_empty(),
        "case {case:?}: an explicitly empty list must clear the stack's topics"
    );
    let after_clear = event_ids(&router, case, stack).await;
    let cleared: Vec<String> = delivered_notifications(&router, case, stack, &url)
        .await
        .iter()
        .filter_map(|lines| lines.get("EventId").cloned())
        .collect();
    let new_events: Vec<&String> = after_clear
        .iter()
        .filter(|id| !baseline.contains(id))
        .collect();
    assert!(
        !new_events.is_empty(),
        "case {case:?}: the clearing execution must record events of its own"
    );
    for id in &new_events {
        assert!(
            !cleared.contains(id),
            "case {case:?}: event {id} was recorded with no topic in force and must not be \
             delivered; the queue had {cleared:?}"
        );
    }

    // ...and a change set naming the topic again resumes the stream.
    execute(&router, case, stack, "restore-set", "UPDATE", Some(&[topic.as_str()])).await;
    assert_eq!(
        reported_topics(&router, case, stack).await,
        vec![topic.clone()],
        "case {case:?}: naming the topic again must put it back in force"
    );
    let after_restore = event_ids(&router, case, stack).await;
    let restored: Vec<String> = delivered_notifications(&router, case, stack, &url)
        .await
        .iter()
        .filter_map(|lines| lines.get("EventId").cloned())
        .collect();
    let resumed: Vec<&String> = after_restore
        .iter()
        .filter(|id| !after_clear.contains(id))
        .collect();
    assert!(
        !resumed.is_empty(),
        "case {case:?}: the restoring execution must record events of its own"
    );
    for id in &resumed {
        assert!(
            restored.contains(id),
            "case {case:?}: event {id} of the restored stream must be delivered; the queue had \
             {restored:?}"
        );
    }

    // A change set that names NO topics at all states nothing about them, so the ones in
    // force stay in force.
    execute(&router, case, stack, "silent-set", "UPDATE", None).await;
    assert_eq!(
        reported_topics(&router, case, stack).await,
        vec![topic.clone()],
        "case {case:?}: omitting NotificationARNs must keep the stack's topics"
    );
}

/// The keys one notification body carries, in the order AWS states them — the field set a
/// subscriber parses, by key or positionally, for one event.
///
/// `DetailedStatus` is `StackEvent$DetailedStatus` ("an optional field that contains
/// information about the detailed status of the stack event", the `DetailedStatus` enum), and
/// it sits between `ResourceStatusReason` and `ResourceType`: a member the event has nothing
/// to report for still states its line. The two exceptions are the members a body states only
/// where the event HAS one — see [`expected_field_order`].
const NOTIFICATION_FIELD_ORDER: [&str; 14] = [
    "StackId",
    "Timestamp",
    "EventId",
    "LogicalResourceId",
    "Namespace",
    "PhysicalResourceId",
    "PrincipalId",
    "ResourceProperties",
    "ResourceStatus",
    "ResourceStatusReason",
    "DetailedStatus",
    "ResourceType",
    "StackName",
    "ClientRequestToken",
];

/// The resource type an event about the STACK itself carries, which is the event a body
/// states `PrincipalId` on.
const STACK_RESOURCE_TYPE: &str = "AWS::CloudFormation::Stack";

/// The keys the body of ONE event's notification carries, in order: [`NOTIFICATION_FIELD_ORDER`]
/// without `PhysicalResourceId` for an event that names no physical resource, and without
/// `PrincipalId` for an event that is not about the stack itself.
fn expected_field_order(event: &ReportedEvent) -> Vec<&'static str> {
    NOTIFICATION_FIELD_ORDER
        .into_iter()
        .filter(|key| match *key {
            "PhysicalResourceId" => !event.physical_resource_id.is_empty(),
            "PrincipalId" => event.resource_type == STACK_RESOURCE_TYPE,
            _ => true,
        })
        .collect()
}

/// A template of TWO resources, so one stack's events cover more than one resource beside the
/// stack's own events.
const TWO_RESOURCES: &str = r#"{"Resources":{
    "First":{"Type":"AWS::CloudFormation::WaitConditionHandle"},
    "Second":{"Type":"AWS::CloudFormation::WaitConditionHandle"}}}"#;

/// What the API read reports for one stack event — the source of what its notification has to
/// state: the empty string stands for a member the event reports nothing for.
#[derive(Debug, Clone, Default)]
struct ReportedEvent {
    physical_resource_id: String,
    resource_type: String,
    resource_status: String,
    resource_status_reason: String,
    detailed_status: String,
}

/// Every event the stack reports, keyed by `EventId` ([`ReportedEvent`]).
async fn reported_events(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> std::collections::BTreeMap<String, ReportedEvent> {
    let body = cfn_ok(router, case, "DescribeStackEvents", &[("StackName", stack)]).await;
    let events = block_of(&body, "StackEvents").unwrap_or_default();
    members_of(events)
        .into_iter()
        .filter_map(|member| {
            let id = elements(member, "EventId").into_iter().next()?;
            let read = |name: &str| {
                elements(member, name)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            };
            Some((
                id,
                ReportedEvent {
                    physical_resource_id: read("PhysicalResourceId"),
                    resource_type: read("ResourceType"),
                    resource_status: read("ResourceStatus"),
                    resource_status_reason: read("ResourceStatusReason"),
                    detailed_status: read("DetailedStatus"),
                },
            ))
        })
        .collect()
}

/// The `UserId` AWS STS reports for the credential every request of this suite is signed
/// with — "the unique identifier of the calling entity", which is the IAM principal a stack
/// event's notification names.
async fn caller_user_id(router: &MultiServiceRouter, case: &str) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("sts.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("sts", REGION))
        .body(Bytes::from("Action=GetCallerIdentity&Version=2011-06-15"))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from STS GetCallerIdentity, got {status} with {body}"
    );
    element(&body, "UserId")
}

/// EVERY notification body states the field set AWS states for that event, in AWS's order —
/// `DetailedStatus` among them — with `PrincipalId` the caller's own principal on the stack's
/// own events and `DetailedStatus` the one the event itself reports.
///
/// The rows differ along what the rule generalises over: the operation that recorded the
/// events (a `CreateStack` over two resources, and the execution of a change set), and within
/// each row the assertion runs over every delivered message — the stack's own events (whose
/// `ResourceType` is the stack) and its resources' alike, events that name a physical resource
/// and events that name none, events with a `ResourceStatusReason` and events without.
#[tokio::test]
async fn every_notification_body_carries_the_whole_field_set_in_order() {
    // (case, the stack, the topic and queue name, whether a change set records the events)
    let rows: [(&str, &str, &str, bool); 2] = [
        (
            "a CreateStack over two resources",
            "field-set-create",
            "field-set-create",
            false,
        ),
        (
            "the execution of a CREATE change set",
            "field-set-change-set",
            "field-set-change-set",
            true,
        ),
    ];

    // One emulator for both rows: each row deploys its own stack and subscribes its own
    // queue to its own topic, so no row reads another's events, and the suite stands up one
    // service set rather than two.
    let router = router();
    let caller = caller_user_id(&router, "the suite's own credential").await;
    for (case, stack, name, by_change_set) in rows {
        let (topic, url) =
            given_topic_with_subscriber(&router, case, REGION, name, Grant::ScopedToTopic).await;
        if by_change_set {
            execute(&router, case, stack, "field-set", "CREATE", Some(&[topic.as_str()])).await;
        } else {
            cfn_ok(
                &router,
                case,
                "CreateStack",
                &[
                    ("StackName", stack),
                    ("TemplateBody", TWO_RESOURCES),
                    ("NotificationARNs.member.1", &topic),
                ],
            )
            .await;
            let (status, body) = settled(&router, case, stack).await;
            assert_eq!(
                status, "CREATE_COMPLETE",
                "case {case:?}: the stack must complete, got {status} with {body}"
            );
        }

        // Drained first, so every event the settled stack reports below has had its
        // notification delivered and each delivered message's event is one the API read
        // reports.
        let delivered = delivered_fields(&router, case, stack, &url).await;
        let reported = reported_events(&router, case, stack).await;
        assert!(
            !delivered.is_empty(),
            "case {case:?}: the stack's events must reach the topic it notifies"
        );
        for id in reported.keys() {
            assert!(
                delivered
                    .iter()
                    .any(|fields| field_of(fields, "EventId").as_ref() == Some(id)),
                "case {case:?}: event {id} was never delivered; the queue had {delivered:?}"
            );
        }
        for fields in &delivered {
            let id = field_of(fields, "EventId").unwrap_or_default();
            // A notification whose event the stack-event stream does not carry at all is a
            // PRE-DEPLOYMENT validation result, which `DescribeEvents` reports instead: its
            // body has no reported event here to be asserted against.
            let Some(event) = reported.get(&id).cloned() else {
                continue;
            };
            let keys: Vec<&str> = fields.iter().map(|(key, _)| key.as_str()).collect();
            let expected = expected_field_order(&event);
            assert_eq!(
                keys, expected,
                "case {case:?}: the body of event {id} ({event:?}) must state {expected:?} in \
                 that order, got {fields:?}"
            );
            // The members a body states from the event itself, each against what the API read
            // reports for the same event — with the empty string where it reports none.
            for (key, reported) in [
                ("PhysicalResourceId", &event.physical_resource_id),
                ("ResourceType", &event.resource_type),
                ("ResourceStatus", &event.resource_status),
                ("ResourceStatusReason", &event.resource_status_reason),
                ("DetailedStatus", &event.detailed_status),
            ] {
                if key == "PhysicalResourceId" && reported.is_empty() {
                    // Stated only where the event names one, which the key assertion covers.
                    continue;
                }
                let stated = field_of(fields, key).unwrap_or_default();
                assert_eq!(
                    &stated, reported,
                    "case {case:?}: the {key} of event {id} must be the one the event reports \
                     ({reported:?}), got {stated:?}"
                );
            }
            if event.resource_type == STACK_RESOURCE_TYPE {
                let principal = field_of(fields, "PrincipalId").unwrap_or_default();
                assert_eq!(
                    principal, caller,
                    "case {case:?}: the PrincipalId of event {id} must be the caller STS \
                     reports ({caller:?}), got {principal:?}"
                );
            }
        }

        // The field set is asserted above over the stack's OWN events and over its
        // resources' events, so both classes have to be among what was delivered — and so
        // does an event that names no physical resource, whose body states one line fewer.
        let types: std::collections::BTreeSet<String> = delivered
            .iter()
            .filter_map(|fields| field_of(fields, "ResourceType"))
            .collect();
        assert!(
            types.contains(STACK_RESOURCE_TYPE),
            "case {case:?}: the stack's own events must be among the delivered, got {types:?}"
        );
        assert!(
            types.iter().any(|kind| kind != STACK_RESOURCE_TYPE),
            "case {case:?}: its resources' events must be among the delivered, got {types:?}"
        );
        assert!(
            delivered
                .iter()
                .any(|fields| field_of(fields, "PhysicalResourceId").is_none()),
            "case {case:?}: an event naming no physical resource must be among the delivered, \
             got {delivered:?}"
        );
    }
}

/// Create one change set of `change_set_type` and execute it, waiting for both to settle.
///
/// `topics` is what the request states for `NotificationARNs`: `Some(&[])` is the explicitly
/// empty list, and `None` states nothing at all.
async fn execute(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    change_set: &str,
    change_set_type: &str,
    topics: Option<&[&str]>,
) {
    // Each execution deploys a template of its own, so the update has something to change.
    let body = format!(
        r#"{{"Resources":{{"Marker":{{"Type":"AWS::CloudFormation::WaitConditionHandle",
            "Metadata":{{"Revision":"{change_set}"}}}}}},
            "Outputs":{{"NotificationArns":{{"Value":{{"Fn::Join":[",",
            {{"Ref":"AWS::NotificationARNs"}}]}}}}}}}}"#
    );
    let mut members = vec![
        ("StackName", stack),
        ("ChangeSetName", change_set),
        ("ChangeSetType", change_set_type),
        ("TemplateBody", body.as_str()),
    ];
    let named: Vec<(String, String)> = topics
        .map(|topics| {
            topics
                .iter()
                .enumerate()
                .map(|(index, topic)| {
                    (
                        format!("NotificationARNs.member.{}", index + 1),
                        (*topic).to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    for (key, value) in &named {
        members.push((key.as_str(), value.as_str()));
    }
    // An explicitly EMPTY list is stated as the list's own empty member set, which is what
    // an SDK sends for `NotificationARNs=[]`.
    let empty_marker = ("NotificationARNs", "");
    if topics == Some(&[]) {
        members.push(empty_marker);
    }
    cfn_ok(router, case, "CreateChangeSet", &members).await;
    let mut status = String::new();
    for _ in 0..POLLS {
        let body = cfn_ok(
            router,
            case,
            "DescribeChangeSet",
            &[("StackName", stack), ("ChangeSetName", change_set)],
        )
        .await;
        status = element(&body, "Status");
        if status != "CREATE_PENDING" && status != "CREATE_IN_PROGRESS" {
            break;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {case:?}: the change set {change_set:?} must be ready to execute"
    );
    cfn_ok(
        router,
        case,
        "ExecuteChangeSet",
        &[("StackName", stack), ("ChangeSetName", change_set)],
    )
    .await;
    let (status, reported) = settled(router, case, stack).await;
    assert!(
        status.ends_with("_COMPLETE") && !status.starts_with("ROLLBACK"),
        "case {case:?}: executing {change_set:?} must complete the stack, got {status} with \
         {reported}"
    );
}
