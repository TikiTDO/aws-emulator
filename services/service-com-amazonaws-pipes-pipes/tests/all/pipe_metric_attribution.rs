// SPDX-License-Identifier: CC0-1.0

//! WHICH pipe a datapoint in `AWS/EventBridge/Pipes` belongs to, with several pipes of the
//! same account running at the same time — end to end over the wire, on the harness of
//! `tests/all/pipe_metrics.rs` (IAM, CloudWatch, Amazon SQS and Step Functions beside
//! pipes on ONE engine), reused rather than copied.
//!
//! `tests/all/pipe_metrics.rs` asserts WHAT a single running pipe publishes about itself.
//! The rule here is the other half, and it cannot be seen with one pipe in the account: a
//! per-pipe series is dimensioned on the pipe's name ALONE, so every datapoint is
//! attributed to the pipe whose execution produced it — a counter reads its own pipe's
//! executions, and reads 0 rather than nothing for a kind of execution that pipe never
//! had, however many other pipes are having them at that very moment.
//!
//! That is what tells two pipes on identical wiring apart: the one whose role may not
//! reach its target accumulates its stage counter while its twin's same counter exists and
//! holds at 0, and the two targets look identical (one holds the record, the other holds
//! nothing) only because the counters say which happened.
//!
//! The rows differ along what the rule generalises over — which stage of the row's own pipe
//! fails (none, the target, the enrichment, the source) and how its role bars that stage
//! (a grant that omits the action, an explicit deny, a grant naming only the other
//! queue, a grant narrowed once the pipe is running) — so every counter of the set is moved
//! by at least one pipe of the table and quiet for at least one other.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::metrics::{
    COUNTER_UNIT, ENRICHMENT_STAGE_FAILED_METRIC, EVENT_COUNT_METRIC, EXECUTION_FAILED_METRIC,
    PIPE_COUNTERS, PIPE_NAME_DIMENSION, TARGET_STAGE_FAILED_METRIC,
};

use crate::pipe_data_plane::{
    collect_at_least, given_express_workflow, given_queue, receive_now, send, QUIET,
};
use crate::pipe_lifecycle::{call, REGION};
use crate::pipe_metrics::{
    datapoints, emulator, given_role_stating, listed_dimension_sets, series, wait_for_sum,
    withdraw_from_role,
};

/// How long a wait for something a pipe has to do — a delivery, or a counter moving — is
/// given before the row is a failure. Well above the poll interval and the state-settle
/// span, so a row fails only when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(25);

/// An enrichment that hands back what it was given, for the row whose pipe carries one its
/// role may not invoke.
const PASS_THROUGH_DEFINITION: &str = r#"{
  "StartAt": "PassThrough",
  "States": {
    "PassThrough": {
      "Type": "Pass",
      "End": true
    }
  }
}"#;

/// One row: one pipe of the account, with its own queues, its own role and its own
/// expectation of what its counters read.
struct Row {
    /// The pipe's name, which is also the value of the one dimension its series carries.
    case: &'static str,
    /// The statements of the pipe's role, written against that pipe's OWN source and
    /// target.
    policy: fn(&str, &str) -> Value,
    /// Whether this pipe carries an enrichment between its source and its target.
    enrichment: bool,
    /// What this pipe's role is left allowing once the pipe is RUNNING, for a row whose
    /// stage must fail on a call the role allowed while the pipe was being brought up.
    withdrawn_to: Option<&'static [&'static str]>,
    /// Whether this pipe's target ends up holding the record.
    delivered: bool,
    /// The counters of THIS pipe expected to read at least 1. Every other counter of this
    /// pipe has to exist and read exactly 0, whatever the other pipes publish.
    moved: &'static [&'static str],
}

/// Every Amazon SQS call a pipe with an SQS source and an SQS target makes, allowed on
/// exactly the resource each is made on.
fn exact_sqs_grant(source: &str, target: &str) -> Value {
    json!([
        {
            "Effect": "Allow",
            "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
            "Resource": source,
        },
        { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target },
    ])
}

/// The `CreatePipe` body of one row: the row's own queues, its own role, one record per
/// batch collected with no batching window, and the enrichment when the row has one.
fn pipe_body(
    source_arn: &str,
    target_arn: &str,
    role_arn: &str,
    enrichment: Option<&str>,
) -> Value {
    let mut body = json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": role_arn,
        "DesiredState": "RUNNING",
        "SourceParameters": {
            "SqsQueueParameters": { "BatchSize": 1, "MaximumBatchingWindowInSeconds": 0 },
        },
    });
    if let Some(enrichment) = enrichment {
        body["Enrichment"] = json!(enrichment);
    }
    body
}

/// What one row's pipe was given, kept so the assertions can read its target and look for
/// its own record.
struct Prepared<'row> {
    row: &'row Row,
    target_url: String,
    marker: String,
}

