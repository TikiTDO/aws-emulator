// SPDX-License-Identifier: CC0-1.0

//! A CUSTOM RESOURCE whose provider never answered its `Create` was never created, so the
//! ROLLBACK of that failed create has nothing to remove for it.
//!
//! RULE: a custom resource is brought into being by the answer its provider PUTs to the
//! request CloudFormation sent it and by nothing else — until that answer arrives the only
//! identifier the resource has is the stand-in CloudFormation computed for it, which
//! addresses no resource any provider minted — so a `Create` no provider answered creates
//! nothing: the rollback of that failed create asks the provider to delete nothing, takes the
//! record down, and the stack reaches `ROLLBACK_COMPLETE`. `ROLLBACK_FAILED` is the status of
//! a rollback that could NOT be carried out, which a resource nothing ever created cannot
//! make true.
//!
//! The complement is here too: a custom resource the provider DID answer exists, so the same
//! rollback sends that provider the `Delete` it owes and is held open until it answers —
//! which is what says the rule turns on the provider's ANSWER and not on the resource being
//! a custom one.
//!
//! SECOND RULE, which is what makes the first one about an answer that never came rather than
//! about one there was no time for: **the window a wait gives whoever was asked runs from the
//! instant the CALL was made, not from the instant the stack operation started** — a stack
//! operation walks its resources one after another, so a resource it reaches late in a long
//! walk is asked about long after the operation began, and measuring the window from the
//! operation would leave such a resource only what is left of it, or nothing at all.
//!
//! Every case drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint, the provider's answer as the plain unsigned `PUT` to
//! the published `ResponseURL` a provider really makes, and the resources the rollback had to
//! remove read back through the service that OWNS them (AWS Systems Manager).
//!
//! The rows differ along what the rules generalise over: how the template spells the custom
//! resource's type (the author's own `Custom::<Name>` namespace and the generic
//! `AWS::CloudFormation::CustomResource`), how much the create had really made beside it that
//! the rollback has to remove (nothing, one resource, two), what the resource's own
//! `DeletionPolicy` says about a physical resource that was never created (the default, and
//! `Retain`), and how long the operation had been walking by the time it reached the resource
//! (just past one window, and many windows).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_cloudformation_cloudformation::state::stabilising::CREATE_STABILISATION_WINDOW;
use service_com_amazonaws_cloudformation_cloudformation::state::{resource_status, status};
use smithy_server_runtime::MultiServiceRouter;

use crate::custom_resource_providers::{
    emulator, provider_token, put_response, POLL_ATTEMPTS, POLL_INTERVAL,
};
use crate::harness::{call, create_stack, member, store, xml_text, REGION};
use crate::update_rollback_recovery::{
    parameter, parameter_value, stack_events, template, template_with_an_unsignalled_gate,
};

/// The logical id every row declares its custom resource under.
const CUSTOM: &str = "Waiter";

/// The `PhysicalResourceId` the responsive provider of the complement below mints.
const MINTED: &str = "waiter-instance-1";

/// The suffix every status a stack operation in flight carries ends in.
const IN_PROGRESS: &str = "_IN_PROGRESS";

/// One row: how the stack declares the custom resource whose provider never answers, and
/// what the create had really made beside it.
struct Row {
    case: &'static str,
    stack: &'static str,
    /// The custom resource's `Type`: `Custom::<Name>`, the author's own namespace, or
    /// `AWS::CloudFormation::CustomResource`, the generic spelling — the two forms a custom
    /// resource's type has.
    resource_type: &'static str,
    /// The `DeletionPolicy` the declaration carries, or `None` for the documented default.
    deletion_policy: Option<&'static str>,
    /// The status the rollback's teardown of the record reports for the custom resource:
    /// `DELETE_COMPLETE` for the default policy, and `DELETE_SKIPPED` for `Retain`, which is
    /// "the resource is kept without deleting the resource or its contents" — the same
    /// reporting as for every other resource the teardown asks nobody about.
    teardown: &'static str,
    /// The Parameter Store names the create really made beside the custom resource, which
    /// the rollback has to delete: what proves the rollback RAN rather than merely reporting
    /// a status.
    siblings: &'static [&'static str],
}

