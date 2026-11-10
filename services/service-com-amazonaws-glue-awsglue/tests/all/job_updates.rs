// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateJob`, and the `GetJob`/`BatchGetJobs` that read back what
//! it wrote.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `UpdateJob` replaces the stored job definition whole with the
//! submitted one — "the previous job definition is completely overwritten by this
//! information", with "unspecified configuration … removed or reset to default values" —
//! re-deriving the members the SERVICE fills exactly as a create does, keeping only what
//! `JobUpdate` has no member for (the job's name, its create instant, its tags and the
//! runs already started of it) and the Glue version when the update names none, and
//! answering with the job's name; a name no job in the account holds is the modelled
//! not-found refusal.
//!
//! The recorded case — one update of a stored job, read back with `BatchGetJobs` — is ONE
//! row of the tables below; the rows differ along how many members the submitted
//! definition names, the command kind, the form the capacity is named in, whether the
//! update names a Glue version or leaves the stored one standing, and whether it names a
//! timeout.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, TaggedResourceKind, DEFAULT_JOB_MODE,
    DEFAULT_MAX_CONCURRENT_RUNS, DEFAULT_PYTHON_SHELL_DPUS, DEFAULT_SPARK_DPUS,
    DEFAULT_SPARK_WORKERS, DEFAULT_SPARK_WORKER_TYPE, DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5,
    DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4, MAX_TIMEOUT_MINUTES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, ok, ACCOUNT_ID};

/// The role ARN the stored definition carries before every update here.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role";

/// The role ARN an update may move the job to, so a row that renames the role is
/// observably a different value from the one the create stored.
const UPDATED_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-job-role-v2";

/// The Region every request in this suite is served in: the one the harness's SigV4
/// credential scope names, which is what a minted resource ARN carries.
const REGION: &str = "us-east-1";

/// The job definition each row creates BEFORE updating it: every optional member a
/// `CreateJob` request can carry, so a member the submitted `JobUpdate` does not name is
/// observably reset rather than merely absent from both definitions.
fn job_before_update(name: &str, glue_version: &str) -> Value {
    json!({
        "Name": name,
        "JobMode": "VISUAL",
        "JobRunQueuingEnabled": true,
        "Description": "the definition before the update",
        "LogUri": "s3://glue-logs/before/",
        "Role": ROLE_ARN,
        "ExecutionProperty": { "MaxConcurrentRuns": 7 },
        "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl-scripts/before.py" },
        "DefaultArguments": { "--before": "yes", "--enable-metrics": "true" },
        "NonOverridableArguments": { "--locked": "before" },
        "Connections": { "Connections": ["warehouse", "lake"] },
        "MaxRetries": 4,
        "Timeout": 123,
        "SecurityConfiguration": "glue-security",
        "NotificationProperty": { "NotifyDelayAfter": 9 },
        "GlueVersion": glue_version,
        "WorkerType": "G.4X",
        "NumberOfWorkers": 5,
        "SourceControlDetails": {
            "Provider": "AWS_CODE_COMMIT",
            "Repository": "etl-source",
            "Branch": "main",
            "Folder": "jobs",
            "AuthStrategy": "AWS_SECRETS_MANAGER",
        },
        "MaintenanceWindow": "Sun:3",
        "Tags": { "owner": "data-platform" },
    })
}

