// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetUpgradeStatus`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the `UpgradeDomain` / `CreateDomain` calls come from
//! [`super::domain_lifecycle`]; this suite adds no second copy of them.
//!
//! The rule under test: `GetUpgradeStatus` reports the most recent run a domain has
//! had — the step that run is performing at the instant of the call and that step's
//! status, plus the description of the run — where the step is the last one the run has
//! started, derived from how far through its span the clock has carried it, and the
//! steps are the ones that KIND of run performs (all three for an upgrade, the
//! pre-upgrade check alone for an eligibility check). A domain no run has ever been
//! performed on has no status to report, and a name the account holds no domain under
//! has none either.
//!
//! So the assertions span SETS of inputs rather than one call: a table of twelve
//! (progress through the span × kind of run) rows covering the start of a run, each
//! step of an upgrade, the boundary where the span falls due and well past it; both
//! kinds of run driven live through the wire; and the two reads that report no step.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    UpgradeRun, UPGRADE_STATUS_IN_PROGRESS, UPGRADE_STATUS_SUCCEEDED,
    UPGRADE_STEP_PRE_UPGRADE_CHECK, UPGRADE_STEP_SNAPSHOT, UPGRADE_STEP_UPGRADE,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The span the seeded runs below are given: long enough that reading a fraction of it
/// back through the wire lands well inside the step that fraction falls in, whatever
/// the few milliseconds a dispatch takes.
pub(crate) const SEEDED_SPAN: Duration = Duration::from_secs(3600);

/// `GetUpgradeStatus`: `GET /2021-01-01/opensearch/upgradeDomain/{DomainName}/status`.
pub(crate) async fn status(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/upgradeDomain/{name}/status"),
            None,
        )
        .await
}

/// `UpgradeDomain`: `POST /2021-01-01/opensearch/upgradeDomain`.
async fn upgrade(emulator: &Emulator, name: &str, target: &str, check_only: bool) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/upgradeDomain"),
            Some(json!({
                "DomainName": name,
                "TargetVersion": target,
                "PerformCheckOnly": check_only,
            })),
        )
        .await
}

/// `GetUpgradeHistory`: `GET /2021-01-01/opensearch/upgradeDomain/{DomainName}/history`.
async fn history(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/upgradeDomain/{name}/history"),
            None,
        )
        .await
}

/// Create a domain on `engine_version` and let the provisioning span fall due, so the
/// record is settled before anything upgrades it.
async fn settled_domain(emulator: &Emulator, name: &str, engine_version: &str) {
    let created = emulator
        .create(json!({ "DomainName": name, "EngineVersion": engine_version }))
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name:?}, {engine_version:?}): expected HTTP 200, got {} with {:?}",
        created.status, created.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
}

/// One row of the progress table: how far through its span the run has got, whether it
/// only checks eligibility, and the step and step status the read must report.
pub(crate) struct Row {
    /// How much of [`SEEDED_SPAN`] has elapsed when the status is read, as a
    /// thousandth, so a row can sit anywhere inside a step rather than only at a
    /// boundary.
    pub(crate) elapsed_per_mille: u32,
    pub(crate) check_only: bool,
    pub(crate) step: &'static str,
    pub(crate) step_status: &'static str,
}

/// The progress table every seeded case is asserted over.
///
/// The rows differ along what the rule generalises over: HOW FAR through its span the
/// run has got (its very start, inside each of the three steps an upgrade performs,
/// the instant the span falls due, and long past it) and the KIND of run (an upgrade,
/// which performs three steps, and an eligibility check, which performs one — so the
/// same fraction of the span reports a different step for each).
pub(crate) const PROGRESS: [Row; 12] = [
    Row {
        elapsed_per_mille: 0,
        check_only: false,
        step: UPGRADE_STEP_PRE_UPGRADE_CHECK,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 150,
        check_only: false,
        step: UPGRADE_STEP_PRE_UPGRADE_CHECK,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 500,
        check_only: false,
        step: UPGRADE_STEP_SNAPSHOT,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 800,
        check_only: false,
        step: UPGRADE_STEP_UPGRADE,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 999,
        check_only: false,
        step: UPGRADE_STEP_UPGRADE,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 1000,
        check_only: false,
        step: UPGRADE_STEP_UPGRADE,
        step_status: UPGRADE_STATUS_SUCCEEDED,
    },
    Row {
        elapsed_per_mille: 4000,
        check_only: false,
        step: UPGRADE_STEP_UPGRADE,
        step_status: UPGRADE_STATUS_SUCCEEDED,
    },
    Row {
        elapsed_per_mille: 0,
        check_only: true,
        step: UPGRADE_STEP_PRE_UPGRADE_CHECK,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 500,
        check_only: true,
        step: UPGRADE_STEP_PRE_UPGRADE_CHECK,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 999,
        check_only: true,
        step: UPGRADE_STEP_PRE_UPGRADE_CHECK,
        step_status: UPGRADE_STATUS_IN_PROGRESS,
    },
    Row {
        elapsed_per_mille: 1000,
        check_only: true,
        step: UPGRADE_STEP_PRE_UPGRADE_CHECK,
        step_status: UPGRADE_STATUS_SUCCEEDED,
    },
    Row {
        elapsed_per_mille: 4000,
        check_only: true,
        step: UPGRADE_STEP_PRE_UPGRADE_CHECK,
        step_status: UPGRADE_STATUS_SUCCEEDED,
    },
];

