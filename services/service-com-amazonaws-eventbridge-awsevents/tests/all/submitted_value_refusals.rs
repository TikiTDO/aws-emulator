// SPDX-License-Identifier: CC0-1.0

//! The submitted VALUES this service refuses, driven over the wire
//! (`awsJson1_1`, `POST /` with `X-Amz-Target: AWSEvents.<Operation>`) through the
//! real `MultiServiceRouter` — so routing, deserialization, the
//! modelled-constraint validator, the handler and serialization are all
//! exercised.
//!
//! Each rule is asserted over a TABLE spanning its class, with the accepted rows
//! beside the refused ones so no rule can pass by refusing everything, and every
//! refusal is read back against the resource it was about: a refused request
//! leaves the stored configuration exactly as it was.
//!
//! The rules:
//!
//! 1. A CUSTOM event bus is not named in the namespace EventBridge reserves for
//!    the buses it names itself; only the partner event bus is, and it is created
//!    by naming the partner event source it matches.
//! 2. A `DeadLetterConfig` names an Amazon SQS queue ("the ARN of the SQS queue
//!    specified as the target for the dead-letter queue"), wherever one is
//!    written: on an event bus, and on a rule's target.
//! 3. A rule's `EventPattern` is within the service's pattern quota, counted over
//!    the characters the request submitted.
//! 4. A `ScheduleExpression` states a schedule in one of the two documented
//!    forms, read field by field — and only on the account's default event bus.
//! 5. A target EventBridge invokes AS the role the target names has to name one;
//!    one invoked under its own resource-based policy does not.
//! 6. A tag key is the caller's own, never one in the reserved `aws:` namespace,
//!    for every operation of this service that writes tags.
//!
//! The non-HTTPS OAuth authorization endpoint, which belongs to the same class of
//! refusal, is pinned by `tests/all/connection_refusal_wording.rs` and
//! `tests/all/connection_authorization.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_eventbridge_awsevents::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below is signed for.
const REGION: &str = "eu-west-2";

/// The pattern quota a rule's `EventPattern` is held to, in characters, as the
/// service publishes it — written out here rather than imported, so a wrong
/// constant in the implementation cannot make a row pass.
const DOCUMENTED_PATTERN_MAX: usize = 2048;

