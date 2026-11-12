// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the declarative-policies account status report surface —
//! `StartDeclarativePoliciesReport`, `GetDeclarativePoliciesReportSummary` and
//! `CancelDeclarativePoliciesReport` — driven over the wire the service actually speaks
//! (`ec2Query`, through the crate's shared [`super::ec2_query`] harness).
//!
//! The rules under test are:
//!
//! 1. ROUND TRIP: a start records the report the request asked for and answers its id, and
//!    the summary reads that report back — "the name of the Amazon S3 bucket where the
//!    report is located", "the prefix for your S3 object", "the root ID, organizational unit
//!    ID, or account ID" and "the time when the report generation started";
//! 2. THE CLOCK OWES THE GENERATION: "the report is generated asynchronously", so a report
//!    is still running straight after the start — no `endTime`, and no account covered yet —
//!    and is complete one [`TRANSITION_DELAY`] later, with the instant it ended and the
//!    accounts of its scope;
//! 3. ONE AT A TIME: "only one report per organization can be generated at a time.
//!    Attempting to generate a report while another is in progress will result in an error",
//!    and a start once the running one has finished is served;
//! 4. CANCELLING: "you can only cancel a report while it has the running status. Reports
//!    with other statuses (complete, cancelled, or error) can't be canceled" — a running
//!    report is cancelled and answers `true`, and a cancelled or completed one is refused
//!    `IncorrectState`; the cancel moves exactly the report its id names, so every other
//!    report the organization holds is left where it was, and the word it leaves is what
//!    every later reader of that report answers;
//! 5. THE ID: a report id this service cannot resolve is refused — the summary reports every
//!    one not found, and the cancel tells a value that does not carry the `report-` prefix
//!    every report id has (`InvalidDeclarativePoliciesReportId.Malformed`) from one that does
//!    but names no report of the organization
//!    (`InvalidDeclarativePoliciesReportId.NotFound`);
//! 6. THE REQUEST'S OWN VALUES: the S3 bucket is read by Amazon S3's published bucket naming
//!    rules — the rule every operation of this crate that takes a bucket name shares — and the
//!    target must be one of the three forms the member documents (root, organizational unit,
//!    account);
//! 7. `DryRun` is the permission probe: it answers `DryRunOperation` and starts, summarises
//!    and cancels nothing.
//!
//! Each table row is a distinct input along what its rule generalises over — which of the
//! three target forms the report covers, whether it names a prefix or tags, and every way a
//! report id and a bucket name can be wrong — so no row is the specification.

use std::time::Duration;

use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, REPORT_STATE_CANCELLED, REPORT_STATE_COMPLETE,
    REPORT_STATE_RUNNING, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message, member};
use super::wire::{Param, child_text, document, param, set_items};

/// The Region every call here is signed for; a report is a resource of the organization, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// A bucket name every row that is not about the bucket uses.
const BUCKET: &str = "declarative-policy-reports";

/// Wait until the generation a start owes the clock has certainly fallen due. Derived from
/// the service's own span, not a literal, so the two cannot drift.
async fn past_the_generation() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `StartDeclarativePoliciesReport` call with `extra` beside the bucket and the target.
async fn start(
    router: &MultiServiceRouter,
    target_id: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![param("S3Bucket", BUCKET), param("TargetId", target_id)];
    params.extend_from_slice(extra);
    call(router, REGION, "StartDeclarativePoliciesReport", &params).await
}

/// One start that must be served, answering the `reportId` it minted.
async fn started(router: &MultiServiceRouter, case: &str, target_id: &str) -> String {
    let (status, body) = start(router, target_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartDeclarativePoliciesReport must succeed, got {status}: {body}"
    );
    member(&body, "reportId")
        .unwrap_or_else(|| panic!("case {case:?}: the start must answer a reportId: {body}"))
}

/// One `GetDeclarativePoliciesReportSummary` call with `extra` beside the report id.
async fn summary(
    router: &MultiServiceRouter,
    report_id: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![param("ReportId", report_id)];
    params.extend_from_slice(extra);
    call(
        router,
        REGION,
        "GetDeclarativePoliciesReportSummary",
        &params,
    )
    .await
}

