// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DescribeDomainAutoTunes`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`]; this suite adds no second copy of them.
//!
//! The rule under test: the Auto-Tune report of a domain is the set of scheduled
//! changes of that domain whose action type is an Auto-Tune one, each as a
//! `SCHEDULED_ACTION` `AutoTune` carrying the stored instant, action type,
//! description and severity, ordered by the instant it is scheduled for, in pages of
//! at most `MaxResults` resumed by the token the previous page handed out; a name the
//! account holds no domain under has no report, and a token naming no Auto-Tune
//! adjustment of that domain is refused rather than answered with a page.
//!
//! So the assertions span a SET of inputs rather than one call: a table of scheduled
//! changes covering BOTH `ScheduledAutoTuneActionType` values and every
//! `ScheduledAutoTuneSeverityType` value, a description and none, instants behind and
//! ahead of the call, two adjustments scheduled for the SAME instant, and changes of
//! the action type that is NOT an Auto-Tune one; then five page sizes and the two ways
//! to ask for no bound, the published bound and values past it at either end, four
//! shapes of unresolvable token, and four names no domain answers to.
//!
//! The scheduled changes themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]), exactly as
//! [`super::scheduled_actions`] does: no wire call MINTS a scheduled change yet — the
//! operations that schedule one are still scaffold stubs, and `UpdateScheduledAction`
//! only re-times a change that is already there. Everything the rule is asserted ON is
//! read back through the wire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    ACTION_SEVERITY_VALUES, ACTION_TYPE_VALUES, AUTO_TUNE_TYPE_VALUES,
    SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES, SCHEDULED_AUTO_TUNE_SEVERITY_TYPE_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::ScheduledChange;