/// Put one run onto an already created domain, started so that `elapsed_per_mille` of
/// [`SEEDED_SPAN`] has passed by now.
///
/// A run that takes the span the emulator's clock owes it (half a second) can only be
/// observed at its very start or once it is over, so the rows that sit INSIDE a step
/// seed the record `UpgradeDomain` writes with a span long enough to read a fraction of
/// — and everything the rule is asserted on is read back through the wire.
pub(crate) fn seed_run(emulator: &Emulator, name: &str, row: &Row) {
    let elapsed = SEEDED_SPAN * row.elapsed_per_mille / 1000;
    let started_at = SystemTime::now() - elapsed;
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case seed({name:?}): the domain must have been created"));
    domain.upgrades.push_back(UpgradeRun {
        upgrade_id: format!("seeded-{}-{}", row.elapsed_per_mille, row.check_only),
        source_version: "OpenSearch_1.0".to_string(),
        target_version: "OpenSearch_1.3".to_string(),
        perform_check_only: row.check_only,
        advanced_options: BTreeMap::new(),
        started_at,
        completes_at: started_at + SEEDED_SPAN,
        // A run whose span has already passed has landed on the domain record; a check
        // lands nothing at all.
        applied: row.check_only || elapsed >= SEEDED_SPAN,
    });
}

/// The status of an accepted read, as `(UpgradeStep, StepStatus, UpgradeName)`.
pub(crate) fn reported(answer: &Answer, case: &str) -> (String, String, String) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    (
        string_member(&answer.body, "UpgradeStep", case),
        string_member(&answer.body, "StepStatus", case),
        string_member(&answer.body, "UpgradeName", case),
    )
}

