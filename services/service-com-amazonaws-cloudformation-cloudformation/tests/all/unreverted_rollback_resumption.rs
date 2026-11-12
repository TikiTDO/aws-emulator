// SPDX-License-Identifier: CC0-1.0

//! What a stack reports when the ROLLBACK of a failed update is itself refused by the
//! provider of a custom resource, and what `ContinueUpdateRollback` then does with it.
//!
//! RULE under test: **a resource held for an answer reports the outcome that answer decides
//! only once the answer has landed — an answer that says the work was NOT carried out leaves
//! the resource in the failed status of that work, never in the completion the walk had
//! planned for it — so a rollback such an answer ends leaves the stack in
//! `UPDATE_ROLLBACK_FAILED` with that resource `UPDATE_FAILED`, and that rollback is still
//! OUTSTANDING: `ContinueUpdateRollback` accepts the resource in `ResourcesToSkip` (which is
//! also what `ForceRollback` selects) and, where it skips nothing, asks the provider for the
//! previous configuration again before the stack reaches
//! `UPDATE_ROLLBACK_COMPLETE`.**
//!
//! Every row drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint, and the provider's answers as the plain, unsigned HTTP
//! `PUT` to the published `ResponseURL` a provider really makes.
//!
//! The rows differ along what the rule generalises over: WHAT ended the update whose rollback
//! then failed (the provider refusing the update, and `CancelUpdateStack` ending it while the
//! provider had not answered at all), and how the resumption disposes of the resource the
//! rollback could not put back (asking the provider again, skipping it by name, and skipping
//! it with `ForceRollback`).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::provisioning::custom_resource::ProviderRequest;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use super::custom_resource_providers::{awaited_request, emulator, put_response, template};
use super::harness::{call, create_stack, member, xml_blocks, xml_text};

/// The three sentences this suite holds the service to, spelled out HERE rather than
/// imported from the crate, because each sentence IS the requirement: a reworded message
/// must fail this suite rather than travel into it from the implementation.
///
/// What CloudFormation reports as the failure of a custom resource whose provider ANSWERED
/// and whose answer says the work was not carried out. Its only variable parts are the
/// message that answer stated and the `RequestId` of the request it answered.
fn failed_answer_reason(message: &str, request_id: &str) -> String {
    format!(
        "Received response status [FAILED] from custom resource. \
         Message returned: {message} (RequestId: {request_id})"
    )
}

/// The `ResourceStatusReason` of a resource a resumption was told to SKIP: it reaches
/// `UPDATE_COMPLETE` without having been put back, and this says why.
const SKIPPED_DURING_UPDATE_ROLLBACK: &str = "Resource skipped during UpdateRollback";

/// The `StackStatusReason` the event that opens an operation a CLIENT started carries.
const USER_INITIATED: &str = "User Initiated";

/// The logical id of the custom resource every row deploys — the one
/// [`super::custom_resource_providers::template`] declares.
const THING: &str = "Thing";

/// The `PhysicalResourceId` the provider mints for it, kept across every later answer so no
/// answer of a row is read as a replacement.
const MINTED: &str = "probe-instance";

/// The `Reason` the provider states when it refuses to put the resource back, which is what
/// the resource's own failure must then quote.
const REFUSAL: &str = "the provider refused to put it back";

/// How long a row waits for a condition it polls for, and how often it looks.
///
/// Every wait here is for a CONDITION — a status the clock owes, a request the emulator
/// publishes out of band — never for a span: the deadline is generous so that a loaded host
/// reaches the condition inside it, and the loop stops the instant the condition holds.
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const POLL_ATTEMPTS: usize = 1_200;

/// The suffix every status a stack operation in flight carries ends in.
const IN_PROGRESS: &str = "_IN_PROGRESS";

