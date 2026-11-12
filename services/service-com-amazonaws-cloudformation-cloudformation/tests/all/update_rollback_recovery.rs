// SPDX-License-Identifier: CC0-1.0

//! The UPDATE path's per-resource event timeline and the recovery
//! `ContinueUpdateRollback` drives, end to end over the awsQuery wire.
//!
//! Three rules are under test.
//!
//! * An UpdateStack records one stack event per resource per transition it puts that
//!   resource through, beside the stack's own events, and `DescribeStackEvents` reports them
//!   newest first — so a client can tell which resources an update had already changed
//!   before it finished.
//! * An update that FAILS is rolled back to the previous stack configuration — "if the
//!   stack update fails, CloudFormation rolls back the stack to the previous stack
//!   configuration" — in the services that own the resources it had already changed there,
//!   whichever way it failed: including an update held at an
//!   `AWS::CloudFormation::WaitCondition` whose wait runs out, which no call of the client's
//!   ends. The stack reaches `UPDATE_ROLLBACK_COMPLETE`, or `UPDATE_ROLLBACK_FAILED` where
//!   an owning service refuses to put a resource back, and a caller that asked for the
//!   failed state to be kept (`DisableRollback`) gets it.
//! * `ContinueUpdateRollback` resumes the rollback of an update that could not be rolled
//!   back: it is accepted ONLY for a stack in `UPDATE_ROLLBACK_FAILED`, the resources
//!   `ResourcesToSkip` names are left exactly as the failed update made them and take
//!   `UPDATE_COMPLETE`, and the stack reaches `UPDATE_ROLLBACK_COMPLETE` — while a resumed
//!   rollback the owning service refuses again re-enters `UPDATE_ROLLBACK_FAILED`.
//!
//! Every case drives the real request path (form-encoded body in, XML envelope out) through
//! the wire harness. The table for the first rule spans the class it generalises over: a
//! resource the update CHANGES, one it ADDS, one it REMOVES and one it leaves alone, over
//! both template spellings. The table for the second spans the three directions a revert
//! runs in (a changed resource put back, an added one removed, a removed one re-created) and
//! the caller that declines the rollback. The table for the third spans the statuses a stack
//! can be in when the call arrives, and the two spellings of what to skip.

use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::wait_conditions::{
    DEFAULT_TIMEOUT, MAX_TIMEOUT,
};
use service_com_amazonaws_cloudformation_cloudformation::state::{
    resource_status, status, CANNOT_CONTINUE_UPDATE_ROLLBACK,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    acknowledging_iam, call, create_stack, iam, member, router, router_with_engine, store,
    wait_for_the_transition, xml_text, xml_texts, REGION,
};

/// Every `(LogicalResourceId, ResourceStatus)` pair one `DescribeStackEvents` response
/// carries, in wire order (newest first).
///
/// Read as PARALLEL member lists: every event reports both members, so the lists line up
/// one-to-one, and a nesting-naive split of the `StackEvents` block into its `<member>`
/// elements is not needed.
pub(crate) async fn stack_events(
    case: &str,
    router: &MultiServiceRouter,
    stack_name: &str,
) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackEvents to succeed, got {status} {body}"
    );
    let logical_ids = xml_texts(&body, "LogicalResourceId");
    let statuses = xml_texts(&body, "ResourceStatus");
    assert_eq!(
        logical_ids.len(),
        statuses.len(),
        "case {case:?}: every event reports a logical id and a status, got {body}"
    );
    logical_ids.into_iter().zip(statuses).collect()
}

/// One row of the first table: a stack to create, the template to update it to, and the
/// per-resource `(LogicalResourceId, ResourceStatus)` events the update must record.
struct UpdateCase {
    case: &'static str,
    stack_name: &'static str,
    created: String,
    updated: String,
    /// Events the update's own log must carry.
    expected: Vec<(&'static str, &'static str)>,
    /// Events no update may record, because the resource was not acted on.
    forbidden: Vec<(&'static str, &'static str)>,
}