#[tokio::test]
async fn the_status_reports_the_step_the_latest_run_is_on() {
    let emulator = Emulator::new();

    for row in &PROGRESS {
        let case = format!(
            "{}/1000 through a {}",
            row.elapsed_per_mille,
            if row.check_only {
                "check"
            } else {
                "an upgrade"
            }
        );
        // Each row gets its own domain, so the run it seeds is that domain's latest.
        let name = format!(
            "status-{}-{}",
            row.elapsed_per_mille,
            u8::from(row.check_only)
        );
        settled_domain(&emulator, &name, "OpenSearch_1.0").await;
        seed_run(&emulator, &name, row);

        let (step, step_status, upgrade_name) = reported(&status(&emulator, &name).await, &case);
        assert_eq!(
            step, row.step,
            "case {case}: expected the step the run is performing, got {step:?}"
        );
        assert_eq!(
            step_status, row.step_status,
            "case {case}: expected step status {:?}, got {step_status:?}",
            row.step_status
        );
        let expected_name = if row.check_only {
            "Upgrade eligibility check from OpenSearch_1.0 to OpenSearch_1.3"
        } else {
            "Upgrade from OpenSearch_1.0 to OpenSearch_1.3"
        };
        assert_eq!(
            upgrade_name, expected_name,
            "case {case}: expected the run described as {expected_name:?}, got {upgrade_name:?}"
        );

        // The status and the history report the SAME run: one record, two views of it.
        let newest = history(&emulator, &name).await.body["UpgradeHistories"][0].clone();
        assert_eq!(
            string_member(&newest, "UpgradeName", &case),
            upgrade_name,
            "case {case}: expected the status and the history to name one run, got {newest:?}"
        );
        let listed = newest["StepsList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a StepsList, got {newest:?}"))
            .last()
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            string_member(&listed, "UpgradeStep", &case),
            step,
            "case {case}: expected the status to report the last step the history lists, \
             got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_run_driven_through_the_wire_is_polled_to_its_terminal_step() {
    let emulator = Emulator::new();

    // An upgrade: it is on its first step the instant it is started, and on its last,
    // succeeded, once the clock has made it due — and the domain then runs the version
    // the upgrade named (the round trip through `DescribeDomain`).
    let upgraded = "status-live-upgrade";
    settled_domain(&emulator, upgraded, "OpenSearch_2.11").await;
    let started = upgrade(&emulator, upgraded, "OpenSearch_2.17", false).await;
    assert_eq!(
        started.status, 200,
        "case live upgrade start: expected HTTP 200, got {} with {:?}",
        started.status, started.body
    );

    let (step, step_status, upgrade_name) = reported(&status(&emulator, upgraded).await, "in flight");
    assert_eq!(
        step, UPGRADE_STEP_PRE_UPGRADE_CHECK,
        "case in flight: a run just started is on its first step, got {step:?}"
    );
    assert_eq!(
        step_status, UPGRADE_STATUS_IN_PROGRESS,
        "case in flight: expected the step in progress, got {step_status:?}"
    );
    assert_eq!(
        upgrade_name, "Upgrade from OpenSearch_2.11 to OpenSearch_2.17",
        "case in flight: expected the run described by the versions it goes between, \
         got {upgrade_name:?}"
    );

    tokio::time::sleep(SETTLE_WAIT).await;
    let (step, step_status, _) = reported(&status(&emulator, upgraded).await, "landed");
    assert_eq!(
        step, UPGRADE_STEP_UPGRADE,
        "case landed: a finished upgrade is on its last step, got {step:?}"
    );
    assert_eq!(
        step_status, UPGRADE_STATUS_SUCCEEDED,
        "case landed: expected the last step succeeded, got {step_status:?}"
    );
    let described = emulator.describe(upgraded).await;
    assert_eq!(
        string_member(&described.body["DomainStatus"], "EngineVersion", "landed"),
        "OpenSearch_2.17",
        "case landed: expected the domain on the version the upgrade named, got {:?}",
        described.body["DomainStatus"]
    );

    // An eligibility check "does not actually perform the upgrade", so it is reported
    // on the pre-upgrade check alone and the domain stays where it was.
    let checked = "status-live-check";
    settled_domain(&emulator, checked, "OpenSearch_2.11").await;
    let started = upgrade(&emulator, checked, "OpenSearch_2.19", true).await;
    assert_eq!(
        started.status, 200,
        "case live check start: expected HTTP 200, got {} with {:?}",
        started.status, started.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;
    let (step, step_status, upgrade_name) = reported(&status(&emulator, checked).await, "checked");
    assert_eq!(
        step, UPGRADE_STEP_PRE_UPGRADE_CHECK,
        "case checked: a check performs the pre-upgrade check alone, got {step:?}"
    );
    assert_eq!(
        step_status, UPGRADE_STATUS_SUCCEEDED,
        "case checked: expected the step succeeded, got {step_status:?}"
    );
    assert_eq!(
        upgrade_name, "Upgrade eligibility check from OpenSearch_2.11 to OpenSearch_2.19",
        "case checked: expected the run described as a check, got {upgrade_name:?}"
    );
    let described = emulator.describe(checked).await;
    assert_eq!(
        string_member(&described.body["DomainStatus"], "EngineVersion", "checked"),
        "OpenSearch_2.11",
        "case checked: a check moves the domain nowhere, got {:?}",
        described.body["DomainStatus"]
    );

    // The status is the LATEST run's: a second run started after the first replaces
    // what is reported.
    let second = upgrade(&emulator, upgraded, "OpenSearch_2.19", false).await;
    assert_eq!(
        second.status, 200,
        "case second run: expected HTTP 200, got {} with {:?}",
        second.status, second.body
    );
    let (step, step_status, upgrade_name) = reported(&status(&emulator, upgraded).await, "second run");
    assert_eq!(
        (step.as_str(), step_status.as_str()),
        (UPGRADE_STEP_PRE_UPGRADE_CHECK, UPGRADE_STATUS_IN_PROGRESS),
        "case second run: expected the newly started run reported, got {step:?} / {step_status:?}"
    );
    assert_eq!(
        upgrade_name, "Upgrade from OpenSearch_2.17 to OpenSearch_2.19",
        "case second run: expected the latest run described, got {upgrade_name:?}"
    );
}

#[tokio::test]
async fn a_domain_with_no_run_and_a_name_with_no_domain_report_no_step() {
    let emulator = Emulator::new();

    // "The most recent status of the last upgrade or upgrade eligibility check
    // performed on" the domain: a domain nothing has upgraded or checked has none.
    let quiet = "status-never-upgraded";
    settled_domain(&emulator, quiet, "OpenSearch_2.11").await;
    let answer = status(&emulator, quiet).await;
    assert_eq!(
        answer.status, 200,
        "case never upgraded: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    for member in ["UpgradeStep", "StepStatus", "UpgradeName"] {
        assert!(
            answer.body[member].is_null(),
            "case never upgraded: expected no {member}, got {:?}",
            answer.body
        );
    }

    // A name the account holds no domain under names nothing to report on.
    for name in ["status-absent", "sta", "status-absent-with-a-much-l"] {
        let answer = status(&emulator, name).await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case absent {name:?}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case absent {name:?}: expected the missing-resource refusal, got {:?}",
            answer.body
        );
        assert_eq!(
            string_member(&answer.body, "message", "absent"),
            format!("Domain not found: {name}"),
            "case absent {name:?}: expected the refusal to name the domain, got {:?}",
            answer.body
        );
    }
}
