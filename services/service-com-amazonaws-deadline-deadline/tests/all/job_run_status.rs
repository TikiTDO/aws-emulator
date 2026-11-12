// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for a job's run status and the reconciliation that settles it.
//!
//! The rules under test:
//!
//! * a read of a job reports `taskRunStatus` — the run status the service has settled its
//!   tasks to — at every point in the job's life, and reports `targetTaskRunStatus`, the
//!   status the service is still working the job towards, only while the lifecycle says
//!   that work is in progress; once the lifecycle has settled, the target is dropped and
//!   the run status carries what was submitted;
//! * once a job's creation has completed, the service reconciles it against the fleets its
//!   queue schedules onto: a job awaiting scheduling on a queue with no fleet associated to
//!   it can never be picked up, so the clock-owed reconciliation settles it to
//!   `taskRunStatus=NOT_COMPATIBLE` / `lifecycleStatus=UPDATE_SUCCEEDED`, leaving every
//!   value the create persisted untouched; a job whose queue does have a fleet associated,
//!   and a job held back from scheduling by a suspended target, are not reconciled;
//! * `CreateQueueFleetAssociation` stores that association under its queue and
//!   `GetQueueFleetAssociation` reports it back, and a fleet a queue is still associated
//!   with cannot be deleted.
//!
//! Every reader of a job applies the same rule, so each assertion is made against `GetJob`,
//! `ListJobs` and `BatchGetJob`.
//!
//! The harness (`Emulator`) is the one `farm_queue_job` drives the service with, so these
//! tests go through routing, deserialization, constraint validation, the handler and
//! serialization, exactly as the router does in production.

use std::time::Duration;

use serde_json::{json, Value};

use crate::farm_queue_job::{absent_id, error_code, string_at, Emulator};
use crate::fleet::{create_fleet, get_fleet_path};

/// Long enough for the job's `CREATE_IN_PROGRESS -> CREATE_COMPLETE` move to be due
/// (`crate::state::JOB_CREATE_COMPLETE_AFTER`) and short enough that the later
/// reconciliation is not.
const PAST_CREATE_SPAN: Duration = Duration::from_millis(900);

/// Long enough for the reconciliation the clock owes a created job
/// (`crate::state::JOB_RECONCILE_AFTER`) to be due as well.
const PAST_RECONCILE_SPAN: Duration = Duration::from_millis(15_400);

fn create_job_path(farm_id: &str, queue_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs")
}

fn get_job_path(farm_id: &str, queue_id: &str, job_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queues/{queue_id}/jobs/{job_id}")
}

fn association_path(farm_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-fleet-associations")
}

fn get_association_path(farm_id: &str, queue_id: &str, fleet_id: &str) -> String {
    format!("/2023-10-12/farms/{farm_id}/queue-fleet-associations/{queue_id}/{fleet_id}")
}

/// A minimal OpenJD-shaped template, as `CreateJob` accepts it with `templateType=JSON`.
fn template(name: &str) -> String {
    json!({
        "specificationVersion": "jobtemplate-2023-09",
        "name": name,
        "steps": [],
    })
    .to_string()
}

/// Create a job from `extra` (the members beyond the template and its type) and return its
/// id.
async fn create_job(emulator: &Emulator, farm_id: &str, queue_id: &str, extra: &Value) -> String {
    let mut body = json!({
        "template": template("run-status"),
        "templateType": "JSON",
    });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        for (member, value) in extra {
            body.insert(member.clone(), value.clone());
        }
    }
    let (status, created) = emulator.post(&create_job_path(farm_id, queue_id), body).await;
    assert_eq!(
        status, 201,
        "CreateJob {extra}: expected 201, got {status} with {created}"
    );
    string_at(&created, "jobId")
}

/// Associate `fleet_id` with `queue_id`, the way `CreateQueueFleetAssociation` is bound:
/// `PUT /2023-10-12/farms/{farmId}/queue-fleet-associations` with the pair in the body.
async fn associate(emulator: &Emulator, farm_id: &str, queue_id: &str, fleet_id: &str) -> Value {
    let (status, body) = emulator
        .call(
            "PUT",
            &association_path(farm_id),
            Some(json!({"queueId": queue_id, "fleetId": fleet_id})),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "CreateQueueFleetAssociation: expected 200, got {status} with {body}"
    );
    body
}

