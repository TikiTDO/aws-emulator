// SPDX-License-Identifier: CC0-1.0

//! What `ListApplicationVersions` summarises: the version HISTORY an application
//! accumulated, read back out of persisted state.
//!
//! Every test drives the service the way a client does — one `POST /` per call through the
//! real `MultiServiceRouter`, with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>`.
//!
//! The rule under test, asserted over a TABLE of lifecycles rather than over one recorded
//! request: `ListApplicationVersions` "lists all the versions for the specified
//! application" with "a summary of the configuration associated with each version" (the
//! operation's own doc), so every call reports one `ApplicationVersionSummary` per version
//! the application has had — oldest first, one per accepted configuration update plus the
//! version in force — and the `ApplicationStatus` a summary carries is the status of the
//! version it summarises: frozen as that version ran for a version the application has
//! LEFT, and the status in force now for the version in force, a clock-owed transition
//! included. An operation that does not mint a version (a start, a stop) therefore adds no
//! summary and changes only the one of the version in force.
//!
//! The rows differ along what the rule generalises over: how many versions the application
//! has, and the status each version was superseded under — so the frozen statuses a row
//! expects are not all the same value, and a summary cannot be answered from the record in
//! force.
//!
//! `tags_versions_and_snapshots.rs` covers the paging of this same listing and
//! `application_version_reads.rs` the per-version reader; this suite is about which
//! versions and which statuses the listing reports.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::harness::{call, create_startable, describe, router};

/// The runtime every application here runs on: the version readers are served only for the
/// Managed Service for Apache Flink family.
const FLINK_RUNTIME: &str = "FLINK-1_20";

/// How long a case waits for the status the clock owes before giving up. Generous on
/// purpose — the condition is polled, so a slow host takes longer rather than failing.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a poll waits before looking at the status again.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

/// One step of an application's life, as a client drives it.
#[derive(Clone, Copy, Debug)]
enum Step {
    /// An accepted configuration update: mints a new version and freezes the one it leaves.
    Update,
    /// `StartApplication`: moves the application to `RUNNING`, minting no version.
    Start,
    /// `StopApplication`: moves the application back to `READY`, minting no version.
    Stop,
}

