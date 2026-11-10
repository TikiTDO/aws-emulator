// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the members a Glue READ fills in itself: the ones a client did not
//! send and the service answers anyway.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a modelled output member whose value the service KNOWS is reported
//! on every read of the resource — the flag it applies when the request named none, the
//! other form of a capacity it holds, the log group a run writes under, the empty list or
//! map where there is nothing in it, a zero where nothing has been accounted yet, and the
//! caller identity in the form that member carries — rather than being left out for a
//! client to guess at.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! member the request named and with which value, the job command, the Glue version, the
//! capacity form, whether a security configuration applies, and whether a run was started
//! by a caller or by a trigger whose predicate named runs.

use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    dpu_per_worker, ComAmazonawsGlueAwsglueState, DEFAULT_GLUE_VERSION, DEFAULT_SESSION_TYPE,
    DEFAULT_SESSION_WORKERS, DEFAULT_SESSION_WORKER_TYPE, DEFAULT_SPARK_WORKERS,
    DEFAULT_SPARK_WORKER_TYPE,
    JOB_RUN_STATE_FAILED, PYTHON_SHELL_JOB_LOG_GROUP, SPARK_JOB_LOG_GROUP,
    TRIGGER_TYPE_CONDITIONAL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::{already_past, emulator_with_engine, set_due_instant};
use crate::wire::{call, emulator, ok, CALLER_PRINCIPAL, REGION};

/// The IAM role a job or session of these suites runs as. Its NAME is the part after the
/// last `/`, which is what an encrypted log group's name carries.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/GlueJobRole";

/// See [`ROLE_ARN`].
const ROLE_NAME: &str = "GlueJobRole";

/// A `CreateJob` body: the three `@required` members with a Spark ETL command, plus
/// whatever `extra` adds or replaces.
fn job_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "Name": name,
        "Role": ROLE_ARN,
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
    });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    body
}

/// The `Job` `GetJob` reports for `name`.
async fn get_job(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetJob", &json!({ "JobName": name })).await;
    body.get("Job")
        .cloned()
        .unwrap_or_else(|| panic!("GetJob must report a Job, got {body}"))
}

/// Start a run of `job` with `extra` applied to the start, and answer the `JobRun`
/// `GetJobRun` reports for it.
async fn run_of(router: &MultiServiceRouter, job: &str, extra: Value) -> Value {
    let mut start = json!({ "JobName": job });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            start[member] = value.clone();
        }
    }
    let started = ok(router, "StartJobRun", &start).await;
    let run_id = started["JobRunId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {started}"))
        .to_string();
    let read = ok(
        router,
        "GetJobRun",
        &json!({ "JobName": job, "RunId": run_id }),
    )
    .await;
    read.get("JobRun")
        .cloned()
        .unwrap_or_else(|| panic!("GetJobRun must report a JobRun, got {read}"))
}

/// The `Session` `GetSession` reports for `id`.
async fn get_session(router: &MultiServiceRouter, id: &str) -> Value {
    let body = ok(router, "GetSession", &json!({ "Id": id })).await;
    body.get("Session")
        .cloned()
        .unwrap_or_else(|| panic!("GetSession must report a Session, got {body}"))
}

// ---------------------------------------------------------------------------
// The queuing flag a definition and a run apply
// ---------------------------------------------------------------------------