/// One summary that must be served.
async fn summarised(router: &MultiServiceRouter, case: &str, report_id: &str) -> String {
    let (status, body) = summary(router, report_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetDeclarativePoliciesReportSummary must succeed, got {status}: {body}"
    );
    body
}

/// One `CancelDeclarativePoliciesReport` call with `extra` beside the report id.
async fn cancel(router: &MultiServiceRouter, report_id: &str, extra: &[Param]) -> (u16, String) {
    let mut params = vec![param("ReportId", report_id)];
    params.extend_from_slice(extra);
    call(router, REGION, "CancelDeclarativePoliciesReport", &params).await
}

/// The status the store holds for `report_id`, as of now — the word every reader of the
/// report derives its answer from.
fn stored_status(engine: &engine::InMemoryEngine, report_id: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .declarative_policies_reports
        .get(report_id)
        .map(|record| record.status_at(std::time::Instant::now()).to_string())
}

/// How many reports the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard.declarative_policies_reports.len()
}

/// The three forms a target takes: "For root: r-ab12; For OU: ou-ab12-cdef1234; For account:
/// 123456789012" (`StartDeclarativePoliciesReportRequest$TargetId`).
const TARGETS: [(&str, &str); 3] = [
    ("a root", "r-ab12"),
    ("an organizational unit", "ou-ab12-cdef1234"),
    ("an account", "123456789012"),
];

