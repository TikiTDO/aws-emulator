// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH bound ends a run of the `python3.x` sandbox
//! (`state::execution::python`), driven at the engine the way `run_invocation` drives it: one
//! run per row, on a thread with the stack a run is given.
//!
//! The rule under test: a run is ended by the bound it actually breached, and which bound
//! that is follows from the run's own budget and from what the guest did — never from how
//! fast this host happens to be running or from what the rest of this process is doing. So a
//! row that names ONE bound is given a run only that bound can reach: the DEADLINE is the one
//! bound whose reading is a span of time, so it is read against the clock the HOST hands the
//! run (`state::execution::run_clock`) — a run handed a clock that does not advance can be
//! ended only by a bound that is not a span of time (the step budget, the memory ceiling, the
//! response cap), and a run handed a clock its own readings drive reaches its deadline after a
//! number of traced events rather than after a span of this host's time — while a row about
//! any other bound is given a memory ceiling out of reach, because that ceiling is read from
//! the whole process and this binary's own growth belongs to no row. A run whose budget
//! nothing breaches is not ended by a bound at all.
//!
//! The rows differ along what the rule generalises over: which bound the run breaches, which
//! PHASE of the run breaches it (the module-level code of the package, which runs while the
//! execution environment is being initialized, or the handler's own call), and which clock
//! the run is judged against. One row is run repeatedly, because a bound that answers a run
//! is the bound that answers it every time.

use std::collections::BTreeMap;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use service_com_amazonaws_lambda_awsgirapiservice::state::execution::package::PackageModule;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::python::{
    run_against, Run,
};
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::{
    Budget, Clock, Completion, Finished, Stop, RUN_STACK_BYTES,
};

/// The module one run's package holds, and the handler it is entered through.
const MODULE: &str = "lambda_function.py";

/// The `Handler` every run here is configured with.
const HANDLER: &str = "lambda_function.handler";

/// How long a row waits for its run's own thread to end before it reports the run as one that
/// did not end at all.
///
/// NOT a bound on the run: every bound a run has is the host's own, held inside the run. This
/// is the backstop that keeps a run which will not end from wedging the suite, the way the
/// service gives such a run up rather than waiting on it for ever. It is far above any span a
/// row's own bound takes, so no row is ever answered by it.
const RUN_ANSWER_CEILING: Duration = Duration::from_secs(240);

/// How far ONE reading drives the clock a row about the DEADLINE is judged against.
///
/// Small against the span it has to drive the clock across, so a run reaches the phase the
/// row is about long before the readings taken getting there have driven the deadline past
/// it.
const DRIVEN_TICK: Duration = Duration::from_micros(10);

/// A memory ceiling a row about any OTHER bound cannot reach: the largest `MemorySize` a
/// function may have (`smithy_query(query="shape", shape="MemorySize")`:
/// `@range(min: 128, max: 32768)`), in bytes.
///
/// The ceiling is read from the whole PROCESS as well as from the run
/// (`state::execution::python`): a run that allocated nothing of its own is stopped for
/// memory once this process has grown past its ceiling by more than the run's own ceiling
/// allows it to add — and in a test binary running many suites at once, that growth is other
/// suites' and no row's own. A row that names another bound is therefore given the largest
/// ceiling a function may have, which neither half of the memory bound can reach.
const CEILING_OUT_OF_REACH: u64 = 32_768 * 1024 * 1024;

/// The run's own ceiling the memory rows are given: the smallest `MemorySize` a function may
/// have (the same `@range`), which is far above what building the interpreter itself faults
/// in and far below what a loop that allocates without stopping reaches.
const MEMORY_CEILING_BYTES: u64 = 128 * 1024 * 1024;

