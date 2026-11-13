// SPDX-License-Identifier: CC0-1.0

//! The documented agent loop of `ReportInstrumentationConfigurationStatus` and
//! `GetInstrumentationConfigurationStatus`, over HTTP through the router production mounts: a
//! configuration transitions through statuses, each transition is reported as it happens, and
//! the read that follows it answers over the window it derives for itself.
//!
//! The rule under test: a status read reports every event whose report named an instant inside
//! the window it answers over — "when both are omitted, the time range defaults to the last
//! hour" (`smithy_query(query="shape",shape="GetInstrumentationConfigurationStatusRequest")`)
//! — so a status reported moments before the read is in that read's answer whatever second of
//! the minute its report named, and the events of each status are a chronology of their own,
//! accumulating as further transitions are reported, each event carrying the `ErrorCause` its
//! report carried (`InstrumentationStatusEvent.ErrorCause` is "The error cause when the status
//! is ERROR"). An event is held at the whole minute NEAREST the instant its report named
//! (`InstrumentationStatusEvent.Time` is "The time when the status was reported, rounded to the
//! nearest minute"), which for a report past the half-minute is the boundary AHEAD of the call,
//! so the write's grid and the read's window have to agree for that report to be queryable at
//! all.
//!
//! Two tests:
//!
//! * the transitions a row reports accumulate: each one is visible to the very next read of its
//!   status, the chronologies of the other statuses are untouched, and a read naming no status
//!   answers the ACTIVE one;
//! * a page sequence over the derived window reaches the newest minute of the history, so the
//!   modelled `NextToken` paging covers an event reported at the paging call's own instant.
//!
//! The rows differ along what the rule generalises over: how many transitions a configuration
//! walks, which statuses they are and in which order, whether the ERROR ones carry a cause and
//! which cause, how far before the read each instant falls, where in its minute it falls,
//! whether one status is reported twice inside one minute, the instrumentation type and the page
//! size. The harness is the one `instrumentation_configuration_status` drives the same two
//! operations through, so these suites agree about the wire surface.

use std::collections::BTreeMap;

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::instrumentation_configuration_status::{
    create_configuration, create_request, events_of, get_status, nearest_minute, report,
    report_entry, router, status_request, status_wire_token, Query, BREAKPOINT, PROBE, STATUSES,
};
use crate::status_event_default_window::{
    clock, clock_past_the_half_minute, second_of_the_minute,
};

/// The configuration every row of this suite reports for.
const SERVICE: &str = "checkout";
const ENVIRONMENT: &str = "eks:cluster-prod/checkout";
const FILE_PATH: &str = "src/checkout/PricingService.java";

/// The status a read that names none answers for: `Status` is "The single status to query for.
/// If omitted, only ACTIVE status events are returned"
/// (`smithy_query(query="shape",shape="GetInstrumentationConfigurationStatusRequest")`).
const DEFAULT_STATUS: &str = "ACTIVE";

/// One event as a read reports it: the whole minute it is held at, and the `ErrorCause` it
/// carries.
type Event = (f64, Option<String>);