/// The emulator: one router carrying only this service over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let credential = format!(
        "AKIDTESTTESTTEST/20260101/{REGION}/events/aws4_request, SignedHeaders=host, \
         Signature=0000"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "events.eu-west-2.amazonaws.com")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", format!("AWS4-HMAC-SHA256 Credential={credential}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            panic!(
                "{operation} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The wire error code (`__type`), without its namespace; the empty string on a
/// success.
fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> &str {
    body.get("message").and_then(Value::as_str).unwrap_or_default()
}

/// The account every ARN below belongs to.
fn account() -> String {
    engine::emulator_account_id().to_string()
}

/// The ARN of a queue of this account, the one form a dead-letter destination may
/// take.
fn queue_arn(name: &str) -> String {
    format!("arn:aws:sqs:{REGION}:{}:{name}", account())
}

/// An event pattern of exactly `characters` characters: a `source` filter padded
/// out with blanks inside its own JSON, so the padding counts toward the length
/// while the pattern stays one the compiler accepts.
fn pattern_of(characters: usize) -> String {
    let skeleton = json!({ "source": ["com.demo.widgets"] }).to_string();
    let padding = characters.saturating_sub(skeleton.len());
    format!(
        "{}{}{}",
        &skeleton[..skeleton.len() - 1],
        " ".repeat(padding),
        "}"
    )
}

/// A rule carrying `pattern` on the default bus, asserted to be accepted.
async fn given_rule(router: &MultiServiceRouter, name: &str, pattern: &str) {
    let (status, body) = call(
        router,
        "PutRule",
        &json!({ "Name": name, "EventPattern": pattern }),
    )
    .await;
    assert_eq!(status, 200, "PutRule {name} answered {status} {body}");
}

/// What `DescribeRule` reports for `member` of the rule `name`, or the empty
/// string when it reports none.
async fn described_rule(router: &MultiServiceRouter, name: &str, member: &str) -> String {
    let (status, body) = call(router, "DescribeRule", &json!({ "Name": name })).await;
    assert_eq!(status, 200, "DescribeRule {name} answered {status} {body}");
    body[member].as_str().unwrap_or_default().to_string()
}

/// A custom event bus is named neither in the namespace EventBridge reserves for
/// the buses it names itself — only a partner event bus is, and it is created by
/// naming the partner event source it is matched to, not by naming the bus — nor
/// with the character only a partner event bus name may carry.
///
/// The rows differ along WHERE the reserved prefix sits and what follows it: a
/// bare service-style name, a partner-shaped name nobody was offered, the prefix
/// alone — and along the `/` a custom bus name may not contain, against names
/// that merely resemble the prefix (without its dot, with it inside the name) as
/// the accepted controls.
#[tokio::test]
async fn create_event_bus_refuses_a_name_no_custom_bus_may_carry() {
    let router = emulator();
    /// The refusal a name in the reserved namespace answers.
    const RESERVED: &str = "EventBus name starting with 'aws.' is not valid.";
    /// The refusal a custom bus name carrying a `/` answers.
    const SLASHED: &str = "Event bus name must not contain '/'.";
    // (case, the Name, the refusal it answers — `None` for a name this service
    //  takes)
    let rows = [
        ("a service-style reserved name", "aws.mybus", Some(RESERVED)),
        (
            "a partner-shaped name",
            "aws.partner/example.com/1/stream",
            Some(RESERVED),
        ),
        ("the reserved prefix alone", "aws.", Some(RESERVED)),
        ("a deeper reserved name", "aws.ec2.events", Some(RESERVED)),
        ("a name carrying a /", "orders/child", Some(SLASHED)),
        ("a name that is nothing but a /", "/", Some(SLASHED)),
        ("the prefix without its dot", "aws-bus", None),
        ("a name merely starting with aws", "awsome-bus", None),
        ("the prefix inside the name", "my.aws.bus", None),
    ];

    for (case, name, refusal) in rows {
        let (status, body) = call(&router, "CreateEventBus", &json!({ "Name": name })).await;
        let Some(expected) = refusal else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {name:?} to be created, got {status} {body}"
            );
            continue;
        };
        assert_eq!(
            status, 400,
            "case {case:?}: expected {name:?} to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let actual = message(&body);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?}, got {actual:?}"
        );
        // Nothing was created: the name is still free, and no bus answers for it.
        let (status, described) =
            call(&router, "DescribeEventBus", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused create stores no bus, got {status} {described}"
        );
    }
}

