// SPDX-License-Identifier: CC0-1.0

//! `FilterCriteria` RE-INSTALLED on a pipe that is already RUNNING — end to end over the
//! wire, with IAM, CloudWatch, Amazon SQS and Step Functions mounted beside pipes on ONE
//! engine (the harness of `tests/all/pipe_metrics.rs` and `tests/all/pipe_data_plane.rs`,
//! reused rather than copied).
//!
//! The RULE asserted over the table of legs below: the `SourceParameters` an `UpdatePipe`
//! carries REPLACE the stored ones atomically, so the patterns a pipe admits records by are
//! exactly the ones its record holds when the record is DECIDED — a tightened pattern starts
//! withholding what only the superseded one admitted, a restored pattern re-admits it, and
//! an empty `Filters` array leaves the pipe admitting everything. That holds for the records
//! the pipe has already taken as well: a record checked out under the criteria an update
//! replaced is decided by the criteria the pipe now carries, so the source is drained either
//! way and nothing of it is parked until the queue's whole `VisibilityTimeout` lapses.
//!
//! The legs differ along what the rule generalises over: which direction the patterns move
//! (tightened, restored, widened, dropped altogether), how many patterns the replacement
//! carries, the batching parameters they are installed beside, whether the records are sent
//! after the install has settled or while it still is, whether a record is already checked
//! out of the source when the install lands, and the shape of the records each leg sends
//! (admitted by both pattern sets, by one of them only, and by neither).
//!
//! The second test here is the other half of the same reading: what a RUNNING pipe does is
//! the record's to say, so an update that leaves it a configuration this data plane serves
//! has it polling under that configuration — and until then its source is left alone.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::pipe_state::RUNNING;
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    collect_at_least, collect_during, counted, given_queue, sqs, wait_for_counted,
    VISIBLE_ATTRIBUTE, QUIET,
};
use crate::pipe_execution_role::{set_visibility, wait_for_state, IN_FLIGHT_ATTRIBUTE};
use crate::pipe_lifecycle::{call, describe, REGION};
use crate::pipe_metrics::{emulator, given_role_stating};

/// How long work the pipe owes is given before a leg fails. Far above the data plane's poll
/// interval and the span a pipe takes to settle, and WELL BELOW the source's
/// `VisibilityTimeout` below, so a record the pipe parked until that timeout lapsed fails
/// the leg instead of passing it slowly.
const BUDGET: Duration = Duration::from_secs(20);

/// The `VisibilityTimeout` the source queue carries: the span a record the pipe took stays
/// invisible to every other consumer. A minute — the value a client that wants a pipe's
/// retries paced at all configures — so that a record the emulator parks for the whole
/// checkout cannot be mistaken for one it handled.
const SOURCE_VISIBILITY: &str = "60";

/// The attributes that together say how many records the source still HOLDS: the ones
/// waiting on it ([`VISIBLE_ATTRIBUTE`]) plus the ones a consumer has taken and not deleted
/// ([`IN_FLIGHT_ATTRIBUTE`]).
const HELD_ATTRIBUTES: [&str; 2] = [VISIBLE_ATTRIBUTE, IN_FLIGHT_ATTRIBUTE];

/// `MaximumBatchingWindowInSeconds` of the leg whose successor catches a record in flight:
/// long enough for this test to see the poller holding a record that has not filled its
/// batch and to install the next criteria while it does, and far below the source's
/// `VisibilityTimeout`.
const IN_FLIGHT_WINDOW_SECONDS: i64 = 5;

