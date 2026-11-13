// SPDX-License-Identifier: CC0-1.0

//! `Job$MaxRetries`: the further attempts the SERVICE runs after a job run fails.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! RULE: "The maximum number of times to retry this job after a JobRun fails"
//! (`Job$MaxRetries`) is a policy of the JOB, so a run that comes to rest in the failure
//! state is run again by the service while that budget lasts — one `StartJobRun` of a job
//! with `MaxRetries` N settles into N + 1 runs of it, every one of them terminal and
//! failed, carrying consecutive `Attempt` values ("The number of the attempt to run this
//! job") from the original attempt's, and every attempt after the first naming the
//! attempt it retries in its `PreviousRunId` ("The ID of the previous run of this job"),
//! so the attempts form ONE chain whose last link nothing names. A run that ended any
//! other way — a submitted stop — failed at nothing and is retried by nothing, and a job
//! with no retry budget is run once.
//!
//! The rows differ along what the rule generalises over: the retry budget (none, one,
//! several, and a budget well above any other row's), the command kind,
//! the overrides the start carries, and whether the run came to rest in the failure state
//! or in the state a submitted stop brings it to.
//!
//! The attempts are owed by the CLOCK, so the suite drives them the way the rest of this
//! crate's run suites drive an owed transition: the due instants are written THROUGH THE
//! STORE and materialised by the next read, with one test sleeping instead, to pin that
//! the chain is produced by the wall clock and not by a reader.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    FIRST_JOB_RUN_ATTEMPT, JOB_RUN_SETTLES_AFTER, JOB_RUN_STATE_FAILED, JOB_RUN_STATE_STOPPED,
    TERMINAL_JOB_RUN_STATES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{emulator_with_engine, get_run, job_body, runs_at_rest, start_run};
use crate::wire::ok;

/// The retry budget a job definition that names none carries: the `@default` of
/// `com.amazonaws.glue#MaxRetries`, the shape `Job$MaxRetries` targets
/// (`smithy_query query="shape" shape="com.amazonaws.glue#MaxRetries"`).
const DEFAULT_MAX_RETRIES: i64 = 0;

/// How far apart two instants this suite compares may be rendered: a `JobRun`'s instants
/// cross the wire as epoch seconds in a JSON number, so two instants a fixed span apart
/// come back a rendering's worth of either side of that span.
const RENDERING_TOLERANCE: Duration = Duration::from_millis(10);

/// Every run `GetJobRuns` reports for `job_name`, read in ONE page: a row holds far fewer
/// runs than a page does, so a token on the answer would mean runs these assertions never
/// saw.
async fn listed_runs(router: &MultiServiceRouter, case: &str, job_name: &str) -> Vec<Value> {
    let page = ok(router, "GetJobRuns", &json!({ "JobName": job_name })).await;
    assert!(
        page.get("NextToken").is_none_or(Value::is_null),
        "case {case:?}: every run of the chain must fit one page, got {page}"
    );
    page.get("JobRuns")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: GetJobRuns must report JobRuns, got {page}"))
}

/// Drive everything the clock owes this job's runs to completion, and read the chain back.
///
/// Each attempt of a chain is a further EXECUTION of the job's script, which the service
/// performs once the attempt it retries has come to rest — so the chain advances one attempt
/// per owed span, exactly as a client polling the job's runs observes it. Here the span is
/// made due rather than slept for and the runs are read back until no run of the job is in
/// flight any more, through the one poll this crate's run suites share
/// ([`crate::job_runs::runs_at_rest`]), which is the state a client that polled long enough
/// would see.
async fn settled_runs(
    router: &MultiServiceRouter,
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    case: &str,
    job_name: &str,
) -> Vec<Value> {
    runs_at_rest(router, engine, case, job_name).await
}

/// The listed runs keyed by the `Attempt` each reports, which must name them uniquely.
fn by_attempt(case: &str, runs: &[Value]) -> BTreeMap<i64, Value> {
    let mut found: BTreeMap<i64, Value> = BTreeMap::new();
    for run in runs {
        let attempt = run
            .get("Attempt")
            .and_then(Value::as_i64)
            .unwrap_or_else(|| panic!("case {case:?}: every run reports an Attempt, got {run}"));
        let clash = found.insert(attempt, run.clone());
        assert!(
            clash.is_none(),
            "case {case:?}: two runs report attempt {attempt}, got {run} and {clash:?}"
        );
    }
    found
}

