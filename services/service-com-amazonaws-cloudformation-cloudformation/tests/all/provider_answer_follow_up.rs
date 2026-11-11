// SPDX-License-Identifier: CC0-1.0

//! What a custom resource provider's LANDED answer owes is carried out without a further
//! client call.
//!
//! The rule under test: **a provider answers CloudFormation out of band, at the
//! `ResponseURL` of the request it was sent, and the next request the stack operation behind
//! that answer owes — the rollback of a failed update, the cleanup `Delete` of a replaced
//! resource — is published by CloudFormation itself; a client that only waits for that
//! request, without calling CloudFormation again, still receives it.**
//!
//! Every row drives the emulator through its own wire: the stack operations over
//! CloudFormation's awsQuery endpoint and the provider's answer as the plain, unsigned HTTP
//! `PUT` to the published `ResponseURL`. What the provider is SENT is read off the request
//! the emulator is waiting for an answer to, which is the record that `ResponseURL`
//! addresses — and between the answer and the assertion NOTHING calls CloudFormation, which
//! is the whole point.
//!
//! The rows differ along what the rule generalises over: what the answer makes due (a failed
//! update's rollback, and a replacement's cleanup `Delete` of the resource it replaced).

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::custom_resource_providers::{
    awaited_request, emulator, put_response, settled, template, POLL_INTERVAL,
};
use super::harness::{call, create_stack, member, store};

/// How long a row waits for the request a landed answer owes, and how often it looks: the
/// request is published by a task the answer started, which looks again once per status
/// transition, so the window spans several of them.
const FOLLOW_UP_ATTEMPTS: usize = 120;

/// One row: what the provider answers the `Update` with, and the request that answer owes.
struct Row {
    case: &'static str,
    stack_name: &'static str,
    /// The `Status` the provider answers the `Update` with.
    status: &'static str,
    /// The `PhysicalResourceId` it answers with: the same one is an in-place update, a
    /// different one is a replacement.
    physical_resource_id: &'static str,
    /// The `RequestType` of the request that answer owes.
    owed: &'static str,
    /// The `Marker` property that owed request must carry.
    marker: &'static str,
}

/// The id the create is answered with.
const CREATED_ID: &str = "probe-created";

fn rows() -> Vec<Row> {
    vec![
        // A FAILED answer to an Update fails the resource, and the stack's rollback puts the
        // previous declaration back — which is an `Update` request of its own, carrying the
        // properties the resource was last deployed with.
        Row {
            case: "a failed update owes its rollback's own request",
            stack_name: "driven-rollback",
            status: "FAILED",
            physical_resource_id: CREATED_ID,
            owed: "Update",
            marker: "v1",
        },
        // The same, for an answer that names a DIFFERENT physical resource id: the resource
        // it REPLACED is the one the cleanup `Delete` addresses, with the OLD properties.
        Row {
            case: "a replacement owes the cleanup delete of what it replaced",
            stack_name: "driven-cleanup",
            status: "SUCCESS",
            physical_resource_id: "probe-replaced",
            owed: "Delete",
            marker: "v1",
        },
    ]
}

/// The request the stack is waiting for an answer to about `Thing`, polled until one appears
/// — with NOTHING calling CloudFormation in between, so the only thing that can publish it
/// is the emulator itself.
async fn follow_up_request(
    engine: &Arc<engine::InMemoryEngine>,
    stack_name: &str,
    answered: &str,
) -> Option<(String, Value, Option<String>)> {
    for _ in 0..FOLLOW_UP_ATTEMPTS {
        let held = store(engine);
        let found = {
            let held = held.lock();
            held.stacks
                .iter()
                .find(|stack| stack.stack_name == stack_name)
                .and_then(|stack| {
                    stack
                        .stabilisations
                        .iter()
                        .find(|barrier| {
                            !barrier.released()
                                && barrier.provider_response.is_none()
                                && barrier.provider_request.as_ref().is_some_and(|request| {
                                    request.request_id != answered
                                })
                        })
                        .and_then(|barrier| barrier.provider_request.clone())
                })
        };
        if let Some(request) = found {
            return Some((
                request.request_type,
                request.properties,
                request.physical_resource_id,
            ));
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    None
}

/// Update one stack's custom resource to the `v2` marker.
async fn update(router: &MultiServiceRouter, stack_name: &str) -> (u16, String) {
    call(
        router,
        "UpdateStack",
        &[
            member("StackName", stack_name),
            member("TemplateBody", &template("Custom::Probe", "v2", &[])),
        ],
    )
    .await
}

#[tokio::test]
async fn an_answer_that_lands_publishes_the_request_it_owes() {
    for row in rows() {
        let case = row.case;
        let (router, engine) = emulator(case).await;
        let (status_code, created) =
            create_stack(&router, row.stack_name, &template("Custom::Probe", "v1", &[]), &[]).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected CreateStack to be accepted, got {status_code} with {created}"
        );
        let (stack_id, request, _) = awaited_request(&engine, row.stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the create must be waiting for a request"));
        put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &request.request_id,
            &json!({ "Status": "SUCCESS", "PhysicalResourceId": CREATED_ID }),
        )
        .await;
        let (status, described) = settled(&router, case, row.stack_name).await;
        assert_eq!(
            status, "CREATE_COMPLETE",
            "case {case:?}: the created resource must complete the stack, got {described}"
        );

        let (status_code, updated) = update(&router, row.stack_name).await;
        assert_eq!(
            status_code, 200,
            "case {case:?}: expected UpdateStack to be accepted, got {status_code} with {updated}"
        );
        let (_, update_request, _) = awaited_request(&engine, row.stack_name, "Thing")
            .unwrap_or_else(|| panic!("case {case:?}: the update must be waiting for a request"));
        put_response(
            &router,
            &engine,
            &stack_id,
            "Thing",
            &update_request.request_id,
            &json!({
                "Status": row.status,
                "PhysicalResourceId": row.physical_resource_id,
                "Reason": "the row's own answer",
            }),
        )
        .await;

        // From here on NOTHING calls CloudFormation: the request the answer owes has to be
        // published by the emulator itself.
        let owed = follow_up_request(&engine, row.stack_name, &update_request.request_id).await;
        let Some((request_type, properties, physical_resource_id)) = owed else {
            panic!(
                "case {case:?}: expected a {} request to be published with no further \
                 CloudFormation call",
                row.owed
            );
        };
        assert_eq!(
            request_type, row.owed,
            "case {case:?}: expected a {} request, got {request_type} with {properties}",
            row.owed
        );
        assert_eq!(
            properties.get("Marker").and_then(Value::as_str),
            Some(row.marker),
            "case {case:?}: expected the owed request to carry the Marker {:?}, got {properties}",
            row.marker
        );
        if row.owed == "Delete" {
            assert_eq!(
                physical_resource_id.as_deref(),
                Some(CREATED_ID),
                "case {case:?}: the cleanup must address the resource that was replaced, got \
                 {physical_resource_id:?}"
            );
        }
    }
}