/// `Job$JobRunQueuingEnabled` is reported on every read of a definition: the flag the
/// create or the update named, and the flag the service applies ("if false or not
/// populated, the job runs will not be considered for queueing") when it named none.
///
/// The rows differ along which value the request named — nothing, `true`, `false` — on the
/// create and then on the update that overwrites the definition.
#[tokio::test]
async fn the_queuing_flag_a_job_definition_applies_is_always_reported() {
    let router = emulator();

    // (case, what the create names, the flag the definition must report). The flag a
    // definition naming none applies is stated as the literal the recorded reads of such a
    // job report, so what the service fills in is under test and not re-asserted from the
    // constant that carries it.
    let rows: Vec<(&str, Value, bool)> = vec![
        ("a create naming no flag", json!({}), false),
        (
            "a create enabling queuing",
            json!({ "JobRunQueuingEnabled": true }),
            true,
        ),
        (
            "a create disabling queuing",
            json!({ "JobRunQueuingEnabled": false }),
            false,
        ),
    ];

    for (case, named, expected) in &rows {
        let name = format!("queuing-{}", named["JobRunQueuingEnabled"]);
        ok(&router, "CreateJob", &job_body(&name, named.clone())).await;
        let job = get_job(&router, &name).await;
        assert_eq!(
            job["JobRunQueuingEnabled"].as_bool(),
            Some(*expected),
            "case {case:?}: expected JobRunQueuingEnabled {expected}, got {job}"
        );

        // "The previous job definition is completely overwritten by this information": an
        // update that names no flag leaves the definition applying the service's, and one
        // that names a flag applies it.
        for (update_case, update_flag, after_update) in [
            ("then an update naming none", json!({}), false),
            (
                "then an update enabling queuing",
                json!({ "JobRunQueuingEnabled": true }),
                true,
            ),
            (
                "then an update disabling queuing",
                json!({ "JobRunQueuingEnabled": false }),
                false,
            ),
        ] {
            let mut job_update = json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" },
            });
            if let Some(members) = update_flag.as_object() {
                for (member, value) in members {
                    job_update[member] = value.clone();
                }
            }
            ok(
                &router,
                "UpdateJob",
                &json!({ "JobName": name, "JobUpdate": job_update }),
            )
            .await;
            let job = get_job(&router, &name).await;
            assert_eq!(
                job["JobRunQueuingEnabled"].as_bool(),
                Some(after_update),
                "case {case:?} {update_case:?}: expected JobRunQueuingEnabled {after_update}, \
                 got {job}"
            );
        }
    }
}

/// `JobRun$JobRunQueuingEnabled` is reported on every read of a run: "if this field does
/// not match the value set in the job run, then the value from the job run field will be
/// used", so the run carries what the START named, else its job's flag — never nothing.
///
/// The rows are every combination of named and unnamed on the pair.
#[tokio::test]
async fn the_queuing_flag_a_run_applies_is_always_reported() {
    let router = emulator();

    // (case, what the create names, what the start names, the flag the run must report)
    let rows: Vec<(&str, Value, Value, bool)> = vec![
        (
            "neither names the flag",
            json!({}),
            json!({}),
            false,
        ),
        (
            "the job enables it and the start is silent",
            json!({ "JobRunQueuingEnabled": true }),
            json!({}),
            true,
        ),
        (
            "the start overrides a job that disabled it",
            json!({ "JobRunQueuingEnabled": false }),
            json!({ "JobRunQueuingEnabled": true }),
            true,
        ),
        (
            "the start overrides a job that enabled it",
            json!({ "JobRunQueuingEnabled": true }),
            json!({ "JobRunQueuingEnabled": false }),
            false,
        ),
    ];

    for (index, (case, job, start, expected)) in rows.iter().enumerate() {
        let name = format!("run-queuing-{index}");
        ok(&router, "CreateJob", &job_body(&name, job.clone())).await;
        // The flag a run applies is written at the start and does not move with the run's
        // state, so the read needs no clock positioning.
        let run = run_of(&router, &name, start.clone()).await;
        assert_eq!(
            run["JobRunQueuingEnabled"].as_bool(),
            Some(*expected),
            "case {case:?}: expected JobRunQueuingEnabled {expected}, got {run}"
        );
    }
}

// ---------------------------------------------------------------------------
// A capacity named as a DPU figure
// ---------------------------------------------------------------------------

