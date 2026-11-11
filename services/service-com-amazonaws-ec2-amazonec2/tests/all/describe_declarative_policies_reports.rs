// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeDeclarativePoliciesReports` — the report of the
//! declarative-policies account status reports an organization holds — driven over the wire
//! the service actually speaks (`ec2Query`, through the crate's shared [`super::ec2_query`]
//! harness).
//!
//! The rules under test are:
//!
//! 1. IT REPORTS THE STORE: the set answered is every account status report the organization
//!    holds, in `ReportId` order, and every member the modelled `DeclarativePoliciesReport`
//!    carries — "the name of the Amazon S3 bucket where the report is located", "the prefix
//!    for your S3 object", "the root ID, organizational unit ID, or account ID", "the time
//!    when the report generation started" and "any tags assigned to the report" — round-trips
//!    out of it unchanged;
//! 2. `ReportIds` NARROWS it: "one or more report IDs" selects exactly the reports named, and
//!    a request naming none reports them all;
//! 3. THE STATUS IS THE CLOCK'S: "describes the metadata of an account status report,
//!    INCLUDING THE STATUS of the report" — a generation still running reports `running` and
//!    no `endTime`, one the clock has completed reports `complete` with the instant it ended,
//!    and a cancelled one reports `cancelled`; describing writes nothing, so two describes in
//!    a row agree and the store is left where it was;
//! 4. PAGES: `MaxResults` cuts the set into pages and `NextToken` continues "from the end of
//!    the items returned by the previous request", so a walk visits every report exactly once
//!    in `ReportId` order and the last page carries no token — "this value is null when there
//!    are no more items to return"; a token this service never issued is refused;
//! 5. THE IDS: every report id this operation cannot resolve to a report of the organization
//!    is reported `InvalidDeclarativePoliciesReportId.NotFound`, whatever its form — the half
//!    a READER of this surface answers, where the operation that acts on one named report
//!    (`CancelDeclarativePoliciesReport`) tells a value that is no report id from one that is;
//! 6. `DryRun` is the permission probe: it answers `DryRunOperation` and reports nothing.
//!
//! Each table row is a distinct input along what its rule generalises over — which target
//! form a report covers, whether it names a prefix or tags, which subset of the ids is asked
//! for, which page size cuts the set, and every way an id or a token can be wrong — so no row
//! is the specification.

use std::time::{Duration, SystemTime};

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DeclarativePoliciesReportSpec,
    REPORT_STATE_CANCELLED, REPORT_STATE_COMPLETE, REPORT_STATE_RUNNING, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, call, error_code, error_message, member};
use super::wire::{Param, child_text, document, is_long_id, param, set_items};

/// The operation under test.
const ACTION: &str = "DescribeDeclarativePoliciesReports";

/// The Region every call here is signed for; a report is a resource of the organization, so
/// the Region only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// A bucket name the rows that are not about the bucket use.
const BUCKET: &str = "declarative-policy-reports";

/// The id prefix every report id carries, as `ec2::state::DECLARATIVE_POLICIES_REPORT_ID_PREFIX`
/// spells it, with the hyphen a long EC2 id puts after it.
const REPORT_ID_PREFIX: &str = ec2::state::DECLARATIVE_POLICIES_REPORT_ID_PREFIX;

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Wait until the completion a started generation owes the clock has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_generation() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// One `DescribeDeclarativePoliciesReports` call with `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, ACTION, params).await
}

/// One describe that must be served, answering its body.
async fn described(router: &MultiServiceRouter, case: &str, params: &[Param]) -> String {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {ACTION}, got {status} with body {body}"
    );
    body
}

/// One report as the response reported it: the members of the modelled
/// `DeclarativePoliciesReport`, as the ec2Query envelope spells them.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    report_id: String,
    s3_bucket: Option<String>,
    s3_prefix: Option<String>,
    target_id: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    status: Option<String>,
    tags: Vec<(String, String)>,
}

