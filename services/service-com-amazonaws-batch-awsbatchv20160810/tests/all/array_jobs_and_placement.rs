// SPDX-License-Identifier: CC0-1.0

//! ARRAY jobs and where a `RUNNABLE` job is PLACED, end to end over the wire.
//!
//! Two rules are under test, each over a set of inputs.
//!
//! **An array submit creates the array.** `SubmitJob$arrayProperties`: "If you specify
//! array properties for a job, it becomes an array job", and `ArrayProperties$size` is "the
//! size of the array job", documented as "between 2 and 10,000". So the submit answers 200
//! with ONE job id — the array's PARENT — and `size` children exist under it, each
//! addressable by the parent's id and its own index and each reporting
//! `ArrayPropertiesDetail$index` ("the job index within the array that's associated with
//! this job ... returned for array job children"), while the parent reports
//! `ArrayPropertiesDetail$size` and the `statusSummary` its children add up to ("a summary
//! of the number of array job children in each available job status ... returned for parent
//! array jobs"). A size outside the documented range is the modelled `ClientException` and
//! creates nothing.
//!
//! **A job leaves `RUNNABLE` only where its queue has capacity to place it on.** `RUNNABLE`
//! is where "the job is ready to be scheduled to a host", and `computeResources` is "details
//! of the compute resources managed by the compute environment"
//! (`ComputeEnvironmentDetail$computeResources`) — so the capacity the SERVICE schedules onto
//! is a `MANAGED` environment's, and an `UNMANAGED` one holds only instances the caller
//! registered. `CEState$DISABLED` withholds placement until a caller re-enables the
//! environment. A job whose queue offers none of that waits in `RUNNABLE`; a job whose queue
//! does is placed, and because this emulator has no container runtime the placement ends in
//! the modelled failure state with `state::NO_CONTAINER_RUNTIME_REASON` — never with an
//! invented `startedAt`, `stoppedAt` or `exitCode`. What the PLACEMENT itself records — the
//! one job attempt, the task it was placed as and the log stream that task writes to — is
//! pinned in `job_attempt_records.rs`.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::ops::types::JOB_STATUS_VALUES;
use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{
    compute_environment, container_properties, describe_compute_environment, describe_job,
    error_code, error_message, job_definition, job_queue, until_status, Batch,
};

/// The compute resources a `MANAGED` Fargate environment is created with here: the two
/// required members of `ComputeResource` (`type`, `maxvCpus`) plus the networking a
/// Fargate environment needs.
fn fargate_compute_resources() -> Value {
    json!({
        "type": "FARGATE",
        "maxvCpus": 16,
        "subnets": ["subnet-1111"],
        "securityGroupIds": ["sg-1111"],
    })
}

/// A `MANAGED` Fargate compute environment called `name` in `ce_state`, polled until it
/// reports `VALID`; answers its ARN.
pub(crate) async fn managed_compute_environment(
    batch: &Batch,
    name: &str,
    ce_state: &str,
) -> String {
    let body = batch
        .ok(
            "/v1/createcomputeenvironment",
            json!({
                "computeEnvironmentName": name,
                "type": "MANAGED",
                "state": ce_state,
                "computeResources": fargate_compute_resources(),
            }),
        )
        .await;
    let arn = body["computeEnvironmentArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case create {name}: expected an ARN, got {body}"))
        .to_string();
    until_environment_valid(batch, name).await;
    arn
}

/// Poll `DescribeComputeEnvironments` until `name` reports `VALID`, through the harness's
/// own reader.
async fn until_environment_valid(batch: &Batch, name: &str) {
    let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
    for _ in 0..240 {
        let described = describe_compute_environment(batch, name).await;
        if described["status"].as_str() == Some(state::STATUS_VALID) {
            return;
        }
        tokio::time::sleep(Duration::from_millis((span / 10).max(5))).await;
    }
    panic!("case {name}: the compute environment never reached VALID");
}

/// One emulator with a queue over one compute environment, and a job definition to submit
/// into.
struct Fixture {
    batch: Batch,
    environment: String,
    queue_arn: String,
    definition: String,
}