/// The value each `Job` member carries when the submitted `JobUpdate` names none:
/// "unspecified configuration is removed or reset to default values"
/// (`UpdateJobRequest$JobUpdate`).
///
/// The member list is `JobUpdate`'s own
/// (`smithy_query(query="shape",shape="JobUpdate")`), less three: `Role` and `Command`,
/// which that shape documents as required, and `GlueVersion`, which "retains the value
/// from the existing job definition" when an update names none. `Timeout` is stated by
/// each row instead, because the default it resets to depends on the Glue version that
/// then applies.
fn reset_values() -> Value {
    json!({
        // "When the JobMode field is missing or null, SCRIPT is assigned as the default
        // value."
        "JobMode": DEFAULT_JOB_MODE,
        // "If false or not populated, the job runs will not be considered for queueing":
        // the flag a definition naming none applies is false, and that is what it reports.
        "JobRunQueuingEnabled": false,
        "Description": Value::Null,
        "LogUri": Value::Null,
        // "The maximum number of concurrent runs allowed for the job. The default is 1."
        "ExecutionProperty": { "MaxConcurrentRuns": DEFAULT_MAX_CONCURRENT_RUNS },
        "DefaultArguments": Value::Null,
        "NonOverridableArguments": Value::Null,
        "Connections": Value::Null,
        // The model's own `@default` for the two integers.
        "MaxRetries": 0,
        "AllocatedCapacity": 0,
        "MaxCapacity": Value::Null,
        "WorkerType": Value::Null,
        "NumberOfWorkers": Value::Null,
        "SecurityConfiguration": Value::Null,
        "NotificationProperty": Value::Null,
        "CodeGenConfigurationNodes": Value::Null,
        "ExecutionClass": Value::Null,
        "SourceControlDetails": Value::Null,
        "MaintenanceWindow": Value::Null,
    })
}

/// One update: the definition stored first, the `JobUpdate` submitted over it, and the
/// members the SERVICE derives from that submitted definition.
struct UpdateRow {
    case: &'static str,
    name: &'static str,
    /// The `GlueVersion` the created definition carries.
    created_with: &'static str,
    /// The submitted `JobUpdate`.
    update: Value,
    /// What the service derives for the submitted definition: always the Glue version
    /// that applies and the timeout, plus the capacity figures when the definition names
    /// a capacity in any of its forms.
    derived: Value,
}

/// The `Job` structure `GetJob` reports for `name`.
async fn get_job(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetJob", &json!({ "JobName": name })).await;
    body.get("Job")
        .cloned()
        .unwrap_or_else(|| panic!("GetJob must report a Job, got {body}"))
}

/// The single `Job` `BatchGetJobs` reports for `name`, which must be the same projection
/// `GetJob` serves.
async fn batch_get_job(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "BatchGetJobs", &json!({ "JobNames": [name] })).await;
    body.get("Jobs")
        .and_then(Value::as_array)
        .and_then(|jobs| jobs.first())
        .cloned()
        .unwrap_or_else(|| panic!("BatchGetJobs must report job {name:?}, got {body}"))
}

/// Submit one `JobUpdate` for `name` and assert the operation answers that name.
async fn update_job(router: &MultiServiceRouter, case: &str, name: &str, update: &Value) {
    let answered = ok(
        router,
        "UpdateJob",
        &json!({ "JobName": name, "JobUpdate": update }),
    )
    .await;
    assert_eq!(
        answered.get("JobName").and_then(Value::as_str),
        Some(name),
        "case {case:?}: UpdateJob must answer the name of the updated job, got {answered}"
    );
}

// ---------------------------------------------------------------------------
// The overwrite
// ---------------------------------------------------------------------------

