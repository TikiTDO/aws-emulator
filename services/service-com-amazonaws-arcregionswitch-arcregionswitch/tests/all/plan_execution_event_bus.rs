// SPDX-License-Identifier: CC0-1.0

//! What EventBridge is told when a Region switch starts.
//!
//! THE RULE UNDER TEST: starting a plan execution is PUBLISHED onto the account's default
//! event bus, in the Region the execution runs in, under the `source` this service owns
//! and the `detail-type` of a plan execution, naming the plan in the envelope's
//! `resources` and the execution in `detail.executionId`. So a rule on that Region's bus
//! whose `EventPattern` matches that source and detail-type delivers the event to that
//! rule's targets; a rule on another Region's bus, and a rule written for another
//! service's events, receive nothing — and a call that started nothing (a refused start,
//! or a replay of a `clientToken`, which answers with the execution the FIRST use
//! started) publishes nothing at all.
//!
//! Everything is driven over the real wire path: this service, EventBridge and Amazon SQS
//! are mounted on ONE shared engine, each Region's rule and target are created through
//! EventBridge's and SQS's own APIs, and the delivered message is read back through SQS's.

use serde_json::{json, Value};
use service_com_amazonaws_arcregionswitch_arcregionswitch::state::execution_events::{
    EVENT_SOURCE, PLAN_EXECUTION_DETAIL_TYPE,
};

use crate::switch_support::{
    approval_step, error_code, lambda_step, plan_with_steps, Emulator, ACCOUNT, DELIVERY_DEADLINE,
    POLL_INTERVAL,
};

/// The two Regions every plan below covers, which are the two Regions an execution of one
/// can run in.
const REGIONS: [&str; 2] = ["us-east-1", "us-west-2"];

/// An SQS queue called `name` in `region`; answers its url and the ARN a rule names it by.
async fn given_queue(emulator: &Emulator, region: &str, name: &str) -> (String, String) {
    let (status, body) = emulator
        .sqs(region, "CreateQueue", &json!({ "QueueName": name }))
        .await;
    assert_eq!(status, 200, "CreateQueue answered {status} {body}");
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueUrl in {body}"))
        .to_string();
    let (status, body) = emulator
        .sqs(
            region,
            "GetQueueAttributes",
            &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
        )
        .await;
    assert_eq!(status, 200, "GetQueueAttributes answered {status} {body}");
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// Grant `events.amazonaws.com` the `sqs:SendMessage` a rule's target needs on the queue
/// at `url`: EventBridge makes that call as its own service principal and relies on the
/// queue's resource-based policy.
async fn grant_delivery(
    emulator: &Emulator,
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
    })
    .to_string();
    let (status, body) = emulator
        .sqs(
            region,
            "SetQueueAttributes",
            &json!({ "QueueUrl": url, "Attributes": { "Policy": policy } }),
        )
        .await;
    assert_eq!(status, 200, "SetQueueAttributes answered {status} {body}");
}

/// Put the rule `name` with `pattern` on the bus of `region` and point it at `queue_arn`;
/// answers the rule's ARN.
async fn given_rule(
    emulator: &Emulator,
    region: &str,
    name: &str,
    pattern: Value,
    queue_arn: &str,
) -> String {
    let (status, body) = emulator
        .events(
            region,
            "PutRule",
            &json!({ "Name": name, "EventPattern": pattern.to_string(), "State": "ENABLED" }),
        )
        .await;
    assert_eq!(status, 200, "PutRule answered {status} {body}");
    let rule_arn = body["RuleArn"]
        .as_str()
        .unwrap_or_else(|| panic!("no RuleArn in {body}"))
        .to_string();
    let (status, body) = emulator
        .events(
            region,
            "PutTargets",
            &json!({ "Rule": name, "Targets": [{ "Id": "sqs-target", "Arn": queue_arn }] }),
        )
        .await;
    assert_eq!(status, 200, "PutTargets answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutTargets reported a failed entry: {body}"
    );
    rule_arn
}

/// A queue in `region` behind a rule of that Region's bus matching `pattern`; answers the
/// queue's url.
async fn given_target(emulator: &Emulator, region: &str, name: &str, pattern: Value) -> String {
    let (url, queue_arn) = given_queue(emulator, region, name).await;
    let rule_arn = given_rule(emulator, region, name, pattern, &queue_arn).await;
    grant_delivery(emulator, region, &url, &queue_arn, &rule_arn).await;
    url
}