/// The job `job_id` as each implemented reader of a job reports it: `GetJob`, the entry
/// `ListJobs` reports for it, and the item `BatchGetJob` reports for it.
///
/// A rule about a job's reported run status holds for all three, so every case asserts
/// against every one of them.
async fn read_three_ways(
    emulator: &Emulator,
    farm_id: &str,
    queue_id: &str,
    job_id: &str,
) -> Vec<(&'static str, Value)> {
    let (status, single) = emulator
        .get(&get_job_path(farm_id, queue_id, job_id))
        .await;
    assert_eq!(
        status, 200,
        "GetJob {job_id}: expected 200, got {status} with {single}"
    );

    let (status, listed) = emulator.get(&create_job_path(farm_id, queue_id)).await;
    assert_eq!(
        status, 200,
        "ListJobs: expected 200, got {status} with {listed}"
    );
    let summary = listed
        .get("jobs")
        .and_then(Value::as_array)
        .and_then(|jobs| {
            jobs.iter()
                .find(|job| job.get("jobId").and_then(Value::as_str) == Some(job_id))
                .cloned()
        })
        .unwrap_or_else(|| panic!("ListJobs did not report the job {job_id}: {listed}"));

    let (status, batch) = emulator
        .post(
            "/2023-10-12/batch-get-job",
            json!({"identifiers": [{"farmId": farm_id, "queueId": queue_id, "jobId": job_id}]}),
        )
        .await;
    assert_eq!(
        status, 200,
        "BatchGetJob: expected 200, got {status} with {batch}"
    );
    let item = batch
        .get("jobs")
        .and_then(Value::as_array)
        .and_then(|jobs| jobs.first().cloned())
        .unwrap_or_else(|| panic!("BatchGetJob did not report the job {job_id}: {batch}"));

    vec![("GetJob", single), ("ListJobs", summary), ("BatchGetJob", item)]
}