/// The class the rule covers: any update, over any template, acting on any subset of the
/// stack's resources.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "json template, one resource changed and one left alone",
            stack_name: "changed-one",
            created: r#"{"Resources":{
                 "Changed":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"before"}},
                 "Untouched":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"same"}}}}"#
                .to_string(),
            updated: r#"{"Resources":{
                 "Changed":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"after"}},
                 "Untouched":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"same"}}}}"#
                .to_string(),
            expected: vec![
                ("Changed", resource_status::UPDATE_IN_PROGRESS),
                ("Changed", resource_status::UPDATE_COMPLETE),
            ],
            forbidden: vec![
                ("Untouched", resource_status::UPDATE_IN_PROGRESS),
                ("Untouched", resource_status::UPDATE_COMPLETE),
            ],
        },
        UpdateCase {
            case: "yaml template, a resource added by the update",
            stack_name: "added-one",
            created: concat!(
                "Resources:\n",
                "  Kept:\n",
                "    Type: AWS::SSM::Parameter\n",
                "    Properties:\n",
                "      Type: String\n",
                "      Value: kept\n",
            )
            .to_string(),
            updated: concat!(
                "Resources:\n",
                "  Kept:\n",
                "    Type: AWS::SSM::Parameter\n",
                "    Properties:\n",
                "      Type: String\n",
                "      Value: kept\n",
                "  Added:\n",
                "    Type: AWS::SSM::Parameter\n",
                "    Properties:\n",
                "      Type: String\n",
                "      Value: added\n",
            )
            .to_string(),
            expected: vec![
                ("Added", resource_status::CREATE_IN_PROGRESS),
                ("Added", resource_status::CREATE_COMPLETE),
            ],
            forbidden: vec![("Kept", resource_status::UPDATE_COMPLETE)],
        },
        UpdateCase {
            case: "json template, a resource removed by the update",
            stack_name: "removed-one",
            created: r#"{"Resources":{
                 "Kept":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"kept"}},
                 "Dropped":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"dropped"}}}}"#
                .to_string(),
            updated: r#"{"Resources":{
                 "Kept":{"Type":"AWS::SSM::Parameter",
                   "Properties":{"Type":"String","Value":"kept"}}}}"#
                .to_string(),
            expected: vec![
                ("Dropped", resource_status::DELETE_IN_PROGRESS),
                ("Dropped", resource_status::DELETE_COMPLETE),
            ],
            forbidden: vec![("Kept", resource_status::UPDATE_COMPLETE)],
        },
    ]
}

#[tokio::test]
async fn an_update_records_one_event_per_resource_it_acts_on() {
    let router = router();

    for case in update_cases() {
        let (http_status, body) =
            create_stack(&router, case.stack_name, &case.created, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let before = stack_events(case.case, &router, case.stack_name).await;
        let (http_status, body) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", case.stack_name),
                member("TemplateBody", &case.updated),
            ],
        )
        .await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected UpdateStack to succeed, got {http_status} {body}",
            case.case
        );
        wait_for_the_transition().await;

        let events = stack_events(case.case, &router, case.stack_name).await;
        // The update's own events are the ones the create's log did not already carry.
        let added: Vec<(String, String)> = events
            .iter()
            .filter(|event| !before.contains(event))
            .cloned()
            .collect();
        for (logical_id, resource) in &case.expected {
            let wanted = ((*logical_id).to_string(), (*resource).to_string());
            assert!(
                added.contains(&wanted),
                "case {:?}: expected the update to record {wanted:?}, got {added:?}",
                case.case
            );
        }
        for (logical_id, resource) in &case.forbidden {
            let unwanted = ((*logical_id).to_string(), (*resource).to_string());
            assert!(
                !added.contains(&unwanted),
                "case {:?}: expected no {unwanted:?} event for a resource the update did not \
                 act on, got {added:?}",
                case.case
            );
        }
        // The stack's own terminal event closes the operation, after the resources'.
        assert!(
            added.contains(&(
                case.stack_name.to_string(),
                status::UPDATE_COMPLETE.to_string()
            )),
            "case {:?}: expected the stack's own completion event, got {added:?}",
            case.case
        );
        let stack_terminal = added
            .iter()
            .position(|event| {
                event == &(
                    case.stack_name.to_string(),
                    status::UPDATE_COMPLETE.to_string(),
                )
            })
            .unwrap_or_default();
        for (logical_id, resource) in &case.expected {
            let wanted = ((*logical_id).to_string(), (*resource).to_string());
            let at = added
                .iter()
                .position(|event| event == &wanted)
                .unwrap_or_default();
            // Newest first, so the stack's terminal event comes BEFORE every resource
            // event of the same operation in the reported order.
            assert!(
                at > stack_terminal,
                "case {:?}: expected {wanted:?} to be reported after the stack's own \
                 completion in a newest-first log, got {added:?}",
                case.case
            );
        }
    }
}