/// A job whose capacity IS a worker configuration — "for Glue version 2.0 or later jobs,
/// you cannot specify a `Maximum capacity`. Instead, you should specify a `Worker type` and
/// the `Number of workers`" — reports that configuration whichever form the request named
/// the capacity in: a DPU figure is reported as the workers it amounts to, at the per-worker
/// figures the model publishes (`CreateJob$WorkerType`).
///
/// The rows differ along the form the capacity was named in (`MaxCapacity`, the deprecated
/// `AllocatedCapacity`, a worker configuration, nothing at all), the command, and the Glue
/// version — including a version and a command whose capacity is a DPU figure rather than a
/// worker configuration, where no worker count is reported because none applies.
#[tokio::test]
async fn a_dpu_figure_is_reported_as_the_worker_configuration_it_amounts_to() {
    let router = emulator();

    let spark = json!({ "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/etl.py" });
    let streaming =
        json!({ "Name": "gluestreaming", "ScriptLocation": "s3://etl-scripts/stream.py" });
    let python_shell = json!({
        "Name": "pythonshell",
        "ScriptLocation": "s3://etl-scripts/report.py",
        "PythonVersion": "3",
    });

    // (case, what the create names, expected (WorkerType, NumberOfWorkers, MaxCapacity))
    let rows: Vec<(&str, Value, Value, Value, Option<f64>)> = vec![
        (
            "a DPU figure on a Spark job",
            json!({ "MaxCapacity": 2.0 }),
            json!(DEFAULT_SPARK_WORKER_TYPE),
            json!(2),
            Some(2.0),
        ),
        (
            "the smallest whole DPU figure",
            json!({ "MaxCapacity": 1.0 }),
            json!(DEFAULT_SPARK_WORKER_TYPE),
            json!(1),
            Some(1.0),
        ),
        (
            // "This parameter is deprecated. Use MaxCapacity instead": the two name one
            // quantity, so the configuration is derived from either.
            "the deprecated DPU figure",
            json!({ "AllocatedCapacity": 4 }),
            json!(DEFAULT_SPARK_WORKER_TYPE),
            json!(4),
            Some(4.0),
        ),
        (
            "a DPU figure on a streaming job",
            json!({ "Command": streaming, "MaxCapacity": 6.0 }),
            json!(DEFAULT_SPARK_WORKER_TYPE),
            json!(6),
            Some(6.0),
        ),
        (
            // "For Glue version 1.0 or earlier jobs … the number of Glue data processing
            // units (DPUs) that can be allocated": such a job's capacity is the figure.
            "a DPU figure on a legacy Glue version",
            json!({ "GlueVersion": "1.0", "MaxCapacity": 2.0 }),
            Value::Null,
            Value::Null,
            Some(2.0),
        ),
        (
            // "When you specify a Python shell job … you can allocate either 0.0625 or 1
            // DPU": a Python shell job's capacity is a DPU figure, not workers.
            "a DPU figure on a Python shell job",
            json!({ "Command": python_shell.clone(), "MaxCapacity": 1.0 }),
            Value::Null,
            Value::Null,
            Some(1.0),
        ),
        (
            // A fraction of a G.1X worker is no worker count, so none is reported.
            "a fractional DPU figure",
            json!({ "Command": python_shell, "MaxCapacity": 0.0625 }),
            Value::Null,
            Value::Null,
            Some(0.0625),
        ),
        (
            // A worker configuration the request named is reported as it was named, and the
            // DPU figure follows from it: "each worker maps to 2 DPU" for G.2X.
            "a worker configuration named outright",
            json!({ "Command": spark, "WorkerType": "G.2X", "NumberOfWorkers": 3 }),
            json!("G.2X"),
            json!(3),
            Some(6.0),
        ),
        (
            // Nothing named a capacity, so the service fills in the documented default.
            "no capacity at all",
            json!({}),
            json!(DEFAULT_SPARK_WORKER_TYPE),
            json!(DEFAULT_SPARK_WORKERS),
            dpu_per_worker(DEFAULT_SPARK_WORKER_TYPE)
                .map(|per_worker| per_worker * f64::from(DEFAULT_SPARK_WORKERS)),
        ),
    ];

    for (index, (case, named, worker_type, workers, capacity)) in rows.iter().enumerate() {
        let name = format!("capacity-form-{index}");
        let (status, body) = call(&router, "CreateJob", &job_body(&name, named.clone())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateJob must answer 200, got {status} with {body}"
        );
        let job = get_job(&router, &name).await;
        for (member, expected) in [("WorkerType", worker_type), ("NumberOfWorkers", workers)] {
            let actual = job.get(member).unwrap_or(&Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected}, got {actual} in {job}"
            );
        }
        assert_eq!(
            job["MaxCapacity"].as_f64(),
            *capacity,
            "case {case:?}: expected MaxCapacity {capacity:?}, got {job}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a run reports about where it ran
// ---------------------------------------------------------------------------

/// `JobRun$LogGroupName` is "the name of the log group for secure logging … This name can
/// be `/aws-glue/jobs/`": a run reports the group its job's command logs under, and — when
/// a security configuration applies to it — that group with the role and configuration
/// names appended, which is the form the member documents for an encrypted group.
///
/// The rows differ along the command (each of the four `JobCommand$Name` values documents)
/// and along where the security configuration comes from: nowhere, the job definition, or
/// the start that overrides it.
#[tokio::test]
async fn a_run_reports_the_log_group_its_job_logs_under() {
    let router = emulator();

    // (case, what the create names, what the start names, the group the run must report)
    let rows: Vec<(&str, Value, Value, String)> = vec![
        (
            "a Spark ETL job",
            json!({}),
            json!({}),
            SPARK_JOB_LOG_GROUP.to_string(),
        ),
        (
            "a Python shell job",
            json!({
                "Command": {
                    "Name": "pythonshell",
                    "ScriptLocation": "s3://etl-scripts/report.py",
                    "PythonVersion": "3",
                },
            }),
            json!({}),
            PYTHON_SHELL_JOB_LOG_GROUP.to_string(),
        ),
        (
            "a streaming job",
            json!({
                "Command": {
                    "Name": "gluestreaming",
                    "ScriptLocation": "s3://etl-scripts/stream.py",
                },
            }),
            json!({}),
            SPARK_JOB_LOG_GROUP.to_string(),
        ),
        (
            "a Ray job",
            json!({
                "Command": {
                    "Name": "glueray",
                    "ScriptLocation": "s3://etl-scripts/ray.py",
                    "Runtime": "Ray2.4",
                },
            }),
            json!({}),
            SPARK_JOB_LOG_GROUP.to_string(),
        ),
        (
            "a job whose definition names a security configuration",
            json!({ "SecurityConfiguration": "jobs-cmk" }),
            json!({}),
            format!("{SPARK_JOB_LOG_GROUP}-{ROLE_NAME}-jobs-cmk"),
        ),
        (
            // "This value overrides the … value set in the parent job": the run's own
            // configuration is the one its group is encrypted with.
            "a start that overrides the configuration",
            json!({
                "Command": {
                    "Name": "pythonshell",
                    "ScriptLocation": "s3://etl-scripts/report.py",
                    "PythonVersion": "3",
                },
                "SecurityConfiguration": "jobs-cmk",
            }),
            json!({ "SecurityConfiguration": "run-cmk" }),
            format!("{PYTHON_SHELL_JOB_LOG_GROUP}-{ROLE_NAME}-run-cmk"),
        ),
    ];

    for (index, (case, job, start, expected)) in rows.iter().enumerate() {
        let name = format!("log-group-{index}");
        ok(&router, "CreateJob", &job_body(&name, job.clone())).await;
        let run = run_of(&router, &name, start.clone()).await;
        assert_eq!(
            run["LogGroupName"].as_str(),
            Some(expected.as_str()),
            "case {case:?}: expected LogGroupName {expected:?}, got {run}"
        );
    }
}

/// A run reports the Glue version of the job definition it is a run of — `StartJobRun`
/// carries no version of its own — and its predecessors as a LIST: the runs "used in the
/// predicate of a conditional trigger that triggered this job run", which is the empty list
/// for a run nothing preceded.
///
/// The rows differ along whether the definition named a version.
#[tokio::test]
async fn a_run_reports_its_jobs_glue_version_and_an_empty_predecessor_list() {
    let router = emulator();

    // (case, what the create names, the version the run must report). The version a
    // definition naming none is given is stated as the literal the recorded reads of such a
    // job and such a run report, so the constant itself is under test here.
    let rows: Vec<(&str, Value, &str)> = vec![
        ("a definition naming no version", json!({}), "6.0"),
        (
            "a definition naming a version",
            json!({ "GlueVersion": "4.0" }),
            "4.0",
        ),
        (
            "a definition naming the automatic version",
            json!({ "GlueVersion": "auto" }),
            "auto",
        ),
    ];
    assert_eq!(
        DEFAULT_GLUE_VERSION, rows[0].2,
        "the version a definition naming none is given must be the recorded one, got \
         {DEFAULT_GLUE_VERSION}"
    );

    for (index, (case, job, version)) in rows.iter().enumerate() {
        let name = format!("run-version-{index}");
        ok(&router, "CreateJob", &job_body(&name, job.clone())).await;
        let run = run_of(
            &router,
            &name,
            json!({ "Arguments": { "--run-argument": "sent-by-the-start" } }),
        )
        .await;
        assert_eq!(
            run["GlueVersion"].as_str(),
            Some(*version),
            "case {case:?}: expected GlueVersion {version:?}, got {run}"
        );
        assert_eq!(
            run["PredecessorRuns"].as_array(),
            Some(&Vec::new()),
            "case {case:?}: a run a caller started is preceded by nothing, so \
             PredecessorRuns is the empty list, got {run}"
        );
        // "The job arguments associated with this run": the arguments the start sent are
        // reported under the member the modelled `JobRun` carries them in.
        assert_eq!(
            run["Arguments"],
            json!({ "--run-argument": "sent-by-the-start" }),
            "case {case:?}: expected the start's arguments on the run, got {run}"
        );
    }
}

/// A run a trigger's firing began reports, as its predecessors, the runs that satisfied the
/// firing trigger's predicate: "a job run that was used in the predicate of a conditional
/// trigger that triggered this job run" (`Predecessor`).
///
/// A `CONDITIONAL` trigger fires on its predicate rather than on a call, so the firing is
/// initiated through the store's own firing path — the one `StartTrigger` and
/// `StartWorkflowRun` drive — and what it recorded is then read back over the wire.
#[tokio::test]
async fn a_run_a_firing_began_reports_the_runs_its_predicate_named() {
    let (router, engine) = emulator_with_engine();

    let predicate_job = "predicate-job";
    let triggered_job = "triggered-job";
    for job in [predicate_job, triggered_job] {
        ok(&router, "CreateJob", &job_body(job, json!({}))).await;
    }

    // The run the predicate is about, brought to the terminal state the condition names.
    let preceding = run_of(&router, predicate_job, json!({})).await;
    let preceding_id = preceding["Id"]
        .as_str()
        .expect("a run reports its Id")
        .to_string();
    set_due_instant(&engine, predicate_job, already_past());

    let actions = json!([{ "JobName": triggered_job }]);
    ok(
        &router,
        "CreateTrigger",
        &json!({
            "Name": "on-predicate",
            "Type": TRIGGER_TYPE_CONDITIONAL,
            "Actions": actions,
            "Predicate": {
                "Conditions": [{
                    "LogicalOperator": "EQUALS",
                    "JobName": predicate_job,
                    "State": JOB_RUN_STATE_FAILED,
                }],
            },
        }),
    )
    .await;

    // The firing itself: the path every fired trigger's actions are initiated through.
    let begun = {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let actions = store
            .trigger(REGION, "on-predicate")
            .map(|trigger| trigger.actions.clone())
            .expect("the trigger was created");
        store.initiate_trigger_actions(REGION, "on-predicate", &actions, SystemTime::now())
    };
    assert_eq!(
        begun.len(),
        1,
        "the firing must begin one run of {triggered_job}, began {begun:?}"
    );

    // Asked for with `PredecessorsIncluded`: "True if a list of predecessor runs should be
    // returned", so the runs themselves are reported for a read that asks and the empty list
    // for one that does not.
    let read = ok(
        &router,
        "GetJobRun",
        &json!({
            "JobName": triggered_job,
            "RunId": begun[0].run_id.clone(),
            "PredecessorsIncluded": true,
        }),
    )
    .await;
    let run = read["JobRun"].clone();
    assert_eq!(
        run["PredecessorRuns"],
        json!([{ "JobName": predicate_job, "RunId": preceding_id }]),
        "the run the firing began must report the run its predicate named, got {run}"
    );

    let unasked = ok(
        &router,
        "GetJobRun",
        &json!({ "JobName": triggered_job, "RunId": begun[0].run_id.clone() }),
    )
    .await;
    let unasked = unasked["JobRun"].clone();
    assert_eq!(
        unasked["PredecessorRuns"].as_array(),
        Some(&Vec::new()),
        "a read that does not ask for the predecessors reports the empty list, got {unasked}"
    );
}

// ---------------------------------------------------------------------------
// What a session read fills in
// ---------------------------------------------------------------------------

/// One session row: the create and the members the read must report for it.
struct SessionRow {
    case: &'static str,
    request: Value,
    /// `Session$DefaultArguments`, which is the empty map when the create sent none.
    arguments: Value,
    /// `Session$SessionType`.
    session_type: &'static str,
    /// `Session$WorkerType` and `Session$NumberOfWorkers`, `null` for a session whose
    /// capacity is a DPU figure.
    worker_type: Value,
    workers: Value,
    /// `Session$MaxCapacity`.
    capacity: Option<f64>,
}

/// A session reports the kind it is, the arguments its statements run with, the capacity it
/// was provisioned with and its execution accounting on every read — the values the create
/// sent where it sent them, and the service's own otherwise: the empty argument map, the
/// default session type, the worker configuration a create that named no capacity is
/// provisioned with, and zero for the time and the DPU-seconds nothing has consumed yet.
///
/// The rows differ along which of those members the create named and how the capacity was
/// expressed.
#[tokio::test]
async fn a_session_reports_the_members_the_service_fills_in() {
    let router = emulator();
    let role = "arn:aws:iam::000000000000:role/GlueSessionRole";
    // The kind and the worker configuration a create that names neither is provisioned with
    // are stated as the literals the recorded reads of such a session report, so the
    // constants that carry them are under test here and not only under replay.
    assert_eq!(
        (
            DEFAULT_SESSION_TYPE,
            DEFAULT_SESSION_WORKER_TYPE,
            DEFAULT_SESSION_WORKERS
        ),
        ("LIVY", "G.1X", 2),
        "the session defaults must be the recorded ones"
    );
    let default_capacity = dpu_per_worker(DEFAULT_SESSION_WORKER_TYPE)
        .expect("the default session worker type maps to DPUs")
        * f64::from(DEFAULT_SESSION_WORKERS);

    let rows: Vec<SessionRow> = vec![
        SessionRow {
            case: "the required members alone",
            request: json!({
                "Id": "filled-plain", "Role": role, "Command": { "Name": "glueetl" },
            }),
            arguments: json!({}),
            session_type: "LIVY",
            worker_type: json!("G.1X"),
            workers: json!(2),
            // Two G.1X workers, each of which "maps to 1 DPU".
            capacity: Some(2.0),
        },
        SessionRow {
            case: "every filled member named",
            request: json!({
                "Id": "filled-named",
                "Role": role,
                "Command": { "Name": "glueetl", "PythonVersion": "3" },
                "DefaultArguments": { "--enable-metrics": "true" },
                "SessionType": "SPARK_CONNECT",
                "WorkerType": "G.2X",
                "NumberOfWorkers": 3,
            }),
            arguments: json!({ "--enable-metrics": "true" }),
            session_type: "SPARK_CONNECT",
            worker_type: json!("G.2X"),
            workers: json!(3),
            // "each worker maps to 2 DPU" for G.2X.
            capacity: Some(6.0),
        },
        SessionRow {
            case: "a capacity named as a DPU figure",
            request: json!({
                "Id": "filled-dpus",
                "Role": role,
                "Command": { "Name": "glueetl" },
                "MaxCapacity": 5.0,
            }),
            arguments: json!({}),
            session_type: DEFAULT_SESSION_TYPE,
            worker_type: Value::Null,
            workers: Value::Null,
            capacity: Some(5.0),
        },
        SessionRow {
            case: "an empty argument map sent outright",
            request: json!({
                "Id": "filled-empty-arguments",
                "Role": role,
                "Command": { "Name": "glueetl" },
                "DefaultArguments": {},
                "SessionType": "LIVY",
            }),
            arguments: json!({}),
            session_type: "LIVY",
            worker_type: json!(DEFAULT_SESSION_WORKER_TYPE),
            workers: json!(DEFAULT_SESSION_WORKERS),
            capacity: Some(default_capacity),
        },
    ];

    for row in &rows {
        let case = row.case;
        let id = row.request["Id"].as_str().expect("the case carries an Id");
        ok(&router, "CreateSession", &row.request).await;
        let session = get_session(&router, id).await;

        assert_eq!(
            session["DefaultArguments"], row.arguments,
            "case {case:?}: expected DefaultArguments {}, got {session}",
            row.arguments
        );
        assert_eq!(
            session["SessionType"].as_str(),
            Some(row.session_type),
            "case {case:?}: expected SessionType {:?}, got {session}",
            row.session_type
        );
        for (member, expected) in [
            ("WorkerType", &row.worker_type),
            ("NumberOfWorkers", &row.workers),
        ] {
            let actual = session.get(member).unwrap_or(&Value::Null);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected}, got {actual} in {session}"
            );
        }
        assert_eq!(
            session["MaxCapacity"].as_f64(),
            row.capacity,
            "case {case:?}: expected MaxCapacity {:?}, got {session}",
            row.capacity
        );
        // "The total time the session ran for" and "the DPUs consumed by the session": a
        // session that has not run has consumed neither, which is a zero and not nothing.
        for member in ["ExecutionTime", "DPUSeconds"] {
            assert_eq!(
                session[member].as_f64(),
                Some(0.0),
                "case {case:?}: expected {member} 0.0 on a session that has not run, got \
                 {session}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The identity a connection read reports
// ---------------------------------------------------------------------------

/// `Connection$LastUpdatedBy` is "the user, group, or role that last updated this
/// connection definition", reported as that identity's RELATIVE id — the resource part of
/// its IAM ARN — by every read of a connection: the single read, the listing, and the read
/// after an update rewrote the definition.
#[tokio::test]
async fn every_connection_read_reports_the_updater_in_its_relative_form() {
    let router = emulator();

    // The identity the emulator's credentials speak for is the one STS reports
    // (`crate::wire::CALLER_PRINCIPAL`); its relative id is the resource part of that ARN.
    let expected = CALLER_PRINCIPAL
        .rsplit(':')
        .next()
        .expect("the caller principal ARN carries a resource part");
    assert!(
        !expected.is_empty() && !expected.starts_with("arn:"),
        "the relative id must be the ARN's resource part, got {expected:?}"
    );

    let names = ["relative-one", "relative-two"];
    for name in names {
        ok(
            &router,
            "CreateConnection",
            &json!({
                "ConnectionInput": {
                    "Name": name,
                    "ConnectionType": "JDBC",
                    "ConnectionProperties": {
                        "JDBC_CONNECTION_URL": "jdbc:postgresql://db.internal:5432/warehouse",
                        "USERNAME": "etl",
                        "PASSWORD": "secret",
                    },
                },
            }),
        )
        .await;
    }

    // Every read of each connection: the single read and the one after an update.
    for name in names {
        for case in ["after the create", "after an update"] {
            if case == "after an update" {
                ok(
                    &router,
                    "UpdateConnection",
                    &json!({
                        "Name": name,
                        "ConnectionInput": {
                            "Name": name,
                            "ConnectionType": "JDBC",
                            "ConnectionProperties": {
                                "JDBC_CONNECTION_URL": "jdbc:mysql://db.internal:3306/w",
                            },
                            "Description": "rewritten",
                        },
                    }),
                )
                .await;
            }
            let body = ok(&router, "GetConnection", &json!({ "Name": name })).await;
            let connection = body["Connection"].clone();
            assert_eq!(
                connection["LastUpdatedBy"].as_str(),
                Some(expected),
                "case {case:?} on {name:?}: expected LastUpdatedBy {expected:?}, got \
                 {connection}"
            );
        }
    }

    // The listing reports it the same way for every entry.
    let listed = ok(&router, "GetConnections", &json!({})).await;
    let entries = listed["ConnectionList"]
        .as_array()
        .unwrap_or_else(|| panic!("GetConnections must report a ConnectionList, got {listed}"))
        .clone();
    assert_eq!(
        entries.len(),
        names.len(),
        "the listing must report both connections, got {listed}"
    );
    for entry in &entries {
        assert_eq!(
            entry["LastUpdatedBy"].as_str(),
            Some(expected),
            "the listed entry {} must report LastUpdatedBy {expected:?}, got {entry}",
            entry["Name"]
        );
    }
}
