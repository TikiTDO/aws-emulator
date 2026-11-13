// SPDX-License-Identifier: CC0-1.0

//! Who decides what becomes of an ACCEPTED asynchronous invocation — and who does not.
//!
//! The accepting call promises the queueing and nothing about the run ("A success response
//! (202 Accepted) indicates that the request is queued for invocation"), so everything a
//! client depends on afterwards is that the event is DELIVERED. Two rules are pinned here,
//! both about an event this emulator has already answered 202 for, and both about an outcome
//! that must not turn on how fast or how busy the host running the emulator happens to be.
//!
//! THE CLOCK OWES "nothing delivered it" ONLY WHERE NO ATTEMPT WAS EVER MADE. An event a
//! delivery attempt is pending for is being delivered, whether or not the host has got round
//! to the attempt yet, so the attempt's own outcome is the one the entry keeps however long
//! past the attempt's due instant anything else reads the function. An event nothing is
//! delivering — what an acceptance in a process with no asynchronous runtime leaves behind —
//! is the one the clock settles, and it still is.
//!
//! HOW LONG A QUEUED EVENT KEEPS ITS PLACE IS WHAT THE CONFIGURATION STATES. An attempt that
//! found no capacity to run the function stops nothing about the event: "If your function is
//! throttled, Lambda returns the event to the queue and attempts to run the function again
//! for up to six hours." How long that is is "the maximum age of a request that Lambda sends
//! to a function for processing"
//! (`smithy_query(query="shape", shape="MaximumEventAgeInSeconds")`, which the
//! asynchronous-invocation configuration of a function, version or alias carries), and the
//! documented six hours for a qualifier nobody has configured.
//!
//! Nothing here waits out a span: where a row needs the clock to have passed a due instant it
//! MOVES the clock past it rather than sleeping until it, and the one wait is for the
//! condition that the delivery settled, looked at again until it holds or the ceiling the
//! emulator's own bounds give it passes.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    event_retention_of, AsyncInvocationOutcome, AsyncInvocationRecord,
    ComAmazonawsLambdaAwsgirapiserviceState, ASYNC_NOT_DELIVERED_REASON,
    DEFAULT_ASYNC_EVENT_RETENTION,
};

// Every helper below is the one the asynchronous-invoke suite publishes: the same router, the
// same request path, the same function fixture (code published as version 1 with the alias
// `live` on it) and the same wait for a delivery to settle, whose ceiling that suite derives
// from the emulator's own bounds — so these rows and that suite's agree about what they drive.
use crate::asynchronous_invoke_run::{
    accept_invocation, call, create_function, held, router_on, settled_newest, Accept,
    PYTHON_HANDLER, PYTHON_SOURCE,
};

/// The runtime every function here is created with, which this emulator embeds an engine for.
const RUNTIME: &str = "python3.12";

/// The module file the python package carries its handler in.
const ENTRY: &str = "lambda_function.py";

/// An event the handler returns on, so the outcome recorded for a delivery that HAPPENED is
/// a run with no `FunctionError` — distinguishable from every outcome of a run that did not.
const RETURNING_EVENT: &[u8] = br#"{"fail":false}"#;

/// Where `PutFunctionEventInvokeConfig` is bound
/// (`smithy_query(query="operation", operation="PutFunctionEventInvokeConfig")`:
/// `PUT /2019-09-25/functions/{FunctionName}/event-invoke-config`).
const EVENT_INVOKE_CONFIG_URI: &str = "/2019-09-25/functions";

/// Bring every function the store holds up to the clock as it will stand `ahead` from now.
///
/// This is what WAITING that long would do to the entries, without waiting: the materialising
/// a read performs is a function of the instant it is given, so handing it a later instant
/// settles exactly what the clock will owe then. A row that needs the attempt's due instant
/// to have passed uses this, so no row here depends on the host reaching that instant while
/// the row is looking.
fn settle_what_the_clock_will_owe(engine: &Arc<engine::InMemoryEngine>, ahead: Duration) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let then = SystemTime::now() + ahead;
    store.materialize_all(then);
}

/// The request id of the newest event held for `name`, which is the one the row just
/// accepted.
fn newest_request_id(engine: &Arc<engine::InMemoryEngine>, case: &str, name: &str) -> String {
    held(engine, name)
        .into_iter()
        .next_back()
        .unwrap_or_else(|| panic!("case {case:?}: the accepted event must be held for {name}"))
        .request_id
}

/// One row of the ownership table: which call accepted the event, which reference it
/// addressed, and how far past the attempt's due instant the clock is read.
struct Ownership {
    case: &'static str,
    accept: Accept,
    /// The reference the accepting call addresses, which decides the qualifier the event is
    /// queued against.
    reference: &'static str,
    /// How far ahead of now the clock is settled, which is always past the attempt's own due
    /// instant.
    ahead: Duration,
}

