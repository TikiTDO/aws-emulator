// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpgradeDomain` and `GetUpgradeHistory`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`]; this suite adds no second copy of them.
//!
//! The rule under test: a domain's upgrade history is the runs `UpgradeDomain`
//! recorded on it — an upgrade or an upgrade eligibility check, each with the instant
//! its call was made, the status of the run and the steps it has performed so far —
//! reported newest first, bounded to the last ten runs, in pages of at most
//! `MaxResults` resumed by the token the previous page handed out. An upgrade moves
//! the domain onto the version it names once the clock makes the run due; a check
//! performs none — and so records no second run when it is repeated, which
//! [`super::upgrade_eligibility_checks`] asserts; a target version the domain's
//! current version cannot be upgraded to
//! is refused, as is a second run started while one is still being performed, and a
//! name the account holds no domain under has no history at all.
//!
//! So the assertions span SETS of inputs rather than one call: a sequence of five runs
//! covering both kinds of run, a minor hop, a two-digit minor and a crossing from the
//! Elasticsearch line to the OpenSearch one; nine refused target versions spanning
//! the ways a version fails to go forwards; seven page sizes including the published
//! bound and values past it at either end; four shapes of unresolvable token; and the
//! history cap read through the eleventh run.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    UpgradeRun, UPGRADE_HISTORY, UPGRADE_STEP_PRE_UPGRADE_CHECK, UPGRADE_STEP_SNAPSHOT,
    UPGRADE_STEP_UPGRADE,
};

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS, SETTLE_WAIT,
};

/// The steps an upgrade performs, in order — the catalog `UpgradeStep` declares, as
/// `state`'s own constants, so a case naming a step outside it would assert nothing
/// about this service's wire surface.
const UPGRADE_STEPS: [&str; 3] = [
    UPGRADE_STEP_PRE_UPGRADE_CHECK,
    UPGRADE_STEP_SNAPSHOT,
    UPGRADE_STEP_UPGRADE,
];

/// One run of the sequence every ordering case below is asserted over: the target
/// version it names and whether it only checks eligibility.
struct Run {
    target: &'static str,
    check_only: bool,
}

/// The engine version the sequence of runs starts from: far enough back on the
/// Elasticsearch line that the runs below cover a minor hop, the last version of that
/// line and the crossing into the OpenSearch one.
const SEQUENCE_SOURCE: &str = "Elasticsearch_7.9";

/// The runs performed, in order, by the ordering cases.
///
/// The rows differ along what the rule generalises over: the KIND of run (an upgrade
/// and an eligibility check, including a check for a version a later row then
/// upgrades to), a single-minor hop, a two-digit minor, a crossing from one engine
/// line to the next, and a hop of several published versions.
const SEQUENCE: [Run; 5] = [
    Run {
        target: "Elasticsearch_7.10",
        check_only: true,
    },
    Run {
        target: "Elasticsearch_7.10",
        check_only: false,
    },
    Run {
        target: "OpenSearch_1.0",
        check_only: true,
    },
    Run {
        target: "OpenSearch_1.3",
        check_only: false,
    },
    Run {
        target: "OpenSearch_2.19",
        check_only: false,
    },
];