/// The class the rule covers: a create no provider answered, however the stack declares it
/// and whatever else the create had made.
const ROWS: &[Row] = &[
    Row {
        case: "the generic type, with one sibling the rollback has to delete",
        stack: "uncreated-generic",
        resource_type: "AWS::CloudFormation::CustomResource",
        deletion_policy: None,
        teardown: resource_status::DELETE_COMPLETE,
        siblings: &["/uncreated-generic/sibling"],
    },
    Row {
        case: "the author's own namespace, with two siblings the rollback has to delete",
        stack: "uncreated-namespaced",
        resource_type: "Custom::SlowProvisioner",
        deletion_policy: None,
        teardown: resource_status::DELETE_COMPLETE,
        siblings: &["/uncreated-namespaced/first", "/uncreated-namespaced/second"],
    },
    Row {
        case: "the custom resource is the whole stack, so the rollback has nothing else",
        stack: "uncreated-alone",
        resource_type: "Custom::Waiter",
        deletion_policy: None,
        teardown: resource_status::DELETE_COMPLETE,
        siblings: &[],
    },
    Row {
        case: "a Retain policy on a physical resource that was never created",
        stack: "uncreated-retained",
        resource_type: "Custom::RetainedWaiter",
        deletion_policy: Some("Retain"),
        teardown: resource_status::DELETE_SKIPPED,
        siblings: &["/uncreated-retained/sibling"],
    },
];

/// The custom resource one row declares: the provider topic as its `ServiceToken`, the row's
/// own `DeletionPolicy`, and a dependency on every sibling, so the custom resource is the
/// LAST resource the create walks — the shape a stack that deploys its own provider has.
fn custom_resource(row: &Row) -> Value {
    let mut declaration = Map::new();
    declaration.insert("Type".to_string(), json!(row.resource_type));
    declaration.insert(
        "Properties".to_string(),
        json!({"ServiceToken": provider_token()}),
    );
    if let Some(policy) = row.deletion_policy {
        declaration.insert("DeletionPolicy".to_string(), json!(policy));
    }
    if !row.siblings.is_empty() {
        declaration.insert(
            "DependsOn".to_string(),
            json!(sibling_ids(row.siblings.len())),
        );
    }
    Value::Object(declaration)
}

/// The logical ids the siblings of one row are declared under.
fn sibling_ids(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("Sibling{index}")).collect()
}

/// The template one row deploys.
fn template_of(row: &Row) -> String {
    let mut resources = Map::new();
    for (logical_id, name) in sibling_ids(row.siblings.len())
        .into_iter()
        .zip(row.siblings)
    {
        resources.insert(logical_id, parameter(name, "made-by-the-create"));
    }
    resources.insert(CUSTOM.to_string(), custom_resource(row));
    template(Value::Object(resources))
}

/// Every provider request the stack's open waits carry right now: the `RequestType` of each
/// one and the logical id it is about.
///
/// This is what says WHICH requests CloudFormation sent the provider, because a request it
/// sent is one it is waiting for an answer to.
fn awaited_requests(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
) -> Vec<(String, String)> {
    let held = store(engine);
    let held = held.lock();
    let Some(stack) = held
        .stacks
        .iter()
        .find(|stack| stack.stack_name == stack_name)
    else {
        return Vec::new();
    };
    stack
        .stabilisations
        .iter()
        .filter_map(|barrier| {
            let request = barrier.provider_request.as_ref()?;
            Some((request.request_type.clone(), barrier.logical_id.clone()))
        })
        .collect()
}