use super::domain_lifecycle::{
    create_baseline, error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// One row of the scheduled-change table: the action id, its type, its severity, how
/// many seconds from the base instant it is scheduled for, and its description.
struct Row {
    id: &'static str,
    action_type: &'static str,
    severity: &'static str,
    offset: i64,
    description: Option<&'static str>,
}

impl Row {
    /// Whether this row is an Auto-Tune adjustment — the rows the report must hold —
    /// decided from the model's own `ScheduledAutoTuneActionType` catalog rather than
    /// from a second list of its values.
    fn is_auto_tune(&self) -> bool {
        SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES.contains(&self.action_type)
    }

    /// How this row is recognised in a page: the report carries no action id, so an
    /// entry is named by the pair that identifies it on the wire — its action type and
    /// the instant it is scheduled for.
    fn mark(&self, base: SystemTime) -> String {
        format!("{}@{}", self.action_type, expected_date(self, base))
    }
}

/// The scheduled changes every case below is asserted over.
///
/// The rows differ along what the rule generalises over: the kind of change — both
/// `ScheduledAutoTuneActionType` values and the `ActionType` value that is no
/// Auto-Tune adjustment at all — its severity (every `ScheduledAutoTuneSeverityType`
/// value between them), whether it carries a description, and whether its instant is
/// behind or ahead of the call. `heap-tie` and `young-tie` fall on the SAME instant,
/// so the order of two adjustments that fall together is exercised too.
fn rows() -> Vec<Row> {
    vec![
        Row {
            id: "heap-future",
            action_type: "JVM_HEAP_SIZE_TUNING",
            severity: "HIGH",
            offset: 86_400,
            description: Some("Auto-Tune change to the JVM heap size"),
        },
        Row {
            id: "young-tie",
            action_type: "JVM_YOUNG_GEN_TUNING",
            severity: "MEDIUM",
            offset: 3_600,
            description: None,
        },
        Row {
            id: "heap-tie",
            action_type: "JVM_HEAP_SIZE_TUNING",
            severity: "LOW",
            offset: 3_600,
            description: Some("Auto-Tune change scheduled for the same window"),
        },
        Row {
            id: "young-past",
            action_type: "JVM_YOUNG_GEN_TUNING",
            severity: "LOW",
            offset: -7_200,
            description: None,
        },
        Row {
            id: "heap-applied",
            action_type: "JVM_HEAP_SIZE_TUNING",
            severity: "MEDIUM",
            offset: -600,
            description: Some("Auto-Tune change to the JVM young generation size"),
        },
        Row {
            id: "ssu-pending",
            action_type: "SERVICE_SOFTWARE_UPDATE",
            severity: "HIGH",
            offset: 60,
            description: Some("Service software update to a newer release"),
        },
        Row {
            id: "ssu-done",
            action_type: "SERVICE_SOFTWARE_UPDATE",
            severity: "LOW",
            offset: -3_600,
            description: None,
        },
    ]
}

/// The ids of the Auto-Tune rows of [`rows`] in the order the report must hold them:
/// by the instant each adjustment is scheduled for, earliest first, the action id
/// breaking a tie. The `SERVICE_SOFTWARE_UPDATE` rows are not Auto-Tune adjustments,
/// so they are absent however they are ordered.
const EXPECTED_ORDER: [&str; 5] = [
    "young-past",
    "heap-applied",
    "heap-tie",
    "young-tie",
    "heap-future",
];

/// The instant the offsets of [`rows`] are measured from, rounded down to the second
/// so the epoch-second count a case computes for itself is exact.
fn base_instant() -> SystemTime {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch");
    UNIX_EPOCH + Duration::from_secs(since.as_secs())
}

/// `base` as an epoch-second count, computed here rather than taken from the emulator,
/// so a case asserts the UNIT `AutoTuneDate` is carried in and not just its value.
fn base_seconds(base: SystemTime) -> i64 {
    i64::try_from(
        base.duration_since(UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_secs(),
    )
    .expect("an epoch-second count of this era fits a Long")
}

/// The epoch-second count `row`'s adjustment is scheduled for.
fn expected_date(row: &Row, base: SystemTime) -> i64 {
    base_seconds(base) + row.offset
}

/// `row` as the record the store holds, scheduled `row.offset` seconds from `base`.
fn stored(row: &Row, base: SystemTime) -> ScheduledChange {
    let scheduled_at = if row.offset < 0 {
        base - Duration::from_secs(row.offset.unsigned_abs())
    } else {
        base + Duration::from_secs(row.offset.unsigned_abs())
    };
    ScheduledChange {
        id: row.id.to_string(),
        action_type: row.action_type.to_string(),
        severity: row.severity.to_string(),
        scheduled_at,
        description: row.description.map(str::to_owned),
        // Auto-Tune schedules its own adjustments, and the lifecycle and flags of a
        // change are reported by the scheduled-action listing rather than by this
        // report, so one value of each is enough here.
        scheduled_by: "SYSTEM".to_string(),
        status: "ELIGIBLE".to_string(),
        mandatory: false,
        cancellable: true,
    }
}

/// Put the whole [`rows`] table into `name`'s store entry, measured from `base`.
fn seed(emulator: &Emulator, name: &str, base: SystemTime) {
    let store = emulator.store();
    let mut guard = store.lock();
    let domain = guard
        .domains
        .get_mut(name)
        .unwrap_or_else(|| panic!("case seed({name:?}): the domain must have been created"));
    for row in rows() {
        let record = stored(&row, base);
        domain.scheduled_actions.insert(record.id.clone(), record);
    }
}

/// `GET /2021-01-01/opensearch/domain/{name}/autoTunes[?query]`.
async fn describe(emulator: &Emulator, name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/domain/{name}/autoTunes")
    } else {
        format!("{PREFIX}/opensearch/domain/{name}/autoTunes?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// `GET /2021-01-01/opensearch/domain/{name}/scheduledActions` — the listing the
/// Auto-Tune report must leave untouched.
async fn list_scheduled(emulator: &Emulator, name: &str) -> Answer {
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/opensearch/domain/{name}/scheduledActions"),
            None,
        )
        .await
}

/// The `AutoTunes` of an accepted report.
fn auto_tunes(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["AutoTunes"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an AutoTunes list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `ScheduledAutoTuneDetails` container of one reported adjustment.
fn details(entry: &Value) -> &Value {
    &entry["AutoTuneDetails"]["ScheduledAutoTuneDetails"]
}

/// The mark ([`Row::mark`]) of every adjustment of a page, in the order the page
/// reports them.
fn marks(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|entry| {
            let detail = details(entry);
            format!(
                "{}@{}",
                detail["ActionType"].as_str().unwrap_or_default(),
                detail["Date"].as_f64().unwrap_or_default().trunc() as i64
            )
        })
        .collect()
}

/// The marks of [`EXPECTED_ORDER`], which is what a whole report must concatenate to.
fn expected_marks(base: SystemTime) -> Vec<String> {
    EXPECTED_ORDER
        .iter()
        .map(|id| {
            rows()
                .into_iter()
                .find(|row| row.id == *id)
                .unwrap_or_else(|| panic!("the expected order names only rows of the table: {id}"))
                .mark(base)
        })
        .collect()
}

/// A domain with the whole table scheduled on it, and the base instant its offsets
/// were measured from.
async fn seeded(emulator: &Emulator, name: &str) -> SystemTime {
    create_baseline(emulator, name).await;
    let base = base_instant();
    seed(emulator, name, base);
    base
}

#[tokio::test]
async fn the_report_holds_every_auto_tune_adjustment_of_the_domain_as_its_stored_record() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "tune-report").await;
    let before = list_scheduled(&emulator, "tune-report").await;

    let answer = describe(&emulator, "tune-report", "").await;
    let page = auto_tunes(&answer, "whole report");
    assert_eq!(
        marks(&page),
        expected_marks(base),
        "case whole report: expected the Auto-Tune adjustments in the order they fall due, got \
         {:?}",
        marks(&page)
    );

    for row in rows() {
        let case = format!("AutoTune({:?})", row.id);
        // The catalogs these rows draw on are the model's own enums, as generated into
        // `ops::types`: a row naming a value outside one would assert nothing about
        // this service's wire surface.
        assert!(
            ACTION_TYPE_VALUES.contains(&row.action_type)
                && ACTION_SEVERITY_VALUES.contains(&row.severity),
            "case {case}: the row must use modelled values, got {:?}/{:?}",
            row.action_type,
            row.severity
        );

        let reported = page
            .iter()
            .find(|entry| marks(std::slice::from_ref(entry))[0] == row.mark(base));
        if !row.is_auto_tune() {
            // A change of an action type outside `ScheduledAutoTuneActionType` has no
            // `AutoTune` form, so the report holds none for it even though the domain
            // holds the change.
            assert!(
                reported.is_none(),
                "case {case}: a change that is no Auto-Tune adjustment must not be reported, got \
                 {reported:?}"
            );
            continue;
        }

        let reported =
            reported.unwrap_or_else(|| panic!("case {case}: expected the adjustment to be reported"));
        assert_eq!(
            reported["AutoTuneType"].as_str(),
            Some("SCHEDULED_ACTION"),
            "case {case}: expected the scheduled-action Auto-Tune type, got {:?}",
            reported["AutoTuneType"]
        );
        assert!(
            AUTO_TUNE_TYPE_VALUES.contains(&reported["AutoTuneType"].as_str().unwrap_or_default()),
            "case {case}: the reported type must be a modelled AutoTuneType, got {:?}",
            reported["AutoTuneType"]
        );
        let detail = details(reported);
        assert_eq!(
            detail["ActionType"].as_str(),
            Some(row.action_type),
            "case {case}: expected the stored action type, got {:?}",
            detail["ActionType"]
        );
        assert_eq!(
            detail["Date"].as_f64(),
            Some(expected_date(&row, base) as f64),
            "case {case}: expected the instant it is scheduled for as an epoch-second count, got \
             {:?}",
            detail["Date"]
        );
        assert_eq!(
            detail["Action"].as_str(),
            row.description,
            "case {case}: expected the stored description, got {:?}",
            detail["Action"]
        );
        assert_eq!(
            detail["Severity"].as_str(),
            Some(row.severity),
            "case {case}: expected the stored severity, got {:?}",
            detail["Severity"]
        );
        assert!(
            SCHEDULED_AUTO_TUNE_SEVERITY_TYPE_VALUES.contains(&row.severity),
            "case {case}: the reported severity must be a modelled ScheduledAutoTuneSeverityType, \
             got {:?}",
            row.severity
        );
    }

    // Between them the Auto-Tune rows use every value of both Auto-Tune catalogs, so
    // the assertions above cover the whole surface rather than one entry of it.
    for action_type in SCHEDULED_AUTO_TUNE_ACTION_TYPE_VALUES {
        assert!(
            rows().iter().any(|row| row.action_type == *action_type),
            "case catalog coverage: the table must exercise {action_type}"
        );
    }
    for severity in SCHEDULED_AUTO_TUNE_SEVERITY_TYPE_VALUES {
        assert!(
            rows()
                .iter()
                .any(|row| row.is_auto_tune() && row.severity == *severity),
            "case catalog coverage: the table must exercise severity {severity}"
        );
    }

    // The report is a read: a second one answers exactly the same, and the scheduled
    // changes it was derived from are still listed as they were.
    let again = describe(&emulator, "tune-report", "").await;
    assert_eq!(
        again.body, answer.body,
        "case whole report(twice): a read must leave the adjustments as they were, got {:?}",
        again.body
    );
    let after = list_scheduled(&emulator, "tune-report").await;
    assert_eq!(
        after.body, before.body,
        "case whole report(scheduled actions): reading the Auto-Tune report must change no \
         scheduled change, got {:?}",
        after.body
    );
}

#[tokio::test]
async fn pages_partition_the_report_of_a_domain() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "tune-paging").await;
    let whole = expected_marks(base);

    // One row per page size: the smallest, two that do not divide the report, the
    // report's own size and the largest the modelled range allows.
    for page_size in [1_i32, 2, 3, 5, 100] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
            };
            let answer = describe(&emulator, "tune-paging", &query).await;
            let page = auto_tunes(&answer, &case);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case}: a page holds at most MaxResults adjustments, got {}",
                page.len()
            );
            assert!(
                !page.is_empty(),
                "case {case}: a page the service issued a token for reports adjustments, got none"
            );
            walked.extend(marks(&page));
            pages += 1;
            token = answer.body["NextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= whole.len(),
                "case {case}: paging kept issuing tokens past the end of the report"
            );
        }
        assert_eq!(
            walked, whole,
            "case {case}: expected the pages to concatenate back to the whole report in order, \
             got {walked:?} over {pages} pages"
        );
        let expected_pages = whole
            .len()
            .div_ceil(usize::try_from(page_size).unwrap_or(1));
        assert_eq!(
            pages, expected_pages,
            "case {case}: expected {expected_pages} pages, got {pages}"
        );
    }

    // `MaxResults` is `@default(0)`, so a request that asks for no bound gets the whole
    // report in one page, with no token to resume from.
    for unbounded in ["", "maxResults=0"] {
        let case = format!("unbounded {unbounded:?}");
        let answer = describe(&emulator, "tune-paging", unbounded).await;
        assert_eq!(
            marks(&auto_tunes(&answer, &case)),
            whole,
            "case {case}: expected the whole report in one page"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no NextToken, got {:?}",
            answer.body["NextToken"]
        );
    }

    // The published bound is accepted; a page size past it at either end is refused.
    let at_bound = describe(&emulator, "tune-paging", "maxResults=100").await;
    assert_eq!(
        at_bound.status, 200,
        "case maxResults=100: the published bound is accepted, got {} {:?}",
        at_bound.status, at_bound.body
    );
    for out_of_bound in ["maxResults=101", "maxResults=1000", "maxResults=-1"] {
        let answer = describe(&emulator, "tune-paging", out_of_bound).await;
        assert_eq!(
            answer.status, 400,
            "case {out_of_bound}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {out_of_bound}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_token_that_names_no_auto_tune_adjustment_of_the_domain_is_refused() {
    let emulator = Emulator::new();
    seeded(&emulator, "tune-token").await;
    create_baseline(&emulator, "tune-other").await;

    // A token is only ever a value this operation handed out: one invented, one
    // belonging to the report of a domain it was not issued for, one naming a change
    // of this domain that is NO Auto-Tune adjustment, and one shaped like an
    // identifier.
    for (name, token, label) in [
        ("tune-token", "not-a-token", "an invented token"),
        ("tune-other", "heap-tie", "a token from another report"),
        (
            "tune-token",
            "ssu-pending",
            "a token naming a change outside the report",
        ),
        (
            "tune-token",
            "00000000-0000-0000-0000-000000000000",
            "a token shaped like an identifier",
        ),
    ] {
        let case = format!("{label} ({token:?})");
        let answer = describe(&emulator, name, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_domain_the_account_does_not_hold_has_no_report() {
    let emulator = Emulator::new();
    seeded(&emulator, "tune-present").await;
    create_baseline(&emulator, "tune-deleted").await;
    let deleted = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/tune-deleted"),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(tune-deleted): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // Rows differ along the shape of the name: one never created, one at the shortest
    // the model admits, one at its longest, and one the account held until it was
    // deleted.
    for name in [
        "never-created",
        "abc",
        "a-domain-name-at-its-upper-b",
        "tune-deleted",
    ] {
        let answer = describe(&emulator, name, "").await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case DescribeDomainAutoTunes({name:?}): expected {RECORDED_NOT_FOUND_STATUS}, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case DescribeDomainAutoTunes({name:?}): expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_domain_auto_tune_has_adjusted_nothing_on_reports_an_empty_set() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "tune-loaded").await;
    create_baseline(&emulator, "tune-empty").await;

    // The domain holds a scheduled change that is no Auto-Tune adjustment, so an empty
    // report is the Auto-Tune filter at work rather than an empty domain.
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let domain = guard
            .domains
            .get_mut("tune-empty")
            .expect("the domain must have been created");
        for row in rows().into_iter().filter(|row| !row.is_auto_tune()) {
            let record = stored(&row, base);
            domain.scheduled_actions.insert(record.id.clone(), record);
        }
    }

    for query in ["", "maxResults=1", "maxResults=100"] {
        let case = format!("empty report {query:?}");
        let answer = describe(&emulator, "tune-empty", query).await;
        assert!(
            auto_tunes(&answer, &case).is_empty(),
            "case {case}: expected no adjustments, got {:?}",
            answer.body["AutoTunes"]
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no NextToken, got {:?}",
            answer.body["NextToken"]
        );
    }

    let loaded = describe(&emulator, "tune-loaded", "").await;
    assert_eq!(
        marks(&auto_tunes(&loaded, "sibling report")),
        expected_marks(base),
        "case sibling report: the sibling domain's own adjustments are still reported, got {:?}",
        loaded.body
    );
}