/// Every member of the submitted definition is stored, and every member it does not name
/// is reset to its documented default — the previous definition survives nowhere.
#[tokio::test]
async fn an_update_overwrites_the_whole_job_definition() {
    let router = emulator();

    let rows = vec![
        UpdateRow {
            // The two members `JobUpdate` documents as required and nothing else, so
            // every other member of the rich definition stored first is reset.
            case: "the required members only, on a glue 4 job",
            name: "reset-to-defaults",
            created_with: "4.0",
            update: json!({
                "Role": UPDATED_ROLE_ARN,
                "Command": { "Name": "pythonshell", "ScriptLocation": "s3://etl/shell.py" },
            }),
            derived: json!({
                // "If not provided in the update request, the Glue version retains the
                // value from the existing job definition."
                "GlueVersion": "4.0",
                // "the timeout is defaulted to 2,880 minutes for Glue version 4.0 and
                // earlier"
                "Timeout": DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4,
                // The submitted definition names no capacity, so the service fills in the
                // documented default for its command: "When you specify a Python shell job
                // … you can allocate either 0.0625 or 1 DPU. The default is 0.0625 DPU."
                "MaxCapacity": DEFAULT_PYTHON_SHELL_DPUS,
                // A fraction of a DPU is no whole DPU.
                "AllocatedCapacity": 0,
            }),
        },
        UpdateRow {
            // The same update over a job whose retained Glue version turns the timeout
            // default the other way.
            case: "the required members only, on a glue 5 job",
            name: "reset-on-glue-5",
            created_with: "5.0",
            update: json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/etl.py" },
            }),
            derived: json!({
                "GlueVersion": "5.0",
                // "or 480 minutes for Glue version 5.0 and later"
                "Timeout": DEFAULT_TIMEOUT_MINUTES_FROM_GLUE_5,
                // No capacity named on a Spark job, so the documented ten DPUs, which
                // from Glue 2.0 on are ten G.1X workers.
                "WorkerType": DEFAULT_SPARK_WORKER_TYPE,
                "NumberOfWorkers": DEFAULT_SPARK_WORKERS,
                "MaxCapacity": DEFAULT_SPARK_DPUS,
                "AllocatedCapacity": DEFAULT_SPARK_WORKERS,
            }),
        },
        UpdateRow {
            case: "a new glue version, command kind and worker configuration",
            name: "new-version",
            created_with: "4.0",
            update: json!({
                "Role": UPDATED_ROLE_ARN,
                "Command": { "Name": "gluestreaming", "ScriptLocation": "s3://etl/stream.py" },
                "GlueVersion": "5.0",
                "WorkerType": "G.1X",
                "NumberOfWorkers": 3,
                "Timeout": 60,
                "MaintenanceWindow": "Mon:10",
            }),
            derived: json!({
                "GlueVersion": "5.0",
                "Timeout": 60,
                // 1 DPU per G.1X worker x 3 workers.
                "MaxCapacity": 3.0,
                "AllocatedCapacity": 3,
            }),
        },
        UpdateRow {
            // "When you specify a Python shell job … you can allocate either 0.0625 or
            // 1 DPU."
            case: "an explicit fractional max capacity",
            name: "fractional-capacity",
            created_with: "4.0",
            update: json!({
                "Role": ROLE_ARN,
                "Command": {
                    "Name": "pythonshell",
                    "ScriptLocation": "s3://etl/shell.py",
                    "PythonVersion": "3",
                },
                "GlueVersion": "3.0",
                "MaxCapacity": 0.0625,
            }),
            derived: json!({
                "GlueVersion": "3.0",
                "Timeout": DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4,
                "MaxCapacity": 0.0625,
                // A fraction of a DPU is no whole DPU.
                "AllocatedCapacity": 0,
            }),
        },
        UpdateRow {
            // "This field is deprecated. Use MaxCapacity instead" — the two name one
            // quantity, so the updated job reports the same number for both.
            case: "the deprecated allocated capacity",
            name: "allocated-capacity",
            created_with: "1.0",
            update: json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/old.py" },
                "AllocatedCapacity": 10,
            }),
            derived: json!({
                "GlueVersion": "1.0",
                "Timeout": DEFAULT_TIMEOUT_MINUTES_THROUGH_GLUE_4,
                "MaxCapacity": 10.0,
            }),
        },
        UpdateRow {
            case: "every member the job update can carry",
            name: "fully-updated",
            created_with: "4.0",
            update: json!({
                "JobMode": "NOTEBOOK",
                "JobRunQueuingEnabled": false,
                "Description": "the definition after the update",
                "LogUri": "s3://glue-logs/after/",
                "Role": UPDATED_ROLE_ARN,
                "ExecutionProperty": { "MaxConcurrentRuns": 3 },
                "Command": {
                    "Name": "glueetl",
                    "ScriptLocation": "s3://etl-scripts/after.py",
                    "PythonVersion": "3",
                },
                "DefaultArguments": { "--after": "yes" },
                "NonOverridableArguments": { "--locked": "after" },
                "Connections": { "Connections": ["lakehouse"] },
                "MaxRetries": 1,
                "Timeout": 90,
                "SecurityConfiguration": "glue-security-v2",
                "NotificationProperty": { "NotifyDelayAfter": 2 },
                "GlueVersion": "5.0",
                "WorkerType": "G.2X",
                "NumberOfWorkers": 10,
                // "Only jobs with Glue version 3.0 and above and command type glueetl
                // will be allowed to set ExecutionClass to FLEX."
                "ExecutionClass": "FLEX",
                "SourceControlDetails": {
                    "Provider": "GITHUB",
                    "Repository": "etl-source-v2",
                    "Branch": "release",
                },
                "MaintenanceWindow": "Fri:23",
            }),
            derived: json!({
                "GlueVersion": "5.0",
                "Timeout": 90,
                "MaxCapacity": 20.0,
                "AllocatedCapacity": 20,
            }),
        },
        UpdateRow {
            // The upper bound the service documents: "Jobs must have timeout values less
            // than 7 days or 10080 minutes."
            case: "a ray job at the documented maximum timeout",
            name: "ray-at-the-bound",
            created_with: "4.0",
            update: json!({
                "Role": ROLE_ARN,
                "Command": {
                    "Name": "glueray",
                    "ScriptLocation": "s3://etl/ray.py",
                    "Runtime": "Ray2.4",
                },
                "WorkerType": "Z.2X",
                "NumberOfWorkers": 2,
                "Timeout": MAX_TIMEOUT_MINUTES,
            }),
            derived: json!({
                "GlueVersion": "4.0",
                "Timeout": MAX_TIMEOUT_MINUTES,
                "MaxCapacity": 4.0,
                "AllocatedCapacity": 4,
            }),
        },
    ];

    for row in rows {
        let case = row.case;
        ok(
            &router,
            "CreateJob",
            &job_before_update(row.name, row.created_with),
        )
        .await;
        let before = get_job(&router, row.name).await;

        update_job(&router, case, row.name, &row.update).await;
        let job = get_job(&router, row.name).await;

        // Everything the submitted definition named is what the job now carries.
        assert_echoed(case, &row.update, &job);
        // And the members the service derives from it are the documented ones.
        assert_echoed(case, &row.derived, &job);

        // Every member of `JobUpdate` the submitted definition did NOT name is back at
        // its default: nothing of the previous definition survives.
        let mut resets = reset_values();
        {
            let members = resets.as_object_mut().expect("the reset table is an object");
            for named in row
                .update
                .as_object()
                .expect("a JobUpdate is a structure")
                .keys()
                .chain(
                    row.derived
                        .as_object()
                        .expect("the derived table is an object")
                        .keys(),
                )
            {
                members.remove(named);
            }
        }
        assert_echoed(case, &resets, &job);

        // The job's name is not a `JobUpdate` member, so the update cannot move it, and
        // the create instant stays where the create put it.
        assert_eq!(
            job.get("Name").and_then(Value::as_str),
            Some(row.name),
            "case {case:?}: an update does not rename the job, got {job}"
        );
        assert_eq!(
            job.get("CreatedOn"),
            before.get("CreatedOn"),
            "case {case:?}: an update does not move the create instant, got {job}"
        );
        let created_on = job
            .get("CreatedOn")
            .and_then(Value::as_f64)
            .unwrap_or_else(|| panic!("case {case:?}: CreatedOn must be a timestamp, got {job}"));
        let last_modified_on = job.get("LastModifiedOn").and_then(Value::as_f64);
        assert!(
            last_modified_on.is_some_and(|at| at >= created_on),
            "case {case:?}: expected LastModifiedOn at or after CreatedOn {created_on}, got \
             {last_modified_on:?}"
        );

        // The batch reader serves the same projection as the single one, and neither
        // read changes what the next caller sees.
        assert_eq!(
            batch_get_job(&router, row.name).await,
            job,
            "case {case:?}: BatchGetJobs must report the updated definition too"
        );
        assert_eq!(
            get_job(&router, row.name).await,
            job,
            "case {case:?}: reading the updated job must be repeatable"
        );
    }
}