/// A fresh emulator whose queue is backed by the compute environment `kind` describes, all
/// named after `prefix`.
async fn fixture(prefix: &str, kind: Capacity) -> Fixture {
    let batch = Batch::new();
    let name = format!("ce-{prefix}");
    let environment = match kind {
        Capacity::Unmanaged => compute_environment(&batch, &name).await,
        Capacity::ManagedEnabled => {
            managed_compute_environment(&batch, &name, state::STATE_ENABLED).await
        }
        Capacity::ManagedDisabled => {
            managed_compute_environment(&batch, &name, state::STATE_DISABLED).await
        }
    };
    let queue_arn = job_queue(&batch, &format!("q-{prefix}"), &environment).await;
    let (definition, _) =
        job_definition(&batch, &format!("jd-{prefix}"), container_properties()).await;
    Fixture {
        batch,
        environment,
        queue_arn,
        definition,
    }
}

/// What the compute environment behind a fixture's queue offers the scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Capacity {
    /// `UNMANAGED`: only instances the caller registered, of which this store holds none.
    Unmanaged,
    /// `MANAGED` and `ENABLED`: capacity the service itself schedules onto.
    ManagedEnabled,
    /// `MANAGED` but `DISABLED`: Batch places no job on it.
    ManagedDisabled,
}

/// Submit one job called `name`, with `extra` merged into the request; answers the response
/// body and its status.
async fn submit(fixture: &Fixture, name: &str, extra: Value) -> (u16, Value) {
    let mut body = json!({
        "jobName": name,
        "jobQueue": &fixture.queue_arn,
        "jobDefinition": &fixture.definition,
    });
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    fixture.batch.call("/v1/submitjob", body).await
}

/// Submit one job, requiring the modelled 200; answers its job id.
async fn submit_ok(fixture: &Fixture, name: &str, extra: Value) -> String {
    let (status, body) = submit(fixture, name, extra).await;
    assert_eq!(
        status, 200,
        "case submit {name}: expected HTTP 200, got {status} with {body}"
    );
    body["jobId"]
        .as_str()
        .unwrap_or_else(|| panic!("case submit {name}: expected a jobId, got {body}"))
        .to_string()
}

/// Assert that `detail` reports a job whose container nothing RAN: no `startedAt`, no
/// `stoppedAt` and no exit code — neither on the job nor on any attempt a placement
/// recorded for it. The attempt record itself belongs to the PLACEMENT, not to a run, and
/// what a placed job's attempt carries is pinned in `job_attempt_records.rs`.
fn assert_nothing_ran(case: &str, detail: &Value) {
    for member in ["startedAt", "stoppedAt"] {
        assert_eq!(
            detail[member],
            Value::Null,
            "case {case}: a job nothing ran reports no {member}, got {detail}"
        );
    }
    assert!(
        detail["container"]["exitCode"].is_null(),
        "case {case}: no exit code is invented, got {detail}"
    );
    for attempt in detail["attempts"].as_array().into_iter().flatten() {
        for member in ["startedAt", "stoppedAt"] {
            assert!(
                attempt[member].is_null(),
                "case {case}: an attempt nothing ran reports no {member}, got {attempt}"
            );
        }
        assert!(
            attempt["container"]["exitCode"].is_null(),
            "case {case}: no attempt exit code is invented, got {attempt}"
        );
    }
}

/// The `jobSummaryList` of `ListJobs` for `request`.
async fn list_jobs(batch: &Batch, request: Value) -> Vec<Value> {
    let body = batch.ok("/v1/listjobs", request).await;
    body["jobSummaryList"].as_array().cloned().unwrap_or_default()
}