/// A `DeadLetterConfig` names an Amazon SQS queue and nothing else, wherever one
/// is written: on a bus (`CreateEventBus`, `UpdateEventBus`) and on a rule's
/// target (`PutTargets`). A refused write leaves the prior configuration in
/// place.
///
/// The rows differ along the service the submitted ARN belongs to — another
/// service whose resource could receive a message (an SNS topic), one whose could
/// not (a Lambda function, a state machine), this service's own event bus — and
/// along text that is no ARN at all, with a queue ARN as the accepted control.
#[tokio::test]
async fn a_dead_letter_destination_is_only_ever_an_sqs_queue() {
    let router = emulator();
    let keeper = queue_arn("bus-dlq");
    // (case, the DeadLetterConfig Arn, whether this service takes it, the refusal
    //  it answers — the service the ARN named, or the ARN form for text that names
    //  no service at all)
    let rows = [
        (
            "an SNS topic",
            format!("arn:aws:sns:{REGION}:{}:spill", account()),
            false,
            "sns is not supported as a dead letter resource.".to_string(),
        ),
        (
            "a Lambda function",
            format!("arn:aws:lambda:{REGION}:{}:function:spill", account()),
            false,
            "lambda is not supported as a dead letter resource.".to_string(),
        ),
        (
            "a Step Functions state machine",
            format!("arn:aws:states:{REGION}:{}:stateMachine:spill", account()),
            false,
            "states is not supported as a dead letter resource.".to_string(),
        ),
        (
            "an event bus of this service",
            format!("arn:aws:events:{REGION}:{}:event-bus/spill", account()),
            false,
            "events is not supported as a dead letter resource.".to_string(),
        ),
        (
            "text that is no ARN at all",
            "spill-queue".to_string(),
            false,
            "Parameter spill-queue is not valid. Reason: Provided Arn is not in correct \
             format."
                .to_string(),
        ),
        ("an SQS queue", queue_arn("spill"), true, String::new()),
    ];

    // The bus every row updates, created with a dead-letter queue the refused rows
    // must leave untouched.
    let (status, body) = call(
        &router,
        "CreateEventBus",
        &json!({ "Name": "dlq-bus", "DeadLetterConfig": { "Arn": keeper } }),
    )
    .await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");

    for (index, (case, arn, accepted, expected_refusal)) in rows.iter().enumerate() {
        // On a create, where the bus is the one the request would bring into being.
        let created = format!("dlq-create-{index}");
        let (status, body) = call(
            &router,
            "CreateEventBus",
            &json!({ "Name": created, "DeadLetterConfig": { "Arn": arn } }),
        )
        .await;
        assert_eq!(
            status,
            if *accepted { 200 } else { 400 },
            "case {case:?}: CreateEventBus answered {status} {body}"
        );

        // On an update, where a refusal has a prior configuration to leave alone.
        let (status, body) = call(
            &router,
            "UpdateEventBus",
            &json!({ "Name": "dlq-bus", "DeadLetterConfig": { "Arn": arn } }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: UpdateEventBus answered {status} {body}"
            );
            assert_eq!(
                body["DeadLetterConfig"]["Arn"],
                json!(arn),
                "case {case:?}: the accepted destination is reported back, got {body}"
            );
            // Put the keeper back, so the next row refuses against it.
            let (status, body) = call(
                &router,
                "UpdateEventBus",
                &json!({ "Name": "dlq-bus", "DeadLetterConfig": { "Arn": keeper } }),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: restoring answered {status} {body}");
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected UpdateEventBus to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let actual = message(&body);
        assert_eq!(
            actual, expected_refusal.as_str(),
            "case {case:?}: expected the dead-letter requirement, got {actual:?}"
        );
        let (status, described) =
            call(&router, "DescribeEventBus", &json!({ "Name": "dlq-bus" })).await;
        assert_eq!(status, 200, "case {case:?}: DescribeEventBus {status} {described}");
        assert_eq!(
            described["DeadLetterConfig"]["Arn"],
            json!(keeper),
            "case {case:?}: a refused update leaves the prior configuration, got {described}"
        );
    }

    // The same rule on a rule's TARGET, where the refusal is of the whole request.
    given_rule(&router, "dlq-rule", r#"{"source":["com.demo.widgets"]}"#).await;
    for (case, arn, accepted, _) in &rows {
        let (status, body) = call(
            &router,
            "PutTargets",
            &json!({
                "Rule": "dlq-rule",
                "Targets": [{
                    "Id": "t1",
                    "Arn": queue_arn("orders"),
                    "DeadLetterConfig": { "Arn": arn },
                }],
            }),
        )
        .await;
        assert_eq!(
            status,
            if *accepted { 200 } else { 400 },
            "case {case:?}: PutTargets answered {status} {body}"
        );
    }
}

/// A rule's `EventPattern` is held to the service's pattern quota, counted over
/// the characters the request submitted — whitespace included, since the quota is
/// on the text and not on a normalisation of it. A refused `PutRule` leaves the
/// rule carrying the pattern it had.
///
/// The rows differ along the length: the quota exactly, one character past it,
/// far past it, and a pattern whose padding is what takes it past — with a short
/// pattern as the accepted control.
#[tokio::test]
async fn put_rule_refuses_an_event_pattern_past_the_pattern_quota() {
    let router = emulator();
    let kept = json!({ "source": ["com.demo.kept"] }).to_string();
    given_rule(&router, "pattern-rule", &kept).await;

    // (case, the pattern, whether this service takes it)
    let rows = [
        ("a short pattern", pattern_of(64), true),
        ("the quota exactly", pattern_of(DOCUMENTED_PATTERN_MAX), true),
        (
            "one character past the quota",
            pattern_of(DOCUMENTED_PATTERN_MAX + 1),
            false,
        ),
        (
            "far past the quota",
            pattern_of(DOCUMENTED_PATTERN_MAX + 900),
            false,
        ),
    ];

    for (case, pattern, accepted) in rows {
        assert_eq!(
            pattern.chars().count(),
            pattern.len(),
            "case {case:?}: the row's pattern is built from one-byte characters"
        );
        let (status, body) = call(
            &router,
            "PutRule",
            &json!({ "Name": "pattern-rule", "EventPattern": pattern }),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a pattern of {} characters to be accepted, got \
                 {status} {body}",
                pattern.len()
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected a pattern of {} characters to be refused, got {status} \
             {body}",
            pattern.len()
        );
        assert_eq!(
            error_code(&body),
            "InvalidEventPatternException",
            "case {case:?}: expected InvalidEventPatternException, got {body}"
        );
        let actual = message(&body);
        assert_eq!(
            actual, "Event pattern is not valid. Reason: Event pattern is too long.",
            "case {case:?}: expected the pattern-length refusal, got {actual:?}"
        );
    }

    // The last accepted row is the pattern the rule still carries: a refused
    // PutRule changed nothing about it.
    let stored = described_rule(&router, "pattern-rule", "EventPattern").await;
    assert_eq!(
        stored,
        pattern_of(DOCUMENTED_PATTERN_MAX),
        "a refused PutRule leaves the rule's pattern as the last accepted one"
    );
}

/// A `ScheduleExpression` states a schedule in one of the two documented forms,
/// read FIELD BY FIELD: a body outside either grammar is refused, and no rule is
/// created for it.
///
/// The rows differ along which part of the grammar the expression breaks: the
/// rate's value, its unit, its agreement in number, the cron field count, each
/// field's own range, and the day-of-month/day-of-week exclusivity — with the
/// documented forms as the accepted controls.
#[tokio::test]
async fn put_rule_reads_a_schedule_expressions_grammar_field_by_field() {
    // The refusal names the parameter alone, which is what this service states
    // for every schedule expression it will not take.
    let expected = "Parameter ScheduleExpression is not valid.";
    // (case, the expression, whether this service takes it)
    let rows = [
        ("a rate whose value is no number", "rate(nonsense)", false),
        ("a rate with no unit", "rate(5)", false),
        ("a rate of no time at all", "rate(0 minutes)", false),
        ("a rate in a unit nothing measures", "rate(5 fortnights)", false),
        ("a plural unit with one", "rate(1 minutes)", false),
        ("a singular unit with five", "rate(5 minute)", false),
        ("a cron with three fields", "cron(1 2 3)", false),
        ("a cron with five fields", "cron(0 20 * * ?)", false),
        ("a cron with seven fields", "cron(0 20 * * ? * *)", false),
        ("a minute past the hour", "cron(60 20 * * ? *)", false),
        ("an hour past the day", "cron(0 24 * * ? *)", false),
        ("a day-of-month past the month", "cron(0 20 32 * ? *)", false),
        ("a month nothing names", "cron(0 20 * NOV30 ? *)", false),
        ("both day fields specified", "cron(0 20 1 * MON *)", false),
        ("both day fields wildcarded", "cron(0 20 * * * *)", false),
        ("the documented rate form", "rate(5 minutes)", true),
        ("a rate of one of its unit", "rate(1 hour)", true),
        ("a rate in days", "rate(2 days)", true),
        ("the documented cron form", "cron(0 20 * * ? *)", true),
        ("a cron naming days of the week", "cron(0 18 ? * MON-FRI *)", true),
        ("a cron naming a day of the month", "cron(0 8 1 * ? *)", true),
    ];

    for (index, (case, expression, accepted)) in rows.iter().enumerate() {
        let router = emulator();
        let rule = format!("scheduled-{index}");
        let (status, body) = call(
            &router,
            "PutRule",
            &json!({ "Name": rule, "ScheduleExpression": expression }),
        )
        .await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected {expression:?} to be accepted, got {status} {body}"
            );
            let stored = described_rule(&router, &rule, "ScheduleExpression").await;
            assert_eq!(
                stored, *expression,
                "case {case:?}: the rule reports back the expression it was given"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected {expression:?} to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            message(&body),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message(&body)
        );
        let (status, described) = call(&router, "DescribeRule", &json!({ "Name": rule })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused PutRule creates no rule, got {status} {described}"
        );
    }
}

/// A scheduled rule lives on the account's default event bus only: EventBridge
/// itself triggers such a rule, and it triggers none on any other bus, so a
/// schedule asked for elsewhere is refused rather than stored as a rule nothing
/// would ever fire.
///
/// The rows differ along how the bus is named (not at all, as the default by
/// name, as a custom bus) and along what the rule is triggered by (a schedule, a
/// pattern, both).
#[tokio::test]
async fn a_schedule_is_accepted_only_on_the_default_event_bus() {
    let router = emulator();
    let (status, body) = call(&router, "CreateEventBus", &json!({ "Name": "orders" })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
    let pattern = json!({ "source": ["com.demo.widgets"] }).to_string();

    // (case, the EventBusName member, whether the rule carries a schedule,
    //  whether it carries a pattern, whether this service takes it)
    let rows = [
        ("a schedule with no bus named", None, true, false, true),
        ("a schedule on the default bus", Some("default"), true, false, true),
        ("a schedule on a custom bus", Some("orders"), true, false, false),
        (
            "a schedule and a pattern on a custom bus",
            Some("orders"),
            true,
            true,
            false,
        ),
        ("a pattern on a custom bus", Some("orders"), false, true, true),
    ];

    for (index, (case, bus, scheduled, matched, accepted)) in rows.iter().enumerate() {
        let rule = format!("bus-scheduled-{index}");
        let mut request = json!({ "Name": rule });
        if let Some(bus) = bus {
            request["EventBusName"] = json!(bus);
        }
        if *scheduled {
            request["ScheduleExpression"] = json!("rate(5 minutes)");
        }
        if *matched {
            request["EventPattern"] = json!(pattern);
        }
        let (status, body) = call(&router, "PutRule", &request).await;
        if *accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the rule to be written, got {status} {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected the rule to be refused, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        let actual = message(&body);
        assert_eq!(
            actual, "ScheduleExpression is supported only on the default event bus.",
            "case {case:?}: expected the default-bus requirement, got {actual:?}"
        );
        let mut describe = json!({ "Name": rule });
        if let Some(bus) = bus {
            describe["EventBusName"] = json!(bus);
        }
        let (status, described) = call(&router, "DescribeRule", &describe).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a refused PutRule creates no rule, got {status} {described}"
        );
    }
}

/// A target of a class EventBridge invokes AS the role the target names has to
/// name one: "for Lambda and Amazon SNS resources, EventBridge relies on
/// resource-based policies; for EC2 instances, Kinesis Data Streams, Step
/// Functions state machines and API Gateway APIs, EventBridge relies on IAM roles
/// that you specify in the `RoleARN` argument in `PutTargets`" — and the event bus
/// of another account is in that second class too. A refused request leaves the
/// rule's targets as they were.
///
/// The rows differ along the SERVICE the target belongs to, spanning the classes
/// that documentation enumerates (a state machine, an event bus, a data stream,
/// an EC2 instance, an API Gateway API) against the targets it does not name (a
/// queue, a topic, a function, a log group) and against another resource of a
/// service that is in the list (an activity of Step Functions, an api destination
/// of this service), which is not one of them.
#[tokio::test]
async fn put_targets_requires_a_role_for_the_targets_it_assumes_one_for() {
    let router = emulator();
    let account = account();
    // (case, the target Arn, whether a RoleArn is required for it)
    let rows = [
        (
            "a Step Functions state machine",
            format!("arn:aws:states:{REGION}:{account}:stateMachine:flow"),
            true,
        ),
        (
            "an event bus of this service",
            format!("arn:aws:events:{REGION}:{account}:event-bus/other"),
            true,
        ),
        (
            "a Kinesis data stream",
            format!("arn:aws:kinesis:{REGION}:{account}:stream/orders"),
            true,
        ),
        (
            "an EC2 instance",
            format!("arn:aws:ec2:{REGION}:{account}:instance/i-0123456789abcdef0"),
            true,
        ),
        (
            "an API Gateway API",
            format!("arn:aws:execute-api:{REGION}:{account}:abc123/prod/POST/orders"),
            true,
        ),
        (
            // Another resource of a service the list names, which the list does
            // not name: an activity is not a state machine.
            "a Step Functions activity",
            format!("arn:aws:states:{REGION}:{account}:activity:handoff"),
            false,
        ),
        (
            // This service's own HTTP endpoint, invoked through the credentials
            // its connection holds rather than as a role the target names.
            "an api destination of this service",
            format!("arn:aws:events:{REGION}:{account}:api-destination/hook/0000-1111"),
            false,
        ),
        (
            "a Firehose delivery stream",
            format!("arn:aws:firehose:{REGION}:{account}:deliverystream/orders"),
            false,
        ),
        ("an SQS queue", queue_arn("orders"), false),
        (
            "an SNS topic",
            format!("arn:aws:sns:{REGION}:{account}:orders"),
            false,
        ),
        (
            "a Lambda function",
            format!("arn:aws:lambda:{REGION}:{account}:function:orders"),
            false,
        ),
        (
            "a CloudWatch Logs log group",
            format!("arn:aws:logs:{REGION}:{account}:log-group:/demo:*"),
            false,
        ),
    ];
    let role = format!("arn:aws:iam::{account}:role/events-invoke");

    for (index, (case, arn, role_required)) in rows.iter().enumerate() {
        let rule = format!("role-rule-{index}");
        given_rule(&router, &rule, r#"{"source":["com.demo.widgets"]}"#).await;
        let (status, body) = call(
            &router,
            "PutTargets",
            &json!({
                "Rule": rule,
                "Targets": [{ "Id": "kept", "Arn": queue_arn("kept") }],
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: PutTargets answered {status} {body}");

        // Without a RoleArn: refused for the kinds EventBridge must assume one
        // for, accepted for the kinds invoked under their own policy.
        let (status, body) = call(
            &router,
            "PutTargets",
            &json!({ "Rule": rule, "Targets": [{ "Id": "t1", "Arn": arn }] }),
        )
        .await;
        if *role_required {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a target with no RoleArn to be refused, got {status} \
                 {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            let actual = message(&body);
            assert_eq!(
                actual,
                format!("RoleArn is required for target {arn}."),
                "case {case:?}: expected the RoleArn requirement naming {arn:?}, got {actual:?}"
            );
            let (status, listed) =
                call(&router, "ListTargetsByRule", &json!({ "Rule": rule })).await;
            assert_eq!(status, 200, "case {case:?}: ListTargetsByRule {status} {listed}");
            let ids: Vec<String> = listed["Targets"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|stored| stored["Id"].as_str().unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                ids,
                vec!["kept".to_string()],
                "case {case:?}: a refused request leaves the rule's targets alone, got {ids:?}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: expected a target needing no role to be stored, got {status} \
                 {body}"
            );
        }

        // With a RoleArn: every kind is accepted, so the rows above are refused
        // for the member they omitted and nothing else.
        let (status, body) = call(
            &router,
            "PutTargets",
            &json!({
                "Rule": rule,
                "Targets": [{ "Id": "t1", "Arn": arn, "RoleArn": role }],
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a target naming a role to be stored, got {status} {body}"
        );
    }
}

/// A tag key is the caller's own: a key in the namespace reserved for Amazon Web
/// Services' own tags is refused by every operation of this service that writes
/// tags, and the resource's tags are left as they were.
///
/// The rows differ along the spelling of the prefix (lower case, upper case,
/// mixed) and along what follows it, against keys that merely resemble it (the
/// prefix without its colon, the prefix inside the key) as the accepted controls.
#[tokio::test]
async fn a_tag_write_refuses_a_key_in_the_reserved_namespace() {
    let router = emulator();
    let (status, body) = call(&router, "CreateEventBus", &json!({ "Name": "tagged" })).await;
    assert_eq!(status, 200, "CreateEventBus answered {status} {body}");
    let bus = format!("arn:aws:events:{REGION}:{}:event-bus/tagged", account());
    let (status, body) = call(
        &router,
        "TagResource",
        &json!({ "ResourceARN": bus, "Tags": [{ "Key": "owner", "Value": "ada" }] }),
    )
    .await;
    assert_eq!(status, 200, "TagResource answered {status} {body}");

    // (case, the tag key, whether this service stores it)
    let rows = [
        ("the reserved prefix", "aws:foo", false),
        ("the reserved prefix in upper case", "AWS:foo", false),
        ("the reserved prefix in mixed case", "Aws:cloudformation", false),
        ("the reserved prefix alone", "aws:", false),
        ("the prefix without its colon", "aws-foo", true),
        ("a key merely starting with aws", "awsome", true),
        ("the prefix inside the key", "mine:aws:foo", true),
    ];

    for (index, (case, key, accepted)) in rows.iter().enumerate() {
        let tags = json!([{ "Key": key, "Value": "v" }]);
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({ "ResourceARN": bus, "Tags": tags }),
        )
        .await;
        assert_eq!(
            status,
            if *accepted { 200 } else { 400 },
            "case {case:?}: TagResource answered {status} {body}"
        );
        if !accepted {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
            let actual = message(&body);
            assert_eq!(
                actual, "Tag keys starting with 'aws:' are reserved for AWS use.",
                "case {case:?}: expected the reserved-key refusal, got {actual:?}"
            );
        }

        // The same key through the two other operations of this service that write
        // tags: a create and a rule write.
        let (status, body) = call(
            &router,
            "CreateEventBus",
            &json!({ "Name": format!("tagged-{index}"), "Tags": tags }),
        )
        .await;
        assert_eq!(
            status,
            if *accepted { 200 } else { 400 },
            "case {case:?}: CreateEventBus answered {status} {body}"
        );
        let (status, body) = call(
            &router,
            "PutRule",
            &json!({
                "Name": format!("tagged-rule-{index}"),
                "EventPattern": r#"{"source":["com.demo.widgets"]}"#,
                "Tags": tags,
            }),
        )
        .await;
        assert_eq!(
            status,
            if *accepted { 200 } else { 400 },
            "case {case:?}: PutRule answered {status} {body}"
        );
    }

    // Every refused write left the bus carrying exactly the tags the accepted
    // writes put there.
    let (status, listed) =
        call(&router, "ListTagsForResource", &json!({ "ResourceARN": bus })).await;
    assert_eq!(status, 200, "ListTagsForResource answered {status} {listed}");
    let mut keys: Vec<String> = listed["Tags"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|tag| tag["Key"].as_str().unwrap_or_default().to_string())
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            "aws-foo".to_string(),
            "awsome".to_string(),
            "mine:aws:foo".to_string(),
            "owner".to_string(),
        ],
        "a refused tag write leaves the resource's tags alone, got {keys:?}"
    );
}