/// An update leaves the four things `JobUpdate` carries no member for exactly as they
/// were — the job's name, its create instant, its tags and the runs already started of
/// it — and moves only the last-modified instant.
#[tokio::test]
async fn an_update_keeps_what_a_job_update_cannot_carry() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let name = "job-with-history";

    ok(&router, "CreateJob", &job_before_update(name, "4.0")).await;
    let started = ok(&router, "StartJobRun", &json!({ "JobName": name })).await;
    let run_id = started
        .get("JobRunId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("StartJobRun must answer a JobRunId, got {started}"))
        .to_string();

    // Hold both server-set instants at a fixed past one, so the move the update owes
    // `LastModifiedOn` is observable without waiting on the clock's resolution.
    let seeded = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let job = store
            .job_mut(crate::wire::REGION, name)
            .unwrap_or_else(|| panic!("the store must hold job {name:?}"));
        job.created_on = seeded;
        job.last_modified_on = seeded;
    }
    let seeded_epoch = 1_700_000_000_f64;

    update_job(
        &router,
        "keeps what a job update cannot carry",
        name,
        &json!({
            "Role": UPDATED_ROLE_ARN,
            "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/after.py" },
        }),
    )
    .await;

    let job = get_job(&router, name).await;
    assert_eq!(
        job.get("Name").and_then(Value::as_str),
        Some(name),
        "case name: an update does not rename the job, got {job}"
    );
    assert_eq!(
        job.get("CreatedOn").and_then(Value::as_f64),
        Some(seeded_epoch),
        "case create instant: an update does not move CreatedOn, got {job}"
    );
    let last_modified_on = job.get("LastModifiedOn").and_then(Value::as_f64);
    assert!(
        last_modified_on.is_some_and(|at| at > seeded_epoch),
        "case last-modified instant: expected an instant after {seeded_epoch}, got \
         {last_modified_on:?}"
    );

    // `Tags` is not a `JobUpdate` member, so the overwrite cannot clear it.
    let arn = TaggedResourceKind::Job.arn(REGION, name);
    let tags = ok(&router, "GetTags", &json!({ "ResourceArn": arn })).await;
    assert_eq!(
        tags.get("Tags"),
        Some(&json!({ "owner": "data-platform" })),
        "case tags: an update keeps the job's tags, got {tags}"
    );
    assert!(
        arn.contains(ACCOUNT_ID),
        "case tags: the job ARN must name this account, got {arn}"
    );

    // Nor the runs already started of the definition.
    let read = ok(
        &router,
        "GetJobRun",
        &json!({ "JobName": name, "RunId": run_id }),
    )
    .await;
    assert_eq!(
        read.get("JobRun")
            .and_then(|run| run.get("Id"))
            .and_then(Value::as_str),
        Some(run_id.as_str()),
        "case runs: an update keeps the runs already started of the job, got {read}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A request `UpdateJob` cannot serve is refused with the error the model names for it,
/// and the stored definition is left exactly as it was. The rows differ along what is at
/// fault: the job addressed, the two members `JobUpdate` documents as required, the
/// command kind, the timeout, the capacity named twice, and the execution class.
#[tokio::test]
async fn an_unservable_update_is_refused_with_the_modeled_error() {
    let router = emulator();
    let name = "untouched";
    ok(&router, "CreateJob", &job_before_update(name, "2.0")).await;
    let before = get_job(&router, name).await;

    // (case, the job addressed, the submitted JobUpdate, the expected error)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "a job no name in the account holds",
            "absent",
            json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
            }),
            "EntityNotFoundException",
        ),
        (
            // `Job$Name` carries no documented case fold, so a different casing names a
            // different job.
            "a stored name in another casing",
            "UNTOUCHED",
            json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
            }),
            "EntityNotFoundException",
        ),
        (
            // "The name or Amazon Resource Name (ARN) of the IAM role associated with
            // this job (required)."
            "a definition naming no role",
            name,
            json!({ "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" } }),
            "InvalidInputException",
        ),
        (
            "a definition with a blank role",
            name,
            json!({
                "Role": "   ",
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
            }),
            "InvalidInputException",
        ),
        (
            // "The JobCommand that runs this job (required)."
            "a definition naming no command",
            name,
            json!({ "Role": ROLE_ARN }),
            "InvalidInputException",
        ),
        (
            "a command naming no job kind at all",
            name,
            json!({ "Role": ROLE_ARN, "Command": { "ScriptLocation": "s3://etl/x.py" } }),
            "InvalidInputException",
        ),
        (
            // "For an Apache Spark ETL job, this must be glueetl. For a Python shell
            // job, it must be pythonshell. … gluestreaming … glueray."
            "a command the service does not run",
            name,
            json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "sparkmagic", "ScriptLocation": "s3://etl/x.py" },
            }),
            "InvalidInputException",
        ),
        (
            // "Jobs must have timeout values less than 7 days or 10080 minutes."
            "a timeout one minute past the documented maximum",
            name,
            json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
                "Timeout": MAX_TIMEOUT_MINUTES + 1,
            }),
            "InvalidInputException",
        ),
        (
            // "Do not set MaxCapacity if using WorkerType and NumberOfWorkers."
            "a capacity named twice",
            name,
            json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
                "MaxCapacity": 10.0,
                "WorkerType": "G.1X",
                "NumberOfWorkers": 10,
            }),
            "InvalidInputException",
        ),
        (
            // "Only jobs with Glue version 3.0 and above and command type glueetl will
            // be allowed to set ExecutionClass to FLEX."
            "the flexible execution class on a python shell job",
            name,
            json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "pythonshell", "ScriptLocation": "s3://etl/x.py" },
                "GlueVersion": "5.0",
                "ExecutionClass": "FLEX",
            }),
            "InvalidInputException",
        ),
        (
            // The same class on the right command but a Glue version below the bound —
            // and here the version is the one the stored definition retains, because the
            // submitted definition names none.
            "the flexible execution class under the retained glue version",
            name,
            json!({
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
                "ExecutionClass": "FLEX",
            }),
            "InvalidInputException",
        ),
    ];

    for (case, job_name, update, expected) in rows {
        let (status, answer) = call(
            &router,
            "UpdateJob",
            &json!({ "JobName": job_name, "JobUpdate": update }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            error_code(&answer).contains(expected),
            "case {case:?}: expected {expected}, got {answer}"
        );
        // A refused update wrote nothing: the stored definition is the one the create
        // stored, down to its last-modified instant.
        assert_eq!(
            get_job(&router, name).await,
            before,
            "case {case:?}: a refused update must leave the stored definition untouched"
        );
    }

    // And one below the modelled minimum: `Timeout`'s `range 1..` is refused at the
    // request boundary, before the handler runs, so only the refusal itself is asserted.
    let (status, answer) = call(
        &router,
        "UpdateJob",
        &json!({
            "JobName": name,
            "JobUpdate": {
                "Role": ROLE_ARN,
                "Command": { "Name": "glueetl", "ScriptLocation": "s3://etl/x.py" },
                "Timeout": 0,
            },
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case timeout 0: expected 400, got {status} with {answer}"
    );
    assert!(
        answer.get("__type").is_some(),
        "case timeout 0: the refusal must name a modeled error, got {answer}"
    );
    assert_eq!(
        get_job(&router, name).await,
        before,
        "case timeout 0: a refused update must leave the stored definition untouched"
    );
}

/// The update is addressed to one job: it changes no other definition the account holds.
#[tokio::test]
async fn an_update_reaches_only_the_job_it_names() {
    let router = emulator();
    for name in ["first", "second", "third"] {
        ok(&router, "CreateJob", &job_before_update(name, "4.0")).await;
    }
    let untouched: Vec<(&str, Value)> = vec![
        ("first", get_job(&router, "first").await),
        ("third", get_job(&router, "third").await),
    ];

    update_job(
        &router,
        "reaches only the job it names",
        "second",
        &json!({
            "Role": UPDATED_ROLE_ARN,
            "Command": { "Name": "glueray", "ScriptLocation": "s3://etl/ray.py" },
        }),
    )
    .await;

    for (name, before) in untouched {
        assert_eq!(
            get_job(&router, name).await,
            before,
            "case {name:?}: updating another job must not change this one"
        );
    }
    let updated = get_job(&router, "second").await;
    assert_eq!(
        updated.get("Role").and_then(Value::as_str),
        Some(UPDATED_ROLE_ARN),
        "case second: the named job carries the submitted role, got {updated}"
    );
}