/// The ownership table: both accepting operations, each documented qualifier form, and a
/// clock read just past the attempt's due instant as well as one long past it — including
/// past the whole window the event would be retained for.
fn ownership_cases() -> Vec<Ownership> {
    vec![
        Ownership {
            case: "an Invoke Event read just past its attempt instant",
            accept: Accept::InvokeEvent,
            reference: "clock-owns-nothing",
            ahead: Duration::from_secs(1),
        },
        Ownership {
            case: "an InvokeAsync read just past its attempt instant",
            accept: Accept::InvokeAsync,
            reference: "clock-owns-nothing",
            ahead: Duration::from_secs(1),
        },
        Ownership {
            case: "an Invoke Event addressed to a published version, read an hour past it",
            accept: Accept::InvokeEvent,
            reference: "clock-owns-nothing:1",
            ahead: Duration::from_secs(3_600),
        },
        Ownership {
            case: "an Invoke Event addressed through an alias, read a day past it",
            accept: Accept::InvokeEvent,
            reference: "clock-owns-nothing:live",
            ahead: Duration::from_secs(86_400),
        },
        Ownership {
            case: "an InvokeAsync read past the whole window the event is retained for",
            accept: Accept::InvokeAsync,
            reference: "clock-owns-nothing",
            ahead: DEFAULT_ASYNC_EVENT_RETENTION + Duration::from_secs(1),
        },
    ]
}

/// An event a delivery attempt is pending for is the ATTEMPT's to settle: the clock, however
/// far past the attempt's due instant it is read, never records that nothing delivered it.
#[tokio::test]
async fn the_clock_leaves_an_event_its_attempt_is_pending_for_to_that_attempt() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "clock-owns-nothing",
        RUNTIME,
        PYTHON_HANDLER,
        ENTRY,
        PYTHON_SOURCE,
    )
    .await;

    for case in ownership_cases() {
        let accepted =
            accept_invocation(&router, case.accept, case.reference, RETURNING_EVENT).await;
        assert_eq!(
            accepted.status,
            202,
            "case {:?}: expected the documented 202 Accepted, got {} {}",
            case.case,
            accepted.status,
            accepted.text()
        );

        // The clock as it will stand well past this event's attempt instant, applied to the
        // store the way every read of the function applies it.
        settle_what_the_clock_will_owe(&engine, case.ahead);
        let read = held(&engine, "clock-owns-nothing")
            .into_iter()
            .next_back()
            .unwrap_or_else(|| panic!("case {:?}: the accepted event must be held", case.case));
        assert!(
            !matches!(read.outcome, AsyncInvocationOutcome::Failed(_)),
            "case {:?}: expected the outcome to be left to the pending delivery attempt, got \
             {:?}",
            case.case,
            read.outcome
        );

        // And the attempt then settles it, with the run the handler really made on it: the
        // clock passing the due instant took nothing away from the delivery.
        let settled = settled_newest(&engine, case.case, "clock-owns-nothing").await;
        assert_eq!(
            settled.outcome,
            AsyncInvocationOutcome::Delivered {
                function_error: None
            },
            "case {:?}: expected the event delivered and the handler's run recorded, got {:?}",
            case.case,
            settled.outcome
        );
        assert!(
            settled.payload == RETURNING_EVENT,
            "case {:?}: expected the delivery to carry the event the call did, got {:?}",
            case.case,
            String::from_utf8_lossy(&settled.payload)
        );
    }
}

/// The other half of that rule, which the change above leaves standing: an event NO attempt
/// is pending for — what an acceptance in a process with no asynchronous runtime to deliver
/// on leaves behind — is the one the clock settles, as soon as its attempt instant has
/// passed.
#[tokio::test]
async fn an_event_no_attempt_is_pending_for_is_settled_by_the_clock() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(
        &router,
        "clock-settles-it",
        RUNTIME,
        PYTHON_HANDLER,
        ENTRY,
        PYTHON_SOURCE,
    )
    .await;

    // One row per queue position an unattended event can be in: its attempt already due, and
    // its attempt still to come.
    let rows: &[(&str, Duration, bool)] = &[
        ("an attempt instant already passed", Duration::ZERO, true),
        (
            "an attempt instant still to come",
            Duration::from_secs(3_600),
            false,
        ),
    ];
    for (case, due_in, settled_by_the_clock) in rows {
        let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
        let request_id = format!("unattended-{case}");
        {
            let mut store = state.lock();
            let function = store
                .functions
                .get_mut("clock-settles-it")
                .unwrap_or_else(|| panic!("case {case:?}: the function must be held"));
            function.async_invocations.push_back(AsyncInvocationRecord {
                version: "$LATEST".to_string(),
                request_id: request_id.clone(),
                payload: RETURNING_EVENT.to_vec(),
                received: "2026-01-01T00:00:00.000Z".to_string(),
                attempt_due: SystemTime::now() + *due_in,
                attempts: 0,
                // Nothing is delivering this event, which is the whole of the case.
                attempt_pending: false,
                outcome: AsyncInvocationOutcome::Queued,
            });
        }

        settle_what_the_clock_will_owe(&engine, Duration::ZERO);
        let outcome = held(&engine, "clock-settles-it")
            .into_iter()
            .find(|entry| entry.request_id == request_id)
            .unwrap_or_else(|| panic!("case {case:?}: the held event must still be there"))
            .outcome;
        let expected = if *settled_by_the_clock {
            AsyncInvocationOutcome::Failed(ASYNC_NOT_DELIVERED_REASON.to_string())
        } else {
            AsyncInvocationOutcome::Queued
        };
        assert_eq!(
            outcome, expected,
            "case {case:?}: expected the clock to leave the event {expected:?}, got {outcome:?}"
        );
    }
}