/// RULE: a submit carrying `arrayProperties` creates the array — one parent reporting the
/// size and the summary of its children, and `size` children each carrying its own index,
/// all of them readable back through `DescribeJobs` and `ListJobs`.
///
/// Rows differ along the array's SIZE, which is what the rule generalises over: the
/// smallest array the member admits, one in between, and a sharded one.
#[tokio::test]
async fn an_array_submit_creates_children_each_carrying_its_own_index() {
    // (case, size) — 2 is the documented minimum, 24 a sharded workload.
    let rows: &[(&str, i32)] = &[
        ("the smallest array", state::MIN_ARRAY_SIZE),
        ("three children", 3),
        ("twenty-four shards", 24),
    ];

    for (index, (case, size)) in rows.iter().enumerate() {
        // An UNMANAGED environment places nothing, so every child comes to rest in
        // RUNNABLE and the parent's summary is stable while it is read.
        let fixture = fixture(&format!("arr{index}"), Capacity::Unmanaged).await;
        let parent_id = submit_ok(
            &fixture,
            &format!("array{index}"),
            json!({"arrayProperties": {"size": size}}),
        )
        .await;

        // The parent: the size it was submitted with, a summary counting every child, and
        // NO index of its own.
        let parent = describe_job(&fixture.batch, &parent_id).await;
        assert_eq!(
            parent["arrayProperties"]["size"].as_i64(),
            Some(i64::from(*size)),
            "case {case}: the parent reports the size it was submitted with, got {parent}"
        );
        assert_eq!(
            parent["arrayProperties"]["index"],
            Value::Null,
            "case {case}: a parent has no array index of its own, got {parent}"
        );
        let summary = parent["arrayProperties"]["statusSummary"]
            .as_object()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: the parent must summarise its children, got {parent}"));
        let counted: i64 = summary.values().filter_map(Value::as_i64).sum();
        assert_eq!(
            counted,
            i64::from(*size),
            "case {case}: the summary must count every child, got {parent}"
        );
        // Every modelled JobStatus is a key, including the ones no child is in.
        for status in JOB_STATUS_VALUES {
            assert!(
                summary.contains_key(*status),
                "case {case}: the summary must carry the {status} count, got {parent}"
            );
        }
        assert!(
            parent["arrayProperties"]["statusSummaryLastUpdatedAt"]
                .as_i64()
                .unwrap_or_default()
                > 0,
            "case {case}: the summary reports when it last changed, got {parent}"
        );

        // The children: one per index, addressed by the parent's id and the index, each
        // reporting that index and no size.
        for child_index in 0..*size {
            let child_id = state::array_child_job_id(&parent_id, child_index);
            let child = describe_job(&fixture.batch, &child_id).await;
            assert_eq!(
                child["jobId"].as_str(),
                Some(child_id.as_str()),
                "case {case}: child {child_index} is addressed by its own id, got {child}"
            );
            assert_eq!(
                child["arrayProperties"]["index"].as_i64(),
                Some(i64::from(child_index)),
                "case {case}: child {child_index} reports its own index, got {child}"
            );
            assert_eq!(
                child["arrayProperties"]["size"],
                Value::Null,
                "case {case}: a child reports no array size, got {child}"
            );
            assert_eq!(
                child["jobQueue"].as_str(),
                Some(fixture.queue_arn.as_str()),
                "case {case}: child {child_index} is on the parent's queue, got {child}"
            );
            assert_eq!(
                child["jobName"].as_str(),
                Some(format!("array{index}").as_str()),
                "case {case}: child {child_index} carries the submitted job name, got {child}"
            );
            assert_nothing_ran(case, &child);
        }

        // `ListJobs$arrayJobId`: "lists all child jobs from within the specified array" —
        // every child, with its index, and nothing else.
        let listed = list_jobs(
            &fixture.batch,
            json!({"arrayJobId": &parent_id, "jobStatus": state::JOB_RUNNABLE}),
        )
        .await;
        let mut indexes: Vec<i64> = listed
            .iter()
            .map(|job| {
                job["arrayProperties"]["index"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("case {case}: a listed child reports its index, got {job}"))
            })
            .collect();
        indexes.sort_unstable();
        let wanted: Vec<i64> = (0..i64::from(*size)).collect();
        // Every child reaches RUNNABLE one span after the submit, so the listing is polled
        // off observed state rather than a fixed sleep.
        if indexes != wanted {
            let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
            for _ in 0..240 {
                tokio::time::sleep(Duration::from_millis((span / 10).max(5))).await;
                let listed = list_jobs(
                    &fixture.batch,
                    json!({"arrayJobId": &parent_id, "jobStatus": state::JOB_RUNNABLE}),
                )
                .await;
                indexes = listed
                    .iter()
                    .filter_map(|job| job["arrayProperties"]["index"].as_i64())
                    .collect();
                indexes.sort_unstable();
                if indexes == wanted {
                    break;
                }
            }
        }
        assert_eq!(
            indexes, wanted,
            "case {case}: listing by arrayJobId answers every child exactly once"
        );

        // An id that names no array parent selects nothing — an empty page, not a refusal.
        let none = list_jobs(
            &fixture.batch,
            json!({"arrayJobId": "00000000-0000-4000-8000-000000000000"}),
        )
        .await;
        assert!(
            none.is_empty(),
            "case {case}: an unknown array parent selects nothing, got {none:?}"
        );

        // `ListJobs$jobStatus`: "Array job parents are updated to PENDING when any child
        // job is updated to RUNNABLE and remain in PENDING status while child jobs are
        // running. To view these jobs, filter by PENDING status until all child jobs reach
        // a terminal state." So the parent is PENDING here, not RUNNABLE: it runs no
        // container, nothing places it, and it holds no position in the queue's dispatch
        // ordering.
        let parent = until_status(&fixture.batch, case, &parent_id, state::JOB_PENDING).await;
        assert_eq!(
            parent["arrayProperties"]["statusSummary"][state::JOB_RUNNABLE].as_i64(),
            Some(i64::from(*size)),
            "case {case}: a PENDING parent's children are the RUNNABLE ones, got {parent}"
        );
        let pending = list_jobs(
            &fixture.batch,
            json!({"jobQueue": &fixture.queue_arn, "jobStatus": state::JOB_PENDING}),
        )
        .await;
        let pending_ids: Vec<String> = pending
            .iter()
            .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            pending_ids,
            vec![parent_id.clone()],
            "case {case}: the PENDING filter is where a client finds the parent, got {pending:?}"
        );
        let runnable = list_jobs(
            &fixture.batch,
            json!({"jobQueue": &fixture.queue_arn, "jobStatus": state::JOB_RUNNABLE}),
        )
        .await;
        let runnable_ids: Vec<String> = runnable
            .iter()
            .map(|job| job["jobId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !runnable_ids.contains(&parent_id),
            "case {case}: the parent is not one of the queue's RUNNABLE jobs, got \
             {runnable_ids:?}"
        );
        assert_eq!(
            runnable_ids.len(),
            usize::try_from(*size).unwrap_or_default(),
            "case {case}: only the children are RUNNABLE, got {runnable_ids:?}"
        );
        // And the queue dispatches only the children: the parent occupies no position in
        // the ordering `GetJobQueueSnapshot` reports.
        let snapshot = fixture
            .batch
            .ok(
                "/v1/getjobqueuesnapshot",
                json!({"jobQueue": &fixture.queue_arn}),
            )
            .await;
        let front: Vec<String> = snapshot["frontOfQueue"]["jobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|job| job["jobArn"].as_str().unwrap_or_default().to_string())
            .collect();
        let parent_arn = parent["jobArn"].as_str().unwrap_or_default().to_string();
        assert!(
            !front.contains(&parent_arn),
            "case {case}: the parent holds no position in the queue, got {front:?}"
        );
    }
}

/// RULE: `ArrayProperties$size` is documented as "between 2 and 10,000", so a submit
/// outside that range is the modelled `ClientException` (HTTP 400) and creates no job at
/// all; a size INSIDE it is never refused for being out of range.
///
/// Rows cover both published bounds and one past each of them.
#[tokio::test]
async fn an_array_size_outside_the_documented_range_is_refused() {
    let fixture = fixture("size", Capacity::Unmanaged).await;
    let refusal = state::array_size_refusal();

    // (case, size, whether the documented range admits it)
    let rows: &[(&str, i32, bool)] = &[
        ("one below the minimum", state::MIN_ARRAY_SIZE - 1, false),
        ("zero", 0, false),
        ("negative", -1, false),
        ("the minimum", state::MIN_ARRAY_SIZE, true),
        ("the maximum", state::MAX_ARRAY_SIZE, true),
        ("one above the maximum", state::MAX_ARRAY_SIZE + 1, false),
    ];

    for (case, size, admitted) in rows {
        let name = format!("size{size}");
        let (status, body) = submit(&fixture, &name, json!({"arrayProperties": {"size": size}})).await;
        if *admitted {
            // The store this emulator keeps is bounded, so the largest documented array
            // may still be refused — but for its SIZE against that bound, never for being
            // outside the documented range.
            assert!(
                !error_message(&body).contains(&refusal),
                "case {case}: a size the member admits is not refused as out of range, got \
                 {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "ClientException",
            "case {case}: expected a ClientException, got {body}"
        );
        assert!(
            error_message(&body).contains(&refusal),
            "case {case}: expected the message to state {refusal:?}, got {body}"
        );
        // Nothing was created: no job of that name is on the queue, in any status.
        for status_filter in JOB_STATUS_VALUES {
            let listed = list_jobs(
                &fixture.batch,
                json!({
                    "jobQueue": &fixture.queue_arn,
                    "jobStatus": status_filter,
                    "filters": [{"name": "JOB_NAME", "values": [&name]}],
                }),
            )
            .await;
            assert!(
                listed.is_empty(),
                "case {case}: a refused submit stores nothing, got {listed:?}"
            );
        }
    }
}

/// What a row expects of a job submitted to a queue over each kind of capacity.
#[derive(Debug, Clone, Copy)]
struct PlacementRow {
    case: &'static str,
    capacity: Capacity,
    /// Whether the queue's environments offer the scheduler capacity to place the job on.
    placed: bool,
}

const PLACEMENT_ROWS: &[PlacementRow] = &[
    PlacementRow {
        case: "a MANAGED, ENABLED environment places the job",
        capacity: Capacity::ManagedEnabled,
        placed: true,
    },
    PlacementRow {
        case: "a MANAGED environment that is DISABLED places nothing",
        capacity: Capacity::ManagedDisabled,
        placed: false,
    },
    PlacementRow {
        case: "an UNMANAGED environment places nothing",
        capacity: Capacity::Unmanaged,
        placed: false,
    },
];

/// RULE: a `RUNNABLE` job is placed on the capacity its queue's compute environments offer
/// the scheduler, and only there — a queue with none leaves the job waiting in `RUNNABLE`
/// indefinitely. A placed job becomes terminal; because no container runtime is available
/// it ends `FAILED` saying so, and neither outcome invents a start instant, a stop instant,
/// an attempt or an exit code.
#[tokio::test]
async fn a_job_is_placed_only_where_its_queue_offers_capacity() {
    for (index, row) in PLACEMENT_ROWS.iter().enumerate() {
        let case = row.case;
        let fixture = fixture(&format!("place{index}"), row.capacity).await;
        let job_id = submit_ok(&fixture, &format!("p{index}"), json!({})).await;

        if row.placed {
            let detail = until_status(&fixture.batch, case, &job_id, state::JOB_FAILED).await;
            assert_eq!(
                detail["statusReason"].as_str(),
                Some(state::NO_CONTAINER_RUNTIME_REASON),
                "case {case}: a placed job whose container nothing ran says so, got {detail}"
            );
            assert_nothing_ran(case, &detail);
            // Terminal is FINAL: further spans of the clock leave it exactly as it is.
            let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
            tokio::time::sleep(Duration::from_millis(span * 3)).await;
            let again = describe_job(&fixture.batch, &job_id).await;
            assert_eq!(
                again, detail,
                "case {case}: a terminal job is left as it is, got {again}"
            );
            continue;
        }

        let detail = until_status(&fixture.batch, case, &job_id, state::JOB_RUNNABLE).await;
        assert_nothing_ran(case, &detail);
        // RESTING means staying: several further spans move it nowhere, and no reason is
        // invented for a job that simply has nowhere to go.
        let span = u64::try_from(state::JOB_PHASE_MILLIS).unwrap_or(500);
        tokio::time::sleep(Duration::from_millis(span * 5)).await;
        let again = describe_job(&fixture.batch, &job_id).await;
        assert_eq!(
            again["status"].as_str(),
            Some(state::JOB_RUNNABLE),
            "case {case}: the job must still be RUNNABLE five spans later, got {again}"
        );
        assert_eq!(
            again["statusReason"],
            Value::Null,
            "case {case}: a waiting job reports no statusReason, got {again}"
        );
        assert_nothing_ran(case, &again);
    }
}

/// RULE: `CEState$DISABLED` withholds placement, so the job waits — and the call that
/// re-enables the environment is what makes its placement due, with no further submit.
#[tokio::test]
async fn re_enabling_an_environment_releases_the_jobs_waiting_on_it() {
    let fixture = fixture("reenable", Capacity::ManagedDisabled).await;
    let job_id = submit_ok(&fixture, "held", json!({})).await;
    let waiting = until_status(&fixture.batch, "held", &job_id, state::JOB_RUNNABLE).await;
    assert_nothing_ran("held", &waiting);

    fixture
        .batch
        .ok(
            "/v1/updatecomputeenvironment",
            json!({"computeEnvironment": &fixture.environment, "state": state::STATE_ENABLED}),
        )
        .await;

    let placed = until_status(&fixture.batch, "re-enabled", &job_id, state::JOB_FAILED).await;
    assert_eq!(
        placed["statusReason"].as_str(),
        Some(state::NO_CONTAINER_RUNTIME_REASON),
        "case re-enabled: the placement is what ends the job, got {placed}"
    );
    assert_nothing_ran("re-enabled", &placed);
}

/// RULE: an ending a caller asks of an array PARENT is asked of every child it spawned —
/// "when you try to cancel an array parent job ... Batch attempts to cancel all child
/// jobs" — so no child is left running behind a parent that has been ended.
///
/// Rows differ along which call ends the array, both of which end a job that never
/// progressed past `RUNNABLE` as cancelled.
#[tokio::test]
async fn ending_an_array_parent_ends_every_child_it_spawned() {
    const SIZE: i32 = 3;
    const REASON: &str = "end the whole array";

    // (case, the URI the ending call is bound to)
    let rows: &[(&str, &str)] = &[
        ("CancelJob on the parent", "/v1/canceljob"),
        ("TerminateJob on the parent", "/v1/terminatejob"),
    ];

    for (index, (case, uri)) in rows.iter().enumerate() {
        // An UNMANAGED environment places nothing, so every child is still waiting in
        // RUNNABLE when the caller ends the array.
        let fixture = fixture(&format!("end{index}"), Capacity::Unmanaged).await;
        let parent_id = submit_ok(
            &fixture,
            &format!("ending{index}"),
            json!({"arrayProperties": {"size": SIZE}}),
        )
        .await;
        for child_index in 0..SIZE {
            let child_id = state::array_child_job_id(&parent_id, child_index);
            until_status(&fixture.batch, case, &child_id, state::JOB_RUNNABLE).await;
        }

        fixture
            .batch
            .ok(uri, json!({"jobId": &parent_id, "reason": REASON}))
            .await;

        for child_index in 0..SIZE {
            let child_id = state::array_child_job_id(&parent_id, child_index);
            let child = describe_job(&fixture.batch, &child_id).await;
            assert_eq!(
                child["status"].as_str(),
                Some(state::JOB_FAILED),
                "case {case}: child {child_index} is ended with its parent, got {child}"
            );
            assert_eq!(
                child["statusReason"].as_str(),
                Some(REASON),
                "case {case}: child {child_index} records the caller's reason, got {child}"
            );
            assert_nothing_ran(case, &child);
        }
        let parent = describe_job(&fixture.batch, &parent_id).await;
        assert_eq!(
            parent["status"].as_str(),
            Some(state::JOB_FAILED),
            "case {case}: the parent is ended too, got {parent}"
        );
        assert_eq!(
            parent["statusReason"].as_str(),
            Some(REASON),
            "case {case}: the parent records the caller's reason, got {parent}"
        );
        assert_eq!(
            parent["arrayProperties"]["statusSummary"][state::JOB_FAILED].as_i64(),
            Some(i64::from(SIZE)),
            "case {case}: the summary counts every ended child, got {parent}"
        );
    }
}

/// RULE: an array parent runs no container of its own — its status is what its children
/// add up to, and its `statusSummary` counts them wherever they got to.
#[tokio::test]
async fn an_array_parent_reports_what_its_children_add_up_to() {
    const SIZE: i32 = 3;
    let fixture = fixture("arrplace", Capacity::ManagedEnabled).await;
    let parent_id = submit_ok(
        &fixture,
        "shards",
        json!({"arrayProperties": {"size": SIZE}}),
    )
    .await;

    // Every child is placed and every placement ends in the modelled failure state, so the
    // parent becomes terminal too — FAILED, because its children did not succeed.
    for index in 0..SIZE {
        let child_id = state::array_child_job_id(&parent_id, index);
        let child = until_status(&fixture.batch, "a placed child", &child_id, state::JOB_FAILED).await;
        assert_eq!(
            child["statusReason"].as_str(),
            Some(state::NO_CONTAINER_RUNTIME_REASON),
            "case child {index}: a placed child whose container nothing ran says so, got {child}"
        );
        assert_nothing_ran("a placed child", &child);
    }

    let parent = until_status(&fixture.batch, "the parent", &parent_id, state::JOB_FAILED).await;
    assert_eq!(
        parent["statusReason"],
        Value::Null,
        "case the parent: what ended the parent is its children's outcomes, which the \
         summary reports, so no statusReason is authored for it, got {parent}"
    );
    assert_eq!(
        parent["arrayProperties"]["statusSummary"][state::JOB_FAILED].as_i64(),
        Some(i64::from(SIZE)),
        "case the parent: the summary counts every failed child, got {parent}"
    );
    assert_eq!(
        parent["arrayProperties"]["size"].as_i64(),
        Some(i64::from(SIZE)),
        "case the parent: the parent keeps reporting its size, got {parent}"
    );
    assert_nothing_ran("the parent", &parent);
}