/// Assert, for every reader, that the job reports `lifecycle` and `run_status`, and that
/// `targetTaskRunStatus` is `target` (`None` meaning the member is absent).
fn assert_statuses(
    case: &str,
    reports: &[(&'static str, Value)],
    lifecycle: &str,
    run_status: &str,
    target: Option<&str>,
) {
    for (reader, job) in reports {
        assert_eq!(
            job.get("lifecycleStatus").and_then(Value::as_str),
            Some(lifecycle),
            "case {case:?}: expected {reader} to report lifecycleStatus {lifecycle:?}, got {job}"
        );
        assert_eq!(
            job.get("taskRunStatus").and_then(Value::as_str),
            Some(run_status),
            "case {case:?}: expected {reader} to report taskRunStatus {run_status:?}, got {job}"
        );
        assert_eq!(
            job.get("targetTaskRunStatus").and_then(Value::as_str),
            target,
            "case {case:?}: expected {reader} to report targetTaskRunStatus {target:?}, got {job}"
        );
    }
}

#[tokio::test]
async fn a_jobs_run_status_settles_on_the_submitted_target_when_creation_completes() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("run-status").await;
    let queue_id = emulator.queue(&farm_id, "run-status").await;

    // The rule generalises over the run status the request asks the job to start in:
    // `CreateJobTargetTaskRunStatus` is `READY | SUSPENDED` with `READY` the default, from
    // `smithy_query(query="operation", operation="CreateJob")`, so the rows are both
    // published values plus the request that names none. They differ in the other members
    // too, so the rule is not read off one shape of job.
    let cases: Vec<(&str, Value, &str)> = vec![
        ("no target submitted", json!({"priority": 50}), "READY"),
        (
            "submitted READY",
            json!({"priority": 0, "targetTaskRunStatus": "READY", "maxRetriesPerTask": 3}),
            "READY",
        ),
        (
            "submitted SUSPENDED",
            json!({
                "priority": 100,
                "targetTaskRunStatus": "SUSPENDED",
                "descriptionOverride": "held back",
            }),
            "SUSPENDED",
        ),
    ];

    for (case, request, expected) in cases {
        let job_id = create_job(&emulator, &farm_id, &queue_id, &request).await;

        // While the service is still assembling the job its tasks cannot be scheduled, and
        // the target it is converging to is what a read reports alongside them.
        let reports = read_three_ways(&emulator, &farm_id, &queue_id, &job_id).await;
        assert_statuses(
            case,
            &reports,
            "CREATE_IN_PROGRESS",
            "PENDING",
            Some(expected),
        );

        // Once creation has converged, the run status carries the submitted target and the
        // target itself is dropped: it is no longer something the job is working towards.
        tokio::time::sleep(PAST_CREATE_SPAN).await;
        let reports = read_three_ways(&emulator, &farm_id, &queue_id, &job_id).await;
        assert_statuses(case, &reports, "CREATE_COMPLETE", expected, None);

        // A read does not change what the next reader sees.
        let again = read_three_ways(&emulator, &farm_id, &queue_id, &job_id).await;
        for ((reader, first), (_, second)) in reports.iter().zip(again.iter()) {
            assert_eq!(
                first, second,
                "case {case:?}: two {reader} reads of one job disagreed: {first} then {second}"
            );
        }
    }
}

#[tokio::test]
async fn a_job_awaiting_scheduling_on_a_fleetless_queue_settles_not_compatible() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("reconcile").await;
    let queue_id = emulator.queue(&farm_id, "reconcile").await;

    // The rows differ along what the rule generalises over: the priority and the other
    // values the create persisted (which the reconciliation must leave alone), and whether
    // the job is awaiting scheduling at all. A job started suspended is held back from
    // scheduling until it is requeued, so nothing places it and nothing finds it
    // incompatible.
    let cases: Vec<(&str, Value, &str, &str)> = vec![
        (
            "the default priority band",
            json!({"priority": 42, "nameOverride": "mid"}),
            "UPDATE_SUCCEEDED",
            "NOT_COMPATIBLE",
        ),
        (
            "at the bottom of the priority range",
            json!({"priority": 0, "nameOverride": "low", "maxFailedTasksCount": 7}),
            "UPDATE_SUCCEEDED",
            "NOT_COMPATIBLE",
        ),
        (
            "at the top of the priority range",
            json!({"priority": 100, "nameOverride": "high", "maxWorkerCount": -1}),
            "UPDATE_SUCCEEDED",
            "NOT_COMPATIBLE",
        ),
        (
            "explicitly ready",
            json!({"priority": 25, "nameOverride": "ready", "targetTaskRunStatus": "READY"}),
            "UPDATE_SUCCEEDED",
            "NOT_COMPATIBLE",
        ),
        (
            "submitted suspended, so nothing schedules it",
            json!({"priority": 25, "nameOverride": "suspended", "targetTaskRunStatus": "SUSPENDED"}),
            "CREATE_COMPLETE",
            "SUSPENDED",
        ),
    ];

    // Every job is created first, so one wait covers the span the clock owes them all.
    let mut job_ids = Vec::new();
    for (_, request, _, _) in &cases {
        job_ids.push(create_job(&emulator, &farm_id, &queue_id, request).await);
    }
    tokio::time::sleep(PAST_RECONCILE_SPAN).await;

    for ((case, request, lifecycle, run_status), job_id) in cases.iter().zip(job_ids.iter()) {
        let reports = read_three_ways(&emulator, &farm_id, &queue_id, job_id).await;
        assert_statuses(case, &reports, lifecycle, run_status, None);

        for (reader, job) in &reports {
            // The reconciliation moves the job's status, not the job: everything the
            // create persisted still reads back exactly as it was submitted.
            assert_eq!(
                job.get("priority"),
                request.get("priority"),
                "case {case:?}: expected {reader} to report the submitted priority, got {job}"
            );
            assert_eq!(
                job.get("name").and_then(Value::as_str),
                request.get("nameOverride").and_then(Value::as_str),
                "case {case:?}: expected {reader} to report the submitted name, got {job}"
            );
            assert!(
                !job.get("lifecycleStatusMessage")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .is_empty(),
                "case {case:?}: expected {reader} to report a lifecycleStatusMessage, got {job}"
            );
        }

        // The settled job is settled: a later read reports the same thing.
        let again = read_three_ways(&emulator, &farm_id, &queue_id, job_id).await;
        for ((reader, first), (_, second)) in reports.iter().zip(again.iter()) {
            assert_eq!(
                first, second,
                "case {case:?}: two {reader} reads of a settled job disagreed: {first} then \
                 {second}"
            );
        }
    }
}