/// The `Id` a listed run reports.
fn run_id(case: &str, run: &Value) -> String {
    run.get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: every run reports an Id, got {run}"))
        .to_string()
}

/// The `PreviousRunId` a listed run reports, absent for a run that retries nothing.
fn previous_run_id(run: &Value) -> Option<String> {
    run.get("PreviousRunId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert the whole rule over the runs of one job: the attempt count the budget allows,
/// every attempt failed and terminal, consecutive attempt numbers from the first, the
/// original attempt being `first_id` and naming no previous run, each later attempt naming
/// the one before it, and the single-run read agreeing with the listing on both members.
async fn assert_retry_chain(
    router: &MultiServiceRouter,
    case: &str,
    job_name: &str,
    first_id: &str,
    max_retries: i64,
    runs: &[Value],
) {
    let expected = max_retries + 1;
    assert_eq!(
        i64::try_from(runs.len()).unwrap_or(i64::MAX),
        expected,
        "case {case:?}: expected {expected} runs for MaxRetries={max_retries}, one original \
         attempt plus its retries, got {runs:?}"
    );

    let chain = by_attempt(case, runs);
    let attempts: Vec<i64> = chain.keys().copied().collect();
    let wanted: Vec<i64> = (0..expected)
        .map(|offset| i64::from(FIRST_JOB_RUN_ATTEMPT) + offset)
        .collect();
    assert_eq!(
        attempts, wanted,
        "case {case:?}: expected consecutive attempts {wanted:?}, got {attempts:?}"
    );

    let mut expected_previous: Option<String> = None;
    for attempt in &wanted {
        let run = &chain[attempt];
        assert!(
            TERMINAL_JOB_RUN_STATES.contains(&run["JobRunState"].as_str().unwrap_or_default()),
            "case {case:?}: attempt {attempt} must have finished, got {run}"
        );
        assert_eq!(
            run["JobRunState"].as_str(),
            Some(JOB_RUN_STATE_FAILED),
            "case {case:?}: attempt {attempt} must report the state the clock owed it, got {run}"
        );
        crate::job_runs::assert_script_was_unreadable(case, run);
        assert_eq!(
            previous_run_id(run),
            expected_previous,
            "case {case:?}: attempt {attempt} must name the attempt it retries, got {run}"
        );
        assert!(
            run["TriggerName"].is_null(),
            "case {case:?}: attempt {attempt} was started by a caller, not a trigger, got {run}"
        );

        // The single-run read agrees with the listing on both members of the chain.
        let id = run_id(case, run);
        let read = get_run(router, job_name, &id).await;
        assert_eq!(
            read.get("Attempt").and_then(Value::as_i64),
            Some(*attempt),
            "case {case:?}: GetJobRun must report attempt {attempt}, got {read}"
        );
        assert_eq!(
            previous_run_id(&read),
            expected_previous,
            "case {case:?}: GetJobRun must agree on the attempt {attempt} retries, got {read}"
        );
        expected_previous = Some(id);
    }

    assert_eq!(
        run_id(case, &chain[&i64::from(FIRST_JOB_RUN_ATTEMPT)]),
        first_id,
        "case {case:?}: the original attempt is the run the start minted, got {runs:?}"
    );
    // The chain's last link is the end of it: nothing retries the final attempt.
    let last = expected_previous.unwrap_or_default();
    assert!(
        !runs
            .iter()
            .any(|run| previous_run_id(run).as_deref() == Some(last.as_str())),
        "case {case:?}: nothing may name the final attempt {last:?} as the run it retries, got \
         {runs:?}"
    );
}

/// One start of a job settles into its original attempt plus one run per retry the job's
/// `MaxRetries` allows, chained by `PreviousRunId` and numbered by `Attempt`.
///
/// Each row is a job definition and the start of it, differing along the retry budget, the
/// command kind, and the overrides the start carries — and a budget of zero is the row
/// that pins that a job asking for no retry is run exactly once.
#[tokio::test]
async fn a_failed_run_is_retried_once_per_unit_of_the_jobs_retry_budget() {
    let (router, engine) = emulator_with_engine();

    // (case, job name, CreateJob members beyond the required three, StartJobRun body)
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            // "A job with MaxRetries=0 must yield one run": the budget's lower end.
            "no retry budget at all",
            "retries-none",
            json!({ "MaxRetries": 0 }),
            json!({ "JobName": "retries-none" }),
        ),
        (
            "an absent retry budget, which is the model's own default of zero",
            "retries-default",
            json!({}),
            json!({ "JobName": "retries-default" }),
        ),
        (
            "a budget of one retry",
            "retries-one",
            json!({ "MaxRetries": 1 }),
            json!({ "JobName": "retries-one" }),
        ),
        (
            // The recorded case: a Python shell job with two retries, started by name.
            "a budget of two retries on a Python shell job",
            "retries-two",
            json!({
                "Command": {
                    "Name": "pythonshell",
                    "PythonVersion": "3.9",
                    "ScriptLocation": "s3://sim2real/scripts/fail.py",
                },
                "MaxCapacity": 0.0625,
                "GlueVersion": "3.0",
                "MaxRetries": 2,
                "Timeout": 10,
                "ExecutionProperty": { "MaxConcurrentRuns": 5 },
            }),
            json!({ "JobName": "retries-two" }),
        ),
        (
            "a budget of five retries, with the start overriding the job",
            "retries-five",
            json!({ "MaxRetries": 5, "Timeout": 60, "GlueVersion": "4.0" }),
            json!({
                "JobName": "retries-five",
                "Timeout": 7,
                "Arguments": { "--scale": "3" },
            }),
        ),
        (
            "a large retry budget",
            "retries-many",
            json!({ "MaxRetries": 12, "Command": { "Name": "gluestreaming", "ScriptLocation": "s3://etl/s.py" } }),
            json!({ "JobName": "retries-many" }),
        ),
    ];

    for (case, name, job, start) in cases {
        let max_retries = job
            .get("MaxRetries")
            .and_then(Value::as_i64)
            .unwrap_or(DEFAULT_MAX_RETRIES);
        ok(&router, "CreateJob", &job_body(name, job)).await;
        let first = start_run(&router, &start).await;

        let runs = settled_runs(&router, &engine, case, name).await;
        assert_retry_chain(&router, case, name, &first, max_retries, &runs).await;

        // Reading the chain again reports the same runs: the attempts were owed by the
        // clock, so a read materialises them once rather than adding one per read.
        let again = listed_runs(&router, case, name).await;
        assert_eq!(
            again, runs,
            "case {case:?}: a second read must report the same chain, got {again:?}"
        );
    }
}