/// Every report in the `reportSet` of a describe's response, in the order it listed them.
fn reported(case: &str, body: &str) -> Vec<Reported> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "reportSet")
        .into_iter()
        .map(|item| Reported {
            report_id: child_text(item, "reportId").unwrap_or_else(|| {
                panic!("case {case:?}: every reported report must name its id: {body}")
            }),
            s3_bucket: child_text(item, "s3Bucket"),
            s3_prefix: child_text(item, "s3Prefix"),
            target_id: child_text(item, "targetId"),
            start_time: child_text(item, "startTime"),
            end_time: child_text(item, "endTime"),
            status: child_text(item, "status"),
            tags: set_items(item, "tagSet")
                .into_iter()
                .map(|tag| {
                    (
                        child_text(tag, "key").unwrap_or_default(),
                        child_text(tag, "value").unwrap_or_default(),
                    )
                })
                .collect(),
        })
        .collect()
}

/// The ids a describe reported, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|report| report.report_id)
        .collect()
}

/// One report spec, as `StartDeclarativePoliciesReport` would have recorded it.
fn spec(
    target_id: &str,
    s3_bucket: &str,
    s3_prefix: Option<&str>,
    tags: &[(&str, &str)],
) -> DeclarativePoliciesReportSpec {
    DeclarativePoliciesReportSpec {
        s3_bucket: s3_bucket.to_string(),
        s3_prefix: s3_prefix.map(str::to_string),
        target_id: target_id.to_string(),
        tags: tags
            .iter()
            .map(|(key, value)| Tag {
                key: Some((*key).to_string()),
                value: Some((*value).to_string()),
            })
            .collect(),
    }
}

/// Record `specs` in the store through the one writer `StartDeclarativePoliciesReport`
/// itself uses, answering the ids it minted in the order it minted them.
///
/// Seeded rather than started over the wire because "only one report per organization can be
/// generated at a time": a SET of reports is what this operation reports, and the writer is
/// the same one, so what is under test is still the stored state. The round trip through the
/// start is pinned by [`the_status_reported_is_the_clocks`].
fn seed(
    engine: &engine::InMemoryEngine,
    specs: Vec<DeclarativePoliciesReportSpec>,
) -> Vec<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    specs
        .into_iter()
        .map(|spec| guard.insert_declarative_policies_report(spec, SystemTime::now()))
        .collect()
}

/// The status the store holds for `report_id` as of now — the word every reader of the
/// report derives its answer from.
fn stored_status(engine: &engine::InMemoryEngine, report_id: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .declarative_policies_reports
        .get(report_id)
        .map(|record| record.status_at(std::time::Instant::now()).to_string())
}

/// The seeded set: rows differ in which of the three target forms the report covers
/// ("For root: r-ab12; For OU: ou-ab12-cdef1234; For account: 123456789012" —
/// `StartDeclarativePoliciesReportRequest$TargetId`), whether the report names an S3 prefix,
/// which bucket it is saved to, and how many tags it carries.
fn fixtures() -> Vec<(&'static str, DeclarativePoliciesReportSpec)> {
    vec![
        (
            "a root, no prefix, no tags",
            spec("r-ab12", BUCKET, None, &[]),
        ),
        (
            "an organizational unit, its own bucket and a prefix",
            spec(
                "ou-ab12-cdef1234",
                "another-report-bucket",
                Some("reports/ec2"),
                &[("team", "platform")],
            ),
        ),
        (
            "an account, a deep prefix and two tags",
            spec(
                "123456789012",
                BUCKET,
                Some("one/two/three"),
                &[("team", "platform"), ("env", "prod")],
            ),
        ),
        (
            "a second root with one tag",
            spec("r-cd34", BUCKET, None, &[("owner", "security")]),
        ),
    ]
}