#[tokio::test]
async fn a_job_on_a_queue_with_an_associated_fleet_is_not_reconciled() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("associated").await;
    let queue_id = emulator.queue(&farm_id, "associated").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "associated").await;
    // A second queue of the same farm, with no fleet associated to it, is what makes the
    // association — rather than the farm holding a fleet at all — the thing that decides
    // the reconciliation.
    let lonely_queue_id = emulator.queue(&farm_id, "lonely").await;

    // The association is created before one job and after the other, so neither order
    // leaves a job the association does not cover.
    let before = create_job(&emulator, &farm_id, &queue_id, &json!({"priority": 42})).await;
    associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
    let after = create_job(&emulator, &farm_id, &queue_id, &json!({"priority": 42})).await;
    let lonely = create_job(&emulator, &farm_id, &lonely_queue_id, &json!({"priority": 42})).await;

    tokio::time::sleep(PAST_RECONCILE_SPAN).await;

    for (case, job_id) in [("created before the association", &before), ("created after the association", &after)] {
        let reports = read_three_ways(&emulator, &farm_id, &queue_id, job_id).await;
        assert_statuses(case, &reports, "CREATE_COMPLETE", "READY", None);
    }
    let reports = read_three_ways(&emulator, &farm_id, &lonely_queue_id, &lonely).await;
    assert_statuses(
        "a queue of the same farm with no association",
        &reports,
        "UPDATE_SUCCEEDED",
        "NOT_COMPATIBLE",
        None,
    );
}

#[tokio::test]
async fn create_queue_fleet_association_then_get_reports_what_was_stored() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("qfa").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "qfa").await;

    // One row per queue, so the association is read back for more than one pair.
    for display_name in ["first", "second"] {
        let queue_id = emulator.queue(&farm_id, display_name).await;
        associate(&emulator, &farm_id, &queue_id, &fleet_id).await;

        let (status, association) = emulator
            .get(&get_association_path(&farm_id, &queue_id, &fleet_id))
            .await;
        assert_eq!(
            status, 200,
            "case {display_name:?}: expected GetQueueFleetAssociation to answer 200, got \
             {status} with {association}"
        );
        assert_eq!(
            string_at(&association, "queueId"),
            queue_id,
            "case {display_name:?}: expected the associated queue, got {association}"
        );
        assert_eq!(
            string_at(&association, "fleetId"),
            fleet_id,
            "case {display_name:?}: expected the associated fleet, got {association}"
        );
        // `QueueFleetAssociationStatus` is `ACTIVE | STOPPED |
        // STOP_SCHEDULING_AND_CANCEL_TASKS | STOP_SCHEDULING_AND_COMPLETE_TASKS`, from
        // `smithy_query(query="shape", shape="QueueFleetAssociationStatus")`: a fresh
        // association schedules the queue's work onto the fleet.
        assert_eq!(
            string_at(&association, "status"),
            "ACTIVE",
            "case {display_name:?}: expected a scheduling association, got {association}"
        );
        assert!(
            !string_at(&association, "createdBy").is_empty(),
            "case {display_name:?}: expected a createdBy principal, got {association}"
        );
        assert!(
            // Modelled `@timestampFormat("date-time")`: an RFC 3339 string, never the
            // protocol-default epoch number (`date_time_wire_form` owns that rule).
            association.get("createdAt").and_then(Value::as_str).is_some(),
            "case {display_name:?}: expected createdAt as an RFC 3339 string, got \
             {association}"
        );
        assert!(
            association.get("updatedAt").is_none() && association.get("updatedBy").is_none(),
            "case {display_name:?}: expected no update members before an update, got \
             {association}"
        );

        // The operation is `@idempotent` and models no conflict, so associating the same
        // pair again answers the same success and leaves the stored association alone.
        associate(&emulator, &farm_id, &queue_id, &fleet_id).await;
        let (_, again) = emulator
            .get(&get_association_path(&farm_id, &queue_id, &fleet_id))
            .await;
        assert_eq!(
            again, association,
            "case {display_name:?}: a repeated association changed the stored one: \
             {association} then {again}"
        );
    }
}