/// Every attempt of a chain runs under the configuration the attempt it retries ran under:
/// a retry is the same work again, so the members a start overrode are carried by each
/// retry rather than falling back to the job's own.
#[tokio::test]
async fn every_retry_carries_the_configuration_of_the_attempt_it_retries() {
    let (router, engine) = emulator_with_engine();
    let case = "a start that overrides the job it runs";
    let name = "retried-overrides";
    ok(
        &router,
        "CreateJob",
        &job_body(
            name,
            json!({
                "MaxRetries": 3,
                "Timeout": 90,
                "GlueVersion": "4.0",
                "DefaultArguments": { "--job-language": "python" },
                "WorkerType": "G.1X",
                "NumberOfWorkers": 2,
            }),
        ),
    )
    .await;
    start_run(
        &router,
        &json!({
            "JobName": name,
            "Timeout": 11,
            "WorkerType": "G.2X",
            "NumberOfWorkers": 3,
            "SecurityConfiguration": "run-security",
            "Arguments": { "--scale": "7" },
        }),
    )
    .await;

    let runs = settled_runs(&router, &engine, case, name).await;
    let chain = by_attempt(case, &runs);
    // The members the start resolved, which every attempt of the chain must repeat: the
    // overrides themselves, the capacity derived from them (2 DPU per G.2X worker), and
    // the job's own members a start cannot override.
    let expected = json!({
        "JobName": name,
        "Timeout": 11,
        "WorkerType": "G.2X",
        "NumberOfWorkers": 3,
        "MaxCapacity": 6.0,
        "AllocatedCapacity": 6,
        "SecurityConfiguration": "run-security",
        "GlueVersion": "4.0",
        "Arguments": { "--job-language": "python", "--scale": "7" },
    });
    for (attempt, run) in &chain {
        for (member, value) in expected.as_object().expect("the expectation is an object") {
            assert_eq!(
                run.get(member),
                Some(value),
                "case {case:?}: attempt {attempt} must carry {member} {value}, got {run}"
            );
        }
    }
}

