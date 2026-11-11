// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests that `GetUpgradeStatus` only REPORTS.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness comes from [`super::domain_lifecycle`], the
//! `UpgradeDomain` / `GetUpgradeHistory` calls from [`super::upgrade_history`] and the
//! `GetUpgradeStatus` call, the progress table and its seeding from
//! [`super::upgrade_status`]; this suite adds no second copy of any of them.
//!
//! The rule under test: `GetUpgradeStatus` "returns the most recent status of the last
//! upgrade or upgrade eligibility check performed on" a domain, so it is a REPORT —
//! reading it leaves the record exactly as another reader of the same record would have
//! found it. Polling it records no run, starts no step, advances none beyond where the
//! clock has carried it, and moves the domain nowhere: the only difference a poll can
//! make is materialising what the clock already owed the record at that instant had
//! nobody called at all. So repeated polls agree with each other, never report a step
//! earlier than one already reported, and leave the domain descriptor and the upgrade
//! history as the reads taken before them.
//!
//! So the assertions span SETS of inputs rather than one call: the twelve-row progress
//! table of [`super::upgrade_status::PROGRESS`] (progress through the span × kind of
//! run), each row polled five times; four runs driven live through the wire, differing
//! in kind of run, engine line and how far the target is; and the two reads that report
//! no step at all, each polled repeatedly.

use serde_json::Value;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    UPGRADE_STATUS_SUCCEEDED, UPGRADE_STEP_PRE_UPGRADE_CHECK, UPGRADE_STEP_SNAPSHOT,
    UPGRADE_STEP_UPGRADE,
};

use super::domain_lifecycle::{
    error_code, string_member, Emulator, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};
use super::upgrade_history::{entries, expected_name, history, settled_domain, upgrade};
use super::upgrade_status::{reported, seed_run, status, PROGRESS};

/// How many times each case polls the status. More than one poll is the whole point —
/// a report a caller can poll is a report whose answer does not depend on how often it
/// was asked for.
const POLLS: usize = 5;

/// The engine version [`seed_run`] records its runs as starting from, and so the one
/// the seeded cases create their domain on.
const SEEDED_SOURCE: &str = "OpenSearch_1.0";

/// One live case: the version the domain is created on, the version the run names and
/// whether the run only checks eligibility.
struct Live {
    source: &'static str,
    target: &'static str,
    check_only: bool,
}

/// The live runs every polling assertion below is made over.
///
/// The rows differ along what the rule generalises over: the KIND of run (an upgrade,
/// which performs three steps, and an eligibility check, which performs one), the
/// engine line the run moves along (within the OpenSearch line, within the
/// Elasticsearch line and across the crossing between them) and how far the target
/// sits from the source (the next published minor, and several versions ahead). Every
/// target is one `GetCompatibleVersions` offers for its source, so each call is one the
/// service accepts.
const LIVE: [Live; 4] = [
    Live {
        source: "OpenSearch_2.11",
        target: "OpenSearch_2.17",
        check_only: false,
    },
    Live {
        source: "Elasticsearch_7.9",
        target: "Elasticsearch_7.10",
        check_only: false,
    },
    Live {
        source: "Elasticsearch_7.10",
        target: "OpenSearch_1.0",
        check_only: false,
    },
    Live {
        source: "OpenSearch_1.3",
        target: "OpenSearch_2.19",
        check_only: true,
    },
];

/// How far through its steps a report has got, as a number that only ever grows while
/// one run is being performed: twice the step's place in the steps that KIND of run
/// performs, plus one once that step has succeeded.
///
/// The step catalog is `UpgradeStep` (`PRE_UPGRADE_CHECK | SNAPSHOT | UPGRADE`), taken
/// from `state`'s own constants, and an eligibility check performs the first of them
/// alone.
fn rank(step: &str, step_status: &str, check_only: bool, case: &str) -> usize {
    let steps: &[&str] = if check_only {
        &[UPGRADE_STEP_PRE_UPGRADE_CHECK]
    } else {
        &[
            UPGRADE_STEP_PRE_UPGRADE_CHECK,
            UPGRADE_STEP_SNAPSHOT,
            UPGRADE_STEP_UPGRADE,
        ]
    };
    let place = steps
        .iter()
        .position(|known| *known == step)
        .unwrap_or_else(|| {
            panic!("case {case}: expected a step this kind of run performs, got {step:?}")
        });
    place * 2 + usize::from(step_status == UPGRADE_STATUS_SUCCEEDED)
}