/// One report a row made: the status it named, the instant it named and the cause it carried.
type Report = (&'static str, f64, Option<String>);

/// One transition of a row's sequence: the status reported, the cause the report carried, and
/// how far before the report the instant it names falls.
struct Transition {
    status: &'static str,
    cause: Option<&'static str>,
    seconds_ago: f64,
}

/// One row: a configuration of `kind` walking `transitions` in order.
struct Row {
    why: &'static str,
    kind: &'static str,
    /// Whether the sequence is driven from the SECOND half of a minute, the part where an
    /// event is held at the boundary ahead of the call that reported it.
    past_the_half_minute: bool,
    transitions: Vec<Transition>,
}

/// One transition, stated short.
fn at(status: &'static str, cause: Option<&'static str>, seconds_ago: f64) -> Transition {
    Transition {
        status,
        cause,
        seconds_ago,
    }
}

/// The table: sequences of transitions a configuration walks, from the single ACTIVE report of
/// the documented loop to a walk of the whole modelled status catalogue.
///
/// The causes are drawn from the modelled `InstrumentationErrorCause` catalogue
/// (`smithy_query(query="shape",shape="InstrumentationErrorCause")` -> `["FILE_NOT_FOUND",
/// "METHOD_NOT_FOUND", "LINE_NOT_EXECUTABLE", "OVERLOADED_METHODS", "LANGUAGE_MISMATCH",
/// "RUNTIME_ERROR"]`), including entries the issue this suite pins did not cite.
fn rows() -> Vec<Row> {
    vec![
        Row {
            why: "the documented loop: report ACTIVE, then read it back",
            kind: PROBE,
            past_the_half_minute: false,
            transitions: vec![at("ACTIVE", None, 0.0)],
        },
        Row {
            why: "an error reported after the active loop",
            kind: PROBE,
            past_the_half_minute: false,
            transitions: vec![
                at("ACTIVE", None, 0.0),
                at("ERROR", Some("RUNTIME_ERROR"), 0.0),
            ],
        },
        Row {
            why: "the whole status catalogue, in the order a configuration walks it",
            kind: BREAKPOINT,
            past_the_half_minute: false,
            transitions: vec![
                at("READY", None, 0.0),
                at("ACTIVE", None, 0.0),
                at("ERROR", Some("FILE_NOT_FOUND"), 0.0),
                at("DISABLED", None, 0.0),
            ],
        },
        Row {
            why: "a sequence reported from the second half of a minute, where an event is held \
                  after the call that reported it",
            kind: PROBE,
            past_the_half_minute: true,
            transitions: vec![
                at("ACTIVE", None, 0.0),
                at("ERROR", Some("LANGUAGE_MISMATCH"), 0.25),
                at("READY", None, 2.0),
            ],
        },
        Row {
            why: "the agent loop repeating its ACTIVE report",
            kind: BREAKPOINT,
            past_the_half_minute: true,
            transitions: vec![
                at("ACTIVE", None, 0.0),
                at("ACTIVE", None, 0.5),
                at("ACTIVE", None, 1.5),
            ],
        },
        Row {
            why: "transitions spread over the window, the newest at the call's own instant",
            kind: PROBE,
            past_the_half_minute: false,
            transitions: vec![
                at("READY", None, 900.0),
                at("ACTIVE", None, 181.0),
                at("ERROR", Some("OVERLOADED_METHODS"), 61.0),
                at("ACTIVE", None, 0.0),
            ],
        },
        Row {
            why: "an error whose cause is replaced by a later error of the same instant",
            kind: BREAKPOINT,
            past_the_half_minute: false,
            transitions: vec![
                at("ERROR", Some("METHOD_NOT_FOUND"), 0.0),
                at("ERROR", Some("LINE_NOT_EXECUTABLE"), 0.0),
                at("ACTIVE", None, 0.0),
            ],
        },
    ]
}

/// The chronology `status` has once `reported` has been reported, most recent first.
///
/// Reports of one status whose instants fall in one minute are ONE event, held at that minute
/// with the cause the LAST of them carried, so the expectation is the reports collapsed onto
/// the minutes they are held at.
fn chronology(reported: &[Report], status: &str) -> Vec<Event> {
    let mut held: BTreeMap<i64, Option<String>> = BTreeMap::new();
    for (_, instant, cause) in reported.iter().filter(|(named, _, _)| *named == status) {
        held.insert(nearest_minute(*instant) as i64, cause.clone());
    }
    held.into_iter()
        .rev()
        .map(|(minute, cause)| (minute as f64, cause))
        .collect()
}

/// Report one transition for `hash` at `instant`, asserting the report of a stored
/// configuration was processed.
async fn report_one(
    router: &MultiServiceRouter,
    kind: &str,
    hash: &str,
    status: &str,
    cause: Option<&str>,
    instant: f64,
    case: &str,
) {
    let (code, body) = report(
        router,
        &json!({
            "Service": SERVICE,
            "Environment": ENVIRONMENT,
            "Configurations": [report_entry(kind, hash, status, instant, cause)],
        }),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected HTTP 200 from the report, got {code} with {body}"
    );
    assert_eq!(
        body["UnprocessedStatusEvents"],
        json!([]),
        "case {case:?}: expected the report of a stored configuration to be processed, got {body}"
    );
}

/// The events a read of `status` reports over the window it derives for itself: the request
/// names NEITHER `StartTime` nor `EndTime`, and names no `Status` at all when `status` is
/// `None`.
async fn read_without_a_window(
    router: &MultiServiceRouter,
    kind: &str,
    hash: &str,
    status: Option<&str>,
    case: &str,
) -> Vec<Event> {
    let (code, body) = get_status(
        router,
        &status_request(
            SERVICE,
            ENVIRONMENT,
            kind,
            json!({ "LocationHash": hash }),
            &Query {
                status,
                ..Query::default()
            },
        ),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: expected HTTP 200 from the read, got {code} with {body}"
    );
    assert_eq!(
        body["Status"].as_str(),
        Some(status_wire_token(status.unwrap_or(DEFAULT_STATUS))),
        "case {case:?}: expected the status queried to be echoed in its wire spelling, got {body}"
    );
    events_of(&body)
}

/// Each transition reported is in the window the very next read derives for itself, and the
/// chronologies of the statuses accumulate independently of one another.
#[tokio::test]
async fn reported_transitions_accumulate_in_the_window_a_read_derives() {
    for row in rows() {
        let router = router();
        let request = create_request(SERVICE, ENVIRONMENT, row.kind, FILE_PATH);
        let hash = create_configuration(&router, &request).await;
        if row.past_the_half_minute {
            clock_past_the_half_minute();
        }

        let mut reported: Vec<Report> = Vec::new();
        for transition in &row.transitions {
            let instant = clock() - transition.seconds_ago;
            let case = format!(
                "{} / {} reported {:.2}s before the read, {:.2}s into its minute",
                row.why,
                transition.status,
                transition.seconds_ago,
                second_of_the_minute(instant)
            );
            report_one(
                &router,
                row.kind,
                &hash,
                transition.status,
                transition.cause,
                instant,
                &case,
            )
            .await;
            reported.push((
                transition.status,
                instant,
                transition.cause.map(ToString::to_string),
            ));

            let events =
                read_without_a_window(&router, row.kind, &hash, Some(transition.status), &case)
                    .await;
            assert!(
                !events.is_empty(),
                "case {case:?}: expected the status just reported to be queryable, got no event \
                 at all"
            );
            let expected = chronology(&reported, transition.status);
            assert_eq!(
                events, expected,
                "case {case:?}: expected {expected:?}, got {events:?}"
            );
        }

        // Every status the model declares, so the statuses a row never reported are asserted
        // to hold nothing and the ones it did are asserted to hold their own events only.
        for status in STATUSES {
            let case = format!("{} / the {status} chronology", row.why);
            let events = read_without_a_window(&router, row.kind, &hash, Some(status), &case).await;
            let expected = chronology(&reported, status);
            assert_eq!(
                events, expected,
                "case {case:?}: expected {expected:?}, got {events:?}"
            );

            if status == DEFAULT_STATUS {
                let defaulted = read_without_a_window(&router, row.kind, &hash, None, &case).await;
                assert_eq!(
                    defaulted, expected,
                    "case {case:?}: expected a read naming no status to answer the {status} \
                     chronology {expected:?}, got {defaulted:?}"
                );
            }
        }
    }
}

/// A page sequence over the window a read derives for itself reaches the NEWEST minute of the
/// history, whatever the page size — including an event reported at the instant of the call
/// that pages over it, which is held at the minute ahead of that call.
#[tokio::test]
async fn the_derived_window_pages_to_the_newest_minute_of_the_history() {
    let router = router();
    let request = create_request(SERVICE, ENVIRONMENT, PROBE, FILE_PATH);
    let hash = create_configuration(&router, &request).await;

    // One report per minute over the last few minutes, the newest at the call's own instant
    // taken past the half-minute: the minute it is held at is then AHEAD of every call that
    // pages over it.
    let newest = clock_past_the_half_minute();
    assert!(
        nearest_minute(newest) > newest,
        "expected the event of a report {:.2}s into its minute to be held at the minute ahead of \
         it, got {} for a report at {newest}",
        second_of_the_minute(newest),
        nearest_minute(newest)
    );

    let mut reported: Vec<Report> = Vec::new();
    for minutes_ago in [4.0_f64, 3.0, 2.0, 1.0, 0.0] {
        let instant = newest - 60.0 * minutes_ago;
        let case = format!("{minutes_ago} minute(s) before the newest report");
        report_one(&router, PROBE, &hash, "ACTIVE", None, instant, &case).await;
        reported.push(("ACTIVE", instant, None));
    }
    let expected = chronology(&reported, "ACTIVE");

    // Page sizes spanning one, several, exactly the event count and more than it, inside the
    // modelled `MaxResults` range 1..100
    // (`smithy_query(query="shape",shape="GetInstrumentationConfigurationStatusRequest")`).
    for page_size in [1_i64, 2, 4, 5, 6, 100] {
        let mut seen: Vec<Event> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let (code, body) = get_status(
                &router,
                &status_request(
                    SERVICE,
                    ENVIRONMENT,
                    PROBE,
                    json!({ "LocationHash": hash.clone() }),
                    &Query {
                        status: Some("ACTIVE"),
                        max_results: Some(page_size),
                        next_token: token.as_deref(),
                        ..Query::default()
                    },
                ),
            )
            .await;
            assert_eq!(
                code, 200,
                "case page size {page_size}: expected HTTP 200 from the read, got {code} with \
                 {body}"
            );
            let page = events_of(&body);
            assert!(
                page.len() <= page_size as usize,
                "case page size {page_size}: expected at most {page_size} events on a page, got \
                 {page:?}"
            );
            if pages == 0 {
                assert_eq!(
                    page.first(),
                    expected.first(),
                    "case page size {page_size}: expected the newest minute on the first page, \
                     got {page:?}"
                );
            }
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= 10,
                "case page size {page_size}: expected the pages to run out, got {pages} pages so \
                 far"
            );
            match body["NextToken"].as_str() {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case page size {page_size}: expected every event exactly once, most recent first, \
             got {seen:?}"
        );
    }
}