/// `UpgradeDomain`: `POST /2021-01-01/opensearch/upgradeDomain`.
pub(crate) async fn upgrade(
    emulator: &Emulator,
    name: &str,
    target: &str,
    check_only: bool,
) -> Answer {
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

/// `GetUpgradeHistory`: `GET /2021-01-01/opensearch/upgradeDomain/{name}/history`,
/// with `MaxResults` and `NextToken` as its `@httpQuery` members.
pub(crate) async fn history(emulator: &Emulator, name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/upgradeDomain/{name}/history")
    } else {
        format!("{PREFIX}/opensearch/upgradeDomain/{name}/history?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `UpgradeHistories` of an accepted read.
pub(crate) fn entries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["UpgradeHistories"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an UpgradeHistories list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `UpgradeName` of every entry of a page, in the order the page reports them.
pub(crate) fn names(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|entry| entry["UpgradeName"].as_str().unwrap_or_default().to_owned())
        .collect()
}

/// `UpgradeHistory$UpgradeName` for a run: "a string that describes the upgrade" —
/// which versions it went between, and whether it performed the upgrade or only
/// checked that it could.
pub(crate) fn expected_name(source: &str, target: &str, check_only: bool) -> String {
    if check_only {
        format!("Upgrade eligibility check from {source} to {target}")
    } else {
        format!("Upgrade from {source} to {target}")
    }
}

/// One entry's `StartTimestamp`, as the epoch-second count `restJson1` carries a
/// timestamp as.
fn started_at(entry: &Value, case: &str) -> f64 {
    entry["StartTimestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a StartTimestamp, got {entry:?}"))
}

/// One entry's `StepsList`, as `(UpgradeStep, UpgradeStepStatus, ProgressPercent)`
/// triples in the order the entry reports them.
fn steps(entry: &Value, case: &str) -> Vec<(String, String, f64)> {
    entry["StepsList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a StepsList, got {entry:?}"))
        .iter()
        .map(|step| {
            (
                string_member(step, "UpgradeStep", case),
                string_member(step, "UpgradeStepStatus", case),
                step["ProgressPercent"].as_f64().unwrap_or_else(|| {
                    panic!("case {case}: expected a ProgressPercent, got {step:?}")
                }),
            )
        })
        .collect()
}

/// Create a domain on `engine_version` and let the provisioning span fall due, so the
/// record is settled before anything upgrades it.
pub(crate) async fn settled_domain(emulator: &Emulator, name: &str, engine_version: &str) {
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

/// Perform [`SEQUENCE`] on a freshly created domain, letting each run fall due before
/// the next starts, and answer the source version each run started from.
async fn perform_sequence(emulator: &Emulator, name: &str) -> Vec<String> {
    settled_domain(emulator, name, SEQUENCE_SOURCE).await;
    let mut sources = Vec::new();
    let mut current = SEQUENCE_SOURCE.to_string();
    for run in &SEQUENCE {
        let case = expected_name(&current, run.target, run.check_only);
        let answer = upgrade(emulator, name, run.target, run.check_only).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            string_member(&answer.body, "TargetVersion", &case),
            run.target,
            "case {case}: expected the requested TargetVersion echoed, got {:?}",
            answer.body
        );
        assert_eq!(
            answer.body["PerformCheckOnly"].as_bool(),
            Some(run.check_only),
            "case {case}: expected PerformCheckOnly {}, got {:?}",
            run.check_only,
            answer.body
        );
        assert!(
            !string_member(&answer.body, "UpgradeId", &case).is_empty(),
            "case {case}: expected an UpgradeId, got {:?}",
            answer.body
        );
        sources.push(current.clone());
        if !run.check_only {
            // Only an upgrade moves the domain; a check leaves it where it was.
            current = run.target.to_string();
        }
        tokio::time::sleep(SETTLE_WAIT).await;
    }
    sources
}

/// A completed run put into the store directly, started `age` before `now`.
///
/// The paging and cap cases need more runs than a wall-clock-bounded test can
/// perform through the wire — each run takes the span the clock owes it — so the
/// history they page over is seeded as the records `UpgradeDomain` writes, and
/// everything the rule is asserted ON is read back through the wire.
fn stored_run(upgrade_id: &str, target: &str, check_only: bool, age: Duration) -> UpgradeRun {
    let started_at = SystemTime::now() - age;
    UpgradeRun {
        upgrade_id: upgrade_id.to_string(),
        source_version: "OpenSearch_1.0".to_string(),
        target_version: target.to_string(),
        perform_check_only: check_only,
        advanced_options: std::collections::BTreeMap::new(),
        started_at,
        completes_at: started_at + Duration::from_millis(500),
        // A run whose span has passed has already landed on the domain record.
        applied: !check_only,
    }
}

/// Seed `count` completed runs, oldest first, onto a created domain.
fn seed(emulator: &Emulator, name: &str, count: usize) -> Vec<String> {
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case seed({name:?}): the domain must have been created"));
    let mut ids = Vec::new();
    for index in 0..count {
        let id = format!("seeded-{index:02}");
        // Older runs are further in the past, so the seeded history carries the same
        // strictly increasing start instants a performed sequence does.
        let age = Duration::from_secs(u64::try_from(count - index).unwrap_or(1) * 60);
        domain
            .upgrades
            .push_back(stored_run(&id, "OpenSearch_1.3", index % 2 == 0, age));
        ids.push(id);
    }
    ids
}

#[tokio::test]
async fn the_history_reports_every_run_of_the_domain_newest_first() {
    let emulator = Emulator::new();
    let name = "upgrade-sequence";
    let sources = perform_sequence(&emulator, name).await;

    let answer = history(&emulator, name, "").await;
    let page = entries(&answer, "whole history");
    assert_eq!(
        page.len(),
        SEQUENCE.len(),
        "case whole history: expected one entry per run performed, got {:?}",
        names(&page)
    );

    // The most recent run is reported FIRST: the report is the sequence reversed.
    let expected: Vec<String> = SEQUENCE
        .iter()
        .zip(sources.iter())
        .map(|(run, source)| expected_name(source, run.target, run.check_only))
        .rev()
        .collect();
    assert_eq!(
        names(&page),
        expected,
        "case whole history: expected the runs newest first, got {:?}",
        names(&page)
    );

    // Newest first is an ordering on the instant each call was made, not just on the
    // order the entries happen to be in.
    for pair in page.windows(2) {
        let newer = started_at(&pair[0], "chronology");
        let older = started_at(&pair[1], "chronology");
        assert!(
            newer > older,
            "case chronology: expected each entry to have started after the one below \
             it, got {newer} then {older}"
        );
    }

    // Every run of the sequence has finished, so each reports every step it performs,
    // in order, each succeeded and complete.
    for (entry, run) in page.iter().zip(SEQUENCE.iter().rev()) {
        let case = string_member(entry, "UpgradeName", "steps");
        assert_eq!(
            string_member(entry, "UpgradeStatus", &case),
            "SUCCEEDED",
            "case {case}: a finished run has succeeded, got {entry:?}"
        );
        let performed = steps(entry, &case);
        // An eligibility check "does not actually perform the upgrade", so it performs
        // the pre-upgrade check alone; an upgrade performs all three steps.
        let expected_steps: Vec<&str> = if run.check_only {
            vec![UPGRADE_STEPS[0]]
        } else {
            UPGRADE_STEPS.to_vec()
        };
        assert_eq!(
            performed
                .iter()
                .map(|(step, _, _)| step.as_str())
                .collect::<Vec<_>>(),
            expected_steps,
            "case {case}: expected the steps this kind of run performs, got {performed:?}"
        );
        for (step, status, progress) in &performed {
            assert_eq!(
                status, "SUCCEEDED",
                "case {case} / {step}: a step of a finished run has succeeded, got {status}"
            );
            assert!(
                (*progress - 100.0).abs() < f64::EPSILON,
                "case {case} / {step}: expected a complete step, got {progress}"
            );
        }
    }

    // The domain runs the version its LAST upgrade moved it onto — the check-only runs
    // moved it nowhere.
    let described = emulator.describe(name).await;
    assert_eq!(
        string_member(&described.body["DomainStatus"], "EngineVersion", "landed"),
        "OpenSearch_2.19",
        "case landed: expected the version the last upgrade named, got {:?}",
        described.body["DomainStatus"]
    );
}

#[tokio::test]
async fn an_upgrade_lands_on_the_clock_and_a_check_leaves_the_domain_alone() {
    let emulator = Emulator::new();

    // An upgrade: the domain reports the run in flight before the clock makes it due,
    // and the version it was upgraded to only afterwards.
    let upgraded = "upgrade-lands";
    settled_domain(&emulator, upgraded, "OpenSearch_2.11").await;
    let started = upgrade(&emulator, upgraded, "OpenSearch_2.19", false).await;
    assert_eq!(
        started.status, 200,
        "case upgrade start: expected HTTP 200, got {} with {:?}",
        started.status, started.body
    );
    assert!(
        started.body["ChangeProgressDetails"]["ChangeId"]
            .as_str()
            .is_some(),
        "case upgrade start: an upgrade is a configuration change of the domain, got {:?}",
        started.body
    );

    let in_flight = emulator.describe(upgraded).await.body["DomainStatus"].clone();
    assert_eq!(
        in_flight["UpgradeProcessing"].as_bool(),
        Some(true),
        "case in flight: expected UpgradeProcessing while the run is being performed, \
         got {in_flight:?}"
    );
    assert_eq!(
        string_member(&in_flight, "EngineVersion", "in flight"),
        "OpenSearch_2.11",
        "case in flight: the domain runs the version it started on until the run is due, \
         got {in_flight:?}"
    );
    assert_eq!(
        string_member(&in_flight, "DomainProcessingStatus", "in flight"),
        "UpgradingEngineVersion",
        "case in flight: expected the upgrading processing status, got {in_flight:?}"
    );

    let running = entries(&history(&emulator, upgraded, "").await, "in flight");
    assert_eq!(
        running.len(),
        1,
        "case in flight: expected the one run started, got {:?}",
        names(&running)
    );
    assert_eq!(
        string_member(&running[0], "UpgradeStatus", "in flight"),
        "IN_PROGRESS",
        "case in flight: expected the run in progress, got {:?}",
        running[0]
    );
    let partial = steps(&running[0], "in flight");
    assert!(
        !partial.is_empty() && partial.len() <= UPGRADE_STEPS.len(),
        "case in flight: expected the steps performed so far, got {partial:?}"
    );
    for (index, (step, status, progress)) in partial.iter().enumerate() {
        assert_eq!(
            step, UPGRADE_STEPS[index],
            "case in flight: expected the steps in order, got {partial:?}"
        );
        assert!(
            status == "IN_PROGRESS" || status == "SUCCEEDED",
            "case in flight / {step}: expected a started step's status, got {status}"
        );
        assert!(
            (0.0..=100.0).contains(progress),
            "case in flight / {step}: expected a progress percentage, got {progress}"
        );
    }

    tokio::time::sleep(SETTLE_WAIT).await;
    let landed = emulator.describe(upgraded).await.body["DomainStatus"].clone();
    assert_eq!(
        landed["UpgradeProcessing"].as_bool(),
        Some(false),
        "case landed: no upgrade is in process once the run is due, got {landed:?}"
    );
    assert_eq!(
        string_member(&landed, "EngineVersion", "landed"),
        "OpenSearch_2.19",
        "case landed: expected the version the upgrade named, got {landed:?}"
    );

    // An eligibility check: the history records it, and nothing about the domain
    // moves.
    let checked = "upgrade-checked";
    settled_domain(&emulator, checked, "OpenSearch_2.11").await;
    let before = emulator.describe(checked).await.body["DomainStatus"].clone();
    let check = upgrade(&emulator, checked, "OpenSearch_2.19", true).await;
    assert_eq!(
        check.status, 200,
        "case check: expected HTTP 200, got {} with {:?}",
        check.status, check.body
    );
    assert!(
        check.body["ChangeProgressDetails"].is_null(),
        "case check: a check starts no configuration change, got {:?}",
        check.body
    );
    tokio::time::sleep(SETTLE_WAIT).await;

    let after = emulator.describe(checked).await.body["DomainStatus"].clone();
    assert_eq!(
        string_member(&after, "EngineVersion", "check"),
        string_member(&before, "EngineVersion", "check"),
        "case check: a check performs no upgrade, got {after:?}"
    );
    assert_eq!(
        after["UpgradeProcessing"].as_bool(),
        Some(false),
        "case check: a check is no upgrade in process, got {after:?}"
    );
    let checked_history = entries(&history(&emulator, checked, "").await, "check");
    assert_eq!(
        names(&checked_history),
        vec![expected_name(
            "OpenSearch_2.11",
            "OpenSearch_2.19",
            true
        )],
        "case check: expected the check recorded as a run of its own, got {:?}",
        names(&checked_history)
    );
    assert_eq!(
        steps(&checked_history[0], "check")
            .iter()
            .map(|(step, _, _)| step.as_str())
            .collect::<Vec<_>>(),
        vec![UPGRADE_STEPS[0]],
        "case check: a check performs the pre-upgrade check alone, got {:?}",
        checked_history[0]
    );
}

#[tokio::test]
async fn an_upgrade_only_ever_goes_to_a_version_the_domain_can_reach() {
    let emulator = Emulator::new();

    // One row per way a target version fails to be one the domain can be upgraded to:
    // the version it already runs, an earlier one of its own line, an earlier line
    // altogether, the next line reached from the middle of its own rather than from its
    // end, a line two ahead of its own, a version the service does not publish
    // (pattern-valid), and the newest published version, which has nothing ahead of it at
    // all.
    let refused = [
        (
            "refuse-same",
            "OpenSearch_2.11",
            "OpenSearch_2.11",
            "its own version",
        ),
        (
            "refuse-back-minor",
            "OpenSearch_2.11",
            "OpenSearch_2.9",
            "an earlier minor",
        ),
        (
            "refuse-back-major",
            "OpenSearch_2.11",
            "OpenSearch_1.3",
            "an earlier major",
        ),
        (
            "refuse-back-line",
            "OpenSearch_1.0",
            "Elasticsearch_7.10",
            "the line it came from",
        ),
        (
            "refuse-early-cross",
            "OpenSearch_2.3",
            "OpenSearch_3.1",
            "the next line before the end of its own",
        ),
        (
            "refuse-two-lines",
            "OpenSearch_1.3",
            "OpenSearch_3.1",
            "a line two ahead of its own",
        ),
        (
            "refuse-unpublished",
            "OpenSearch_2.11",
            "OpenSearch_2.12",
            "a version the service does not publish",
        ),
        (
            "refuse-terminus",
            "OpenSearch_2.19",
            "OpenSearch_2.19",
            "nothing ahead of it",
        ),
    ];

    for (name, source, target, why) in refused {
        settled_domain(&emulator, name, source).await;
        let answer = upgrade(&emulator, name, target, false).await;
        assert_eq!(
            answer.status, 400,
            "case {name} ({why}): expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "BaseException",
            "case {name} ({why}): expected the service's generic refusal, got {:?}",
            answer.body
        );
        // A refused upgrade is not a run: it records nothing and moves nothing.
        let page = entries(&history(&emulator, name, "").await, name);
        assert!(
            page.is_empty(),
            "case {name} ({why}): a refused upgrade records no run, got {:?}",
            names(&page)
        );
        let described = emulator.describe(name).await;
        assert_eq!(
            string_member(&described.body["DomainStatus"], "EngineVersion", name),
            source,
            "case {name} ({why}): a refused upgrade moves nothing, got {:?}",
            described.body["DomainStatus"]
        );

        // A check of the same target is refused for the same reason: whether the
        // upgrade is possible is the whole of what a check answers.
        let as_check = upgrade(&emulator, name, target, true).await;
        assert_eq!(
            error_code(&as_check),
            "BaseException",
            "case {name} ({why}) as a check: expected the same refusal, got {:?}",
            as_check.body
        );
    }

    // The accepted rows of the same table: a version ahead on the same line, and the
    // crossing a domain on the last published version of its line may make.
    for (name, source, target) in [
        ("accept-next-minor", "OpenSearch_2.11", "OpenSearch_2.13"),
        ("accept-far-minor", "OpenSearch_1.0", "OpenSearch_1.3"),
        ("accept-cross-line", "Elasticsearch_7.10", "OpenSearch_1.0"),
    ] {
        settled_domain(&emulator, name, source).await;
        let answer = upgrade(&emulator, name, target, false).await;
        assert_eq!(
            answer.status, 200,
            "case {name}: expected an upgrade to a version ahead of it to be accepted, \
             got {} with {:?}",
            answer.status, answer.body
        );
    }
}

#[tokio::test]
async fn a_second_run_is_refused_while_one_is_being_performed() {
    let emulator = Emulator::new();
    let name = "upgrade-one-at-a-time";
    settled_domain(&emulator, name, "OpenSearch_2.11").await;

    let first = upgrade(&emulator, name, "OpenSearch_2.13", false).await;
    let upgrade_id = string_member(&first.body, "UpgradeId", "first run");

    for (case, target, check_only) in [
        ("second upgrade", "OpenSearch_2.15", false),
        ("second check", "OpenSearch_2.15", true),
    ] {
        let answer = upgrade(&emulator, name, target, check_only).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected HTTP 409, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceAlreadyExistsException",
            "case {case}: expected the modelled already-exists refusal, got {:?}",
            answer.body
        );
        assert!(
            answer.body["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&upgrade_id),
            "case {case}: expected the refusal to name the run in flight {upgrade_id}, \
             got {:?}",
            answer.body
        );
    }

    // The refused calls recorded nothing: the one run in flight is the whole history.
    let page = entries(&history(&emulator, name, "").await, "while in flight");
    assert_eq!(
        page.len(),
        1,
        "case while in flight: expected the one run started, got {:?}",
        names(&page)
    );

    // Once that run is due, the next one is accepted.
    tokio::time::sleep(SETTLE_WAIT).await;
    let next = upgrade(&emulator, name, "OpenSearch_2.15", false).await;
    assert_eq!(
        next.status, 200,
        "case after the run is due: expected HTTP 200, got {} with {:?}",
        next.status, next.body
    );
}

#[tokio::test]
async fn pages_resume_where_the_previous_page_stopped() {
    let emulator = Emulator::new();
    let name = "upgrade-paging";
    settled_domain(&emulator, name, "OpenSearch_1.0").await;
    let seeded = seed(&emulator, name, 7);
    // Newest first, so the report is the seeded order reversed.
    let newest_first: Vec<String> = seeded.iter().rev().cloned().collect();

    /// Which seeded run each entry of a page is: the seeded runs carry the same
    /// `UpgradeName`, so a page is identified by where its entries sit in the whole
    /// history rather than by that name.
    fn page_ids(page: &[Value], order: &[String], all: &[Value]) -> Vec<String> {
        page.iter()
            .map(|entry| {
                let at = all
                    .iter()
                    .position(|candidate| candidate == entry)
                    .expect("every page entry is an entry of the whole history");
                order[at].clone()
            })
            .collect()
    }

    let whole = entries(&history(&emulator, name, "").await, "whole history");
    assert_eq!(
        whole.len(),
        seeded.len(),
        "case whole history: expected every seeded run, got {} entries",
        whole.len()
    );

    // One row per page size: every size short of the history, its exact length, one
    // past it, the published bound, and the two ways to ask for no bound at all.
    for query in [
        "maxResults=1",
        "maxResults=2",
        "maxResults=3",
        "maxResults=7",
        "maxResults=8",
        "maxResults=100",
        "maxResults=0",
        "",
    ] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match (&token, query.is_empty()) {
                (None, _) => query.to_string(),
                (Some(next), true) => format!("nextToken={next}"),
                (Some(next), false) => format!("{query}&nextToken={next}"),
            };
            let answer = history(&emulator, name, &uri).await;
            let page = entries(&answer, query);
            let bound = query
                .strip_prefix("maxResults=")
                .and_then(|size| size.parse::<usize>().ok())
                .filter(|size| *size > 0)
                .unwrap_or(100);
            assert!(
                page.len() <= bound,
                "case {query}: expected at most {bound} entries a page, got {}",
                page.len()
            );
            walked.extend(page_ids(&page, &newest_first, &whole));
            pages += 1;
            assert!(
                pages <= seeded.len() + 1,
                "case {query}: expected the walk to end, got {pages} pages"
            );
            match answer.body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            walked, newest_first,
            "case {query}: expected the walked pages to be the whole history newest \
             first, got {walked:?}"
        );
    }

    // A page size outside `MaxResults`'s published range is a malformed request at
    // either end, not a page.
    for size in ["101", "1000", "-1"] {
        let answer = history(&emulator, name, &format!("maxResults={size}")).await;
        assert_eq!(
            answer.status, 400,
            "case maxResults={size}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case maxResults={size}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }

    // A token is only ever a value this operation handed out, so one naming no run of
    // this domain is refused rather than answered with a page.
    for token in [
        "not-a-token",
        "seeded-99",
        "00000000-0000-0000-0000-000000000000",
        "seeded-0",
    ] {
        let answer = history(&emulator, name, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case nextToken={token:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case nextToken={token:?}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn the_history_keeps_the_last_runs_and_drops_the_oldest() {
    let emulator = Emulator::new();
    let name = "upgrade-cap";
    settled_domain(&emulator, name, "OpenSearch_1.0").await;
    let seeded = seed(&emulator, name, UPGRADE_HISTORY);

    let full = entries(&history(&emulator, name, "").await, "full history");
    assert_eq!(
        full.len(),
        UPGRADE_HISTORY,
        "case full history: expected the last {UPGRADE_HISTORY} runs, got {} entries",
        full.len()
    );

    // The run that takes the history past its cap drops the OLDEST run, so the newest
    // runs are the ones kept.
    let started = upgrade(&emulator, name, "OpenSearch_1.3", false).await;
    assert_eq!(
        started.status, 200,
        "case the run past the cap: expected HTTP 200, got {} with {:?}",
        started.status, started.body
    );
    let capped = entries(&history(&emulator, name, "").await, "capped history");
    assert_eq!(
        capped.len(),
        UPGRADE_HISTORY,
        "case capped history: expected the history to stay at {UPGRADE_HISTORY} runs, \
         got {} entries",
        capped.len()
    );
    assert_eq!(
        string_member(&capped[0], "UpgradeName", "capped history"),
        expected_name("OpenSearch_1.0", "OpenSearch_1.3", false),
        "case capped history: expected the newest run first, got {:?}",
        capped[0]
    );
    // The oldest seeded run is gone: the history a caller reads is the newest ten.
    let oldest = seeded
        .first()
        .expect("the seeded history is not empty")
        .clone();
    let answer = history(&emulator, name, &format!("nextToken={oldest}")).await;
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case dropped run: a token naming a run the cap dropped resolves to nothing, \
         got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_name_no_domain_answers_to_has_no_upgrade_surface() {
    let emulator = Emulator::new();
    settled_domain(&emulator, "upgrade-present", "OpenSearch_2.11").await;

    for absent in ["no-such-domain", "upgrade-presentx", "abc"] {
        let read = history(&emulator, absent, "").await;
        assert_eq!(
            read.status, RECORDED_NOT_FOUND_STATUS,
            "case GetUpgradeHistory({absent:?}): expected HTTP {RECORDED_NOT_FOUND_STATUS}, got \
             {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            error_code(&read),
            "ResourceNotFoundException",
            "case GetUpgradeHistory({absent:?}): expected the modelled not-found refusal, \
             got {:?}",
            read.body
        );

        let started = upgrade(&emulator, absent, "OpenSearch_2.19", false).await;
        assert_eq!(
            started.status, RECORDED_NOT_FOUND_STATUS,
            "case UpgradeDomain({absent:?}): expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} \
             with {:?}",
            started.status, started.body
        );
        assert_eq!(
            error_code(&started),
            "ResourceNotFoundException",
            "case UpgradeDomain({absent:?}): expected the modelled not-found refusal, got \
             {:?}",
            started.body
        );
    }

    // Reading a history changes nothing a later caller sees: the domain that does
    // exist still has none, and still runs what it was created on.
    let page = entries(&history(&emulator, "upgrade-present", "").await, "untouched");
    assert!(
        page.is_empty(),
        "case untouched: a domain nothing has upgraded has an empty history, got {:?}",
        names(&page)
    );
    let described = emulator.describe("upgrade-present").await;
    assert_eq!(
        string_member(&described.body["DomainStatus"], "EngineVersion", "untouched"),
        "OpenSearch_2.11",
        "case untouched: expected the version the domain was created on, got {:?}",
        described.body["DomainStatus"]
    );
}