/// Guest code that allocates a megabyte at a time and keeps every one of it, as a package's
/// module-level code (the INIT phase) or inside the handler (the INVOKE phase).
///
/// It never stops of its own accord: what ends such a run is the run's own ceiling, reached
/// from whatever this process already held when the run began — which is what makes the row
/// about the ceiling and not about the size of one allocation.
const ALLOCATES_WITHOUT_STOPPING: &str = "KEPT = []\n\
     def _fill():\n    \
         while True:\n        \
             KEPT.append(bytearray(b'x' * (1024 * 1024)))\n";

/// A budget large enough for any handler here and small enough for a row to exhaust the one
/// bound it is about. Its memory ceiling is out of reach; a row about the ceiling gives
/// itself one it can reach.
fn budget() -> Budget {
    Budget {
        wall_clock: Duration::from_secs(5),
        steps: 2_000_000,
        memory_bytes: CEILING_OUT_OF_REACH,
        log_bytes: 16 * 1024,
        response_bytes: 64 * 1024,
        recursion_limit: 2_000,
    }
}

/// Run `source` as the one module of a package, with `budget`, judged against `clock`.
///
/// The answer is waited for rather than joined on, so a run that will not end fails the row
/// that asked for it instead of holding the suite.
fn ran(source: &str, budget: Budget, clock: &Clock, case: &str) -> Finished {
    let modules = vec![PackageModule {
        path: MODULE.to_string(),
        source: source.to_string(),
    }];
    let clock = clock.clone();
    let (answered, answer) = mpsc::channel();
    thread::Builder::new()
        .stack_size(RUN_STACK_BYTES)
        .spawn(move || {
            let environment = BTreeMap::new();
            let finished = run_against(
                &Run {
                    modules: &modules,
                    handler: HANDLER,
                    environment: &environment,
                    event: None,
                    request_id: "11111111-2222-3333-4444-555555555555",
                    invoked_function_arn: "arn:aws:lambda:us-west-2:000000000000:function:probe",
                    client_context: None,
                    tenant_id: None,
                    budget,
                    // The run's own containment and bounds are what these rows are about,
                    // and no emulator stands behind them: there is no SDK seam either.
                    aws: None,
                },
                &clock,
            );
            let _ = answered.send(finished);
        })
        .expect("a run thread");
    answer
        .recv_timeout(RUN_ANSWER_CEILING)
        .unwrap_or_else(|_| panic!("case {case}: the run did not end, so no bound of it ended it"))
}

/// A handler body that allocates `megabytes` whole megabytes, keeps them, and returns how
/// many it holds.
fn allocating_handler(megabytes: usize) -> String {
    format!(
        "def _megabyte():\n    \
             return bytearray(b'x' * (1024 * 1024))\n\
         KEPT = [_megabyte() for _ in range({megabytes})]\n\
         def handler(event, context):\n    \
             return {{'held': len(KEPT)}}\n"
    )
}

/// One row: what it is about, the package's module source, the budget the run is given, the
/// clock it is judged against, and what the run has to come to.
struct BoundCase {
    /// What the row is about.
    case: &'static str,
    /// The module source of the package the run is given.
    source: String,
    /// The budget the run is given.
    budget: Budget,
    /// The clock the run's deadline is read against.
    clock: Clock,
    /// The bound that has to end the run, or `None` for a run no bound ends.
    expected: Option<Stop>,
}