/// RULE 1 and RULE 2: the start records what the request asked for, and the summary reads it
/// back — running first, complete once the clock has had its span.
#[tokio::test]
async fn a_started_report_reads_back_and_completes_on_the_clock() {
    // Rows differ in which target form the report covers and whether it names an S3 prefix
    // and tags.
    let rows: [(&str, &str, Option<&str>, bool); 4] = [
        (TARGETS[0].0, TARGETS[0].1, None, false),
        (TARGETS[1].0, TARGETS[1].1, Some("reports/ec2"), false),
        (TARGETS[2].0, TARGETS[2].1, Some("one/two/three"), true),
        ("a root with tags", TARGETS[0].1, None, true),
    ];

    for (case, target_id, prefix, tagged) in rows {
        let (router, engine) = emulator();
        let mut extra: Vec<Param> = Vec::new();
        if let Some(prefix) = prefix {
            extra.push(param("S3Prefix", prefix));
        }
        if tagged {
            extra.push(param(
                "TagSpecification.1.ResourceType",
                "declarative-policies-report",
            ));
            extra.push(param("TagSpecification.1.Tag.1.Key", "team"));
            extra.push(param("TagSpecification.1.Tag.1.Value", "platform"));
        }
        let (status, body) = start(&router, target_id, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let report_id = member(&body, "reportId")
            .unwrap_or_else(|| panic!("case {case:?}: expected a reportId in {body}"));
        assert!(
            report_id.starts_with("report-"),
            "case {case:?}: expected a report id, got {report_id:?}"
        );

        // ROUND TRIP: the summary reads the report the start recorded.
        let body = summarised(&router, case, &report_id).await;
        assert_eq!(
            member(&body, "reportId").as_deref(),
            Some(report_id.as_str()),
            "case {case:?}: the summary must name the report, got {body}"
        );
        assert_eq!(
            member(&body, "s3Bucket").as_deref(),
            Some(BUCKET),
            "case {case:?}: the summary must report the bucket the start named, got {body}"
        );
        assert_eq!(
            member(&body, "s3Prefix").as_deref(),
            prefix,
            "case {case:?}: the summary must report the prefix the start named, got {body}"
        );
        assert_eq!(
            member(&body, "targetId").as_deref(),
            Some(target_id),
            "case {case:?}: the summary must report the target the start named, got {body}"
        );
        assert!(
            member(&body, "startTime")
                .as_deref()
                .and_then(as_timestamp)
                .is_some(),
            "case {case:?}: the summary must report when the generation started, got {body}"
        );

        // RULE 2, before the span: the generation is still running.
        assert_eq!(
            stored_status(&engine, &report_id).as_deref(),
            Some(REPORT_STATE_RUNNING),
            "case {case:?}: a report must still be running straight after the start"
        );
        assert_eq!(
            member(&body, "endTime"),
            None,
            "case {case:?}: a running generation has not ended, got {body}"
        );
        assert_eq!(
            member(&body, "numberOfAccounts").as_deref(),
            Some("0"),
            "case {case:?}: a running generation has covered no account yet, got {body}"
        );

        // RULE 2, after the span: the generation is complete, and the summary says so.
        past_the_generation().await;
        assert_eq!(
            stored_status(&engine, &report_id).as_deref(),
            Some(REPORT_STATE_COMPLETE),
            "case {case:?}: the clock owed the generation its completion"
        );
        let body = summarised(&router, case, &report_id).await;
        assert!(
            member(&body, "endTime")
                .as_deref()
                .and_then(as_timestamp)
                .is_some(),
            "case {case:?}: a complete generation reports when it ended, got {body}"
        );
        assert_eq!(
            member(&body, "numberOfAccounts").as_deref(),
            Some("1"),
            "case {case:?}: a complete generation covered the accounts of its scope, got {body}"
        );
        assert_eq!(
            member(&body, "numberOfFailedAccounts").as_deref(),
            Some("0"),
            "case {case:?}: no account of this organization failed, got {body}"
        );
    }
}

/// RULE 3: only one report is generated at a time, and a start after the running one has
/// finished is served.
#[tokio::test]
async fn only_one_report_is_generated_at_a_time() {
    for (case, target_id) in TARGETS {
        let (router, engine) = emulator();
        let first = started(&router, case, target_id).await;

        let (status, body) = start(&router, target_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 starting a second report, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got {body}"
        );
        assert!(
            error_message(&body).contains(&first),
            "case {case:?}: expected the refusal to name the running report, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {case:?}: a refused start must store no report"
        );

        // Once the clock has completed the first generation, a second start is served.
        past_the_generation().await;
        let second = started(&router, case, target_id).await;
        assert_ne!(
            second, first,
            "case {case:?}: a second report must carry its own id"
        );
        assert_eq!(
            stored_count(&engine),
            2,
            "case {case:?}: both reports are held"
        );
    }
}

/// RULE 4: a running report is cancelled; one that has reached any other status cannot be.
#[tokio::test]
async fn only_a_running_report_can_be_cancelled() {
    let (router, engine) = emulator();
    let report_id = started(&router, "a running report", TARGETS[0].1).await;

    let (status, body) = cancel(&router, &report_id, &[]).await;
    assert_eq!(
        status, 200,
        "a running report: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        member(&body, "return").as_deref(),
        Some("true"),
        "a running report: expected the cancel to answer true, got {body}"
    );
    assert_eq!(
        stored_status(&engine, &report_id).as_deref(),
        Some(REPORT_STATE_CANCELLED),
        "a cancelled report must read cancelled in every later reader"
    );
    // The cancel ended the generation, so the summary reports when it ended — and the
    // completion the clock owed it never happens.
    let body = summarised(&router, "a cancelled report", &report_id).await;
    assert!(
        member(&body, "endTime")
            .as_deref()
            .and_then(as_timestamp)
            .is_some(),
        "a cancelled report: expected the instant the generation ended, got {body}"
    );
    past_the_generation().await;
    assert_eq!(
        stored_status(&engine, &report_id).as_deref(),
        Some(REPORT_STATE_CANCELLED),
        "a cancelled generation must not complete later"
    );

    // Rows differ in the status the report has reached: already cancelled, and complete.
    let (status, body) = cancel(&router, &report_id, &[]).await;
    assert_eq!(
        status, 400,
        "a second cancel: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "a second cancel: expected IncorrectState, got {body}"
    );

    let completed = started(&router, "a completed report", TARGETS[1].1).await;
    past_the_generation().await;
    let (status, body) = cancel(&router, &completed, &[]).await;
    assert_eq!(
        status, 400,
        "a completed report: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "IncorrectState",
        "a completed report: expected IncorrectState, got {body}"
    );
    assert_eq!(
        stored_status(&engine, &completed).as_deref(),
        Some(REPORT_STATE_COMPLETE),
        "a refused cancel must leave the report where it was"
    );
}

/// RULE 5: every report id the service cannot resolve is refused — the summary reports every
/// one of them not found, and the cancel tells a value that is no report id (malformed) from
/// one inside the form that names no report (not found).
#[tokio::test]
async fn an_id_the_service_cannot_resolve_is_refused() {
    let (router, _engine) = emulator();

    // The rows differ in the FORM of the id: values carrying the prefix every report id has
    // (looked up, and no report started under them), then every way a value can fail to carry
    // it — another resource's prefix, the bare prefix with nothing after it, and something
    // that is not an id at all. The third column is what the CANCEL answers; the summary
    // answers the not-found half for every row, which is its own recorded contract.
    let rows: [(&str, &str, &str); 6] = [
        (
            "a well-formed id no report was started under",
            "report-0123456789abcdef0",
            "InvalidDeclarativePoliciesReportId.NotFound",
        ),
        (
            "the null id",
            "report-00000000000000000",
            "InvalidDeclarativePoliciesReportId.NotFound",
        ),
        (
            "a short suffix",
            "report-12345678",
            "InvalidDeclarativePoliciesReportId.NotFound",
        ),
        (
            "another resource's prefix",
            "cr-00000000000000001",
            "InvalidDeclarativePoliciesReportId.Malformed",
        ),
        (
            "the prefix with nothing after it",
            "report-",
            "InvalidDeclarativePoliciesReportId.Malformed",
        ),
        (
            "not an id at all",
            "not-a-report-id",
            "InvalidDeclarativePoliciesReportId.Malformed",
        ),
    ];

    for (case, report_id, cancel_code) in rows {
        for (action, expected, answered) in [
            (
                "GetDeclarativePoliciesReportSummary",
                "InvalidDeclarativePoliciesReportId.NotFound",
                summary(&router, report_id, &[]).await,
            ),
            (
                "CancelDeclarativePoliciesReport",
                cancel_code,
                cancel(&router, report_id, &[]).await,
            ),
        ] {
            let (status, body) = answered;
            assert_eq!(
                status, 400,
                "case {case:?} on {action}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected,
                "case {case:?} on {action}: expected {expected}, got {body}"
            );
            assert!(
                error_message(&body).contains(report_id),
                "case {case:?} on {action}: expected the message to name the id, got {body}"
            );
        }
    }
}

/// RULE 6: the bucket the report is saved to and the target it covers are read against the
/// forms their own members publish.
#[tokio::test]
async fn the_request_values_are_read_against_their_published_forms() {
    let (router, engine) = emulator();

    // Rows differ in which member is wrong and how: a bucket too short, one too long, one
    // carrying characters a bucket name cannot, one beginning with a separator, one formatted
    // as an IP address, one carrying a reserved prefix, and a target outside all three
    // published forms. A bucket name outside Amazon S3's published rules is the answer every
    // operation of this crate that takes one shares (`InvalidRequest`), and a target outside
    // its three forms is EC2's `InvalidParameterValue`.
    let too_long = "b".repeat(64);
    let rows: [(&str, Vec<Param>, &str); 9] = [
        (
            "a bucket name of two characters",
            vec![param("S3Bucket", "ab"), param("TargetId", TARGETS[0].1)],
            "InvalidRequest",
        ),
        (
            "a bucket name of 64 characters",
            vec![param("S3Bucket", &too_long), param("TargetId", TARGETS[0].1)],
            "InvalidRequest",
        ),
        (
            "a bucket name in capitals",
            vec![param("S3Bucket", "Reports"), param("TargetId", TARGETS[0].1)],
            "InvalidRequest",
        ),
        (
            "a bucket name beginning with a hyphen",
            vec![param("S3Bucket", "-reports"), param("TargetId", TARGETS[0].1)],
            "InvalidRequest",
        ),
        (
            "a bucket name formatted as an IP address",
            vec![param("S3Bucket", "192.168.5.4"), param("TargetId", TARGETS[0].1)],
            "InvalidRequest",
        ),
        (
            "a bucket name carrying a reserved prefix",
            vec![param("S3Bucket", "xn--reports"), param("TargetId", TARGETS[0].1)],
            "InvalidRequest",
        ),
        (
            "a target that is no id at all",
            vec![param("S3Bucket", BUCKET), param("TargetId", "everything")],
            "InvalidParameterValue",
        ),
        (
            "an account id of eleven digits",
            vec![param("S3Bucket", BUCKET), param("TargetId", "12345678901")],
            "InvalidParameterValue",
        ),
        (
            "an organizational unit without its own digits",
            vec![param("S3Bucket", BUCKET), param("TargetId", "ou-ab12")],
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected) in rows {
        let (status, body) = call(
            &router,
            REGION,
            "StartDeclarativePoliciesReport",
            &params,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        0,
        "a refused start must store no report"
    );

    // A bucket name AT each published bound is accepted: three characters and 63.
    for bucket in ["abc".to_string(), "b".repeat(63)] {
        let (router, engine) = emulator();
        let case = format!("a bucket name of {} characters", bucket.len());
        let (status, body) = call(
            &router,
            REGION,
            "StartDeclarativePoliciesReport",
            &[param("S3Bucket", &bucket), param("TargetId", TARGETS[2].1)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            stored_count(&engine),
            1,
            "case {case:?}: the report is held"
        );
    }
}

/// RULE 7: `DryRun` answers the permission probe on each of the three operations, and
/// starts, summarises and cancels nothing.
#[tokio::test]
async fn a_dry_run_answers_the_probe_and_changes_nothing() {
    // A start probe is answered the same whether or not the organization has a report
    // running: the probe asks about the CALLER's permissions, so it is answered before any
    // state is read.
    let (fresh, fresh_engine) = emulator();
    let (status, body) = start(&fresh, TARGETS[1].1, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a start that would have been served: expected HTTP 412, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a start that would have been served: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored_count(&fresh_engine),
        0,
        "a probe must start no report"
    );

    let (router, engine) = emulator();
    let report_id = started(&router, "a running report", TARGETS[0].1).await;

    // Rows differ in the operation probed and in what the probe's id names: a report the
    // organization holds, a well-formed id it does not, and a value that is no report id at
    // all. "Checks whether you have the required permissions … without actually making the
    // request", so every row answers the same thing.
    let unknown = "report-0123456789abcdef0";
    let not_an_id = "not-a-report-id";
    let rows: [(&str, Vec<Param>); 7] = [
        (
            "a start while another report is running",
            vec![
                param("S3Bucket", BUCKET),
                param("TargetId", TARGETS[1].1),
                param("DryRun", "true"),
            ],
        ),
        (
            "a summary of a report the organization holds",
            vec![param("ReportId", &report_id), param("DryRun", "true")],
        ),
        (
            "a summary of an id it does not hold",
            vec![param("ReportId", unknown), param("DryRun", "true")],
        ),
        (
            "a summary of a value that is no id",
            vec![param("ReportId", not_an_id), param("DryRun", "true")],
        ),
        (
            "a cancel of a report the organization holds",
            vec![param("ReportId", &report_id), param("DryRun", "true")],
        ),
        (
            "a cancel of an id it does not hold",
            vec![param("ReportId", unknown), param("DryRun", "true")],
        ),
        (
            "a cancel of a value that is no id",
            vec![param("ReportId", not_an_id), param("DryRun", "true")],
        ),
    ];
    let actions = [
        "StartDeclarativePoliciesReport",
        "GetDeclarativePoliciesReportSummary",
        "GetDeclarativePoliciesReportSummary",
        "GetDeclarativePoliciesReportSummary",
        "CancelDeclarativePoliciesReport",
        "CancelDeclarativePoliciesReport",
        "CancelDeclarativePoliciesReport",
    ];

    for ((case, params), action) in rows.into_iter().zip(actions) {
        let (status, body) = call(&router, REGION, action, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun {action}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: expected the wording EC2 answers a permitted probe, got {body}"
        );
    }

    // Nothing moved: the one report the organization holds is still the running one.
    assert_eq!(
        stored_count(&engine),
        1,
        "a probe must start no report"
    );
    assert_eq!(
        stored_status(&engine, &report_id).as_deref(),
        Some(REPORT_STATE_RUNNING),
        "a probe must cancel nothing"
    );
}

/// The status each report the organization holds is REPORTED with, by report id, read through
/// `DescribeDeclarativePoliciesReports` — the reader of the whole set, so a status a cancel
/// wrote is checked where a caller would see it rather than only in the store.
async fn reported_statuses(
    router: &MultiServiceRouter,
    case: &str,
) -> std::collections::BTreeMap<String, Option<String>> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeDeclarativePoliciesReports",
        &[],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeDeclarativePoliciesReports must succeed, got {status}: {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "reportSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "reportId").unwrap_or_else(|| {
                    panic!("case {case:?}: every reported report must name its id: {body}")
                }),
                child_text(item, "status"),
            )
        })
        .collect()
}