/// RULE: every datapoint a pipe publishes to `AWS/EventBridge/Pipes` is dimensioned on the
/// name of the pipe whose execution produced it and on nothing else, so each counter
/// accounts for its own pipe's executions only — and exists and reads 0 for a kind of
/// execution that pipe never had, while other pipes of the account are accumulating that
/// very counter.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_pipes_counters_account_for_its_own_executions_only() {
    let rows = [
        Row {
            case: "attribution-every-stage-allowed",
            policy: exact_sqs_grant,
            enrichment: false,
            withdrawn_to: None,
            delivered: true,
            moved: &[EVENT_COUNT_METRIC],
        },
        Row {
            // The twin of the row above on identical wiring, whose role simply does not
            // state the delivery: the pipe runs, the target stage is refused, and the
            // record never arrives.
            case: "attribution-target-send-not-stated",
            policy: |source, _target| {
                json!([
                    {
                        "Effect": "Allow",
                        "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage"],
                        "Resource": source,
                    },
                ])
            },
            enrichment: false,
            withdrawn_to: None,
            delivered: false,
            moved: &[
                EVENT_COUNT_METRIC,
                EXECUTION_FAILED_METRIC,
                TARGET_STAGE_FAILED_METRIC,
            ],
        },
        Row {
            case: "attribution-target-send-denied",
            policy: |_source, target| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": "sqs:SendMessage", "Resource": target },
                ])
            },
            enrichment: false,
            withdrawn_to: None,
            delivered: false,
            moved: &[
                EVENT_COUNT_METRIC,
                EXECUTION_FAILED_METRIC,
                TARGET_STAGE_FAILED_METRIC,
            ],
        },
        Row {
            // The enrichment is handed the records before the target is, so a role that
            // may not invoke it fails the pipe at THAT stage: the enrichment counter
            // moves and the target counter stays at 0.
            case: "attribution-enrichment-invoke-not-stated",
            policy: exact_sqs_grant,
            enrichment: true,
            withdrawn_to: None,
            delivered: false,
            moved: &[
                EVENT_COUNT_METRIC,
                EXECUTION_FAILED_METRIC,
                ENRICHMENT_STAGE_FAILED_METRIC,
            ],
        },
        Row {
            // The source poll is the first call the pipe makes, so a role that may not
            // make it takes no record at all: the execution failed before a stage of its
            // own, and this pipe's event counter stays at 0 while its twins' rise. The
            // call is withdrawn from the role once the pipe is RUNNING, because a pipe
            // whose role may not take its source's records when it is brought up is never
            // brought up (`tests/all/pipe_bring_up.rs`) and publishes nothing at all.
            case: "attribution-source-receive-withdrawn-while-running",
            policy: |source, target| {
                json!([
                    { "Effect": "Allow", "Action": "sqs:*", "Resource": source },
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target },
                ])
            },
            enrichment: false,
            withdrawn_to: Some(&["sqs:SendMessage"]),
            delivered: false,
            moved: &[EXECUTION_FAILED_METRIC],
        },
    ];

    // Every row's pipe is created on ONE emulator and every source then written to, so all
    // of them are running and publishing at the same time — which is what makes "this
    // counter is 0" a statement about attribution rather than about an idle account.
    let router = emulator();
    let mut prepared: Vec<Prepared<'_>> = Vec::new();
    for row in &rows {
        let (source_url, source_arn) = given_queue(&router, &format!("{}-source", row.case)).await;
        let (target_url, target_arn) = given_queue(&router, &format!("{}-target", row.case)).await;
        let role_arn = given_role_stating(
            &router,
            &format!("{}-role", row.case),
            SERVICE_PRINCIPAL,
            (row.policy)(&source_arn, &target_arn),
        )
        .await;
        let enrichment = if row.enrichment {
            Some(
                given_express_workflow(
                    &router,
                    &format!("{}-enrichment", row.case),
                    PASS_THROUGH_DEFINITION,
                )
                .await,
            )
        } else {
            None
        };
        let created = call(
            &router,
            "POST",
            &format!("/v1/pipes/{}", row.case),
            Some(pipe_body(
                &source_arn,
                &target_arn,
                &role_arn,
                enrichment.as_deref(),
            )),
            REGION,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreatePipe answered {} {}",
            row.case, created.status, created.body
        );
        // A row whose stage must fail on a call its role allowed while the pipe was being
        // brought up waits for the pipe to be RUNNING and then narrows the role, so the
        // refusal reaches a pipe that is already running rather than its creation.
        if let Some(allowed) = row.withdrawn_to {
            let reported = crate::pipe_execution_role::wait_for_state(
                &router,
                row.case,
                service_com_amazonaws_pipes_pipes::state::pipe_state::RUNNING,
                BUDGET,
            )
            .await;
            assert_eq!(
                reported,
                service_com_amazonaws_pipes_pipes::state::pipe_state::RUNNING,
                "case {:?}: expected the pipe RUNNING before its role is narrowed, it \
                 reports {reported}",
                row.case
            );
            withdraw_from_role(&router, row.case, &format!("{}-role", row.case), allowed).await;
        }
        let marker = format!("{}-probe", row.case);
        send(&router, &source_url, std::slice::from_ref(&marker)).await;
        prepared.push(Prepared {
            row,
            target_url,
            marker,
        });
    }

    // What a client sees at each target: its own record, or nothing at all. The quiet
    // window every "nothing arrived" is decided after is waited out ONCE for the table.
    let quiet_until = Instant::now() + QUIET + QUIET;
    for entry in &prepared {
        if !entry.row.delivered {
            continue;
        }
        let delivered = collect_at_least(&router, &entry.target_url, 1, BUDGET).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {:?}: expected the target to hold the one record this pipe moved, got {delivered:?}",
            entry.row.case
        );
        assert!(
            delivered[0].contains(&entry.marker),
            "case {:?}: expected the delivered record to carry this pipe's own probe {:?}, got {:?}",
            entry.row.case,
            entry.marker,
            delivered[0]
        );
    }
    let remaining = quiet_until.saturating_duration_since(Instant::now());
    if !remaining.is_zero() {
        tokio::time::sleep(remaining).await;
    }
    for entry in &prepared {
        if entry.row.delivered {
            continue;
        }
        let delivered = receive_now(&router, &entry.target_url).await;
        assert!(
            delivered.is_empty(),
            "case {:?}: expected the target of a pipe whose stage was refused to hold \
             nothing, got {delivered:?}",
            entry.row.case
        );
    }

    // First every counter the rows expect to MOVE, so that each "reads 0" below is
    // asserted at an instant when other pipes of the same account have already
    // accumulated that same counter.
    for entry in &prepared {
        for metric in entry.row.moved {
            let sum = wait_for_sum(&router, entry.row.case, metric, 1.0).await;
            assert!(
                sum >= 1.0,
                "case {:?}: expected {metric} to read at least 1 for this pipe, got {sum}",
                entry.row.case
            );
        }
    }

    // Then every counter of every pipe: the ones this pipe never had EXIST and read
    // exactly 0, and each datapoint of each counter carries the unit the namespace
    // publishes in.
    for entry in &prepared {
        for metric in PIPE_COUNTERS {
            let points = datapoints(&router, entry.row.case, metric).await;
            assert!(
                !points.is_empty(),
                "case {:?}: expected the counter {metric} of a running pipe to have a \
                 datapoint, it had none",
                entry.row.case
            );
            for point in &points {
                assert_eq!(
                    point["Unit"],
                    json!(COUNTER_UNIT),
                    "case {:?}: expected {metric} to be published in {COUNTER_UNIT}, got {point}",
                    entry.row.case
                );
            }
            if entry.row.moved.contains(metric) {
                continue;
            }
            let sum = points
                .iter()
                .filter_map(|point| point["Sum"].as_f64())
                .sum::<f64>();
            assert_eq!(
                sum, 0.0,
                "case {:?}: this pipe had no {metric} of its own, so its series reads 0 \
                 whatever the other pipes publish, got {sum}",
                entry.row.case
            );
        }
    }

    // And it HOLDS at 0 for the whole window, while the pipes that are failing go on
    // failing: a counter of one pipe never takes another pipe's execution.
    tokio::time::sleep(QUIET).await;
    for entry in &prepared {
        for metric in PIPE_COUNTERS {
            if entry.row.moved.contains(metric) {
                continue;
            }
            let (points, held) = series(&router, entry.row.case, metric).await;
            assert!(
                points > 0 && held == 0.0,
                "case {:?}: expected the counter {metric} of a pipe that never had one to \
                 hold at 0, got {held} over {points} datapoint(s)",
                entry.row.case
            );
        }
    }

    // The catalogue a dashboard builds from: one series per pipe per counter, each
    // dimensioned on the pipe's name and nothing else.
    for metric in PIPE_COUNTERS {
        let sets = listed_dimension_sets(&router, metric).await;
        for set in &sets {
            let dimensions = set.as_array().cloned().unwrap_or_default();
            assert_eq!(
                dimensions.len(),
                1,
                "metric {metric}: expected every per-pipe series to carry the one \
                 {PIPE_NAME_DIMENSION} dimension, got {set}"
            );
            assert_eq!(
                dimensions[0]["Name"],
                json!(PIPE_NAME_DIMENSION),
                "metric {metric}: expected the one dimension to be {PIPE_NAME_DIMENSION}, \
                 got {set}"
            );
        }
        for row in &rows {
            let want = json!([{ "Name": PIPE_NAME_DIMENSION, "Value": row.case }]);
            let found = sets.iter().filter(|set| **set == want).count();
            assert_eq!(
                found, 1,
                "case {:?}: expected ListMetrics to catalogue exactly one {metric} series \
                 for this pipe, found {found} among {sets:?}",
                row.case
            );
        }
    }
}