/// The patterns the pipe is created with: the two record kinds the first leg admits.
const ORIGINAL: &[&str] = &[r#"{"body": {"kind": ["alpha", "beta"]}}"#];

/// The patterns an update puts in their place: the twin kind both pattern sets admit, and
/// one only this set admits.
const TIGHTENED: &[&str] = &[r#"{"body": {"kind": ["beta", "gamma"]}}"#];

/// A replacement spanning TWO patterns, which are ORed: the twin again, plus a kind neither
/// of the other sets names.
const WIDENED: &[&str] = &[
    r#"{"body": {"kind": ["beta"]}}"#,
    r#"{"body": {"kind": ["delta"]}}"#,
];

/// "To remove a filter, specify a FilterCriteria object with an empty array of Filter
/// objects" (`UpdatePipeSourceParameters$FilterCriteria`), so a pipe left with this admits
/// every record it polls.
const UNFILTERED: &[&str] = &[];

/// One record a leg sends to the source.
struct Sent {
    /// The `kind` the record's JSON body carries, which is what the patterns read.
    kind: &'static str,
    /// Whether the criteria in force must admit it, so it reaches the target.
    admitted: bool,
}

/// One leg of the re-install table: the criteria in force while it runs and what they must
/// decide.
struct Leg {
    case: &'static str,
    /// The patterns `FilterCriteria.Filters` carries while this leg runs, in order.
    patterns: &'static [&'static str],
    /// `BatchSize` and `MaximumBatchingWindowInSeconds` the leg installs beside them: how
    /// many records one poll takes and how long a poll that has not filled its batch keeps
    /// collecting, which is what lets a record be IN FLIGHT when the next leg's update
    /// lands.
    batch_size: i64,
    window: Option<i64>,
    /// Whether this leg sends its records while the install is still settling — the pipe
    /// is `UPDATING` and polls nothing — so the records are already waiting on the source
    /// when the criteria this leg installed come into force.
    sends_while_settling: bool,
    /// A record handed over BEFORE this leg installs its criteria and confirmed checked out
    /// of the source, so the install lands while the pipe holds it: the `kind` it carries,
    /// and whether the criteria this leg installs admit it.
    in_flight: Option<Sent>,
    /// What the leg sends once its criteria are in force.
    messages: &'static [Sent],
}

/// RULE: an `UpdatePipe` that carries `SourceParameters` REPLACES the stored
/// `FilterCriteria` atomically, and a RUNNING pipe admits exactly the records its CURRENT
/// patterns match — so each re-install decides every record the pipe has not decided yet,
/// including one it is already holding, the patterns it replaced decide none, and the
/// records the pipe decided about leave the source either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn re_installed_filter_criteria_decide_every_record_polled_after_them() {
    let legs = [
        Leg {
            case: "the criteria the pipe was created with",
            patterns: ORIGINAL,
            batch_size: 1,
            window: None,
            sends_while_settling: false,
            in_flight: None,
            messages: &[
                Sent {
                    kind: "alpha",
                    admitted: true,
                },
                Sent {
                    kind: "beta",
                    admitted: true,
                },
                Sent {
                    kind: "gamma",
                    admitted: false,
                },
            ],
        },
        Leg {
            case: "the criteria tightened onto another kind",
            patterns: TIGHTENED,
            // A batch of two with a window to fill it in: the same criteria, polled the
            // other way round, and what leaves the next leg's record in flight.
            batch_size: 2,
            window: Some(IN_FLIGHT_WINDOW_SECONDS),
            sends_while_settling: false,
            in_flight: None,
            messages: &[
                // Admitted only by the criteria this leg installed: its arrival is what
                // shows the running pipe polling under them.
                Sent {
                    kind: "gamma",
                    admitted: true,
                },
                // The twin, admitted by both pattern sets.
                Sent {
                    kind: "beta",
                    admitted: true,
                },
                // What only the superseded criteria admitted: a replacement is not an
                // addition, so it is withheld now.
                Sent {
                    kind: "alpha",
                    admitted: false,
                },
            ],
        },
        Leg {
            case: "the criteria restored while a record is in flight",
            patterns: ORIGINAL,
            batch_size: 1,
            window: None,
            sends_while_settling: false,
            // Checked out under the criteria this leg replaces, which admitted it; the
            // criteria it installs do not, so the pipe withholds it and takes it off the
            // source rather than leaving it there until its checkout lapses.
            in_flight: Some(Sent {
                kind: "gamma",
                admitted: false,
            }),
            messages: &[
                Sent {
                    kind: "alpha",
                    admitted: true,
                },
                Sent {
                    kind: "beta",
                    admitted: true,
                },
                Sent {
                    kind: "gamma",
                    admitted: false,
                },
            ],
        },
        Leg {
            case: "the criteria replaced by two ored patterns, the records sent while it \
                   settles",
            patterns: WIDENED,
            batch_size: 1,
            window: None,
            sends_while_settling: true,
            in_flight: None,
            messages: &[
                Sent {
                    kind: "beta",
                    admitted: true,
                },
                Sent {
                    kind: "delta",
                    admitted: true,
                },
                Sent {
                    kind: "alpha",
                    admitted: false,
                },
                Sent {
                    kind: "gamma",
                    admitted: false,
                },
            ],
        },
        Leg {
            case: "the filter removed by an empty filters array",
            patterns: UNFILTERED,
            batch_size: 1,
            window: None,
            sends_while_settling: false,
            in_flight: None,
            messages: &[
                Sent {
                    kind: "alpha",
                    admitted: true,
                },
                Sent {
                    kind: "delta",
                    admitted: true,
                },
                Sent {
                    kind: "omega",
                    admitted: true,
                },
            ],
        },
    ];

    let name = "filter-criteria-re-installed";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{name}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{name}-tgt")).await;
    set_visibility(&router, "the re-install table", &source_url, SOURCE_VISIBILITY).await;
    let role = given_role_stating(
        &router,
        &format!("{name}-role"),
        SERVICE_PRINCIPAL,
        json!([
            {
                "Effect": "Allow",
                "Action": [
                    "sqs:ReceiveMessage",
                    "sqs:DeleteMessage",
                    "sqs:GetQueueAttributes",
                ],
                "Resource": source_arn,
            },
            { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target_arn },
        ]),
    )
    .await;

    for (index, leg) in legs.iter().enumerate() {
        let case = leg.case;
        // A record handed over BEFORE the install and confirmed CHECKED OUT of the source,
        // so the install lands while the pipe holds it: the criteria this leg installs are
        // the ones it is decided by, whichever way they decide it.
        let in_flight_marker = format!("in-flight-{index}");
        if let Some(in_flight) = &leg.in_flight {
            send(
                &router,
                &source_url,
                case,
                &in_flight_marker,
                in_flight.kind,
            )
            .await;
            let taken = wait_for_counted(&router, &source_url, &[IN_FLIGHT_ATTRIBUTE], 1, BUDGET)
                .await;
            assert_eq!(
                taken, 1,
                "case {case:?}: expected the poller to hold the record sent before the \
                 install, in flight {taken}"
            );
        }

        let installed = if index == 0 {
            call(
                &router,
                "POST",
                &format!("/v1/pipes/{name}"),
                Some(json!({
                    "Source": source_arn,
                    "Target": target_arn,
                    "RoleArn": role,
                    "DesiredState": "RUNNING",
                    "SourceParameters": source_parameters(leg),
                })),
                REGION,
            )
            .await
        } else {
            call(
                &router,
                "PUT",
                &format!("/v1/pipes/{name}"),
                Some(json!({
                    "RoleArn": role,
                    "SourceParameters": source_parameters(leg),
                })),
                REGION,
            )
            .await
        };
        assert_eq!(
            installed.status, 200,
            "case {case:?}: installing the criteria answered {} {}",
            installed.status, installed.body
        );

        // A leg that sends while the install SETTLES hands its records over before the
        // pipe polls again: they wait on the source and are decided by the criteria this
        // leg installed, not by the ones it replaced.
        if leg.sends_while_settling {
            send_leg(&router, &source_url, case, index, leg.messages).await;
        }

        // The client's own view of the installation: the pipe is RUNNING again and the
        // criteria it reports are exactly the ones just sent — a replacement, so the
        // patterns it held before are gone rather than kept beside the new ones.
        let state = wait_for_state(&router, name, RUNNING, BUDGET).await;
        assert_eq!(
            state, RUNNING,
            "case {case:?}: expected the pipe running under the criteria just installed, \
             got {state:?}"
        );
        let reported = reported_patterns(&router, name).await;
        assert_eq!(
            reported,
            leg.patterns.to_vec(),
            "case {case:?}: expected DescribePipe to report exactly the patterns installed \
             {:?}, got {reported:?}",
            leg.patterns
        );

        if !leg.sends_while_settling {
            send_leg(&router, &source_url, case, index, leg.messages).await;
        }
        let mut expected: BTreeSet<String> = leg
            .messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.admitted)
            .map(|(position, _)| marker(index, position))
            .collect();
        if leg.in_flight.as_ref().is_some_and(|sent| sent.admitted) {
            expected.insert(in_flight_marker.clone());
        }

        // Every record this leg's criteria admit must arrive, and the ones they withhold
        // must not: the quiet window is what makes an absence an answer rather than a
        // race, and the source draining is what says the withheld ones were decided
        // rather than left.
        let delivered = collect_markers(&router, &target_url, expected.len(), BUDGET).await;
        assert_eq!(
            delivered, expected,
            "case {case:?}: expected the criteria in force to admit exactly {expected:?}, \
             got {delivered:?}"
        );
        let late = collect_during(&router, &target_url, QUIET)
            .await
            .iter()
            .map(|payload| marker_of(case, payload))
            .collect::<BTreeSet<String>>();
        assert!(
            late.is_empty(),
            "case {case:?}: expected nothing further at the target, got {late:?}"
        );
        let left = wait_for_counted(&router, &source_url, &HELD_ATTRIBUTES, 0, BUDGET).await;
        assert_eq!(
            left, 0,
            "case {case:?}: expected the source drained of every record the pipe decided \
             about, it still holds {left}"
        );
    }
}