/// The pattern a rule for this service's plan-execution events carries.
fn plan_execution_pattern() -> Value {
    json!({ "source": [EVENT_SOURCE], "detail-type": [PLAN_EXECUTION_DETAIL_TYPE] })
}

/// Every message waiting on the queue at `url` in `region`, as parsed event envelopes.
async fn drain(emulator: &Emulator, region: &str, url: &str) -> Vec<Value> {
    let (status, body) = emulator
        .sqs(
            region,
            "ReceiveMessage",
            &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
        )
        .await;
    assert_eq!(status, 200, "ReceiveMessage answered {status} {body}");
    body["Messages"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|message| {
            serde_json::from_str::<Value>(message["Body"].as_str().unwrap_or_default())
                .unwrap_or(Value::Null)
        })
        .collect()
}

/// Poll the queue until it has delivered at least `wanted` messages, or the generous
/// [`DELIVERY_DEADLINE`] passes. Waits for the CONDITION, so a fast machine never waits.
async fn delivered(emulator: &Emulator, region: &str, url: &str, wanted: usize) -> Vec<Value> {
    let deadline = std::time::Instant::now() + DELIVERY_DEADLINE;
    let mut collected: Vec<Value> = Vec::new();
    loop {
        collected.extend(drain(emulator, region, url).await);
        if collected.len() >= wanted || std::time::Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One started execution, and the Region whose bus has to carry its event.
struct Started {
    case: &'static str,
    plan_arn: String,
    execution_id: String,
    region: &'static str,
}

/// One row of the publication table: the Region the execution runs in, the action it
/// performs, and the workflow that action runs.
struct Row {
    case: &'static str,
    region: &'static str,
    action: &'static str,
    steps: fn() -> Vec<Value>,
}

#[tokio::test]
async fn starting_an_execution_is_delivered_through_the_default_bus_of_its_region() {
    let emulator = Emulator::with_events();

    // One queue behind a matching rule on EACH Region's bus, so the Region an event is
    // published in is observable; and one behind a rule that cannot match anything this
    // service publishes.
    let mut targets: Vec<(&str, String)> = Vec::new();
    for region in REGIONS {
        let url = given_target(
            &emulator,
            region,
            &format!("region-switch-plan-executions-{region}"),
            plan_execution_pattern(),
        )
        .await;
        targets.push((region, url));
    }
    let unrelated = given_target(
        &emulator,
        REGIONS[0],
        "another-services-events",
        json!({ "source": ["aws.ecs"], "detail-type": ["ECS Task State Change"] }),
    )
    .await;

    // The rows differ along what the rule generalises over: the Region the execution runs
    // in, the action it performs and the workflow it runs — every started execution is
    // published, whatever it then does.
    let rows: [Row; 4] = [
        Row {
            case: "activate the second Region, a workflow waiting for an approval",
            region: "us-west-2",
            action: "activate",
            steps: || vec![approval_step("Approve")],
        },
        Row {
            case: "activate the first Region, a workflow waiting for an approval",
            region: "us-east-1",
            action: "activate",
            steps: || vec![approval_step("Approve")],
        },
        Row {
            case: "deactivate the second Region",
            region: "us-west-2",
            action: "deactivate",
            steps: || vec![approval_step("Approve")],
        },
        Row {
            case: "a workflow this emulator cannot perform a step of",
            region: "us-east-1",
            action: "activate",
            steps: || vec![lambda_step("Invoke")],
        },
    ];

    let mut started: Vec<Started> = Vec::new();
    for (
        index,
        Row {
            case,
            region: target_region,
            action,
            steps,
        },
    ) in rows.into_iter().enumerate()
    {
        // The workflow performs the action the row asks for, so the start is served.
        let plan_arn = crate::switch_support::create_plan(
            &emulator,
            case,
            &crate::switch_support::plan_request(
                &format!("published-{index}"),
                REGIONS,
                &crate::switch_support::role_arn("ArcRegionSwitchRole"),
                json!([crate::switch_support::workflow_of(action, steps())]),
            ),
        )
        .await;
        let (status, body) = emulator
            .call(
                "StartPlanExecution",
                &json!({"planArn": plan_arn, "targetRegion": target_region, "action": action}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: StartPlanExecution expected 200, got {status} with {body}"
        );
        started.push(Started {
            case,
            plan_arn,
            execution_id: body["executionId"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            region: target_region,
        });
    }

    // A refused start publishes nothing: nothing started.
    let (status, refused) = emulator
        .call(
            "StartPlanExecution",
            &json!({
                "planArn": format!("arn:aws:arc-region-switch::{ACCOUNT}:plan/absent:aaaaaa"),
                "targetRegion": "us-west-2",
                "action": "activate",
            }),
        )
        .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a plan that is not there\": expected the not-found refusal, got {status} with \
         {refused}"
    );

    // Each Region's queue holds exactly the events of the executions running in it.
    for (region, url) in &targets {
        let wanted = started
            .iter()
            .filter(|execution| execution.region == *region)
            .count();
        let envelopes = delivered(&emulator, region, url, wanted).await;
        for Started {
            case,
            plan_arn,
            execution_id,
            region: execution_region,
        } in &started
        {
            let mine: Vec<&Value> = envelopes
                .iter()
                .filter(|event| event["detail"]["executionId"] == json!(execution_id))
                .collect();
            if execution_region != region {
                assert!(
                    mine.is_empty(),
                    "case {case:?}: an execution of {execution_region} must not be delivered \
                     by a rule of {region}, got {mine:?}"
                );
                continue;
            }
            assert_eq!(
                mine.len(),
                1,
                "case {case:?}: expected one event for execution {execution_id} on the bus of \
                 {region}, got {envelopes:?}"
            );
            let event = mine[0];
            assert_eq!(
                event["source"],
                json!(EVENT_SOURCE),
                "case {case:?}: expected the source {EVENT_SOURCE}, got {event}"
            );
            assert_eq!(
                event["detail-type"],
                json!(PLAN_EXECUTION_DETAIL_TYPE),
                "case {case:?}: expected the detail-type {PLAN_EXECUTION_DETAIL_TYPE}, got \
                 {event}"
            );
            assert_eq!(
                event["resources"],
                json!([plan_arn]),
                "case {case:?}: expected the plan in resources, got {event}"
            );
            assert_eq!(
                event["detail"]["plan"],
                json!(plan_arn),
                "case {case:?}: expected the plan in the detail, got {event}"
            );
            assert_eq!(
                event["detail"]["executionRegion"],
                json!(region),
                "case {case:?}: expected the execution's Region in the detail, got {event}"
            );
            assert!(
                event["time"].is_string(),
                "case {case:?}: expected the event stamped with when the execution moved, got \
                 {event}"
            );
        }
    }

    // The rule written for another service's events received nothing.
    let others = drain(&emulator, REGIONS[0], &unrelated).await;
    assert!(
        others.is_empty(),
        "case \"a rule for another service\": expected no delivery, got {others:?}"
    );
}

#[tokio::test]
async fn a_client_token_replay_publishes_nothing_more() {
    let emulator = Emulator::with_events();
    let region = REGIONS[0];
    let url = given_target(
        &emulator,
        region,
        "region-switch-replays",
        plan_execution_pattern(),
    )
    .await;

    let case = "the same clientToken twice";
    let plan_arn =
        plan_with_steps(&emulator, case, "replayed", vec![approval_step("Approve")]).await;
    let request = json!({
        "planArn": plan_arn,
        "targetRegion": region,
        "action": "activate",
        "clientToken": "a-token-used-twice",
    });
    let mut answers = Vec::new();
    for attempt in 0..2 {
        let (status, body) = emulator.call("StartPlanExecution", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: attempt {attempt} expected 200, got {status} with {body}"
        );
        answers.push(body["executionId"].as_str().unwrap_or_default().to_string());
    }
    assert_eq!(
        answers[0], answers[1],
        "case {case:?}: a replay answers with the first execution, got {answers:?}"
    );

    // One execution started, so one event — the replay started nothing.
    let envelopes = delivered(&emulator, region, &url, 1).await;
    let mine: Vec<&Value> = envelopes
        .iter()
        .filter(|event| event["detail"]["executionId"] == json!(answers[0]))
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "case {case:?}: expected exactly one event for the one execution, got {envelopes:?}"
    );
}