/// Move the OPERATION behind the wait `by` further into the past — the instant it started,
/// both on the stack and on the wait's own record of it — leaving the instant the provider was
/// asked where it is.
///
/// This is the shape of an operation whose walk took that long to REACH the resource the wait
/// is about: it started long ago, and the request about this resource has only just gone out.
fn age_the_operation_behind_the_wait(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    by: Duration,
) {
    let held = store(engine);
    let mut locked = held.lock();
    let stack = locked
        .resolve_mut(REGION, stack_name)
        .unwrap_or_else(|| panic!("the stack {stack_name:?} this case created"));
    let operation = stack
        .pending
        .as_mut()
        .unwrap_or_else(|| panic!("the operation {stack_name:?} has in flight"));
    operation.started_at -= by;
    for barrier in &mut stack.stabilisations {
        barrier.opened_at -= by;
    }
}

/// Move the wait for the provider's answer `by` further into the past, which is how a case
/// reaches the instant that wait had already run out at without waiting for it.
fn age_the_provider_wait(engine: &Arc<engine::InMemoryEngine>, stack_name: &str, by: Duration) {
    let held = store(engine);
    let mut locked = held.lock();
    let stack = locked
        .resolve_mut(REGION, stack_name)
        .unwrap_or_else(|| panic!("the stack {stack_name:?} this case created"));
    assert!(
        stack
            .stabilisations
            .iter()
            .any(|barrier| barrier.provider_request.is_some()),
        "case {stack_name:?}: expected the create to be waiting for the provider's answer, \
         got no such wait at all"
    );
    for barrier in &mut stack.stabilisations {
        barrier.asked_at -= by;
    }
}