/// RULE 4, across a SET of reports: the cancel moves exactly the report its id names, and only
/// out of `running` — so the other reports the organization holds keep the status they had,
/// and each report's word is what every later reader answers.
#[tokio::test]
async fn a_cancel_moves_only_the_report_its_id_names() {
    let (router, engine) = emulator();

    // Three reports, each of a different target form and each started over the wire: "only one
    // report per organization can be generated at a time", so each start waits for the one
    // before it to have left `running` — the first by the clock, the second by a cancel.
    let completed = started(&router, "a completed report", TARGETS[0].1).await;
    past_the_generation().await;
    let already_cancelled = started(&router, "an already cancelled report", TARGETS[1].1).await;
    let (status, body) = cancel(&router, &already_cancelled, &[]).await;
    assert_eq!(
        status, 200,
        "an already cancelled report: expected HTTP 200 from the first cancel, got {status}: {body}"
    );
    let running = started(&router, "a running report", TARGETS[2].1).await;

    // The cancel of the running report is the ONE move under test.
    let (status, body) = cancel(&router, &running, &[]).await;
    assert_eq!(
        status, 200,
        "a running report: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        member(&body, "return").as_deref(),
        Some("true"),
        "a running report: expected the cancel to answer true, got {body}"
    );

    // Rows differ in the status the report had when that cancel ran: the one it named, one the
    // clock had completed, and one an earlier cancel had already stopped.
    let rows: [(&str, &String, &str); 3] = [
        ("the report the cancel named", &running, REPORT_STATE_CANCELLED),
        ("a report the clock completed", &completed, REPORT_STATE_COMPLETE),
        (
            "a report an earlier cancel stopped",
            &already_cancelled,
            REPORT_STATE_CANCELLED,
        ),
    ];

    let reported = reported_statuses(&router, "after the cancel").await;
    assert_eq!(
        reported.len(),
        3,
        "the organization holds all three reports, got {reported:?}"
    );
    for (case, report_id, expected) in rows {
        assert_eq!(
            stored_status(&engine, report_id).as_deref(),
            Some(expected),
            "case {case:?}: expected the stored status {expected:?} for {report_id}"
        );
        assert_eq!(
            reported.get(report_id.as_str()).and_then(Option::as_deref),
            Some(expected),
            "case {case:?}: expected the describe to report {expected:?}, got {reported:?}"
        );
        let body = summarised(&router, case, report_id).await;
        assert!(
            member(&body, "endTime")
                .as_deref()
                .and_then(as_timestamp)
                .is_some(),
            "case {case:?}: a generation that has ended reports when it did, got {body}"
        );

        // None of the three is running any more, so every one of them refuses a cancel now,
        // and the refusal names the id and the status it found.
        let (status, body) = cancel(&router, report_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 cancelling a {expected} report, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "IncorrectState",
            "case {case:?}: expected IncorrectState, got {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(report_id.as_str()) && message.contains(expected),
            "case {case:?}: expected the refusal to name {report_id} and {expected}, got {message:?}"
        );
        assert_eq!(
            stored_status(&engine, report_id).as_deref(),
            Some(expected),
            "case {case:?}: a refused cancel must leave the report where it was"
        );
    }

    // The completion the clock owed the cancelled reports never happens, and the refused
    // cancels left the completed one alone.
    past_the_generation().await;
    let reported = reported_statuses(&router, "a span after the cancel").await;
    for (case, report_id, expected) in rows {
        assert_eq!(
            reported.get(report_id.as_str()).and_then(Option::as_deref),
            Some(expected),
            "case {case:?}: expected {expected:?} a span later as well, got {reported:?}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        3,
        "cancelling removes no report: all three are still held"
    );
}