/// Every row of the table: one per bound, one per phase a memory ceiling is breached in, and
/// one a bound does not end at all.
fn cases() -> Vec<BoundCase> {
    // A step budget a spinning handler spends at once, and one no row here comes near.
    let few_steps = 50_000;
    let many_steps = 2_000_000_000;
    let spins = "def handler(event, context):\n    \
                     while True:\n        \
                         pass\n";
    vec![
        BoundCase {
            case: "a loop with no exit, under a step budget and a clock that does not advance",
            source: spins.to_string(),
            budget: Budget {
                steps: few_steps,
                ..budget()
            },
            clock: Clock::still(),
            expected: Some(Stop::Steps),
        },
        BoundCase {
            case: "a response larger than the run may return",
            source: "def handler(event, context):\n    \
                         return 'x' * 200000\n"
                .to_string(),
            budget: Budget {
                steps: few_steps,
                ..budget()
            },
            clock: Clock::still(),
            expected: Some(Stop::Response),
        },
        BoundCase {
            case: "a handler that allocates without stopping, under the run's own memory \
                   ceiling",
            source: format!(
                "{ALLOCATES_WITHOUT_STOPPING}\
                 def handler(event, context):\n    \
                     _fill()\n"
            ),
            budget: Budget {
                steps: many_steps,
                memory_bytes: MEMORY_CEILING_BYTES,
                ..budget()
            },
            clock: Clock::still(),
            expected: Some(Stop::Memory),
        },
        BoundCase {
            case: "a package whose MODULE-level code allocates without stopping while the \
                   execution environment is being initialized",
            source: format!(
                "{ALLOCATES_WITHOUT_STOPPING}\
                 _fill()\n\
                 def handler(event, context):\n    \
                     return {{'held': len(KEPT)}}\n"
            ),
            budget: Budget {
                steps: many_steps,
                memory_bytes: MEMORY_CEILING_BYTES,
                ..budget()
            },
            clock: Clock::still(),
            expected: Some(Stop::Memory),
        },
        BoundCase {
            case: "a loop with no exit, with events enough to drive its own clock past its \
                   deadline",
            source: spins.to_string(),
            budget: Budget {
                steps: many_steps,
                wall_clock: Duration::from_secs(1),
                ..budget()
            },
            clock: Clock::driven(DRIVEN_TICK),
            expected: Some(Stop::Deadline),
        },
        BoundCase {
            case: "a handler that allocates well under the ceiling and returns",
            source: allocating_handler(1),
            budget: Budget {
                steps: many_steps,
                ..budget()
            },
            clock: Clock::still(),
            expected: None,
        },
    ]
}

/// What the run came to, as the bound that ended it: `None` for a run no bound ended.
fn stopped_by(finished: &Finished, case: &str) -> Option<Stop> {
    match &finished.completion {
        Completion::Stopped(stop) => Some(*stop),
        Completion::Returned(_) => None,
        other => panic!("case {case}: expected a stop or the handler's own answer, got {other:?}"),
    }
}

/// The bound a run breached is the bound that ends it, and no other bound of the same run
/// answers in its place.
#[test]
fn a_run_is_ended_by_the_bound_it_breached() {
    for case in cases() {
        let finished = ran(&case.source, case.budget, &case.clock, case.case);
        let stopped = stopped_by(&finished, case.case);
        assert_eq!(
            stopped, case.expected,
            "case {}: expected the run to end on {:?}, got {stopped:?} ({:?})",
            case.case, case.expected, finished.completion
        );
    }
}

/// The MEMORY ceiling answers the same run every time it is run: a bound that ends a run is
/// not one of two bounds racing, so the answer is the same on every attempt rather than most
/// of the time.
#[test]
fn the_memory_ceiling_answers_the_same_run_every_time() {
    // Enough attempts to catch an answer that is decided by a race and few enough to keep
    // the suite's work in proportion.
    const ATTEMPTS: usize = 4;
    let source = format!(
        "{ALLOCATES_WITHOUT_STOPPING}\
         def handler(event, context):\n    \
             _fill()\n"
    );
    for attempt in 1..=ATTEMPTS {
        let case = format!("attempt {attempt} of a handler that allocates without stopping");
        let finished = ran(
            &source,
            Budget {
                steps: 2_000_000_000,
                memory_bytes: MEMORY_CEILING_BYTES,
                ..budget()
            },
            &Clock::still(),
            &case,
        );
        let stopped = stopped_by(&finished, &case);
        assert_eq!(
            stopped,
            Some(Stop::Memory),
            "case {case}: expected the run to end on the memory ceiling, got {stopped:?} ({:?})",
            finished.completion
        );
    }
}