#[tokio::test]
async fn continuing_a_rollback_is_refused_unless_the_rollback_failed() {
    let router = router();

    let template = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic",
         "Properties":{"DisplayName":"settled"}}}}"#;
    let (http_status, body) = create_stack(&router, "settled-stack", template, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );

    // While the create is still in flight, and once it has completed: neither is a status a
    // rollback can be continued from, and the stack is left exactly where it is. The refusal
    // names the call that cannot be made and the status the stack IS in, so the two rows
    // differ in the status the sentence reports.
    for (case, reported) in [
        ("while the create is in progress", status::CREATE_IN_PROGRESS),
        ("after the create completed", status::CREATE_COMPLETE),
    ] {
        let (http_status, body) = call(
            &router,
            "ContinueUpdateRollback",
            &[member("StackName", "settled-stack")],
        )
        .await;
        assert_eq!(
            http_status, 400,
            "case {case:?}: expected the call to be refused with 400, got {http_status} {body}"
        );
        // The status gate words ONE sentence whatever status it refused from, so the status
        // the stack is really in (`reported`) reaches no part of it.
        let expected = CANNOT_CONTINUE_UPDATE_ROLLBACK;
        assert!(
            body.contains(expected),
            "case {case:?}: expected the refusal to be {expected:?}, got {body}"
        );
        assert!(
            !body.contains(&format!("{reported} status")),
            "case {case:?}: expected the refusal to carry no part of this run, got {body}"
        );
        wait_for_the_transition().await;
    }

    // A stack this Region does not hold is refused too, and nothing is created for it.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[member("StackName", "no-such-stack")],
    )
    .await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected an unknown stack to be refused with 400, got {http_status} {body}",
        "a stack this Region does not hold"
    );

    // "ForceRollback and ResourcesToSkip are mutually exclusive. Specifying both in the same
    // request returns a validation error."
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", "settled-stack"),
            member("ForceRollback", "true"),
            member("ResourcesToSkip.member.1", "Topic"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected both spellings together to be refused, got {http_status} {body}",
        "ForceRollback beside ResourcesToSkip"
    );
    assert!(
        body.contains("ForceRollback") && body.contains("ResourcesToSkip"),
        "case {:?}: expected the refusal to name both members, got {body}",
        "ForceRollback beside ResourcesToSkip"
    );
}

#[tokio::test]
async fn continuing_a_rollback_leaves_the_skipped_resources_as_the_failed_update_made_them() {
    let (router, engine) = router_with_engine();

    let template = r#"{"Resources":{
         "Stuck":{"Type":"AWS::SSM::Parameter",
           "Properties":{"Type":"String","Value":"stuck"}},
         "Revertible":{"Type":"AWS::SSM::Parameter",
           "Properties":{"Type":"String","Value":"revertible"}}}}"#;
    let (http_status, body) = create_stack(&router, "failed-rollback", template, &[]).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;

    // The state a failed update rollback leaves behind, seeded directly: an owning service
    // that refuses to put one resource back is what produces it, and no wire call of this
    // suite can make a service refuse.
    {
        let state = store(&engine);
        let mut held = state.lock();
        // The create's own clock-owed completion is materialised FIRST, so the status seeded
        // below is not overwritten by the transition the next call would settle.
        held.settle(SystemTime::now());
        let stack = held
            .resolve_mut(REGION, "failed-rollback")
            .expect("the stack just created");
        stack.status = status::UPDATE_ROLLBACK_FAILED.to_string();
        stack.status_reason = Some("the owning service refused to put Stuck back".to_string());
        for deployed in &mut stack.resources {
            if deployed.resource.logical_id == "Stuck" {
                deployed.status = resource_status::UPDATE_FAILED.to_string();
                deployed.status_timestamp = SystemTime::now();
            }
        }
    }

    // "You can specify only resources that are in the UPDATE_FAILED state because a rollback
    // failed": a logical id outside that set is refused rather than silently skipped.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", "failed-rollback"),
            member("ResourcesToSkip.member.1", "Revertible"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 400,
        "case {:?}: expected a resource that did not fail to be refused, got {http_status} \
         {body}",
        "skipping a resource that is not UPDATE_FAILED"
    );

    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", "failed-rollback"),
            member("ResourcesToSkip.member.1", "Stuck"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {:?}: expected the rollback to be continued, got {http_status} {body}",
        "skipping the resource the rollback could not put back"
    );

    // Answered while the rollback is in progress; the terminal status falls due on the
    // clock, exactly as every other stack operation's does.
    let (http_status, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "failed-rollback")],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStacks to succeed, got {http_status} {body}"
    );
    let reported = xml_text(&body, "StackStatus").unwrap_or_default();
    assert!(
        reported == status::UPDATE_ROLLBACK_IN_PROGRESS
            || reported == status::UPDATE_ROLLBACK_COMPLETE,
        "case {:?}: expected the stack to be rolling back or rolled back, got {reported:?} in \
         {body}",
        "skipping the resource the rollback could not put back"
    );

    wait_for_the_transition().await;

    let (http_status, body) = call(
        &router,
        "DescribeStacks",
        &[member("StackName", "failed-rollback")],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStacks to succeed, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "StackStatus").as_deref(),
        Some(status::UPDATE_ROLLBACK_COMPLETE),
        "case {:?}: expected the continued rollback to complete, got {body}",
        "skipping the resource the rollback could not put back"
    );

    // "CloudFormation sets the status of the specified resources to UPDATE_COMPLETE and
    // continues to roll back the stack": the skipped resource keeps the configuration it
    // had and takes that status, and the event log says so.
    let (http_status, body) = call(
        &router,
        "DescribeStackResources",
        &[
            member("StackName", "failed-rollback"),
            member("LogicalResourceId", "Stuck"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected DescribeStackResources to succeed, got {http_status} {body}"
    );
    assert_eq!(
        xml_text(&body, "ResourceStatus").as_deref(),
        Some(resource_status::UPDATE_COMPLETE),
        "case {:?}: expected the skipped resource to be reported UPDATE_COMPLETE, got {body}",
        "skipping the resource the rollback could not put back"
    );
    let events = stack_events(
        "skipping the resource the rollback could not put back",
        &router,
        "failed-rollback",
    )
    .await;
    assert!(
        events.contains(&(
            "Stuck".to_string(),
            resource_status::UPDATE_COMPLETE.to_string()
        )),
        "case {:?}: expected the skipped resource's own event, got {events:?}",
        "skipping the resource the rollback could not put back"
    );
    assert!(
        events.contains(&(
            "failed-rollback".to_string(),
            status::UPDATE_ROLLBACK_COMPLETE.to_string()
        )),
        "case {:?}: expected the stack's own rollback completion event, got {events:?}",
        "skipping the resource the rollback could not put back"
    );
}

/// The `Timeout` the wait conditions this suite declares ask for, in seconds, and how long a
/// case waits for one of them to run out: the declaration's own value plus the margin every
/// other suite here allows a clock-owed transition.
pub(crate) const GATE_TIMEOUT: &str = "1";
pub(crate) const WAITED_OUT: Duration = Duration::from_millis(1_250);

/// AWS Systems Manager's `X-Amz-Target` prefix, from its model's service shape
/// (`com.amazonaws.ssm#AmazonSSM`), which speaks awsJson1_1.
const SSM_TARGET_PREFIX: &str = "AmazonSSM";

/// One Systems Manager call over the awsJson1_1 wire its model declares, signed for `ssm` in
/// the Region these stacks live in — so what a case reads back is the parameter the stack's
/// own deployment really wrote.
pub(crate) async fn ssm(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/ssm/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    );
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://ssm.{REGION}.amazonaws.com/"))
        .header("host", format!("ssm.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{SSM_TARGET_PREFIX}.{operation}"))
        .header("authorization", authorization)
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed Systems Manager request");
    let response = router.dispatch(request).await;
    let status_code = response.status().as_u16();
    let raw = response.into_body();
    let parsed = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw).unwrap_or(Value::Null)
    };
    (status_code, parsed)
}

