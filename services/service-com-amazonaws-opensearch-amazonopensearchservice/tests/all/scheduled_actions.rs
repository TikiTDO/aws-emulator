// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListScheduledActions`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees. The harness and the baseline `CreateDomain` body come from
//! [`super::domain_lifecycle`]; this suite adds no second copy of them.
//!
//! The rule under test: a listing reports the scheduled changes of the domain the
//! request names — every one of them, each as the stored record's own values, ordered
//! by the instant it is scheduled to happen — in pages of at most `MaxResults`
//! resumed by the token the previous page handed out; a name the account holds no
//! domain under has no listing, and a token naming no action of that domain is
//! refused rather than answered with a page.
//!
//! So the assertions span a SET of inputs rather than one call: a table of scheduled
//! changes covering EVERY value of `ActionType`, `ActionSeverity` and `ActionStatus`
//! the model admits, both values of `ScheduledBy`, every combination of `Mandatory`
//! and `Cancellable`, a description and none, instants before and after the call, and
//! two changes scheduled for the SAME instant; then five page sizes and the two ways
//! to ask for no bound, the published bound and values past it at either end, four
//! shapes of unresolvable token, and four names no domain answers to.
//!
//! The scheduled changes themselves are written into the store directly (through
//! [`super::domain_lifecycle::Emulator::store`]): the operations that SCHEDULE a
//! change — `StartServiceSoftwareUpdate` and `UpdateScheduledAction` — are still
//! scaffold stubs, so no wire call can leave one yet. Everything the rule is asserted
//! ON is read back through the wire.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::{
    ACTION_SEVERITY_VALUES, ACTION_STATUS_VALUES, ACTION_TYPE_VALUES, SCHEDULED_BY_VALUES,
};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::ScheduledChange;