/// RULE 1 and RULE 3: the describe reports the store, in id order, with every member
/// round-tripping out of it — and describing twice changes nothing.
#[tokio::test]
async fn the_describe_reports_every_report_the_organization_holds() {
    let (router, engine) = emulator();

    // An organization that has started nothing holds nothing to report, and no page follows.
    let body = described(&router, "an empty organization", &[]).await;
    assert_eq!(
        reported("an empty organization", &body),
        Vec::new(),
        "an empty organization: expected no report at all, got {body}"
    );
    assert_eq!(
        member(&body, "nextToken"),
        None,
        "an empty organization: expected no page to follow, got {body}"
    );

    let rows = fixtures();
    let specs: Vec<DeclarativePoliciesReportSpec> =
        rows.iter().map(|(_case, spec)| spec.clone()).collect();
    let ids = seed(&engine, specs);

    let body = described(&router, "the whole set", &[]).await;
    let answered = reported("the whole set", &body);
    assert_eq!(
        answered.iter().map(|r| r.report_id.clone()).collect::<Vec<_>>(),
        ids,
        "the whole set: expected every report in ReportId order, got {body}"
    );

    for ((case, spec), report) in rows.iter().zip(&answered) {
        assert!(
            is_long_id(REPORT_ID_PREFIX, &report.report_id),
            "case {case:?}: expected a report id, got {:?}",
            report.report_id
        );
        assert_eq!(
            report.s3_bucket.as_deref(),
            Some(spec.s3_bucket.as_str()),
            "case {case:?}: expected the bucket the report is saved to, got {report:?}"
        );
        assert_eq!(
            report.s3_prefix.as_deref(),
            spec.s3_prefix.as_deref(),
            "case {case:?}: expected the prefix of its S3 object, got {report:?}"
        );
        assert_eq!(
            report.target_id.as_deref(),
            Some(spec.target_id.as_str()),
            "case {case:?}: expected the target the report covers, got {report:?}"
        );
        assert!(
            report.start_time.as_deref().and_then(as_timestamp).is_some(),
            "case {case:?}: expected when the generation started, got {report:?}"
        );
        assert_eq!(
            report.status.as_deref(),
            Some(REPORT_STATE_RUNNING),
            "case {case:?}: a generation the clock has not completed is running, got {report:?}"
        );
        assert_eq!(
            report.end_time, None,
            "case {case:?}: a running generation has not ended, got {report:?}"
        );
        let expected_tags: Vec<(String, String)> = spec
            .tags
            .iter()
            .map(|tag| {
                (
                    tag.key.clone().unwrap_or_default(),
                    tag.value.clone().unwrap_or_default(),
                )
            })
            .collect();
        assert_eq!(
            report.tags, expected_tags,
            "case {case:?}: expected the tags assigned to the report, got {report:?}"
        );
    }

    // RULE 3: the report is a READ — a second describe answers exactly the same set, and the
    // store is where it was.
    let again = described(&router, "a second describe", &[]).await;
    assert_eq!(
        reported("a second describe", &again),
        answered,
        "a second describe must answer the same set, got {again}"
    );
    for report_id in &ids {
        assert_eq!(
            stored_status(&engine, report_id).as_deref(),
            Some(REPORT_STATE_RUNNING),
            "describing must leave {report_id} where it was"
        );
    }
}

/// RULE 2: "one or more report IDs" selects exactly the reports named.
#[tokio::test]
async fn report_ids_narrow_the_set() {
    let (router, engine) = emulator();
    let ids = seed(
        &engine,
        fixtures().into_iter().map(|(_case, spec)| spec).collect(),
    );

    // Rows differ in HOW MANY of the held ids the request names and in which: none at all,
    // one, two that are not adjacent, the whole set out of order, and the same id twice.
    let rows: Vec<(&str, Vec<usize>, Vec<usize>)> = vec![
        ("no id at all", vec![], vec![0, 1, 2, 3]),
        ("one id", vec![2], vec![2]),
        ("two ids that are not adjacent", vec![3, 1], vec![1, 3]),
        ("every id, out of order", vec![2, 0, 3, 1], vec![0, 1, 2, 3]),
        ("the same id twice", vec![1, 1], vec![1]),
    ];

    for (case, asked, expected) in rows {
        let params: Vec<Param> = asked
            .iter()
            .enumerate()
            .map(|(position, index)| {
                param(&format!("ReportId.{}", position + 1), &ids[*index])
            })
            .collect();
        let body = described(&router, case, &params).await;
        let expected_ids: Vec<String> = expected.iter().map(|index| ids[*index].clone()).collect();
        assert_eq!(
            reported_ids(case, &body),
            expected_ids,
            "case {case:?}: expected exactly the reports named, in id order, got {body}"
        );
    }
}