/// The value Parameter Store holds for `name`, or `None` when it holds no such parameter —
/// which is what says whether a rollback put a resource back, left it as the failed update
/// made it, or removed one the update had added.
pub(crate) async fn parameter_value(router: &MultiServiceRouter, name: &str) -> Option<String> {
    let (status_code, body) = ssm(router, "GetParameter", &json!({"Name": name})).await;
    if status_code != 200 {
        return None;
    }
    body.pointer("/Parameter/Value")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `AWS::SSM::Parameter` declaration these cases deploy: a parameter of Parameter Store
/// under the name the template asks for, so a case reads the deployed value back through
/// Systems Manager's own wire.
pub(crate) fn parameter(name: &str, value: &str) -> Value {
    json!({
        "Type": "AWS::SSM::Parameter",
        "Properties": {"Name": name, "Type": "String", "Value": value},
    })
}

/// The template whose `Resources` section is `resources`.
pub(crate) fn template(resources: Value) -> String {
    json!({"Resources": resources}).to_string()
}

/// `resources` plus the wait condition an update adds and nothing signals: CloudFormation
/// "doesn't proceed with a stack creation or update until resources receive the required
/// number of signals or the timeout period is exceeded", so this is what holds the update
/// open and then fails it with no call of the client's.
pub(crate) fn template_with_an_unsignalled_gate(resources: Value) -> String {
    let mut declared: Map<String, Value> = match resources {
        Value::Object(members) => members,
        other => panic!("a Resources section must be an object, got {other:?}"),
    };
    declared.insert(
        "Gate".to_string(),
        json!({"Type": "AWS::CloudFormation::WaitConditionHandle"}),
    );
    declared.insert(
        "Wait".to_string(),
        json!({
            "Type": "AWS::CloudFormation::WaitCondition",
            "Properties": {
                "Handle": {"Ref": "Gate"},
                "Timeout": GATE_TIMEOUT,
                "Count": "1",
            },
        }),
    );
    template(Value::Object(declared))
}

/// The `StackStatus` a stack reports right now.
pub(crate) async fn status_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (http_status, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    assert_eq!(
        http_status, 200,
        "DescribeStacks {stack:?}: expected 200, got {http_status} {body}"
    );
    xml_text(&body, "StackStatus").unwrap_or_default()
}

/// The `StackStatusReason` a stack reports right now.
pub(crate) async fn reason_of(router: &MultiServiceRouter, stack: &str) -> String {
    let (_, body) = call(router, "DescribeStacks", &[member("StackName", stack)]).await;
    xml_text(&body, "StackStatusReason").unwrap_or_default()
}

/// The `ResourceStatus` a stack reports for ONE of its resources.
pub(crate) async fn resource_status_of(
    router: &MultiServiceRouter,
    stack: &str,
    logical_id: &str,
) -> String {
    let (http_status, body) = call(
        router,
        "DescribeStackResources",
        &[
            member("StackName", stack),
            member("LogicalResourceId", logical_id),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "DescribeStackResources {stack:?}/{logical_id:?}: expected 200, got {http_status} {body}"
    );
    xml_text(&body, "ResourceStatus").unwrap_or_default()
}

/// One row of the rollback table: what the stack is created from, what the update submits,
/// and what Parameter Store must hold before and after the rollback.
struct RollbackCase {
    case: &'static str,
    stack: &'static str,
    created: String,
    updated: String,
    /// `DisableRollback`, exactly as the update carries it.
    disable_rollback: bool,
    /// The status the stack must reach once the wait has run out.
    settles_to: &'static str,
    /// `(parameter name, the value Parameter Store holds)` once the update has applied its
    /// change and before its wait runs out; `None` is a parameter it must not hold at all.
    after_update: Vec<(String, Option<&'static str>)>,
    /// The same, once the stack has settled.
    after_rollback: Vec<(String, Option<&'static str>)>,
}

/// The class the rule covers: an update that fails after it has already changed the owning
/// services, whichever direction its revert has to run in.
fn rollback_cases() -> Vec<RollbackCase> {
    vec![
        RollbackCase {
            case: "a changed resource is put back as it was",
            stack: "roll-changed",
            created: template(json!({"Param": parameter("/roll-changed/p", "before")})),
            updated: template_with_an_unsignalled_gate(
                json!({"Param": parameter("/roll-changed/p", "after")}),
            ),
            disable_rollback: false,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            after_update: vec![("/roll-changed/p".to_string(), Some("after"))],
            after_rollback: vec![("/roll-changed/p".to_string(), Some("before"))],
        },
        RollbackCase {
            case: "a resource the update added is removed again",
            stack: "roll-added",
            created: template(json!({"Param": parameter("/roll-added/p", "keep")})),
            updated: template_with_an_unsignalled_gate(json!({
                "Param": parameter("/roll-added/p", "keep"),
                "Added": parameter("/roll-added/added", "added"),
            })),
            disable_rollback: false,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            after_update: vec![
                ("/roll-added/p".to_string(), Some("keep")),
                ("/roll-added/added".to_string(), Some("added")),
            ],
            after_rollback: vec![
                ("/roll-added/p".to_string(), Some("keep")),
                ("/roll-added/added".to_string(), None),
            ],
        },
        RollbackCase {
            case: "a resource the update removed is created again",
            stack: "roll-dropped",
            created: template(json!({
                "Param": parameter("/roll-dropped/p", "keep"),
                "Dropped": parameter("/roll-dropped/d", "dropped"),
            })),
            updated: template_with_an_unsignalled_gate(
                json!({"Param": parameter("/roll-dropped/p", "keep")}),
            ),
            disable_rollback: false,
            settles_to: status::UPDATE_ROLLBACK_COMPLETE,
            after_update: vec![("/roll-dropped/d".to_string(), None)],
            after_rollback: vec![
                ("/roll-dropped/p".to_string(), Some("keep")),
                ("/roll-dropped/d".to_string(), Some("dropped")),
            ],
        },
        // "DisableRollback: Preserve the state of previously provisioned resources when an
        // operation fails": the caller said what it wants left behind, so the failed update
        // is not rolled back and the owning service keeps what it put there.
        RollbackCase {
            case: "DisableRollback keeps the failed update as it failed",
            stack: "roll-kept",
            created: template(json!({"Param": parameter("/roll-kept/p", "before")})),
            updated: template_with_an_unsignalled_gate(
                json!({"Param": parameter("/roll-kept/p", "after")}),
            ),
            disable_rollback: true,
            settles_to: status::UPDATE_FAILED,
            after_update: vec![("/roll-kept/p".to_string(), Some("after"))],
            after_rollback: vec![("/roll-kept/p".to_string(), Some("after"))],
        },
    ]
}

#[tokio::test]
async fn an_update_whose_wait_runs_out_is_rolled_back_in_the_services_that_own_its_resources() {
    for case in rollback_cases() {
        let router = router();
        let (http_status, body) = create_stack(&router, case.stack, &case.created, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {body}",
            case.case
        );
        wait_for_the_transition().await;
        let created = status_of(&router, case.stack).await;
        assert_eq!(
            created,
            status::CREATE_COMPLETE,
            "case {:?}: expected the create to complete before the update, got {created:?}",
            case.case
        );

        let mut members = vec![
            member("StackName", case.stack),
            member("TemplateBody", &case.updated),
        ];
        if case.disable_rollback {
            members.push(member("DisableRollback", "true"));
        }
        let (http_status, body) = call(&router, "UpdateStack", &members).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected UpdateStack to succeed, got {http_status} {body}",
            case.case
        );

        // The update has made its changes in the owning services and is HELD at the wait
        // condition: the clock alone does not complete it.
        wait_for_the_transition().await;
        let in_flight = status_of(&router, case.stack).await;
        assert_eq!(
            in_flight,
            status::UPDATE_IN_PROGRESS,
            "case {:?}: expected the update to be held at its wait condition, got \
             {in_flight:?}",
            case.case
        );
        for (name, expected) in &case.after_update {
            let held = parameter_value(&router, name).await;
            assert_eq!(
                held.as_deref(),
                *expected,
                "case {:?}: expected Parameter Store to hold {expected:?} for {name:?} once \
                 the update had applied its change, got {held:?}",
                case.case
            );
        }

        // The wait runs out, which fails the update with no call of the client's: the
        // rollback that failure owes is what the next call materialises.
        tokio::time::sleep(WAITED_OUT).await;
        let settling = status_of(&router, case.stack).await;
        assert!(
            settling == case.settles_to || settling == status::UPDATE_ROLLBACK_IN_PROGRESS,
            "case {:?}: expected the stack to be rolling back or settled at {}, got \
             {settling:?}",
            case.case,
            case.settles_to
        );
        wait_for_the_transition().await;
        let settled = status_of(&router, case.stack).await;
        assert_eq!(
            settled, case.settles_to,
            "case {:?}: expected the stack to settle at {}, got {settled:?}",
            case.case, case.settles_to
        );
        // The reason is the wait condition's own diagnosis, which is what failed the update.
        let reason = reason_of(&router, case.stack).await;
        assert!(
            reason.contains("WaitCondition timed out"),
            "case {:?}: expected the reason to carry the wait condition's diagnosis, got \
             {reason:?}",
            case.case
        );
        for (name, expected) in &case.after_rollback {
            let held = parameter_value(&router, name).await;
            assert_eq!(
                held.as_deref(),
                *expected,
                "case {:?}: expected Parameter Store to hold {expected:?} for {name:?} once \
                 the stack had settled, got {held:?}",
                case.case
            );
        }
        // The stack's own events say what it did, in order.
        let events = stack_events(case.case, &router, case.stack).await;
        let rolled_back = events.contains(&(
            case.stack.to_string(),
            status::UPDATE_ROLLBACK_IN_PROGRESS.to_string(),
        ));
        assert_eq!(
            rolled_back,
            !case.disable_rollback,
            "case {:?}: expected the rollback to be reported only where one was owed, got \
             {events:?}",
            case.case
        );
        // ...and the failure is an event of the STACK only where the stack RESTS in
        // `UPDATE_FAILED`: a stack rolled straight back goes from `UPDATE_IN_PROGRESS` into
        // `UPDATE_ROLLBACK_IN_PROGRESS`, whose own event carries the failure, while a caller
        // that asked for the failed state to be kept gets that status and its event.
        let rested_in_the_failure =
            events.contains(&(case.stack.to_string(), status::UPDATE_FAILED.to_string()));
        assert_eq!(
            rested_in_the_failure, case.disable_rollback,
            "case {:?}: expected the stack's own {} event exactly where the failed state is \
             kept, got {events:?}",
            case.case,
            status::UPDATE_FAILED
        );
    }
}

/// One row of the hold table: the `Timeout` the declaration states, the span that asks for
/// (bounded by the type's own published maximum, defaulted where it states none) and how
/// long the wait is really held for it.
struct HoldCase {
    case: &'static str,
    stack: &'static str,
    /// `Timeout` as the declaration spells it, or `None` for a declaration that states none.
    declared: Option<&'static str>,
    /// The span that declaration ASKS for.
    asked_for: Duration,
}

impl HoldCase {
    /// How long the wait is really HELD for that declaration: the span it asked for, which is
    /// the span the gated resource stays in flight for.
    fn held(&self) -> Duration {
        self.asked_for
    }
}

/// The class the bound covers: every `Timeout` a declaration can state.
///
/// The span the declaration ASKS for is the type's own published one: "the maximum time that
/// can be specified for this property is 12 hours (43200 seconds)"
/// (`AWS::CloudFormation::WaitCondition`'s `Timeout`), with the documented default where it
/// states none — so the rows include that maximum, one second past it, and a declaration
/// that states nothing, either side of the published bound, plus the degenerate zero and
/// spans a client's poll loop measures in seconds. Nothing but a client's own signal ends
/// this wait before the span it asked for is over, so what a declaration asks for is also
/// how long the wait is really held.
fn hold_cases() -> Vec<HoldCase> {
    vec![
        HoldCase {
            case: "a wait that is over the moment it starts",
            stack: "hold-zero",
            declared: Some("0"),
            asked_for: Duration::ZERO,
        },
        HoldCase {
            case: "a wait of one second",
            stack: "hold-one",
            declared: Some("1"),
            asked_for: Duration::from_secs(1),
        },
        HoldCase {
            case: "a wait of fourteen seconds",
            stack: "hold-just-inside",
            declared: Some("14"),
            asked_for: Duration::from_secs(14),
        },
        HoldCase {
            case: "a wait of fifteen seconds",
            stack: "hold-fifteen",
            declared: Some("15"),
            asked_for: Duration::from_secs(15),
        },
        HoldCase {
            case: "a wait of sixteen seconds",
            stack: "hold-sixteen",
            declared: Some("16"),
            asked_for: Duration::from_secs(16),
        },
        HoldCase {
            case: "a wait of a minute and a half",
            stack: "hold-ninety",
            declared: Some("90"),
            asked_for: Duration::from_secs(90),
        },
        HoldCase {
            case: "a wait just under the documented default",
            stack: "hold-just-under-default",
            declared: Some("299"),
            asked_for: Duration::from_secs(299),
        },
        HoldCase {
            case: "a wait of the documented maximum",
            stack: "hold-maximum",
            declared: Some("43200"),
            asked_for: MAX_TIMEOUT,
        },
        HoldCase {
            case: "a wait past the documented maximum",
            stack: "hold-past-maximum",
            declared: Some("43201"),
            asked_for: MAX_TIMEOUT,
        },
        HoldCase {
            case: "a declaration that states no wait at all",
            stack: "hold-unstated",
            declared: None,
            asked_for: DEFAULT_TIMEOUT,
        },
    ]
}

#[tokio::test]
async fn a_wait_is_held_for_what_it_asked() {
    // The default a declaration that states nothing falls back to is inside the type's own
    // published ceiling, so no row can be held longer than the attribute permits.
    assert!(
        DEFAULT_TIMEOUT < MAX_TIMEOUT,
        "the default wait must be inside the documented Timeout maximum, got \
         {DEFAULT_TIMEOUT:?} against {MAX_TIMEOUT:?}"
    );

    let (router, engine) = router_with_engine();
    for case in hold_cases() {
        let properties = match case.declared {
            Some(timeout) => json!({"Handle": {"Ref": "Gate"}, "Timeout": timeout, "Count": "1"}),
            None => json!({"Handle": {"Ref": "Gate"}, "Count": "1"}),
        };
        let body = template(json!({
            "Gate": {"Type": "AWS::CloudFormation::WaitConditionHandle"},
            "Wait": {"Type": "AWS::CloudFormation::WaitCondition", "Properties": properties},
        }));
        let (http_status, created) = create_stack(&router, case.stack, &body, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {:?}: expected CreateStack to succeed, got {http_status} {created}",
            case.case
        );

        let held = store(&engine);
        let locked = held.lock();
        let stack = locked
            .resolve(REGION, case.stack)
            .unwrap_or_else(|| panic!("case {:?}: the stack just created", case.case));
        let barrier = stack
            .wait_conditions
            .first()
            .unwrap_or_else(|| panic!("case {:?}: the wait condition's own barrier", case.case));
        // What the declaration asked for, bounded by the maximum its own attribute
        // publishes and defaulted where it stated nothing...
        let asked_for = barrier.declared.timeout;
        assert_eq!(
            asked_for, case.asked_for,
            "case {:?}: expected a declaration of {:?} to ask for {:?}, got {asked_for:?}",
            case.case, case.declared, case.asked_for
        );
        // ...and how long the wait is really held for it.
        let held_for = barrier
            .deadline()
            .duration_since(barrier.opened_at)
            .unwrap_or(Duration::ZERO);
        assert_eq!(
            held_for,
            case.held(),
            "case {:?}: expected a declaration of {:?} to be held for {:?}, got {held_for:?}",
            case.case,
            case.declared,
            case.held()
        );
    }
}

/// The trust policy the role these cases deploy carries: IAM's own `CreateRole` requires
/// the document, and the principal is immaterial to the rule under test.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "ec2.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
}

/// The documented cause of `UPDATE_ROLLBACK_FAILED`, driven end to end: an update fails, and
/// the rollback cannot put a resource back because that resource was changed outside
/// CloudFormation — "this might occur when a stack attempts to roll back to an old database
/// that was deleted outside of CloudFormation" — so `ContinueUpdateRollback` is what gets
/// the stack out of it.
#[tokio::test]
async fn a_rollback_the_owning_service_refuses_is_resumed_by_skipping_that_resource() {
    let router = router();
    let stack = "stuck-rollback";
    let role_name = "stuck-rollback-role";
    let parameter_name = "/stuck-rollback/p";
    let role = |description: &str| {
        json!({
            "Type": "AWS::IAM::Role",
            "Properties": {
                "RoleName": role_name,
                "AssumeRolePolicyDocument": trust_policy(),
                "Description": description,
            },
        })
    };

    let created = template(json!({
        "TheRole": role("before"),
        "TheParam": parameter(parameter_name, "before"),
    }));
    let (http_status, body) =
        create_stack(&router, stack, &created, &acknowledging_iam(&[])).await;
    assert_eq!(
        http_status, 200,
        "expected CreateStack to succeed, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let (http_status, body) = iam(&router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        http_status, 200,
        "expected the stack to have created the role in IAM, got {http_status} {body}"
    );

    // The update changes both resources and is then held at a wait condition nothing
    // signals.
    let updated = template_with_an_unsignalled_gate(json!({
        "TheRole": role("after"),
        "TheParam": parameter(parameter_name, "after"),
    }));
    let mut members = vec![
        member("StackName", stack),
        member("TemplateBody", &updated),
    ];
    members.extend(acknowledging_iam(&[]));
    let (http_status, body) = call(&router, "UpdateStack", &members).await;
    assert_eq!(
        http_status, 200,
        "expected UpdateStack to succeed, got {http_status} {body}"
    );

    // The role is deleted OUTSIDE CloudFormation, so the rollback cannot put it back.
    let (http_status, body) = iam(&router, "DeleteRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        http_status, 200,
        "expected the role to be deleted outside CloudFormation, got {http_status} {body}"
    );

    tokio::time::sleep(WAITED_OUT).await;
    let failed = status_of(&router, stack).await;
    assert_eq!(
        failed,
        status::UPDATE_ROLLBACK_FAILED,
        "expected a rollback the owning service refused to leave the stack in \
         UPDATE_ROLLBACK_FAILED, got {failed:?}"
    );
    let refused = resource_status_of(&router, stack, "TheRole").await;
    assert_eq!(
        refused,
        resource_status::UPDATE_FAILED,
        "expected the resource the rollback could not put back to be reported UPDATE_FAILED, \
         got {refused:?}"
    );

    // Resumed with nothing skipped, the rollback is attempted again and the same refusal
    // stands: the stack re-enters UPDATE_ROLLBACK_FAILED rather than reporting a rollback
    // that did not happen.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[member("StackName", stack)],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected ContinueUpdateRollback to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let again = status_of(&router, stack).await;
    assert_eq!(
        again,
        status::UPDATE_ROLLBACK_FAILED,
        "expected a resumed rollback the owning service refuses again to re-enter \
         UPDATE_ROLLBACK_FAILED, got {again:?}"
    );

    // Resumed with that resource SKIPPED, the rollback completes: the skipped resource is
    // left exactly as the failed update made it — nothing re-creates the role — and
    // everything else goes back to the configuration in force before the update.
    let (http_status, body) = call(
        &router,
        "ContinueUpdateRollback",
        &[
            member("StackName", stack),
            member("ResourcesToSkip.member.1", "TheRole"),
        ],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "expected the skipping ContinueUpdateRollback to be accepted, got {http_status} {body}"
    );
    wait_for_the_transition().await;
    let completed = status_of(&router, stack).await;
    assert_eq!(
        completed,
        status::UPDATE_ROLLBACK_COMPLETE,
        "expected the skipping rollback to complete, got {completed:?}"
    );
    let skipped = resource_status_of(&router, stack, "TheRole").await;
    assert_eq!(
        skipped,
        resource_status::UPDATE_COMPLETE,
        "expected the skipped resource to be reported UPDATE_COMPLETE, got {skipped:?}"
    );
    let (http_status, body) = iam(&router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        http_status, 404,
        "expected the skipped resource to be left exactly as the failed update made it, got \
         {http_status} {body}"
    );
    let put_back = parameter_value(&router, parameter_name).await;
    assert_eq!(
        put_back.as_deref(),
        Some("before"),
        "expected the resource the rollback did not skip to be put back as it was, got \
         {put_back:?}"
    );
}