/// The `(ApplicationVersionId, ApplicationStatus)` pairs `ListApplicationVersions` reports
/// for `name`, asserting the call succeeded, that each summary carries exactly the two
/// members the model declares for `ApplicationVersionSummary`
/// (`smithy_query(query="shape",shape="ApplicationVersionSummary")`), and that a page
/// holding the whole history carries no token.
async fn summaries(router: &MultiServiceRouter, case: &str, name: &str) -> Vec<(i64, String)> {
    let (status, listed) = call(
        router,
        "ListApplicationVersions",
        &json!({"ApplicationName": name}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListApplicationVersions got {status} {listed}"
    );
    let entries = listed["ApplicationVersionSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected the summaries member, got {listed}"))
        .clone();
    assert!(
        listed.get("NextToken").is_none_or(Value::is_null),
        "case {case}: a page that holds every version carries no token, got {listed}"
    );
    entries
        .iter()
        .map(|summary| {
            let mut members: Vec<&str> = summary
                .as_object()
                .unwrap_or_else(|| panic!("case {case}: expected a summary object, got {summary}"))
                .keys()
                .map(String::as_str)
                .collect();
            members.sort_unstable();
            assert_eq!(
                members,
                vec!["ApplicationStatus", "ApplicationVersionId"],
                "case {case}: expected exactly the modelled summary members, got {summary}"
            );
            (
                summary["ApplicationVersionId"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("case {case}: expected a version id, got {summary}")),
                summary["ApplicationStatus"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a status, got {summary}"))
                    .to_string(),
            )
        })
        .collect()
}

/// Look again until the summary of the version in force carries `settled`, or
/// [`SETTLE_DEADLINE`] passes. Answers the summaries read on the last look.
async fn summaries_once_settled(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    settled: &str,
) -> Vec<(i64, String)> {
    let give_up_at = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let reported = summaries(router, case, name).await;
        let in_force = reported
            .last()
            .unwrap_or_else(|| panic!("case {case}: expected at least the version in force"));
        if in_force.1 == settled {
            return reported;
        }
        assert!(
            tokio::time::Instant::now() < give_up_at,
            "case {case}: expected the version in force to settle at {settled}, got {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Apply one accepted configuration update to `name` against `version`, and answer the
/// version the service moved it to. The parallelism asked for differs per update, so no
/// two versions of one application ran under the same configuration.
async fn update(router: &MultiServiceRouter, case: &str, name: &str, version: i64) -> i64 {
    let (status, updated) = call(
        router,
        "UpdateApplication",
        &json!({
            "ApplicationName": name,
            "CurrentApplicationVersionId": version,
            "ApplicationConfigurationUpdate": {
                "FlinkApplicationConfigurationUpdate": {
                    "ParallelismConfigurationUpdate": {
                        "ConfigurationTypeUpdate": "CUSTOM",
                        "ParallelismUpdate": version + 1,
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateApplication on v{version} got {status} {updated}"
    );
    updated["ApplicationDetail"]["ApplicationVersionId"]
        .as_i64()
        .unwrap_or_else(|| panic!("case {case}: expected a version id, got {updated}"))
}

/// Run `operation` (a start or a stop) on `name`, asserting it was accepted.
async fn lifecycle_call(router: &MultiServiceRouter, case: &str, name: &str, operation: &str) {
    let (status, answered) = call(router, operation, &json!({"ApplicationName": name})).await;
    assert_eq!(
        status, 200,
        "case {case}: {operation} got {status} {answered}"
    );
}

/// Every version an application has had is summarised, oldest first, and each summary
/// carries the status of the version it summarises: the lifecycles differ in how many
/// versions they mint and in the status each superseded version ran under.
#[tokio::test]
async fn every_version_is_summarised_with_the_status_that_version_carries() {
    let router = router();

    // (case, the lifecycle the client drives) — a single-version application, histories
    // built entirely while READY, histories built while RUNNING, and one that moves
    // between the two so the frozen statuses within ONE history differ from each other.
    let cases: [(&str, &[Step]); 5] = [
        ("single-version", &[]),
        ("updated-while-ready", &[Step::Update, Step::Update]),
        ("started-then-updated", &[Step::Start, Step::Update]),
        ("updated-then-started", &[Step::Update, Step::Start]),
        (
            "across-the-lifecycle",
            &[Step::Update, Step::Start, Step::Update, Step::Stop],
        ),
    ];

    for (case, steps) in cases {
        let name = format!("version-summaries-{case}");
        create_startable(&router, &name, FLINK_RUNTIME).await;

        // What every version must be summarised as, oldest first. The create's version is
        // in force and READY; each step appends to or moves the tail of this expectation
        // exactly as the rule says. Polled like every later step, so a create that ever
        // becomes clock-owed waits for the status it owes rather than failing.
        let mut expected: Vec<(i64, String)> = vec![(1, "READY".to_string())];
        assert_eq!(
            summaries_once_settled(&router, case, &name, "READY").await,
            expected,
            "case {case}: a created application has one version to summarise"
        );

        let mut version = 1_i64;
        for (index, step) in steps.iter().enumerate() {
            let step_case = format!("{case}/step-{index}-{step:?}");
            // The status in force when this step runs is the one the version in force
            // carries, which the previous step left settled.
            let in_force_status = expected
                .last()
                .expect("the version in force is always summarised")
                .1
                .clone();

            let settled = match step {
                Step::Update => {
                    let next = update(&router, &step_case, &name, version).await;
                    assert_eq!(
                        next,
                        version + 1,
                        "case {step_case}: an accepted update mints the next version"
                    );
                    version = next;
                    // The version just left keeps the status it ran under, and the new
                    // version in force comes to rest on that same status — an update of a
                    // RUNNING application restarts its job (`UPDATING` meanwhile), an
                    // update of one that is not running is complete when the call answers.
                    expected.push((version, in_force_status.clone()));
                    in_force_status
                }
                Step::Start => {
                    lifecycle_call(&router, &step_case, &name, "StartApplication").await;
                    "RUNNING".to_string()
                }
                Step::Stop => {
                    lifecycle_call(&router, &step_case, &name, "StopApplication").await;
                    "READY".to_string()
                }
            };

            // Read straight back: whatever the clock has done by now, the versions the
            // application has LEFT are summarised exactly as before this step, and only the
            // summary of the version in force may have moved.
            let straight_back = summaries(&router, &step_case, &name).await;
            assert_eq!(
                straight_back.len(),
                expected.len(),
                "case {step_case}: expected one summary per version, got {straight_back:?}"
            );
            assert_eq!(
                &straight_back[..straight_back.len() - 1],
                &expected[..expected.len() - 1],
                "case {step_case}: a version the application has left keeps its summary, got \
                 {straight_back:?}"
            );

            // And once the clock has made the owed transition due, every summary is the one
            // the rule predicts.
            let reported = summaries_once_settled(&router, &step_case, &name, &settled).await;
            if let Some(last) = expected.last_mut() {
                last.1 = settled.clone();
            }
            assert_eq!(
                reported, expected,
                "case {step_case}: expected one summary per version, oldest first, each with \
                 the status of the version it summarises"
            );
        }

        assert_eq!(
            summaries(&router, case, &name).await,
            expected,
            "case {case}: expected the whole history to be summarised from stored state"
        );
        assert_eq!(
            expected.iter().map(|(id, _)| *id).collect::<Vec<i64>>(),
            (1..=expected.len() as i64).collect::<Vec<i64>>(),
            "case {case}: expected one version per accepted update, oldest first"
        );
    }
}

/// Listing versions is reporting, not mutating: a run of listings leaves the version in
/// force, the statuses and the application's own detail exactly as a later caller finds
/// them, and the next update still names the version the listing reported.
#[tokio::test]
async fn listing_versions_does_not_change_what_a_later_caller_sees() {
    let router = router();
    let case = "listing-is-a-read";
    let name = "version-summaries-reads";
    create_startable(&router, name, FLINK_RUNTIME).await;
    let version = update(&router, case, name, 1).await;
    let version = update(&router, case, name, version).await;
    lifecycle_call(&router, case, name, "StartApplication").await;
    let settled = summaries_once_settled(&router, case, name, "RUNNING").await;
    let detail_before = describe(&router, name).await;

    // Pages asked for in different shapes all answer the same history from the top, and
    // asking repeatedly neither advances a version nor moves a status. `Limit`'s own walk
    // is `tags_versions_and_snapshots.rs`'s subject; what matters here is that a first page
    // is the same prefix of the same history however often it is asked for.
    for (round, limit) in [(0, None), (1, Some(50)), (2, Some(2)), (3, Some(1))] {
        let mut body = json!({"ApplicationName": name});
        if let Some(limit) = limit {
            body["Limit"] = json!(limit);
        }
        let (status, listed) = call(&router, "ListApplicationVersions", &body).await;
        assert_eq!(
            status, 200,
            "case {case}/round-{round}: ListApplicationVersions got {status} {listed}"
        );
        let page: Vec<(i64, String)> = listed["ApplicationVersionSummaries"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}/round-{round}: expected summaries, got {listed}"))
            .iter()
            .map(|summary| {
                (
                    summary["ApplicationVersionId"].as_i64().unwrap_or_default(),
                    summary["ApplicationStatus"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let served = limit.unwrap_or(settled.len()).min(settled.len());
        assert_eq!(
            page,
            settled[..served].to_vec(),
            "case {case}/round-{round}: expected the first {served} version(s) of the same \
             history, got {listed}"
        );
        assert_eq!(
            listed["NextToken"].as_str().is_some(),
            served < settled.len(),
            "case {case}/round-{round}: a token is issued exactly when versions are left \
             unserved, got {listed}"
        );
    }

    assert_eq!(
        summaries(&router, case, name).await,
        settled,
        "case {case}: expected the history to be untouched by listing it"
    );
    assert_eq!(
        describe(&router, name).await,
        detail_before,
        "case {case}: expected the application's detail to be untouched by listing its versions"
    );

    // The version in force is still the one an update has to name: no listing consumed it.
    let next = update(&router, case, name, version).await;
    assert_eq!(
        next,
        version + 1,
        "case {case}: expected the next version to follow the one the listing reported"
    );
}