/// RULE 3: the status the describe reports is the one the clock has left the generation in —
/// running, then complete with the instant it ended; a cancelled generation stays cancelled.
#[tokio::test]
async fn the_status_reported_is_the_clocks() {
    let (router, engine) = emulator();

    // ROUND TRIP: the report this describe answers with is the one the start recorded.
    let (status, body) = call(
        &router,
        REGION,
        "StartDeclarativePoliciesReport",
        &[param("S3Bucket", BUCKET), param("TargetId", "r-ab12")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the start must succeed, got {status} with body {body}"
    );
    let cancelled_id = member(&body, "reportId")
        .unwrap_or_else(|| panic!("the start must answer a reportId: {body}"));

    let body = described(&router, "a running generation", &[]).await;
    let running = reported("a running generation", &body);
    assert_eq!(
        running.len(),
        1,
        "a running generation: expected the one report the start recorded, got {body}"
    );
    assert_eq!(
        running[0].report_id, cancelled_id,
        "a running generation: expected the report the start minted, got {body}"
    );
    assert_eq!(
        running[0].status.as_deref(),
        Some(REPORT_STATE_RUNNING),
        "a running generation: expected {REPORT_STATE_RUNNING}, got {body}"
    );

    // A cancelled generation ended when it was cancelled, and never completes afterwards.
    let (status, body) = call(
        &router,
        REGION,
        "CancelDeclarativePoliciesReport",
        &[param("ReportId", &cancelled_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the cancel must succeed, got {status} with body {body}"
    );

    // A second report, started once the first one is no longer running, is the one the clock
    // completes.
    let (status, body) = call(
        &router,
        REGION,
        "StartDeclarativePoliciesReport",
        &[
            param("S3Bucket", BUCKET),
            param("TargetId", "123456789012"),
            param("S3Prefix", "reports/ec2"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the second start must succeed, got {status} with body {body}"
    );
    let completed_id = member(&body, "reportId")
        .unwrap_or_else(|| panic!("the second start must answer a reportId: {body}"));

    past_the_generation().await;
    let body = described(&router, "after the generation span", &[]).await;
    let answered = reported("after the generation span", &body);
    assert_eq!(
        answered.len(),
        2,
        "after the generation span: expected both reports, got {body}"
    );

    // Rows differ in the status the report reached and in how it got there.
    for (case, report_id, expected) in [
        (
            "a cancelled generation",
            &cancelled_id,
            REPORT_STATE_CANCELLED,
        ),
        (
            "a generation the clock completed",
            &completed_id,
            REPORT_STATE_COMPLETE,
        ),
    ] {
        let report = answered
            .iter()
            .find(|report| &report.report_id == report_id)
            .unwrap_or_else(|| panic!("case {case:?}: {report_id} must be reported: {body}"));
        assert_eq!(
            report.status.as_deref(),
            Some(expected),
            "case {case:?}: expected {expected}, got {report:?}"
        );
        let started = report
            .start_time
            .as_deref()
            .and_then(as_timestamp)
            .unwrap_or_else(|| panic!("case {case:?}: expected a startTime, got {report:?}"));
        let ended = report
            .end_time
            .as_deref()
            .and_then(as_timestamp)
            .unwrap_or_else(|| panic!("case {case:?}: expected an endTime, got {report:?}"));
        assert!(
            ended >= started,
            "case {case:?}: a generation cannot end before it started, got {report:?}"
        );
        assert_eq!(
            stored_status(&engine, report_id).as_deref(),
            Some(expected),
            "case {case:?}: describing must leave {report_id} where it was"
        );
    }
}

/// RULE 4: `MaxResults` cuts the set into pages and `NextToken` walks it exactly once.
#[tokio::test]
async fn pages_walk_the_whole_set_exactly_once() {
    let (router, engine) = emulator();
    // Seven reports: enough that a page size divides the set evenly, unevenly, and not at
    // all. Each row of the fixtures is reused so the set is not uniform.
    let mut specs: Vec<DeclarativePoliciesReportSpec> =
        fixtures().into_iter().map(|(_case, spec)| spec).collect();
    specs.extend(
        fixtures()
            .into_iter()
            .take(3)
            .map(|(_case, spec)| spec),
    );
    let ids = seed(&engine, specs);
    assert_eq!(ids.len(), 7, "the walk is over seven reports");

    // Rows differ in the page size: the published minimum, sizes that do and do not divide
    // the set, its exact size, one above it, and the published maximum.
    for size in [1usize, 2, 3, 7, 8, 1000] {
        let case = format!("a page size of {size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = vec![param("MaxResults", &size.to_string())];
            if let Some(token) = &token {
                params.push(param("NextToken", token));
            }
            let body = described(&router, &case, &params).await;
            let page = reported_ids(&case, &body);
            assert!(
                page.len() <= size,
                "case {case:?}: a page must hold at most {size} reports, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case {case:?}: the walk must end, got {pages} pages"
            );
            match member(&body, "nextToken") {
                Some(next) if !next.is_empty() => token = Some(next),
                _ => break,
            }
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: the walk must visit every report exactly once, in id order"
        );
        let expected_pages = ids.len().div_ceil(size.min(ids.len()).max(1));
        assert_eq!(
            pages,
            expected_pages.max(1),
            "case {case:?}: expected {} pages for a page size of {size}",
            expected_pages.max(1)
        );
    }

    // A page size outside the published range (`DeclarativePoliciesMaxResults`, range
    // 1..1000) is refused rather than clamped.
    for size in ["0", "1001"] {
        let case = format!("a page size of {size}");
        let (status, body) = describe(&router, &[param("MaxResults", size)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a page size outside the published range, \
             got {status} with body {body}"
        );
        assert!(
            !error_code(&body).is_empty(),
            "case {case:?}: expected an error code, got {body}"
        );
    }

    // "The token returned from a previous paginated request": a value this service never
    // issued is refused rather than read as a position. Rows differ in how the value fails —
    // a report id sent as a token, that id hex-encoded without the envelope the codec writes,
    // something that is not hex at all, hex that decodes to no cursor, and a token of this
    // service with its tail cut off.
    let served = described(&router, "a first page", &[param("MaxResults", "1")]).await;
    let real = member(&served, "nextToken")
        .unwrap_or_else(|| panic!("a first page must carry a nextToken: {served}"));
    let truncated: String = real.chars().take(real.chars().count() - 2).collect();
    for (case, token) in [
        ("a report id", ids[0].clone()),
        ("a hex-encoded report id", ::hex::encode(&ids[0])),
        ("not a token at all", "zzzz".to_string()),
        ("hex carrying no cursor", "deadbeef".to_string()),
        ("a truncated token", truncated),
    ] {
        let (status, body) = describe(&router, &[param("NextToken", &token)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a token the service never issued, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

/// RULE 5: every report id the service cannot resolve is refused NOT FOUND, whatever its
/// form.
#[tokio::test]
async fn an_id_the_service_cannot_resolve_is_refused() {
    let (router, engine) = emulator();
    let ids = seed(
        &engine,
        fixtures().into_iter().map(|(_case, spec)| spec).collect(),
    );

    // Rows differ in the FORM of the id asked for: values carrying the prefix every report id
    // this service mints has but naming no report of this organization, then every way a
    // value can fail to carry it — another id space's prefix, a prefix with nothing after it,
    // and something that is not an id at all. Two rows mix a report the organization DOES hold
    // with an unresolvable one, so a refusal is never masked by a served row, and the last row
    // names two it cannot resolve, so the refusal is the FIRST one the request listed rather
    // than whichever sorts lowest.
    let held = ids[0].clone();
    let unresolvable: Vec<(&str, Vec<String>)> = vec![
        (
            "a well-formed id no report was started under",
            vec!["report-0123456789abcdef0".to_string()],
        ),
        ("the null id", vec!["report-00000000000000000".to_string()]),
        ("a short suffix", vec!["report-12345678".to_string()]),
        (
            "another id space's prefix",
            vec!["dpr-00000000000000000".to_string()],
        ),
        (
            "the prefix with nothing after it",
            vec!["report-".to_string()],
        ),
        ("not an id at all", vec!["not-a-report-id".to_string()]),
        (
            "a held id beside one no report was started under",
            vec![held.clone(), "report-0123456789abcdef0".to_string()],
        ),
        (
            "a held id beside a value that is no id",
            vec![held.clone(), "not-a-report-id".to_string()],
        ),
        (
            "two ids it cannot resolve, the first not the lowest",
            vec![
                "report-fffffffffffffffff".to_string(),
                "report-00000000000000000".to_string(),
            ],
        ),
    ];

    for (case, asked) in unresolvable {
        let params: Vec<Param> = asked
            .iter()
            .enumerate()
            .map(|(position, report_id)| {
                param(&format!("ReportId.{}", position + 1), report_id)
            })
            .collect();
        let (status, body) = describe(&router, &params).await;
        let expected = "InvalidDeclarativePoliciesReportId.NotFound";
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        let named = asked
            .iter()
            .find(|report_id| *report_id != &held)
            .expect("every row names an id the service cannot resolve");
        // The message is part of the contract: the sentence AWS words this refusal with,
        // with the id taken from the REQUEST.
        assert_eq!(
            error_message(&body),
            format!("The report ID '{named}' does not exist."),
            "case {case:?}: expected the sentence this refusal is worded with, got {body}"
        );
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a refused describe reports nothing, got {body}"
        );
    }

    // A refused describe reported nothing, and the store is untouched.
    let body = described(&router, "after the refusals", &[]).await;
    assert_eq!(
        reported_ids("after the refusals", &body),
        ids,
        "after the refusals: the organization still holds every report, got {body}"
    );
}

/// RULE 6: `DryRun` answers the permission probe and reports nothing.
#[tokio::test]
async fn a_dry_run_answers_the_probe_and_reports_nothing() {
    let (router, engine) = emulator();
    let ids = seed(
        &engine,
        fixtures().into_iter().map(|(_case, spec)| spec).collect(),
    );

    // Rows differ in what the probe would have asked for: the whole set, one report the
    // organization holds, an id it does not, a value that is no report id, a page size, and a
    // token the service never issued. "Checks whether you have the required permissions for
    // the action, WITHOUT ACTUALLY MAKING THE REQUEST", so every row answers the same thing.
    let rows: Vec<(&str, Vec<Param>)> = vec![
        ("the whole set", vec![]),
        (
            "a report the organization holds",
            vec![param("ReportId.1", &ids[1])],
        ),
        (
            "an id it does not hold",
            vec![param("ReportId.1", "report-0123456789abcdef0")],
        ),
        (
            "a value that is no report id",
            vec![param("ReportId.1", "not-a-report-id")],
        ),
        ("a page of one", vec![param("MaxResults", "1")]),
        (
            "a token the service never issued",
            vec![param("NextToken", "next-page-please")],
        ),
    ];

    for (case, mut params) in rows {
        params.push(param("DryRun", "true"));
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from a DryRun {ACTION}, got {status}: {body}"
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
        assert!(
            reported(case, &body).is_empty(),
            "case {case:?}: a probe must report no report at all, got {body}"
        );
    }

    // Nothing moved: every report is still where it was.
    for report_id in &ids {
        assert_eq!(
            stored_status(&engine, report_id).as_deref(),
            Some(REPORT_STATE_RUNNING),
            "a probe must leave {report_id} where it was"
        );
    }
}