/// Poll the stack until it leaves every `*_IN_PROGRESS` status, collecting every provider
/// request its waits carried on the way — and answering each one with `SUCCESS` where
/// `minted` names the identifier a responsive provider mints.
///
/// A read is what asks a wait about, so polling is what a client does. The collected requests
/// are what the provider was really SENT, in the order the operation sent them.
async fn settled_watching_the_provider(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    stack_name: &str,
    minted: Option<&str>,
) -> (String, Vec<(String, String)>) {
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut last = String::new();
    for _ in 0..POLL_ATTEMPTS {
        let (http_status, body) =
            call(router, "DescribeStacks", &[member("StackName", stack_name)]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected DescribeStacks to answer, got {http_status} {body}"
        );
        last = xml_text(&body, "StackStatus").unwrap_or_default();
        for request in awaited_requests(engine, stack_name) {
            if !seen.contains(&request) {
                seen.push(request);
            }
        }
        if !last.ends_with(IN_PROGRESS) {
            return (last, seen);
        }
        if let Some(minted) = minted {
            answer_every_awaited_request(router, engine, stack_name, minted).await;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {stack_name:?} never left an in-progress status; last was {last:?} with \
         the requests {seen:?}"
    );
}

/// Answer every request the stack is waiting for with `SUCCESS`, under the id the request
/// names where it names one (a `Delete`) and under `minted` for a `Create`.
async fn answer_every_awaited_request(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    minted: &str,
) {
    let unanswered = {
        let held = store(engine);
        let held = held.lock();
        let Some(stack) = held
            .stacks
            .iter()
            .find(|stack| stack.stack_name == stack_name)
        else {
            return;
        };
        let stack_id = stack.stack_id.clone();
        let requests: Vec<(String, String, Option<String>)> = stack
            .stabilisations
            .iter()
            .filter(|barrier| !barrier.released() && barrier.provider_response.is_none())
            .filter_map(|barrier| {
                let request = barrier.provider_request.as_ref()?;
                Some((
                    barrier.logical_id.clone(),
                    request.request_id.clone(),
                    request.physical_resource_id.clone(),
                ))
            })
            .collect();
        (stack_id, requests)
    };
    let (stack_id, requests) = unanswered;
    for (logical_id, request_id, physical_resource_id) in requests {
        let id = physical_resource_id.unwrap_or_else(|| minted.to_string());
        put_response(
            router,
            engine,
            &stack_id,
            &logical_id,
            &request_id,
            &json!({"Status": "SUCCESS", "PhysicalResourceId": id}),
        )
        .await;
    }
}

/// One instant past the window a provider's answer is waited for, which is where the wait has
/// run out.
fn past_the_window() -> Duration {
    CREATE_STABILISATION_WINDOW + Duration::from_secs(1)
}

#[tokio::test]
async fn a_create_no_provider_answered_rolls_the_stack_back_to_rollback_complete() {
    for row in ROWS {
        let case = row.case;
        let (router, engine) = emulator(case).await;
        let (http_status, body) = create_stack(&router, row.stack, &template_of(row), &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
        );

        // The request IS with the provider: that is what the create is waiting for, and what
        // nothing is going to answer.
        let awaited = awaited_requests(&engine, row.stack);
        assert_eq!(
            awaited,
            vec![("Create".to_string(), CUSTOM.to_string())],
            "case {case:?}: expected the create to be waiting for one Create answer about \
             {CUSTOM}, got {awaited:?}"
        );

        // The wait runs out. What the clock then owes is the failed create and the rollback
        // its `OnFailure` default asks for — and the rollback has nothing to remove for a
        // resource no provider ever created, so it CONCLUDES.
        age_the_provider_wait(&engine, row.stack, past_the_window());
        let (settled, seen) =
            settled_watching_the_provider(&router, &engine, case, row.stack, None).await;
        assert_eq!(
            settled,
            status::ROLLBACK_COMPLETE,
            "case {case:?}: expected the create whose provider never answered to roll back, \
             got {settled:?} with the provider requests {seen:?}"
        );

        // ...and the provider was asked NOTHING about the resource it never created: a
        // `Delete` for a resource that does not exist is a call no rollback owes, and a
        // rollback that made it would be held open by it.
        assert!(
            !seen
                .iter()
                .any(|(request_type, _)| request_type == "Delete"),
            "case {case:?}: expected the provider to have been sent no Delete at all, got \
             {seen:?}"
        );

        // The event stream says what became of the resource: its create FAILED, the teardown
        // of its record is reported as for every other resource nothing was created for, and
        // the stack's rollback completed.
        let events = stack_events(case, &router, row.stack).await;
        let failed = (CUSTOM.to_string(), resource_status::CREATE_FAILED.to_string());
        assert!(
            events.contains(&failed),
            "case {case:?}: expected the custom resource to report {failed:?}, got {events:?}"
        );
        let taken_down = (CUSTOM.to_string(), row.teardown.to_string());
        assert!(
            events.contains(&taken_down),
            "case {case:?}: expected the custom resource's teardown to report {taken_down:?}, \
             got {events:?}"
        );
        let rolled_back = (row.stack.to_string(), status::ROLLBACK_COMPLETE.to_string());
        assert!(
            events.contains(&rolled_back),
            "case {case:?}: expected the stack's own {rolled_back:?} event, got {events:?}"
        );

        // ...and everything the create REALLY made is gone from the service that owns it: a
        // rollback that reported itself complete while leaving one behind would leave a live,
        // name-claiming resource.
        for sibling in row.siblings {
            let held = parameter_value(&router, sibling).await;
            assert_eq!(
                held, None,
                "case {case:?}: expected the rollback to delete the sibling {sibling:?} the \
                 create had made, got {held:?}"
            );
        }
    }
}

/// One row of the second rule's table: how long the operation had already been running when
/// the provider was sent its request.
struct LongWalk {
    case: &'static str,
    stack: &'static str,
    resource_type: &'static str,
    /// How many windows' worth of walk the operation had behind it by the time it reached the
    /// custom resource: the near bound (just past one window) and a walk many windows long.
    windows: u32,
}

/// The class the second rule covers: an operation that reaches the resource LATE, however
/// late.
const LONG_WALKS: &[LongWalk] = &[
    LongWalk {
        case: "a walk that had just outlasted one window when it reached the resource",
        stack: "late-by-one-window",
        resource_type: "AWS::CloudFormation::CustomResource",
        windows: 1,
    },
    LongWalk {
        case: "a walk many windows long when it reached the resource",
        stack: "late-by-many-windows",
        resource_type: "Custom::LateProvisioner",
        windows: 5,
    },
];

#[tokio::test]
async fn the_wait_for_an_answer_runs_from_the_call_and_not_from_the_operations_start() {
    // The wait is the span the thing that was ASKED is given to carry the call out, so it runs
    // from the call: a resource an operation reaches late in a long walk gets the whole window
    // from its own request rather than what is left of the operation's — and none at all once
    // the operation has outlasted it.
    for row in LONG_WALKS {
        let case = row.case;
        let template_body = template(json!({
            CUSTOM: {
                "Type": row.resource_type,
                "Properties": {"ServiceToken": provider_token()},
            },
        }));
        let (router, engine) = emulator(case).await;
        let (http_status, body) = create_stack(&router, row.stack, &template_body, &[]).await;
        assert_eq!(
            http_status, 200,
            "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
        );

        // The operation has been running far longer than one window; the request to the
        // provider has only just gone out.
        age_the_operation_behind_the_wait(&engine, row.stack, past_the_window() * row.windows);
        for look in 0..3 {
            let (status_code, body) =
                call(&router, "DescribeStacks", &[member("StackName", row.stack)]).await;
            assert_eq!(
                status_code, 200,
                "case {case:?}: expected DescribeStacks to answer, got {status_code} {body}"
            );
            let reported = xml_text(&body, "StackStatus").unwrap_or_default();
            assert_eq!(
                reported,
                status::CREATE_IN_PROGRESS,
                "case {case:?}: expected the create to still be waiting for the provider's \
                 answer on look {look}, got {reported:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        // ...and the wait does END — from the instant of the call, which is what aging THAT
        // instant reaches.
        age_the_provider_wait(&engine, row.stack, past_the_window());
        let (settled, seen) =
            settled_watching_the_provider(&router, &engine, case, row.stack, None).await;
        assert_eq!(
            settled,
            status::ROLLBACK_COMPLETE,
            "case {case:?}: expected the wait measured from the call to run out and the create \
             to roll back, got {settled:?} with the provider requests {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_create_the_provider_answered_is_deleted_by_the_rollback() {
    // The complement of the rule: this provider ANSWERS, so the custom resource really
    // exists — and the create fails anyway, at a wait condition nothing signals. The
    // rollback then owes that provider the `Delete` of the resource it minted.
    let case = "a provider that answered the create, with the create failed by its own gate";
    let stack = "answered-then-rolled-back";
    let (router, engine) = emulator(case).await;
    let declared = json!({
        CUSTOM: {
            "Type": "Custom::AnsweringProvisioner",
            "Properties": {"ServiceToken": provider_token()},
        },
    });
    let (http_status, body) = create_stack(
        &router,
        stack,
        &template_with_an_unsignalled_gate(declared),
        &[],
    )
    .await;
    assert_eq!(
        http_status, 200,
        "case {case:?}: expected CreateStack to succeed, got {http_status} {body}"
    );

    let (settled, seen) =
        settled_watching_the_provider(&router, &engine, case, stack, Some(MINTED)).await;
    assert_eq!(
        settled,
        status::ROLLBACK_COMPLETE,
        "case {case:?}: expected the create failed by its gate to roll back, got {settled:?} \
         with the provider requests {seen:?}"
    );
    for expected in [
        ("Create".to_string(), CUSTOM.to_string()),
        ("Delete".to_string(), CUSTOM.to_string()),
    ] {
        assert!(
            seen.contains(&expected),
            "case {case:?}: expected the provider to have been sent {expected:?}, got {seen:?}"
        );
    }
}
