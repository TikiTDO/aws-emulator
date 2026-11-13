// SPDX-License-Identifier: CC0-1.0

//! What a function's RESERVED concurrency bounds, under a burst of callers that all arrive
//! at once: the simultaneous executions of THAT function, and of no other.
//!
//! RULE: the simultaneous-execution bound an `Invoke` is admitted against is the addressed
//! function's own reservation — "the number of simultaneous executions to reserve for the
//! function", which "applies to the function as a whole"
//! (`PutFunctionConcurrency`) — so of callers arriving together exactly as many as the
//! reservation allows are admitted and every further one is refused with the modelled
//! throttle: "Lambda returns `TooManyRequestsException` if running the function would cause
//! you to exceed a concurrency limit at either the account level
//! (`ConcurrentInvocationLimitExceeded`) or function level
//! (`ReservedFunctionConcurrentInvocationLimitExceeded`)" (`Invoke`), on HTTP 429 with the
//! throttle's own sentence and the function-level `Reason`. A reservation is capacity that
//! belongs to one function, so it decides nothing about another function's callers.
//!
//! The rows differ along what the rule generalises over: the reservation in force, how many
//! callers arrive at once, and the relation between the two (more callers than the
//! reservation admits, and exactly as many as it admits) — and the second test varies the
//! FUNCTION the simultaneous callers address, which is what makes the bound per function
//! rather than per emulator.
//!
//! Each row asserts both halves: how many of the burst were served, each with its OWN run's
//! value, and that every refusal is the modelled throttle naming the reservation.
//!
//! Driven over the wire through the same `MultiServiceRouter` the emulator's front doors
//! use, reusing the harness in [`crate::alias_update`]. Nothing here sleeps on the
//! emulator's behalf: a transition the clock owes is waited for by looking again until it is
//! reported, and the overlap the rule needs is made by the HANDLERS holding their execution
//! environments while the rest of the burst arrives.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::alias_update::{call, router_on, Answer, FUNCTIONS_URI};
use crate::function_concurrency::{get_concurrency, put_concurrency};
use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module every package below holds its handler in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// The `Reason` a throttle that hit the FUNCTION's own reservation names, one of the model's
/// `ThrottleReason` values (`smithy_query(query="shape", shape="ThrottleReason")`).
const RESERVED_REASON: &str = "ReservedFunctionConcurrentInvocationLimitExceeded";

/// The code, status and sentence of that throttle.
const THROTTLE_CODE: &str = "TooManyRequestsException";
const THROTTLE_STATUS: u16 = 429;
const THROTTLE_MESSAGE: &str = "Rate Exceeded.";

/// How long each handler of a burst holds its execution environment, in seconds: long
/// enough that the whole burst has arrived while the first callers are still running, which
/// is what makes their capacity held rather than free. Generous, and well inside the
/// functions' own `Timeout`, so a loaded host does not decide the outcome.
const HELD_SECONDS: f64 = 4.0;

/// The `Timeout` every function here carries, in seconds: far above what its handler does,
/// so no row is decided by a run its own deadline ended.
const FUNCTION_TIMEOUT: u32 = 30;

/// How often to look while waiting for a created function to report `Active`, and how many
/// looks to take. The span is the service's own, so no fixed sleep names the instant it
/// ends, and the deadline is generous enough that a loaded host cannot decide the outcome.
const LOOK: Duration = Duration::from_millis(100);
const LOOKS: usize = 600;

/// A handler that holds its execution environment for as long as its event asks and then
/// answers the caller that sent the event, so one burst's answers can be told apart.
const HOLDS: &str = "import time\n\
                     def lambda_handler(event, context):\n\
                     \x20   time.sleep(event['seconds'])\n\
                     \x20   return {'caller': event['caller']}\n";

/// Create a function running `HOLDS` and wait until Lambda reports it `Active`, which is the
/// state an invocation needs.
async fn given_function(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(HOLDS)),
            },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    let mut last = String::new();
    for _ in 0..LOOKS {
        let configuration = call(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &Value::Null,
        )
        .await;
        last = configuration.text();
        if configuration.string("State").as_deref() == Some("Active") {
            return;
        }
        tokio::time::sleep(LOOK).await;
    }
    panic!("setup: Lambda must report {name:?} Active, last reported {last}");
}

/// `PutFunctionConcurrency(ReservedConcurrentExecutions=reserved)` on `name`, read back
/// through `GetFunctionConcurrency` so what the burst is decided against is what the
/// service really holds.
async fn reserve(router: &MultiServiceRouter, name: &str, reserved: i64) {
    let stored = put_concurrency(router, name, reserved).await;
    assert_eq!(
        stored.status,
        200,
        "setup: PutFunctionConcurrency({reserved}) on {name:?} must succeed, got {} {}",
        stored.status,
        stored.text()
    );
    let read = get_concurrency(router, name).await;
    assert_eq!(
        read.number("/ReservedConcurrentExecutions"),
        Some(reserved),
        "setup: GetFunctionConcurrency on {name:?} must report the reservation just put, \
         got {}",
        read.text()
    );
}

