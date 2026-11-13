// SPDX-License-Identifier: CC0-1.0

//! Every alarm STATE TRANSITION publishes a `CloudWatch Alarm State Change` event onto the
//! account's default event bus, end to end over the wire through the real
//! `MultiServiceRouter` with CloudWatch, EventBridge and Amazon SQS on one engine — so what a
//! client sees is what is asserted: the envelope on the queue a matching rule targets.
//!
//! The RULE asserted here: when an alarm's `StateValue` changes, CloudWatch publishes one event
//! with `source` `aws.cloudwatch`, `detail-type` `CloudWatch Alarm State Change`, the alarm's
//! own ARN in `resources` and a `detail` naming the alarm and carrying the state it moved to,
//! the state it moved out of and the configuration that was evaluated; a rule whose
//! `EventPattern` matches that event delivers it to the rule's targets, and a rule whose pattern
//! names another alarm receives nothing. The publication is owed by the TRANSITION and not by
//! the alarm's action list, so an alarm that carries no action — and one whose actions are
//! switched off — publishes it just the same.
//!
//! The rows differ along what the rule generalises over: the KIND of alarm that moved (a metric
//! alarm, a composite alarm), the pair of states the transition is between, what MADE it move
//! (a state a caller forced, and a datapoint the alarm's own evaluation graded), and whether the
//! alarm carries any action at all.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{given_queue, graded_window_stamp, received_messages, rpc, Emulator, REGION};

/// The `source` every event below is matched on.
const SOURCE: &str = "aws.cloudwatch";

/// The `detail-type` of an alarm state change.
const DETAIL_TYPE: &str = "CloudWatch Alarm State Change";

/// The service principal EventBridge delivers to an SQS target as, which the queue's own policy
/// has to allow the send for.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";

/// The namespace and metric the metric alarms below watch.
const NAMESPACE: &str = "Custom/StateChange";
const METRIC: &str = "Backlog";

/// The period those alarms aggregate over, as the modelled seconds member carries it.
const PERIOD: i64 = 60;

/// How long this suite waits for work the clock owes — the asynchronous publication of an event
/// and its delivery to a target — before giving up on it. Generous on purpose: nothing here is
/// asserted about how fast the host runs, only about what eventually arrives.
const WORK_WAIT: Duration = Duration::from_secs(60);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(100);

/// The state every new alarm starts in.
const INSUFFICIENT_DATA: &str = "INSUFFICIENT_DATA";

/// CloudWatch, EventBridge and Amazon SQS over ONE shared engine, as `core/router/src/lib.rs`
/// mounts them: the event CloudWatch publishes reaches EventBridge's own handler, which matches
/// it against this account's rules and delivers it to the queue this suite reads.
fn emulator_with_a_bus() -> Emulator {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    Emulator { router, engine }
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
///
/// EventBridge serves `awsJson1_1` where this service serves `awsJson1_0`, so the call is made
/// here rather than through the shared RPC helper: the content type is part of the wire.
async fn eventbridge(emulator: &Emulator, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTTESTTEST/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = emulator.router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One CloudWatch call that must answer 200.
async fn monitoring(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(
        emulator,
        "monitoring",
        crate::harness::TARGET_PREFIX,
        operation,
        body,
    )
    .await
}

/// A metric alarm over this suite's own metric, dimensioned by the alarm's own name so one
/// row's datapoints are never another row's.
async fn put_metric_alarm(
    emulator: &Emulator,
    name: &str,
    actions: &[String],
    actions_enabled: bool,
) {
    monitoring(
        emulator,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "AlarmDescription": "the alarm of this row",
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": [{"Name": "QueueName", "Value": name}],
            "Statistic": "Average",
            "Period": PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            // "ignore — the current alarm state is maintained"
            // (`PutMetricAlarmInput$TreatMissingData`): an alarm with no datapoint of its own
            // series keeps the state it is in, so the only transitions these rows see are the
            // ones they drive.
            "TreatMissingData": "ignore",
            "AlarmActions": actions,
            "ActionsEnabled": actions_enabled,
        }),
    )
    .await;
}

/// A queue a rule can target: its url, with its own policy allowing EventBridge the send — the
/// grant AWS documents for a queue a rule delivers to, and what a queue needs to receive
/// anything at all.
async fn given_target_queue(emulator: &Emulator, name: &str) -> String {
    let (url, arn) = given_queue(emulator, name).await;
    rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "SetQueueAttributes",
        &json!({
            "QueueUrl": url,
            "Attributes": {"Policy": json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Sid": "AllowEventBridgeDelivery",
                    "Effect": "Allow",
                    "Principal": {"Service": EVENTS_PRINCIPAL},
                    "Action": "sqs:SendMessage",
                    "Resource": arn,
                }],
            })
            .to_string()},
        }),
    )
    .await;
    url
}