use super::domain_lifecycle::{
    create_baseline, error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// One row of the scheduled-change table: the action id, its type, its severity, how
/// far from the base instant it is scheduled, its description, who scheduled it, its
/// status, whether it is mandatory and whether it is cancellable.
///
/// Visible to the crate's other suites because this table is the ONE set of seeded
/// scheduled changes these tests drive: `super::scheduled_action_updates` re-times
/// these same records rather than seeding a second table beside them.
pub(crate) struct Row {
    pub(crate) id: &'static str,
    pub(crate) action_type: &'static str,
    pub(crate) severity: &'static str,
    pub(crate) offset: i64,
    pub(crate) description: Option<&'static str>,
    pub(crate) scheduled_by: &'static str,
    pub(crate) status: &'static str,
    pub(crate) mandatory: bool,
    pub(crate) cancellable: bool,
}

/// The scheduled changes every case below is asserted over.
///
/// The rows differ along what the rule generalises over: the kind of change, its
/// severity, who scheduled it, where it is in its own lifecycle, whether it carries a
/// description, whether it is mandatory or cancellable, and whether its instant is
/// behind or ahead of the call. Between them they use every value of `ActionType`,
/// `ActionSeverity` and `ActionStatus` and both values of `ScheduledBy` — the
/// catalogs the model declares, as `ops::types`' `ACTION_TYPE_VALUES`,
/// `ACTION_SEVERITY_VALUES`, `ACTION_STATUS_VALUES` and `SCHEDULED_BY_VALUES`
/// (asserted below) — and `alpha-tie` is scheduled for the SAME instant as
/// `ssu-pending`, so the order of two changes that fall together is exercised too.
pub(crate) fn rows() -> Vec<Row> {
    vec![
        Row {
            id: "ssu-pending",
            action_type: "SERVICE_SOFTWARE_UPDATE",
            severity: "HIGH",
            offset: 3600,
            description: Some("Service software update to R20250601"),
            scheduled_by: "SYSTEM",
            status: "PENDING_UPDATE",
            mandatory: true,
            cancellable: false,
        },
        Row {
            id: "alpha-tie",
            action_type: "JVM_YOUNG_GEN_TUNING",
            severity: "HIGH",
            offset: 3600,
            description: None,
            scheduled_by: "CUSTOMER",
            status: "ELIGIBLE",
            mandatory: false,
            cancellable: true,
        },
        Row {
            id: "heap-tuning",
            action_type: "JVM_HEAP_SIZE_TUNING",
            severity: "MEDIUM",
            offset: 86_400,
            description: Some("Auto-Tune change to the JVM heap size"),
            scheduled_by: "SYSTEM",
            status: "ELIGIBLE",
            mandatory: false,
            cancellable: true,
        },
        Row {
            id: "young-gen",
            action_type: "JVM_YOUNG_GEN_TUNING",
            severity: "LOW",
            offset: 172_800,
            description: None,
            scheduled_by: "CUSTOMER",
            status: "NOT_ELIGIBLE",
            mandatory: false,
            cancellable: false,
        },
        Row {
            id: "ssu-done",
            action_type: "SERVICE_SOFTWARE_UPDATE",
            severity: "LOW",
            offset: -7_200,
            description: Some("Service software update to R20250101"),
            scheduled_by: "CUSTOMER",
            status: "COMPLETED",
            mandatory: true,
            cancellable: true,
        },
        Row {
            id: "heap-failed",
            action_type: "JVM_HEAP_SIZE_TUNING",
            severity: "HIGH",
            offset: -600,
            description: Some("Auto-Tune change that could not be applied"),
            scheduled_by: "SYSTEM",
            status: "FAILED",
            mandatory: false,
            cancellable: false,
        },
        Row {
            id: "ssu-running",
            action_type: "SERVICE_SOFTWARE_UPDATE",
            severity: "MEDIUM",
            offset: 60,
            description: Some("Service software update being applied"),
            scheduled_by: "CUSTOMER",
            status: "IN_PROGRESS",
            mandatory: true,
            cancellable: false,
        },
    ]
}

/// The ids of [`rows`] in the order a listing must report them: by the instant each
/// change is scheduled for, earliest first, the action id breaking a tie.
const EXPECTED_ORDER: [&str; 7] = [
    "ssu-done",
    "heap-failed",
    "ssu-running",
    "alpha-tie",
    "ssu-pending",
    "heap-tuning",
    "young-gen",
];

/// The instant the offsets of [`rows`] are measured from, rounded down to the second
/// so the millisecond count a case computes for itself is exact.
pub(crate) fn base_instant() -> SystemTime {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the test clock is after the epoch");
    UNIX_EPOCH + Duration::from_secs(since.as_secs())
}

/// `base` as an epoch-millisecond count, computed here rather than taken from the
/// emulator, so a case asserts the UNIT of `ScheduledTime` and not just its value.
pub(crate) fn base_millis(base: SystemTime) -> i64 {
    i64::try_from(
        base.duration_since(UNIX_EPOCH)
            .expect("the test clock is after the epoch")
            .as_millis(),
    )
    .expect("an epoch-millisecond count of this era fits a Long")
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
        scheduled_by: row.scheduled_by.to_string(),
        status: row.status.to_string(),
        mandatory: row.mandatory,
        cancellable: row.cancellable,
    }
}