/// Every call of `calls` in flight at ONCE, answered in the order they were made.
///
/// A clone of the router per caller — they share the one engine and the one service
/// instance — so on a multi-thread runtime the invocations really do overlap.
async fn simultaneously(router: &MultiServiceRouter, calls: &[(String, Value)]) -> Vec<Answer> {
    let mut callers = tokio::task::JoinSet::new();
    for (index, (name, event)) in calls.iter().enumerate() {
        let router = router.clone();
        let uri = format!("{FUNCTIONS_URI}/{name}/invocations");
        let event = event.clone();
        callers.spawn(async move { (index, call(&router, "POST", &uri, &event).await) });
    }
    let mut answers: Vec<(usize, Answer)> = Vec::with_capacity(calls.len());
    while let Some(joined) = callers.join_next().await {
        answers.push(joined.expect("an invoking caller must not panic"));
    }
    answers.sort_by_key(|(index, _)| *index);
    answers.into_iter().map(|(_, answer)| answer).collect()
}

/// Whether `answered` is the modelled throttle naming the FUNCTION's own reservation, and
/// not some other refusal that happens to carry the same status.
fn refused_on_the_reservation(answered: &Answer) -> bool {
    answered.status == THROTTLE_STATUS
        && answered.error_code() == THROTTLE_CODE
        && answered.json().get("Reason").and_then(Value::as_str) == Some(RESERVED_REASON)
}

/// The event one caller of a burst sends.
fn event_of(caller: usize) -> Value {
    json!({ "seconds": HELD_SECONDS, "caller": caller })
}

/// Of callers arriving together, exactly as many as the reservation allows are admitted and
/// the rest are refused the modelled throttle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_reservation_admits_exactly_as_many_simultaneous_callers_as_it_reserves() {
    let _stream = one_invocation_stream_at_a_time().await;
    // (case, the reservation in force, how many callers arrive at once)
    let rows: &[(&str, i64, usize)] = &[
        ("a reservation of one under three simultaneous callers", 1, 3),
        ("a reservation of two under four simultaneous callers", 2, 4),
        (
            "a reservation of three under three simultaneous callers",
            3,
            3,
        ),
    ];
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (row, (case, reserved, callers)) in rows.iter().enumerate() {
        let name = format!("probe-reserved-burst-{row}");
        given_function(&router, &name).await;
        reserve(&router, &name, *reserved).await;

        let calls: Vec<(String, Value)> = (0..*callers)
            .map(|caller| (name.clone(), event_of(caller)))
            .collect();
        let answers = simultaneously(&router, &calls).await;

        let admitted = usize::try_from(*reserved).unwrap_or(0).min(*callers);
        let mut served = 0usize;
        let mut throttled = 0usize;
        for (caller, answered) in answers.iter().enumerate() {
            if answered.status == 200 {
                served += 1;
                assert_eq!(
                    answered.json(),
                    json!({ "caller": caller }),
                    "case {case:?}, caller {caller}: an admitted invocation answers its OWN \
                     run's value, got {}",
                    answered.text()
                );
                continue;
            }
            assert!(
                refused_on_the_reservation(answered),
                "case {case:?}, caller {caller}: an invocation the reservation had no room \
                 for must be refused on THAT reservation (HTTP {THROTTLE_STATUS} \
                 {THROTTLE_CODE}, Reason {RESERVED_REASON}), got {} {}",
                answered.status,
                answered.text()
            );
            assert_eq!(
                answered.message(),
                THROTTLE_MESSAGE,
                "case {case:?}, caller {caller}: expected the throttle's own sentence, got {}",
                answered.text()
            );
            throttled += 1;
        }
        assert_eq!(
            (served, throttled),
            (admitted, callers - admitted),
            "case {case:?}: a reservation of {reserved} under {callers} simultaneous callers \
             admits exactly {admitted} of them, got {served} served and {throttled} refused \
             on the reservation"
        );
    }
}

/// A reservation bounds the function it was put on and no other: callers of a second
/// function arriving while the first function's reservation is full are admitted against
/// their OWN function's reservation.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_functions_full_reservation_does_not_bound_another_functions_callers() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    // The two functions carry the SAME reservation, so what differs between the callers
    // below is only which function they address.
    let busy = "probe-reserved-busy".to_string();
    let quiet = "probe-reserved-quiet".to_string();
    given_function(&router, &busy).await;
    given_function(&router, &quiet).await;
    reserve(&router, &busy, 1).await;
    reserve(&router, &quiet, 1).await;

    // Two callers of the busy function and one of the quiet one, all in flight at once.
    let calls = vec![
        (busy.clone(), event_of(0)),
        (busy.clone(), event_of(1)),
        (quiet.clone(), event_of(2)),
    ];
    let answers = simultaneously(&router, &calls).await;

    let on_the_busy_function = &answers[0..2];
    let served = on_the_busy_function
        .iter()
        .filter(|answered| answered.status == 200)
        .count();
    let refused = on_the_busy_function
        .iter()
        .filter(|answered| refused_on_the_reservation(answered))
        .count();
    assert_eq!(
        (served, refused),
        (1, 1),
        "case the busy function: its reservation of one admits one of its two simultaneous \
         callers and refuses the other, got {served} served and {refused} refused on the \
         reservation ({} / {})",
        on_the_busy_function[0].text(),
        on_the_busy_function[1].text()
    );

    let beside = &answers[2];
    assert_eq!(
        beside.status,
        200,
        "case the quiet function: a caller of ANOTHER function draws on that function's own \
         reservation, so a full reservation elsewhere cannot refuse it, got {} {}",
        beside.status,
        beside.text()
    );
    assert_eq!(
        beside.json(),
        json!({ "caller": 2 }),
        "case the quiet function: expected that caller's own run's value, got {}",
        beside.text()
    );
}