/// A run that came to rest any other way than by failing is retried by nothing: a
/// submitted stop brings a run to rest having failed at nothing, so the job's retry budget
/// buys no further attempt of it.
#[tokio::test]
async fn a_run_a_submitted_stop_brought_to_rest_is_not_retried() {
    let (router, engine) = emulator_with_engine();
    let case = "a stopped run of a job with retries to spare";
    let name = "stopped-not-retried";
    ok(&router, "CreateJob", &job_body(name, json!({ "MaxRetries": 4 }))).await;
    let started = start_run(&router, &json!({ "JobName": name })).await;

    let stopped = ok(
        &router,
        "BatchStopJobRun",
        &json!({ "JobName": name, "JobRunIds": [started.clone()] }),
    )
    .await;
    assert!(
        stopped
            .get("Errors")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty),
        "case {case:?}: the stop of an in-flight run must be accepted, got {stopped}"
    );

    let runs = settled_runs(&router, &engine, case, name).await;
    assert_eq!(
        runs.len(),
        1,
        "case {case:?}: a stopped run buys no further attempt, got {runs:?}"
    );
    assert_eq!(
        runs[0]["JobRunState"].as_str(),
        Some(JOB_RUN_STATE_STOPPED),
        "case {case:?}: the run must report the state the stop owed it, got {}",
        runs[0]
    );
    assert_eq!(
        runs[0]["Attempt"].as_i64(),
        Some(i64::from(FIRST_JOB_RUN_ATTEMPT)),
        "case {case:?}: the only run is the original attempt, got {}",
        runs[0]
    );
    assert_eq!(
        run_id(case, &runs[0]),
        started,
        "case {case:?}: the only run is the one the start minted, got {runs:?}"
    );
}

/// The chain is owed by the WALL CLOCK: without a test writing a single due instant, one
/// start of a job with a retry budget settles into the whole chain on its own, and each
/// attempt is observed in flight before the next one exists.
#[tokio::test]
async fn the_chain_falls_due_on_the_wall_clock_without_anything_writing_an_instant() {
    let router = crate::wire::emulator();
    let case = "a budget of two retries, driven by the clock alone";
    let name = "clock-driven-retries";
    ok(&router, "CreateJob", &job_body(name, json!({ "MaxRetries": 2 }))).await;
    let first = start_run(&router, &json!({ "JobName": name })).await;

    // The chain is one attempt per owed span, so three attempts are due within a few of
    // them. The poll is bounded, and a window that closes before the chain is complete
    // fails the row rather than hanging.
    let deadline = SystemTime::now() + 10 * JOB_RUN_SETTLES_AFTER;
    let mut runs = listed_runs(&router, case, name).await;
    while SystemTime::now() < deadline {
        if runs.len() == 3 && runs.iter().all(|run| run["CompletedOn"].is_number()) {
            break;
        }
        assert!(
            runs.len() <= 3,
            "case {case:?}: a budget of two retries is at most three attempts, got {runs:?}"
        );
        tokio::time::sleep(JOB_RUN_SETTLES_AFTER / 4).await;
        runs = listed_runs(&router, case, name).await;
    }

    assert_retry_chain(&router, case, name, &first, 2, &runs).await;
    // Every attempt but the first began when the one it retries finished, so the chain's
    // own instants place the attempts in order rather than the reader's clock doing it.
    let chain = by_attempt(case, &runs);
    for attempt in 1..=2i64 {
        let started = chain[&attempt]["StartedOn"].as_f64().unwrap_or_default();
        let retried_finished = chain[&(attempt - 1)]["CompletedOn"]
            .as_f64()
            .unwrap_or_default();
        assert!(
            Duration::from_secs_f64((started - retried_finished).abs()) <= RENDERING_TOLERANCE,
            "case {case:?}: attempt {attempt} must begin when attempt {} finished, got \
             {started} against {retried_finished}",
            attempt - 1
        );
    }
    // A span really passed between the attempts, so a caller polling the chain can observe
    // an attempt in flight before the next exists.
    let first_started = chain[&i64::from(FIRST_JOB_RUN_ATTEMPT)]["StartedOn"]
        .as_f64()
        .unwrap_or_default();
    let last_started = chain[&2]["StartedOn"].as_f64().unwrap_or_default();
    let spanned = Duration::from_secs_f64((last_started - first_started).max(0.0));
    // The instants are compared as the wire renders them, so the bound carries the
    // tolerance that rendering costs rather than demanding exact equality of two doubles.
    assert!(
        spanned + RENDERING_TOLERANCE >= 2 * JOB_RUN_SETTLES_AFTER,
        "case {case:?}: two further attempts take two owed spans, got {spanned:?} over {runs:?}"
    );
}