/// One row: what ends the update, and how its resumption disposes of the failed resource.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// Whether `CancelUpdateStack` ends the update (the provider is never answered) instead
    /// of the provider refusing it.
    cancelled: bool,
    /// The members the resumption is called with besides `StackName`.
    resumption: &'static [(&'static str, &'static str)],
    /// Whether the resumption has to ask the provider for the previous configuration again:
    /// true where it skips nothing.
    asks_again: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "the provider refuses the update, and the resumption skips nothing",
        stack_name: "unreverted-retry",
        cancelled: false,
        resumption: &[],
        asks_again: true,
    },
    Row {
        case: "the provider refuses the update, and the resumption skips the resource by name",
        stack_name: "unreverted-skipped",
        cancelled: false,
        resumption: &[("ResourcesToSkip.member.1", THING)],
        asks_again: false,
    },
    Row {
        case: "the provider refuses the update, and ForceRollback selects the same resource",
        stack_name: "unreverted-forced",
        cancelled: false,
        resumption: &[("ForceRollback", "true")],
        asks_again: false,
    },
    Row {
        case: "the caller cancels the update, and the resumption skips nothing",
        stack_name: "unreverted-cancelled",
        cancelled: true,
        resumption: &[],
        asks_again: true,
    },
];

/// `DescribeStacks` for one stack: the status it reports and the whole body.
async fn describe(router: &MultiServiceRouter, case: &str, stack_name: &str) -> (String, String) {
    let (status_code, body) = call(
        router,
        "DescribeStacks",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStacks to answer, got {status_code} with {body}"
    );
    (xml_text(&body, "StackStatus").unwrap_or_default(), body)
}