/// Put the whole [`rows`] table into `name`'s store entry, measured from `base`.
pub(crate) fn seed(emulator: &Emulator, name: &str, base: SystemTime) {
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

/// `GET /2021-01-01/opensearch/domain/{name}/scheduledActions[?query]`.
async fn list(emulator: &Emulator, name: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/domain/{name}/scheduledActions")
    } else {
        format!("{PREFIX}/opensearch/domain/{name}/scheduledActions?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `ScheduledActions` of an accepted listing.
fn actions(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["ScheduledActions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a ScheduledActions list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `Id` of every action of a page, in the order the page reports them.
fn ids(page: &[Value]) -> Vec<String> {
    page.iter()
        .map(|action| action["Id"].as_str().unwrap_or_default().to_owned())
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
async fn a_listing_reports_every_scheduled_change_of_the_domain_as_its_stored_record() {
    let emulator = Emulator::new();
    let base = seeded(&emulator, "sched-report").await;

    let answer = list(&emulator, "sched-report", "").await;
    let page = actions(&answer, "whole listing");
    assert_eq!(
        ids(&page),
        EXPECTED_ORDER.map(str::to_owned).to_vec(),
        "case whole listing: expected the changes in the order they fall due, got {:?}",
        ids(&page)
    );

    for row in rows() {
        let case = format!("ScheduledAction({:?})", row.id);
        // The catalogs these rows draw on are the model's own enums, as generated into
        // `ops::types`: a row naming a value outside one would assert nothing about
        // this service's wire surface.
        assert!(
            ACTION_TYPE_VALUES.contains(&row.action_type)
                && ACTION_SEVERITY_VALUES.contains(&row.severity)
                && ACTION_STATUS_VALUES.contains(&row.status)
                && SCHEDULED_BY_VALUES.contains(&row.scheduled_by),
            "case {case}: the row must use modelled values, got {:?}/{:?}/{:?}/{:?}",
            row.action_type,
            row.severity,
            row.status,
            row.scheduled_by
        );

        let reported = page
            .iter()
            .find(|action| action["Id"].as_str() == Some(row.id))
            .unwrap_or_else(|| panic!("case {case}: expected the change to be reported"));
        assert_eq!(
            reported["Type"].as_str(),
            Some(row.action_type),
            "case {case}: expected the stored type, got {:?}",
            reported["Type"]
        );
        assert_eq!(
            reported["Severity"].as_str(),
            Some(row.severity),
            "case {case}: expected the stored severity, got {:?}",
            reported["Severity"]
        );
        assert_eq!(
            reported["ScheduledTime"].as_i64(),
            Some(base_millis(base) + row.offset * 1_000),
            "case {case}: expected the instant it is scheduled for as an epoch-millisecond \
             count, got {:?}",
            reported["ScheduledTime"]
        );
        assert_eq!(
            reported["Description"].as_str(),
            row.description,
            "case {case}: expected the stored description, got {:?}",
            reported["Description"]
        );
        assert_eq!(
            reported["ScheduledBy"].as_str(),
            Some(row.scheduled_by),
            "case {case}: expected the stored scheduler, got {:?}",
            reported["ScheduledBy"]
        );
        assert_eq!(
            reported["Status"].as_str(),
            Some(row.status),
            "case {case}: expected the stored status, got {:?}",
            reported["Status"]
        );
        assert_eq!(
            reported["Mandatory"].as_bool(),
            Some(row.mandatory),
            "case {case}: expected the stored Mandatory, got {:?}",
            reported["Mandatory"]
        );
        assert_eq!(
            reported["Cancellable"].as_bool(),
            Some(row.cancellable),
            "case {case}: expected the stored Cancellable, got {:?}",
            reported["Cancellable"]
        );
    }

    // A listing is a report: a second one answers exactly the same, so reading the
    // changes moved none of them.
    let again = list(&emulator, "sched-report", "").await;
    assert_eq!(
        again.body, answer.body,
        "case whole listing(twice): a read must leave the changes as they were, got {:?}",
        again.body
    );
}

#[tokio::test]
async fn pages_partition_the_listing_of_a_domain() {
    let emulator = Emulator::new();
    seeded(&emulator, "sched-paging").await;
    let whole = EXPECTED_ORDER.map(str::to_owned).to_vec();

    // One row per page size: the smallest, two that do not divide the listing, the
    // listing's own size and the largest the modelled range allows.
    for page_size in [1_i32, 2, 3, 7, 100] {
        let case = format!("page size {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(token) => format!("maxResults={page_size}&nextToken={token}"),
            };
            let answer = list(&emulator, "sched-paging", &query).await;
            let page = actions(&answer, &case);
            assert!(
                page.len() <= usize::try_from(page_size).unwrap_or_default(),
                "case {case}: a page holds at most MaxResults changes, got {}",
                page.len()
            );
            assert!(
                !page.is_empty(),
                "case {case}: a page the service issued a token for reports changes, got none"
            );
            walked.extend(ids(&page));
            pages += 1;
            token = answer.body["NextToken"].as_str().map(str::to_owned);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= whole.len(),
                "case {case}: paging kept issuing tokens past the end of the listing"
            );
        }
        assert_eq!(
            walked, whole,
            "case {case}: expected the pages to concatenate back to the whole listing in order, \
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

    // `MaxResults` is `@default(0)`, so a request that asks for no bound gets the
    // whole listing in one page, with no token to resume from.
    for unbounded in ["", "maxResults=0"] {
        let case = format!("unbounded {unbounded:?}");
        let answer = list(&emulator, "sched-paging", unbounded).await;
        assert_eq!(
            ids(&actions(&answer, &case)),
            whole,
            "case {case}: expected the whole listing in one page"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no NextToken, got {:?}",
            answer.body["NextToken"]
        );
    }

    // The published bound is accepted; a page size past it at either end is refused.
    let at_bound = list(&emulator, "sched-paging", "maxResults=100").await;
    assert_eq!(
        at_bound.status, 200,
        "case maxResults=100: the published bound is accepted, got {} {:?}",
        at_bound.status, at_bound.body
    );
    for out_of_bound in ["maxResults=101", "maxResults=1000", "maxResults=-1"] {
        let answer = list(&emulator, "sched-paging", out_of_bound).await;
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
async fn a_token_that_names_no_change_of_the_domain_is_refused() {
    let emulator = Emulator::new();
    seeded(&emulator, "sched-token").await;
    create_baseline(&emulator, "sched-other").await;

    // A token is only ever a value this operation handed out: one invented, one
    // belonging to a change of a domain whose listing it was not issued for, one whose
    // change has since been applied and taken out of the store, and one that is a
    // valid id of no domain at all.
    {
        let store = emulator.store();
        let mut guard = store.lock();
        let domain = guard
            .domains
            .get_mut("sched-token")
            .expect("the domain must have been created");
        domain.scheduled_actions.remove("heap-failed");
    }
    for (name, token, label) in [
        ("sched-token", "not-a-token", "an invented token"),
        ("sched-other", "ssu-pending", "a token from another listing"),
        ("sched-token", "heap-failed", "a token whose change was applied"),
        (
            "sched-token",
            "00000000-0000-0000-0000-000000000000",
            "a token shaped like an identifier",
        ),
    ] {
        let case = format!("{label} ({token:?})");
        let answer = list(&emulator, name, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "InvalidPaginationTokenException",
            "case {case}: expected the modelled pagination-token refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_domain_the_account_does_not_hold_has_no_listing() {
    let emulator = Emulator::new();
    seeded(&emulator, "sched-present").await;
    create_baseline(&emulator, "sched-deleted").await;
    let deleted = emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/sched-deleted"), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case DeleteDomain(sched-deleted): expected 200, got {} with {:?}",
        deleted.status, deleted.body
    );

    // Rows differ along the shape of the name: one never created, one at the shortest
    // the model admits, one at its longest, and one the account held until it was
    // deleted.
    for name in [
        "never-created",
        "abc",
        "a-domain-name-at-its-upper-b",
        "sched-deleted",
    ] {
        let answer = list(&emulator, name, "").await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case ListScheduledActions({name:?}): expected {RECORDED_NOT_FOUND_STATUS}, got {} \
             with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case ListScheduledActions({name:?}): expected ResourceNotFoundException, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn a_domain_with_nothing_scheduled_reports_an_empty_listing() {
    let emulator = Emulator::new();
    seeded(&emulator, "sched-loaded").await;
    create_baseline(&emulator, "sched-empty").await;

    // The listing belongs to ONE domain: a domain nothing is scheduled on reports no
    // changes even while a sibling domain of the same account holds a full table.
    for query in ["", "maxResults=1", "maxResults=100"] {
        let case = format!("empty listing {query:?}");
        let answer = list(&emulator, "sched-empty", query).await;
        assert!(
            actions(&answer, &case).is_empty(),
            "case {case}: expected no changes, got {:?}",
            answer.body["ScheduledActions"]
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {case}: expected no NextToken, got {:?}",
            answer.body["NextToken"]
        );
    }

    let loaded = list(&emulator, "sched-loaded", "").await;
    assert_eq!(
        ids(&actions(&loaded, "sibling listing")).len(),
        EXPECTED_ORDER.len(),
        "case sibling listing: the sibling domain's own changes are still reported, got {:?}",
        loaded.body
    );
}