/// Wire a rule matching the state changes of the alarm `alarm_name` to the queue at `url`.
async fn given_rule_for_alarm(
    emulator: &Emulator,
    case: &str,
    rule_name: &str,
    alarm_name: &str,
    queue_url: &str,
) {
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [DETAIL_TYPE],
        "detail": {"alarmName": [alarm_name]},
    })
    .to_string();
    eventbridge(
        emulator,
        case,
        "PutRule",
        &json!({"Name": rule_name, "EventPattern": pattern, "State": "ENABLED"}),
    )
    .await;
    let queue_arn = queue_arn_of(emulator, queue_url).await;
    let wired = eventbridge(
        emulator,
        case,
        "PutTargets",
        &json!({"Rule": rule_name, "Targets": [{"Id": "q", "Arn": queue_arn}]}),
    )
    .await;
    assert_eq!(
        wired["FailedEntryCount"],
        json!(0),
        "case {case:?}: expected the target to be accepted, got {wired}"
    );
}

/// The ARN of the queue at `url`, as that service reports it.
async fn queue_arn_of(emulator: &Emulator, url: &str) -> String {
    let attributes = rpc(
        emulator,
        "sqs",
        "AmazonSQS",
        "GetQueueAttributes",
        &json!({"QueueUrl": url, "AttributeNames": ["QueueArn"]}),
    )
    .await;
    attributes["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("GetQueueAttributes {url}: no QueueArn in {attributes}"))
        .to_string()
}

/// Take whatever has been delivered to the queue at `url` and add it to `held`, parsed out of
/// the message bodies — accumulated across looks, because a received message is invisible to
/// the next look.
async fn collect(emulator: &Emulator, url: &str, held: &mut Vec<Value>) {
    for message in received_messages(emulator, url).await {
        let body = message["Body"].as_str().unwrap_or_default();
        if let Ok(parsed) = serde_json::from_str::<Value>(body) {
            held.push(parsed);
        }
    }
}

/// Whether `held` already holds an event reporting the state `state`.
fn holds_state(held: &[Value], state: &str) -> bool {
    held.iter()
        .any(|event| event.pointer("/detail/state/value") == Some(&json!(state)))
}

/// Wait until an event whose `detail.state.value` is `state` has reached the queue at `url`,
/// adding everything delivered meanwhile to `held`.
///
/// The wait is for the CONDITION rather than for a span: it looks again until the event is
/// there or [`WORK_WAIT`] has passed.
async fn until_state(emulator: &Emulator, url: &str, state: &str, held: &mut Vec<Value>) {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        collect(emulator, url, held).await;
        if holds_state(held, state) {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    collect(emulator, url, held).await;
}

/// The alarm `DescribeAlarms` reports for `name` under `kind`.
async fn described(emulator: &Emulator, kind: &str, name: &str) -> Value {
    let described = monitoring(
        emulator,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": [kind]}),
    )
    .await;
    let member = if kind == COMPOSITE_KIND {
        "CompositeAlarms"
    } else {
        "MetricAlarms"
    };
    described[member][0].clone()
}

/// Wait until `DescribeAlarms` reports the alarm `name` in `state`, which is how a row that
/// drives a transition OUT of an evaluated state knows the alarm has reached it.
async fn until_alarm_reports(emulator: &Emulator, case: &str, kind: &str, name: &str, state: &str) {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        if described(emulator, kind, name).await["StateValue"] == json!(state) {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    let reported = described(emulator, kind, name).await;
    panic!("case {case:?}: expected the alarm {name:?} to reach {state:?}, got {reported}");
}

/// The two `AlarmType` values a transition can belong to, as the reader names them.
const METRIC_KIND: &str = "MetricAlarm";
const COMPOSITE_KIND: &str = "CompositeAlarm";

/// What makes the alarm of a row move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Driven {
    /// States a caller forces with `SetAlarmState`, in order.
    Forced(&'static [&'static str]),
    /// A datapoint published past the alarm's threshold, which the alarm's own evaluation
    /// grades.
    ABreachingDatapoint,
}

/// One row: which kind of alarm moved, what moved it, and the transition the delivered event
/// must report.
struct Row {
    case: &'static str,
    composite: bool,
    /// Whether the alarm carries an action for the state it moves to at all.
    with_an_action: bool,
    /// Whether its `ActionsEnabled` is switched off.
    actions_enabled: bool,
    driven: Driven,
    expected_state: &'static str,
    expected_previous: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a metric alarm forced into ALARM",
            composite: false,
            with_an_action: true,
            actions_enabled: true,
            driven: Driven::Forced(&["ALARM"]),
            expected_state: "ALARM",
            expected_previous: INSUFFICIENT_DATA,
        },
        Row {
            case: "a metric alarm that moves back to OK",
            composite: false,
            with_an_action: true,
            actions_enabled: true,
            driven: Driven::Forced(&["ALARM", "OK"]),
            expected_state: "OK",
            expected_previous: "ALARM",
        },
        Row {
            case: "a metric alarm that moves to INSUFFICIENT_DATA",
            composite: false,
            with_an_action: true,
            actions_enabled: true,
            driven: Driven::Forced(&["OK", INSUFFICIENT_DATA]),
            expected_state: INSUFFICIENT_DATA,
            expected_previous: "OK",
        },
        Row {
            case: "a metric alarm carrying no action at all",
            composite: false,
            with_an_action: false,
            actions_enabled: true,
            driven: Driven::Forced(&["ALARM"]),
            expected_state: "ALARM",
            expected_previous: INSUFFICIENT_DATA,
        },
        Row {
            case: "a metric alarm whose actions are switched off",
            composite: false,
            with_an_action: true,
            actions_enabled: false,
            driven: Driven::Forced(&["ALARM"]),
            expected_state: "ALARM",
            expected_previous: INSUFFICIENT_DATA,
        },
        Row {
            case: "a metric alarm its own evaluation moved on a breaching datapoint",
            composite: false,
            with_an_action: true,
            actions_enabled: true,
            driven: Driven::ABreachingDatapoint,
            expected_state: "ALARM",
            expected_previous: INSUFFICIENT_DATA,
        },
        Row {
            case: "a composite alarm forced into ALARM",
            composite: true,
            with_an_action: true,
            actions_enabled: true,
            driven: Driven::Forced(&["ALARM"]),
            // A composite alarm's own rule evaluation moves it out of INSUFFICIENT_DATA as
            // soon as it is due — its child is in no ALARM state, so the rule is false and the
            // alarm reports OK — and that is the state this row's force moves it out of.
            expected_state: "ALARM",
            expected_previous: "OK",
        },
        Row {
            case: "a composite alarm that moves back to OK",
            composite: true,
            with_an_action: true,
            actions_enabled: true,
            driven: Driven::Forced(&["ALARM", "OK"]),
            expected_state: "OK",
            expected_previous: "ALARM",
        },
    ]
}

