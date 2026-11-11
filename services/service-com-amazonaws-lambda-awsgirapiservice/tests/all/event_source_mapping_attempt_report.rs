// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHEN an event source mapping reports what its processing attempt
//! came to: `EventSourceMappingConfiguration$LastProcessingResult` is "the result of the
//! event source mapping's last processing attempt".
//!
//! The rule under test, in one sentence: a mapping reports a last processing result once it
//! has made an attempt and that attempt has been decided — so a mapping that has polled
//! nothing reports none, and a mapping whose batch was handed to a function that cannot run
//! reports the unsuccessful attempt it made, however long the queue hides a delivery for and
//! however long its function is allowed to run.
//!
//! That the report follows the ATTEMPT and not the poll is what the batch leaving the queue
//! does not yet say: a message goes out of its queue when the attempt STARTS, and the result
//! is the mapping's only once the invocation it was handed to has answered. Every row here
//! therefore waits for the report ([`processing_result_within_budget`]) rather than reading
//! it at an instant of its own.
//!
//! Every row drives real HTTP requests through the production `MultiServiceRouter` with both
//! Lambda and Amazon SQS on ONE engine — the queue is created, re-configured and read
//! through Amazon SQS exactly as a client does it — and the rows differ along what the rule
//! generalises over: which of the two spans is the longer one, whether the queue's span is
//! lowered under the function's after the mapping was made, how many messages are waiting and
//! how many of them one batch may hold.
//!
//! Nothing here runs a deployment package, so no batch is ever processed: the attempt every
//! row observes is the unsuccessful one, and the messages it was made on stay on the queue.

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED;

use crate::event_source_mappings::{
    await_mapping_state, create_mapping, create_queue, create_timed_function, get_mapping,
    processing_result_within_budget, queue_held, router_with_queues, send_message,
    set_visibility_timeout, STATE_ENABLED,
};

/// One row: the queue's `VisibilityTimeout` when the mapping is made, what a client lowers it
/// to afterwards (when a row does), the mapped function's `Timeout`, how many messages are
/// sent and how many of them one batch may hold.
struct Row {
    case: &'static str,
    visibility_timeout: i64,
    lowered_to: Option<i64>,
    function_timeout: i64,
    messages: usize,
    batch_size: i64,
}

/// The catalog: the two spans equal, the queue's span the longer one, the queue's span
/// lowered under the function's after the pair was wired (the only way a client reaches a
/// shorter one, since a mapping is accepted only on a queue that outlasts its function), one
/// message and several, and a batch ceiling both above and below what is waiting.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a one second queue and a one second function, one message",
            visibility_timeout: 1,
            lowered_to: None,
            function_timeout: 1,
            messages: 1,
            batch_size: 10,
        },
        Row {
            case: "a ten second queue, a two second function, three messages",
            visibility_timeout: 10,
            lowered_to: None,
            function_timeout: 2,
            messages: 3,
            batch_size: 10,
        },
        Row {
            case: "a queue lowered to hide nothing under a five second function",
            visibility_timeout: 5,
            lowered_to: Some(0),
            function_timeout: 5,
            messages: 2,
            batch_size: 1,
        },
        Row {
            case: "a two second queue, a one second function, three messages one per batch",
            visibility_timeout: 2,
            lowered_to: None,
            function_timeout: 1,
            messages: 3,
            batch_size: 1,
        },
    ]
}

#[tokio::test]
async fn a_mapping_reports_the_attempt_it_made_and_reports_none_before_it_made_one() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let router = router_with_queues();
        let function = format!("esm-report-fn-{index}");
        let (url, arn) = create_queue(
            &router,
            case,
            &format!("esm-report-queue-{index}"),
            &json!({ "VisibilityTimeout": row.visibility_timeout.to_string() }),
        )
        .await;
        create_timed_function(&router, case, &function, row.function_timeout).await;

        let created = create_mapping(
            &router,
            &json!({
                "FunctionName": function,
                "EventSourceArn": arn,
                "BatchSize": row.batch_size,
            }),
        )
        .await;
        assert_eq!(
            created.status,
            202,
            "case {case:?}: expected CreateEventSourceMapping to answer 202, got {} {}",
            created.status,
            created.text()
        );
        let uuid = created
            .string("UUID")
            .unwrap_or_else(|| panic!("case {case:?}: no UUID in {}", created.text()));
        await_mapping_state(&router, case, &uuid, STATE_ENABLED).await;
        if let Some(lowered) = row.lowered_to {
            set_visibility_timeout(&router, case, &url, lowered).await;
        }

        // A mapping that has polled nothing has made no attempt, so it reports no result of
        // one: the member is absent, not an empty or a successful reading.
        let before = get_mapping(&router, &uuid).await;
        assert!(
            before.json().get("LastProcessingResult").is_none(),
            "case {case:?}: a mapping that has polled an empty queue has made no processing \
             attempt, so it must report no result of one, got {}",
            before.text()
        );

        for message in 0..row.messages {
            send_message(
                &router,
                case,
                &url,
                &format!("report-{index}-{message}"),
                None,
            )
            .await;
        }

        // The attempt the mapping makes on the batch it polls, as it reports it once that
        // attempt has been decided.
        let reported = processing_result_within_budget(&router, case, &uuid).await;
        assert_eq!(
            reported.as_deref(),
            Some(LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED),
            "case {case:?}: nothing here runs a deployment package, so the attempt this \
             mapping made on its batch must be reported as the unsuccessful one it was, got \
             {reported:?} in {}",
            get_mapping(&router, &uuid).await.text()
        );

        // And an attempt that processed nothing consumed nothing: every message sent is still
        // the queue's, whether it is waiting or in flight with the poller.
        let held = queue_held(&router, case, &url).await;
        assert_eq!(
            held,
            u32::try_from(row.messages).unwrap_or(u32::MAX),
            "case {case:?}: an attempt that processed no message of its batch deletes none of \
             them, so the queue must still hold all {} message(s), got {held}",
            row.messages
        );

        // The mapping is still the one polling that queue: the report is of an attempt it
        // made, not of a mapping that stood down.
        let after = get_mapping(&router, &uuid).await;
        assert_eq!(
            (after.status, after.string("State")),
            (200, Some(STATE_ENABLED.to_string())),
            "case {case:?}: the mapping must still report {STATE_ENABLED}, got {} {}",
            after.status,
            after.text()
        );
    }
}