/// One row of the retention table: what a caller configured, on which qualifier, which
/// reference the accepting call addressed, and the window in force for the event it queued.
struct Retention {
    case: &'static str,
    /// The function this row creates, so each row starts from a function nobody configured.
    function: &'static str,
    /// The qualifier the configuration is put on, when the row puts one.
    configured_qualifier: Option<&'static str>,
    /// The `MaximumEventAgeInSeconds` that configuration states.
    configured_age: Option<i32>,
    /// The reference the accepting call addresses.
    reference: &'static str,
    /// The window the queued event keeps its place in.
    expected: Duration,
}

/// The retention table: a qualifier nobody configured, the smallest and the largest age the
/// model admits (`smithy_query(query="shape", shape="MaximumEventAgeInSeconds")`: 60 to
/// 21600) and one inside that range, a configuration reached through an alias, and a
/// configuration on a qualifier the call did NOT address — which is no window of this event's.
fn retention_cases() -> Vec<Retention> {
    vec![
        Retention {
            case: "no configuration at all",
            function: "retention-default",
            configured_qualifier: None,
            configured_age: None,
            reference: "retention-default",
            expected: DEFAULT_ASYNC_EVENT_RETENTION,
        },
        Retention {
            case: "the smallest age the model admits, on the unpublished version",
            function: "retention-minimum",
            configured_qualifier: Some("$LATEST"),
            configured_age: Some(60),
            reference: "retention-minimum",
            expected: Duration::from_secs(60),
        },
        Retention {
            case: "an age inside the range, on the unpublished version",
            function: "retention-middle",
            configured_qualifier: Some("$LATEST"),
            configured_age: Some(900),
            reference: "retention-middle",
            expected: Duration::from_secs(900),
        },
        Retention {
            case: "the largest age the model admits, on a published version",
            function: "retention-maximum",
            configured_qualifier: Some("1"),
            configured_age: Some(21_600),
            reference: "retention-maximum:1",
            expected: DEFAULT_ASYNC_EVENT_RETENTION,
        },
        Retention {
            case: "an alias's own configuration, reached through that alias",
            function: "retention-alias",
            configured_qualifier: Some("live"),
            configured_age: Some(120),
            reference: "retention-alias:live",
            expected: Duration::from_secs(120),
        },
        Retention {
            case: "a configuration on a qualifier the call did not address",
            function: "retention-elsewhere",
            configured_qualifier: Some("live"),
            configured_age: Some(120),
            reference: "retention-elsewhere",
            expected: DEFAULT_ASYNC_EVENT_RETENTION,
        },
    ]
}

/// How long a queued event keeps its place while this emulator tries to get the function run
/// for it is what the addressed qualifier's asynchronous-invocation configuration states, and
/// the documented six hours where nobody stated one.
#[tokio::test]
async fn the_window_a_queued_event_keeps_its_place_in_is_what_the_configuration_states() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();

    for case in retention_cases() {
        create_function(
            &router,
            case.function,
            RUNTIME,
            PYTHON_HANDLER,
            ENTRY,
            PYTHON_SOURCE,
        )
        .await;
        if let (Some(qualifier), Some(age)) = (case.configured_qualifier, case.configured_age) {
            let configured = call(
                &router,
                "PUT",
                &format!(
                    "{EVENT_INVOKE_CONFIG_URI}/{}/event-invoke-config?Qualifier={qualifier}",
                    case.function
                ),
                &serde_json::to_vec(&json!({ "MaximumEventAgeInSeconds": age }))
                    .expect("the configuration body serializes"),
                &[],
            )
            .await;
            assert_eq!(
                configured.status,
                200,
                "case {:?}: PutFunctionEventInvokeConfig must succeed, got {} {}",
                case.case,
                configured.status,
                configured.text()
            );
        }

        let accepted =
            accept_invocation(&router, Accept::InvokeEvent, case.reference, RETURNING_EVENT).await;
        assert_eq!(
            accepted.status,
            202,
            "case {:?}: expected the documented 202 Accepted, got {} {}",
            case.case,
            accepted.status,
            accepted.text()
        );

        let request_id = newest_request_id(&engine, case.case, case.function);
        let window = event_retention_of(&state, case.function, &request_id);
        assert_eq!(
            window, case.expected,
            "case {:?}: expected the event retained for {:?}, got {window:?}",
            case.case, case.expected
        );

        // The event is delivered either way: the window bounds how long an attempt that
        // found no capacity keeps being made, and takes nothing from an attempt that runs.
        let settled = settled_newest(&engine, case.case, case.function).await;
        assert_eq!(
            settled.outcome,
            AsyncInvocationOutcome::Delivered {
                function_error: None
            },
            "case {:?}: expected the event delivered and run, got {:?}",
            case.case,
            settled.outcome
        );
    }
}