/// RULE: every change of an alarm's state publishes one `CloudWatch Alarm State Change` event
/// naming the alarm, the state it moved to and the state it moved out of, which a rule matching
/// that alarm delivers — and a rule matching another alarm receives nothing.
#[tokio::test]
async fn every_alarm_state_transition_publishes_a_state_change_event() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let emulator = emulator_with_a_bus();
        let alarm = format!("state-change-{index}");
        let other_alarm = format!("state-change-other-{index}");
        let kind = if row.composite {
            COMPOSITE_KIND
        } else {
            METRIC_KIND
        };

        let matching = given_target_queue(&emulator, &format!("state-change-q-{index}")).await;
        let near_miss =
            given_target_queue(&emulator, &format!("state-change-near-miss-q-{index}")).await;
        // An action the alarm carries, for the rows that carry one: a topic this emulator does
        // not serve, because what is asserted here is the EVENT and not the action — an action
        // that cannot be executed must not keep the event from being published.
        let actions: Vec<String> = if row.with_an_action {
            vec![format!("arn:aws:sns:{REGION}:000000000000:state-change-{index}")]
        } else {
            Vec::new()
        };

        if row.composite {
            // A composite alarm's rule reads the states of other alarms, so it needs a child to
            // reference; the child is what makes the rule well formed, and the parent's own
            // forced state is what this row drives.
            let child = format!("state-change-child-{index}");
            put_metric_alarm(&emulator, &child, &[], true).await;
            monitoring(
                &emulator,
                "PutCompositeAlarm",
                &json!({
                    "AlarmName": alarm,
                    "AlarmRule": format!("ALARM(\"{child}\")"),
                    "AlarmDescription": "the composite alarm of this row",
                    "AlarmActions": actions,
                    "ActionsEnabled": row.actions_enabled,
                }),
            )
            .await;
        } else {
            put_metric_alarm(&emulator, &alarm, &actions, row.actions_enabled).await;
        }
        // The near-miss rule's own alarm, so its pattern names an alarm this account really
        // holds and the only reason it receives nothing is that this alarm did not move.
        put_metric_alarm(&emulator, &other_alarm, &[], true).await;

        let arn = described(&emulator, kind, &alarm).await["AlarmArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the alarm {alarm:?} reports no AlarmArn"))
            .to_string();
        given_rule_for_alarm(
            &emulator,
            case,
            &format!("state-change-rule-{index}"),
            &alarm,
            &matching,
        )
        .await;
        given_rule_for_alarm(
            &emulator,
            case,
            &format!("state-change-near-miss-{index}"),
            &other_alarm,
            &near_miss,
        )
        .await;

        let mut held: Vec<Value> = Vec::new();
        if row.composite {
            // The evaluated state the force below moves the alarm out of: waited for, so the
            // transition this row asserts is between the two states it names and not between
            // whichever two the clock happened to produce first.
            until_alarm_reports(&emulator, case, kind, &alarm, "OK").await;
            until_state(&emulator, &matching, "OK", &mut held).await;
            held.clear();
        }

        match row.driven {
            Driven::Forced(states) => {
                for state in states {
                    monitoring(
                        &emulator,
                        "SetAlarmState",
                        &json!({
                            "AlarmName": alarm,
                            "StateValue": state,
                            "StateReason": "driven by the test",
                        }),
                    )
                    .await;
                    // Each forced state is its own transition, and the next force moves out of
                    // the state this one left: the row waits for each in turn so the two are
                    // never read as one.
                    until_state(&emulator, &matching, state, &mut held).await;
                }
            }
            Driven::ABreachingDatapoint => {
                // A datapoint is graded in the window its own time stamp falls in, and an
                // alarm evaluates the windows that have CLOSED — so the breaching datapoint is
                // stamped into the newest closed window and published again on every look,
                // which keeps the row's drive independent of how many period boundaries the
                // clock crosses while it waits.
                let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
                for _ in 0..looks {
                    monitoring(
                        &emulator,
                        "PutMetricData",
                        &json!({
                            "Namespace": NAMESPACE,
                            "MetricData": [{
                                "MetricName": METRIC,
                                "Dimensions": [{"Name": "QueueName", "Value": alarm}],
                                "Timestamp": graded_window_stamp(PERIOD as f64),
                                "Value": 10.0,
                            }],
                        }),
                    )
                    .await;
                    collect(&emulator, &matching, &mut held).await;
                    if holds_state(&held, row.expected_state) {
                        break;
                    }
                    tokio::time::sleep(LOOK_INTERVAL).await;
                }
            }
        }

        // The LAST event reporting the expected state is the transition this row drove: a row
        // that passes through a state twice reports it twice, and the one asserted is the one
        // its own drive produced.
        let reported = held
            .iter()
            .rev()
            .find(|event| event.pointer("/detail/state/value") == Some(&json!(row.expected_state)))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an event reporting the state {:?} on the matching \
                     rule's queue, got {held:?}",
                    row.expected_state
                )
            });

        for (member, pointer, expected) in [
            ("the event's source", "/source", json!(SOURCE)),
            ("the event's detail-type", "/detail-type", json!(DETAIL_TYPE)),
            ("the alarm's name", "/detail/alarmName", json!(alarm)),
            (
                "the state it moved to",
                "/detail/state/value",
                json!(row.expected_state),
            ),
            (
                "the state it moved out of",
                "/detail/previousState/value",
                json!(row.expected_previous),
            ),
        ] {
            let found = reported.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                found, expected,
                "case {case:?}: expected {member} to be {expected} at {pointer}, got {found} in \
                 {reported}"
            );
        }
        // "AWS resources, identified by Amazon Resource Name (ARN), which the event primarily
        // concerns": the alarm that moved.
        let resources = reported["resources"].as_array().cloned().unwrap_or_default();
        assert!(
            resources.contains(&json!(arn)),
            "case {case:?}: expected the alarm's ARN {arn:?} in the event's resources, got \
             {resources:?} in {reported}"
        );
        // What was evaluated: a composite alarm reports the rule it applied, a metric alarm the
        // metric it graded.
        let configuration = reported
            .pointer("/detail/configuration")
            .cloned()
            .unwrap_or(Value::Null);
        if row.composite {
            assert!(
                configuration["alarmRule"].is_string(),
                "case {case:?}: expected the composite alarm's rule in the event's \
                 configuration, got {configuration}"
            );
        } else {
            let name = configuration
                .pointer("/metrics/0/metricStat/metric/name")
                .cloned()
                .unwrap_or(Value::Null);
            assert_eq!(
                name,
                json!(METRIC),
                "case {case:?}: expected the evaluated metric {METRIC:?} in the event's \
                 configuration, got {configuration}"
            );
        }
        // Every event this rule received is about ITS alarm: the pattern is what selects them.
        for event in &held {
            assert_eq!(
                event.pointer("/detail/alarmName"),
                Some(&json!(alarm)),
                "case {case:?}: the matching rule received an event about another alarm: {event}"
            );
        }
        // And the rule whose pattern names an alarm that did not move received nothing, however
        // long the delivery above was given.
        let mut missed: Vec<Value> = Vec::new();
        collect(&emulator, &near_miss, &mut missed).await;
        assert!(
            missed.is_empty(),
            "case {case:?}: expected the rule matching another alarm to receive nothing, got \
             {missed:?}"
        );
    }
}