#[tokio::test]
async fn get_queue_fleet_association_reports_an_unassociated_pair_as_missing() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("qfa-404").await;
    let queue_id = emulator.queue(&farm_id, "qfa-404").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "qfa-404").await;
    let other_queue_id = emulator.queue(&farm_id, "qfa-404-other").await;
    associate(&emulator, &farm_id, &queue_id, &fleet_id).await;

    let no_farm = absent_id("farm", "0000000000000000000000000000000a");
    let no_queue = absent_id("queue", "0000000000000000000000000000000b");
    let no_fleet = absent_id("fleet", "0000000000000000000000000000000c");

    // An association is reached only through its own farm and queue and joins them to a
    // fleet of that farm, so each level that is missing is reported as that level, and a
    // held fleet the queue is simply not associated with is the association that can't be
    // found — which the three ids of its path name, since the model mints it no id of its
    // own.
    let other_queues_association = format!("{farm_id}, {other_queue_id} and {fleet_id}");
    let cases: Vec<(&str, String, &str, String)> = vec![
        (
            "an unheld farm",
            get_association_path(&no_farm, &queue_id, &fleet_id),
            "farm",
            no_farm.clone(),
        ),
        (
            "an unheld queue",
            get_association_path(&farm_id, &no_queue, &fleet_id),
            "queue",
            no_queue.clone(),
        ),
        (
            "an unheld fleet",
            get_association_path(&farm_id, &queue_id, &no_fleet),
            "fleet",
            no_fleet.clone(),
        ),
        (
            "a held fleet this queue is not associated with",
            get_association_path(&farm_id, &other_queue_id, &fleet_id),
            "queue-fleet-association",
            other_queues_association.clone(),
        ),
    ];

    for (case, path, resource_type, resource_id) in cases {
        let (status, body) = emulator.get(&path).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a 404, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected the missing-resource code, got {body}"
        );
        let expected =
            format!("Resource of type {resource_type} with id {resource_id} does not exist.");
        assert_eq!(
            string_at(&body, "message"),
            expected,
            "case {case:?}: expected the message {expected:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn deleting_a_fleet_a_queue_is_associated_with_is_refused() {
    let emulator = Emulator::new();
    let farm_id = emulator.farm("delete-associated").await;
    let fleet_id = create_fleet(&emulator, &farm_id, "associated").await;
    let free_fleet_id = create_fleet(&emulator, &farm_id, "free").await;
    let queue_id = emulator.queue(&farm_id, "delete-associated").await;
    associate(&emulator, &farm_id, &queue_id, &fleet_id).await;

    let (status, body) = emulator
        .call(
            "DELETE",
            &get_fleet_path(&farm_id, &fleet_id),
            None,
            None,
        )
        .await;
    assert_eq!(
        status, 409,
        "DeleteFleet on an associated fleet: expected a 409, got {status} with {body}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "DeleteFleet on an associated fleet: expected a ConflictException, got {body}"
    );
    let expected = format!(
        "The fleet {fleet_id} can't be deleted because it is associated with the queue \
         {queue_id}."
    );
    assert_eq!(
        string_at(&body, "message"),
        expected,
        "DeleteFleet on an associated fleet: expected the message {expected:?}, got {body}"
    );

    // The refusal removed nothing.
    let (status, fleet) = emulator.get(&get_fleet_path(&farm_id, &fleet_id)).await;
    assert_eq!(
        status, 200,
        "GetFleet after the refused delete: expected 200, got {status} with {fleet}"
    );

    // A fleet of the same farm that no queue is associated with is still deleted.
    let (status, body) = emulator
        .call(
            "DELETE",
            &get_fleet_path(&farm_id, &free_fleet_id),
            None,
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "DeleteFleet on an unassociated fleet: expected 200, got {status} with {body}"
    );
}