/// What the upgrade history reports about a domain's runs, with the one clock-derived
/// value in it — how far through each step the run has got — left out, so two reads
/// taken a moment apart differ exactly when a RUN was recorded, started or finished.
async fn recorded_runs(emulator: &Emulator, name: &str, case: &str) -> Vec<(String, String)> {
    entries(&history(emulator, name, "").await, case)
        .iter()
        .map(|entry| {
            (
                entry["UpgradeName"].as_str().unwrap_or_default().to_owned(),
                entry["UpgradeStatus"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        })
        .collect()
}

/// The domain descriptor `DescribeDomain` answers with — the projection of the record a
/// later caller sees.
async fn descriptor(emulator: &Emulator, name: &str, case: &str) -> Value {
    let answer = emulator.describe(name).await;
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DescribeDomain to answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["DomainStatus"].clone()
}

#[tokio::test]
async fn polling_a_seeded_run_agrees_with_itself_and_changes_no_other_read() {
    let emulator = Emulator::new();

    for row in &PROGRESS {
        let case = format!(
            "{}/1000 through {}",
            row.elapsed_per_mille,
            if row.check_only {
                "a check"
            } else {
                "an upgrade"
            }
        );
        let name = format!(
            "report-{}-{}",
            row.elapsed_per_mille,
            u8::from(row.check_only)
        );
        settled_domain(&emulator, &name, SEEDED_SOURCE).await;
        seed_run(&emulator, &name, row);

        // The baseline is taken through the OTHER readers of this record, each of which
        // settles whatever the clock already owed it — so what the polls are asserted
        // to leave alone is what a caller who never polled would see.
        let before_descriptor = descriptor(&emulator, &name, &case).await;
        let before_runs = recorded_runs(&emulator, &name, &case).await;

        let mut answers = Vec::new();
        for poll in 0..POLLS {
            answers.push((poll, reported(&status(&emulator, &name).await, &case)));
        }
        let (_, first) = answers
            .first()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected at least one poll"));
        for (poll, answer) in &answers {
            assert_eq!(
                answer, &first,
                "case {case}: expected poll {poll} to agree with the first poll {first:?}, \
                 got {answer:?}"
            );
        }
        assert_eq!(
            (first.0.as_str(), first.1.as_str()),
            (row.step, row.step_status),
            "case {case}: expected the step the run is performing and its status, got {first:?}"
        );

        let after_descriptor = descriptor(&emulator, &name, &case).await;
        assert_eq!(
            after_descriptor, before_descriptor,
            "case {case}: expected {POLLS} polls to leave the domain record as it was \
             ({before_descriptor:?}), got {after_descriptor:?}"
        );
        let after_runs = recorded_runs(&emulator, &name, &case).await;
        assert_eq!(
            after_runs, before_runs,
            "case {case}: expected {POLLS} polls to record no run and finish none \
             ({before_runs:?}), got {after_runs:?}"
        );
    }
}

#[tokio::test]
async fn polling_a_live_run_never_goes_backwards_and_records_no_second_run() {
    let emulator = Emulator::new();

    for (index, live) in LIVE.iter().enumerate() {
        let case = expected_name(live.source, live.target, live.check_only);
        let name = format!("report-live-{index}");
        settled_domain(&emulator, &name, live.source).await;
        let started = upgrade(&emulator, &name, live.target, live.check_only).await;
        assert_eq!(
            started.status, 200,
            "case {case}: expected UpgradeDomain to answer HTTP 200, got {} with {:?}",
            started.status, started.body
        );

        // A run in flight is polled repeatedly: each answer names the same run and
        // reports a step no earlier than the one before it — the clock carries the run
        // forwards, a poll never does and never rewinds it either.
        let mut highest = 0;
        for poll in 0..POLLS {
            let (step, step_status, upgrade_name) =
                reported(&status(&emulator, &name).await, &case);
            let reached = rank(&step, &step_status, live.check_only, &case);
            assert!(
                reached >= highest,
                "case {case}: expected poll {poll} not to report a step before one already \
                 reported (rank {highest}), got {step:?} / {step_status:?} (rank {reached})"
            );
            highest = reached;
            assert_eq!(
                upgrade_name, case,
                "case {case}: expected every poll to name the same run, got {upgrade_name:?}"
            );
        }

        // Once the clock has made the run due the report is terminal, so polling it
        // again answers exactly the same thing.
        tokio::time::sleep(SETTLE_WAIT).await;
        let landed = reported(&status(&emulator, &name).await, &case);
        let again = reported(&status(&emulator, &name).await, &case);
        assert_eq!(
            again, landed,
            "case {case}: expected a finished run's report to be stable, got {again:?} \
             after {landed:?}"
        );
        let last_step = if live.check_only {
            UPGRADE_STEP_PRE_UPGRADE_CHECK
        } else {
            UPGRADE_STEP_UPGRADE
        };
        assert_eq!(
            (landed.0.as_str(), landed.1.as_str()),
            (last_step, UPGRADE_STATUS_SUCCEEDED),
            "case {case}: expected the last step of the run, succeeded, got {landed:?}"
        );

        // However many times it was polled, the domain holds the ONE run that was
        // started on it.
        let runs = recorded_runs(&emulator, &name, &case).await;
        assert_eq!(
            runs,
            vec![(case.clone(), UPGRADE_STATUS_SUCCEEDED.to_string())],
            "case {case}: expected polling to record no run of its own, got {runs:?}"
        );

        // An upgrade moves the domain onto the version it named — the landing the clock
        // owed it — and a check moves it nowhere, whoever polled in between.
        let described = descriptor(&emulator, &name, &case).await;
        let expected_version = if live.check_only {
            live.source
        } else {
            live.target
        };
        assert_eq!(
            string_member(&described, "EngineVersion", &case),
            expected_version,
            "case {case}: expected the domain on {expected_version}, got {described:?}"
        );
    }
}

#[tokio::test]
async fn repeated_polls_report_no_step_where_there_is_none_and_mint_nothing() {
    let emulator = Emulator::new();

    // A domain nothing has upgraded or checked: every poll reports no step, and no poll
    // starts a run to report on.
    let quiet = "report-never-upgraded";
    settled_domain(&emulator, quiet, "OpenSearch_2.11").await;
    for poll in 0..POLLS {
        let answer = status(&emulator, quiet).await;
        assert_eq!(
            answer.status, 200,
            "case never upgraded poll {poll}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        for member in ["UpgradeStep", "StepStatus", "UpgradeName"] {
            assert!(
                answer.body[member].is_null(),
                "case never upgraded poll {poll}: expected no {member}, got {:?}",
                answer.body
            );
        }
    }
    let runs = recorded_runs(&emulator, quiet, "never upgraded").await;
    assert!(
        runs.is_empty(),
        "case never upgraded: expected polling to record no run, got {runs:?}"
    );

    // A name the account holds no domain under: every poll refuses alike, and no poll
    // mints the domain it could not find.
    for name in ["report-absent", "rep", "report-absent-with-a-long-na"] {
        for poll in 0..POLLS {
            let answer = status(&emulator, name).await;
            assert_eq!(
                answer.status, RECORDED_NOT_FOUND_STATUS,
                "case absent {name:?} poll {poll}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got \
                 {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case absent {name:?} poll {poll}: expected the missing-resource refusal, \
                 got {:?}",
                answer.body
            );
            assert_eq!(
                string_member(&answer.body, "message", "absent"),
                format!("Domain not found: {name}"),
                "case absent {name:?} poll {poll}: expected the refusal to name the domain, \
                 got {:?}",
                answer.body
            );
        }
        let described = emulator.describe(name).await;
        assert_eq!(
            (described.status, error_code(&described)),
            (
                RECORDED_NOT_FOUND_STATUS,
                "ResourceNotFoundException".to_string(),
            ),
            "case absent {name:?}: expected the polls to mint no domain, got {} with {:?}",
            described.status,
            described.body
        );
    }
}