/// Poll the stack until it leaves every `*_IN_PROGRESS` status, and answer the status it
/// settled in with the body that reported it.
async fn settled(router: &MultiServiceRouter, case: &str, stack_name: &str) -> (String, String) {
    let mut last = (String::new(), String::new());
    for _ in 0..POLL_ATTEMPTS {
        last = describe(router, case, stack_name).await;
        if !last.0.ends_with(IN_PROGRESS) {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {stack_name:?} never left an in-progress status; last was {:?} with {}",
        last.0, last.1
    );
}

/// The status `DescribeStackResources` reports for one resource of a stack.
async fn resource_status_of(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    logical_id: &str,
) -> Option<String> {
    let (status_code, body) = call(
        router,
        "DescribeStackResources",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackResources to answer, got {status_code} with {body}"
    );
    xml_blocks(&body, "member").into_iter().find_map(|block| {
        (xml_text(&block, "LogicalResourceId").as_deref() == Some(logical_id))
            .then(|| xml_text(&block, "ResourceStatus"))
            .flatten()
    })
}

/// The newest event `DescribeStackEvents` reports for one resource of a stack: the status it
/// carries and its reason.
///
/// `DescribeStackEvents` reports "all stack related events for a specified stack in reverse
/// chronological order", so the first event naming the resource is the last thing that
/// happened to it.
async fn newest_event_of(
    router: &MultiServiceRouter,
    case: &str,
    stack_name: &str,
    logical_id: &str,
) -> Option<(String, Option<String>)> {
    let (status_code, body) = call(
        router,
        "DescribeStackEvents",
        &[member("StackName", stack_name)],
    )
    .await;
    assert_eq!(
        status_code, 200,
        "case {case:?}: expected DescribeStackEvents to answer, got {status_code} with {body}"
    );
    xml_blocks(&body, "member").into_iter().find_map(|block| {
        if xml_text(&block, "LogicalResourceId").as_deref() != Some(logical_id) {
            return None;
        }
        let status = xml_text(&block, "ResourceStatus")?;
        Some((status, xml_text(&block, "ResourceStatusReason")))
    })
}

/// The next provider request the stack is waiting for an answer to about [`THING`], polled
/// until one the row has not seen appears.
///
/// `seen` is every request id this row has already read, so the request a later stage of the
/// operation publishes is never mistaken for the one before it.
async fn next_request(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    seen: &[String],
) -> Option<(String, String, Value, String)> {
    for _ in 0..POLL_ATTEMPTS {
        if let Some((stack_id, request, _)) = unseen_request(engine, stack_name, seen) {
            return Some((
                stack_id,
                request.request_id,
                request.properties,
                request.request_type,
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    None
}

/// The provider request this stack is holding about [`THING`] that the row has not read yet,
/// if there is one.
fn unseen_request(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    seen: &[String],
) -> Option<(String, ProviderRequest, String)> {
    awaited_request(engine, stack_name, THING)
        .filter(|(_, request, _)| !seen.contains(&request.request_id))
}

#[tokio::test]
async fn a_rollback_the_provider_refused_is_resumable() {
    for row in ROWS {
        let case = row.case;
        let (router, engine) = emulator(case).await;
        let mut seen: Vec<String> = Vec::new();

        // The stack is created and its custom resource answered, so the update below has a
        // resource in force to change.
        let (status_code, created) =
            create_stack(&router, row.stack_name, &template("Custom::Probe", "v1", &[]), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request_id, _, request_type) = next_request(&engine, row.stack_name, &seen)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the create must reach the provider"));
        assert_eq!(
            request_type, "Create",
            "case {case:?}: expected the create's own request, got {request_type}"
        );
        seen.push(request_id.clone());
        put_response(
            &router,
            &engine,
            &stack_id,
            THING,
            &request_id,
            &json!({ "Status": "SUCCESS", "PhysicalResourceId": MINTED }),
        )
        .await;
        let (settled_status, described) = settled(&router, case, row.stack_name).await;
        assert_eq!(
            settled_status, status::CREATE_COMPLETE,
            "case {case:?}: expected the answered create to complete, got {described}"
        );

        // The update changes the resource, so its provider is sent an `Update`.
        let (status_code, updated) = call(
            &router,
            "UpdateStack",
            &[
                member("StackName", row.stack_name),
                member("TemplateBody", &template("Custom::Probe", "v2", &[])),
            ],
        )
        .await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to be accepted, got {status_code} with {updated}"
        );
        let (_, request_id, properties, request_type) = next_request(&engine, row.stack_name, &seen)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the update must reach the provider"));
        assert_eq!(
            request_type, "Update",
            "case {case:?}: expected the update's own request, got {request_type}"
        );
        assert_eq!(
            properties.get("Marker").and_then(Value::as_str),
            Some("v2"),
            "case {case:?}: expected the update to ask for the new declaration, got {properties}"
        );
        seen.push(request_id.clone());
        if row.cancelled {
            // The caller ends the update instead: the provider is never answered at all.
            let (status_code, cancelled) = call(
                &router,
                "CancelUpdateStack",
                &[member("StackName", row.stack_name)],
            )
            .await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected CancelUpdateStack to be accepted, got {status_code} \
                 with {cancelled}"
            );
        } else {
            put_response(
                &router,
                &engine,
                &stack_id,
                THING,
                &request_id,
                &json!({
                    "Status": "FAILED",
                    "PhysicalResourceId": MINTED,
                    "Reason": "the provider refused the change",
                }),
            )
            .await;
        }

        // Either way the update is rolled back, which asks the provider for the configuration
        // the resource was last deployed with — and THAT is the request this row refuses.
        let (_, request_id, properties, request_type) = next_request(&engine, row.stack_name, &seen)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the rollback must reach the provider"));
        assert_eq!(
            request_type, "Update",
            "case {case:?}: expected the rollback to ask for an update, got {request_type}"
        );
        assert_eq!(
            properties.get("Marker").and_then(Value::as_str),
            Some("v1"),
            "case {case:?}: expected the rollback to ask for the previous declaration, got \
             {properties}"
        );
        seen.push(request_id.clone());
        put_response(
            &router,
            &engine,
            &stack_id,
            THING,
            &request_id,
            &json!({
                "Status": "FAILED",
                "PhysicalResourceId": MINTED,
                "Reason": REFUSAL,
            }),
        )
        .await;

        // The rollback could not be carried out: the stack rests in the one status a rollback
        // is continued from, and the resource reports the FAILED update rather than the
        // completion the rollback's walk had planned for it.
        let (settled_status, described) = settled(&router, case, row.stack_name).await;
        assert_eq!(
            settled_status, status::UPDATE_ROLLBACK_FAILED,
            "case {case:?}: expected the refused rollback to rest in {}, got {described}",
            status::UPDATE_ROLLBACK_FAILED
        );
        let reported = resource_status_of(&router, case, row.stack_name, THING).await;
        assert_eq!(
            reported.as_deref(),
            Some(resource_status::UPDATE_FAILED),
            "case {case:?}: expected the resource the rollback could not put back to report \
             {}, got {reported:?}",
            resource_status::UPDATE_FAILED
        );
        // ...and its own event stream says the same: the walk had PLANNED the completion of
        // that revert, and the answer that refused it is what the event reports — as the
        // FAILED response CloudFormation received from the resource, quoting the message that
        // response stated and naming the request it answered.
        let event = newest_event_of(&router, case, row.stack_name, THING).await;
        assert_eq!(
            event.as_ref().map(|(status, _)| status.as_str()),
            Some(resource_status::UPDATE_FAILED),
            "case {case:?}: expected the refused revert's own event to be {}, got {event:?}",
            resource_status::UPDATE_FAILED
        );
        let expected_reason = failed_answer_reason(REFUSAL, &request_id);
        assert_eq!(
            event.as_ref().and_then(|(_, reason)| reason.clone()),
            Some(expected_reason.clone()),
            "case {case:?}: expected the event to read {expected_reason:?}, got {event:?}"
        );

        // ...and the rollback is resumable from there, in each of the three spellings.
        let mut members = vec![member("StackName", row.stack_name)];
        for (key, value) in row.resumption {
            members.push(member(key, value));
        }
        let (status_code, resumed) = call(&router, "ContinueUpdateRollback", &members).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected the resumption to be served, got {status_code} with {resumed}"
        );
        // The resumption is the stack's own operation, started by a CLIENT: the event that
        // opens it says so.
        let opened = newest_event_of(&router, case, row.stack_name, row.stack_name).await;
        assert_eq!(
            opened,
            Some((
                status::UPDATE_ROLLBACK_IN_PROGRESS.to_string(),
                Some(USER_INITIATED.to_string())
            )),
            "case {case:?}: expected the resumption to open with {} ({USER_INITIATED:?}), got \
             {opened:?}",
            status::UPDATE_ROLLBACK_IN_PROGRESS
        );
        if row.asks_again {
            // A resumption that skips nothing has the previous configuration still to put
            // back, so the provider is asked for it AGAIN.
            let (_, request_id, properties, request_type) =
                next_request(&engine, row.stack_name, &seen)
                    .await
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the resumed rollback must reach the provider again")
                    });
            assert_eq!(
                request_type, "Update",
                "case {case:?}: expected the resumption to ask for an update, got {request_type}"
            );
            assert_eq!(
                properties.get("Marker").and_then(Value::as_str),
                Some("v1"),
                "case {case:?}: expected the resumption to ask for the previous declaration, \
                 got {properties}"
            );
            seen.push(request_id.clone());
            put_response(
                &router,
                &engine,
                &stack_id,
                THING,
                &request_id,
                &json!({ "Status": "SUCCESS", "PhysicalResourceId": MINTED }),
            )
            .await;
        } else {
            // A resumption that SKIPS the resource leaves it exactly as the failed update
            // made it: nothing is asked of the provider, which is what lets this row settle
            // with no further answer at all.
            assert!(
                unseen_request(&engine, row.stack_name, &seen).is_none(),
                "case {case:?}: expected a skipped resource to be asked nothing"
            );
        }

        let (settled_status, described) = settled(&router, case, row.stack_name).await;
        assert_eq!(
            settled_status, status::UPDATE_ROLLBACK_COMPLETE,
            "case {case:?}: expected the resumed rollback to reach {}, got {described}",
            status::UPDATE_ROLLBACK_COMPLETE
        );
        // "CloudFormation sets the status of the specified resources to UPDATE_COMPLETE and
        // continues to roll back the stack" — and a resource the resumption really did put
        // back reports the same completion of its own update.
        let reported = resource_status_of(&router, case, row.stack_name, THING).await;
        assert_eq!(
            reported.as_deref(),
            Some(resource_status::UPDATE_COMPLETE),
            "case {case:?}: expected the resumed rollback to leave the resource {}, got \
             {reported:?}",
            resource_status::UPDATE_COMPLETE
        );
        let event = newest_event_of(&router, case, row.stack_name, THING).await;
        assert_eq!(
            event.as_ref().map(|(status, _)| status.as_str()),
            Some(resource_status::UPDATE_COMPLETE),
            "case {case:?}: expected the resumption's own event for the resource to be {}, got \
             {event:?}",
            resource_status::UPDATE_COMPLETE
        );
        // ...and a resource it SKIPPED says so: it reports that completion without having
        // been put back, so the reason names the operation that skipped it, while one the
        // resumption really did put back carries no reason at all.
        let expected_reason = if row.asks_again {
            None
        } else {
            Some(SKIPPED_DURING_UPDATE_ROLLBACK.to_string())
        };
        assert_eq!(
            event.as_ref().and_then(|(_, reason)| reason.clone()),
            expected_reason,
            "case {case:?}: expected the resource's completion to read {expected_reason:?}, got \
             {event:?}"
        );
    }
}