/// The target kinds this one asserts over: a resource no stage of this emulator's data
/// plane delivers to, and the Amazon SQS queue an update puts in its place. An SNS topic is
/// a `Target` the model admits (`Target` is an `Arn`) and this data plane serves no stage
/// for, so a pipe carrying one polls nothing while it does.
const UNSERVED_TARGET: &str = "arn:aws:sns:us-west-2:000000000000:re-install-unserved";

/// RULE: what a RUNNING pipe does is read from its record, so an `UpdatePipe` that leaves it
/// a configuration this data plane serves has it polling under that configuration — the
/// pipe's poller is the one the record decides, whether it had stopped for want of a
/// configuration to run or was already running when the record changed. Until then nothing
/// is consumed: a record this emulator could deliver nowhere stays on the source.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_update_that_leaves_a_servable_configuration_has_the_pipe_polling_again() {
    let name = "re-install-converges";
    let router = emulator();
    let (source_url, source_arn) = given_queue(&router, &format!("{name}-src")).await;
    let (target_url, target_arn) = given_queue(&router, &format!("{name}-tgt")).await;
    set_visibility(
        &router,
        "a target no stage of this data plane delivers to",
        &source_url,
        SOURCE_VISIBILITY,
    )
    .await;
    let role = given_role_stating(
        &router,
        &format!("{name}-role"),
        SERVICE_PRINCIPAL,
        json!([
            {
                "Effect": "Allow",
                "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
                "Resource": source_arn,
            },
            { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": "*" },
        ]),
    )
    .await;

    let case = "a target no stage of this data plane delivers to";
    let created = call(
        &router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(json!({
            "Source": source_arn,
            "Target": UNSERVED_TARGET,
            "RoleArn": role,
            "DesiredState": "RUNNING",
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: CreatePipe answered {} {}",
        created.status, created.body
    );
    let state = wait_for_state(&router, name, RUNNING, BUDGET).await;
    assert_eq!(
        state, RUNNING,
        "case {case:?}: expected the pipe to come up, got {state:?}"
    );

    let marker = "unserved-backlog";
    send(&router, &source_url, case, marker, "alpha").await;
    let quiet = collect_during(&router, &target_url, QUIET).await;
    assert!(
        quiet.is_empty(),
        "case {case:?}: expected nothing delivered while the pipe's target is one this \
         data plane serves no stage for, got {quiet:?}"
    );
    let waiting = counted(&router, &source_url, &HELD_ATTRIBUTES).await;
    assert_eq!(
        waiting, 1,
        "case {case:?}: expected the record left on the source, it holds {waiting}"
    );

    let case = "the target replaced by one it does";
    let updated = call(
        &router,
        "PUT",
        &format!("/v1/pipes/{name}"),
        Some(json!({
            "RoleArn": role,
            "Target": target_arn,
            "SourceParameters": { "SqsQueueParameters": { "BatchSize": 1 } },
        })),
        REGION,
    )
    .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: UpdatePipe answered {} {}",
        updated.status, updated.body
    );
    let state = wait_for_state(&router, name, RUNNING, BUDGET).await;
    assert_eq!(
        state, RUNNING,
        "case {case:?}: expected the pipe running under the new target, got {state:?}"
    );
    let delivered = collect_markers(&router, &target_url, 1, BUDGET).await;
    assert_eq!(
        delivered,
        BTreeSet::from([marker.to_string()]),
        "case {case:?}: expected the backlog delivered to the target the update named, got \
         {delivered:?}"
    );
    let left = wait_for_counted(&router, &source_url, &HELD_ATTRIBUTES, 0, BUDGET).await;
    assert_eq!(
        left, 0,
        "case {case:?}: expected the delivered record to leave the source, it holds {left}"
    );
}

/// The `SourceParameters` a create or an update installs: the batching parameters the leg
/// polls under and the patterns it filters by.
fn source_parameters(leg: &Leg) -> Value {
    let filters: Vec<Value> = leg
        .patterns
        .iter()
        .map(|pattern| json!({ "Pattern": pattern }))
        .collect();
    let mut queue_parameters = serde_json::Map::new();
    queue_parameters.insert("BatchSize".to_string(), json!(leg.batch_size));
    if let Some(window) = leg.window {
        queue_parameters.insert(
            "MaximumBatchingWindowInSeconds".to_string(),
            json!(window),
        );
    }
    json!({
        "SqsQueueParameters": Value::Object(queue_parameters),
        "FilterCriteria": { "Filters": filters },
    })
}

/// The patterns `DescribePipe` reports under `SourceParameters.FilterCriteria.Filters`, in
/// the order it reports them.
async fn reported_patterns(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let described = describe(router, name).await;
    assert_eq!(
        described.status, 200,
        "DescribePipe answered {} {}",
        described.status, described.body
    );
    described.body["SourceParameters"]["FilterCriteria"]["Filters"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|filter| filter["Pattern"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The marker the record at `position` of the leg `index` carries: distinct per leg and per
/// record, so a delivery is attributed to the leg that sent it.
fn marker(index: usize, position: usize) -> String {
    format!("leg-{index}-{position}")
}

/// Send every record one leg sends, in the order the leg lists them.
async fn send_leg(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    index: usize,
    messages: &[Sent],
) {
    for (position, message) in messages.iter().enumerate() {
        send(router, url, case, &marker(index, position), message.kind).await;
    }
}

/// Send one record carrying `marker` and `kind` to the queue at `url`.
async fn send(router: &MultiServiceRouter, url: &str, case: &str, marker: &str, kind: &str) {
    let body = json!({ "kind": kind, "marker": marker }).to_string();
    let (status, answer) = sqs(
        router,
        "SendMessage",
        &json!({ "QueueUrl": url, "MessageBody": body }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: SendMessage of {body:?} answered {status} {answer}"
    );
}

/// The markers of everything that reaches `url`, collected until there are `want` of them
/// or `budget` has elapsed.
async fn collect_markers(
    router: &MultiServiceRouter,
    url: &str,
    want: usize,
    budget: Duration,
) -> BTreeSet<String> {
    collect_at_least(router, url, want, budget)
        .await
        .iter()
        .map(|payload| marker_of("collect", payload))
        .collect()
}

/// The `marker` of the record one delivery carries, read out of the record envelope the
/// target received — which is what says the target was handed the record and not something
/// this test wrote.
fn marker_of(case: &str, payload: &str) -> String {
    let envelope: Value = serde_json::from_str(payload).unwrap_or_else(|error| {
        panic!("case {case:?}: a delivered body is not JSON ({error}): {payload:?}")
    });
    let body = envelope["body"].as_str().unwrap_or_else(|| {
        panic!("case {case:?}: expected the delivered record to carry its body, got {envelope}")
    });
    let parsed: Value = serde_json::from_str(body).unwrap_or_else(|error| {
        panic!("case {case:?}: a delivered record body is not JSON ({error}): {body:?}")
    });
    parsed["marker"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no marker in the delivered record {body:?}"))
        .to_string()
}
