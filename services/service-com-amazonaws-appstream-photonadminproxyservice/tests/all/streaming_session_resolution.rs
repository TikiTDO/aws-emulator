// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what the streaming URL AppStream issues RESOLVES to.
//!
//! Every case drives the real front door through the shared
//! [`Emulator`](crate::stack_fleet_lifecycle::Emulator) harness: the control-plane calls
//! over awsJson1_1, and the issued URL fetched the way a browser fetches it — a plain
//! `GET` of the path and query the emulator signed.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded request:
//!
//! * a streaming URL is a RESERVATION: fetching it resolves the session the issuing call
//!   dedicated and REDIRECTS (302) to that session's own address, which carries the
//!   session's id and the percent-encoded `fleet/<FleetName>` reference of the fleet the
//!   session was allocated against — whatever the stack, fleet and user it was issued for;
//! * that address serves the session, reporting the identity the control plane reports it
//!   under, and nothing else is served from it: a session id nothing is held under, a
//!   reference naming another fleet, and an address carrying no reference at all are each
//!   answered 404;
//! * resolving a URL changes no session: a read of the stack's fleet reports the same
//!   session before and after, in the same state;
//! * a URL for an identity no session is dedicated for, and one whose session the clock
//!   has expired, resolve to nothing.

use std::time::Duration;

use serde_json::{json, Value};

use crate::stack_fleet_lifecycle::Emulator;
use crate::streaming_url::{stack_with_fleet, start_and_await_running};

/// One row of the issuance table: the stack, fleet and user a URL is issued for.
struct Row {
    case: &'static str,
    stack: &'static str,
    fleet: &'static str,
    /// `UserId` is len 2..32 with pattern `^[\w+=,.@-]*$`
    /// (`smithy_query(query="operation", operation="CreateStreamingURL")`).
    user: &'static str,
}

/// The issuance table. The rows differ along what the rule generalises over: the form and
/// length of the stack and fleet names (`^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$`) and of the
/// user id, including both published `UserId` length bounds.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "plain names",
            stack: "session-stack",
            fleet: "session-fleet",
            user: "stream-user1",
        },
        Row {
            case: "punctuated names and an email-shaped user id",
            stack: "9stack_v1.2-beta",
            fleet: "fleet.one_2-3",
            user: "first.last@example.com",
        },
        Row {
            case: "the shortest user id the model admits",
            stack: "shortuser",
            fleet: "shortuserfleet",
            user: "ab",
        },
        Row {
            case: "the longest user id the model admits",
            stack: "longuser",
            fleet: "longuserfleet",
            // 32 characters, the published maximum.
            user: "u+=,.@-abcdefghijklmnopqrstuvwxy",
        },
    ]
}

/// Issue a streaming URL for a row, with the stack's fleet already streaming.
async fn issued_url(emulator: &Emulator, row: &Row, extra: Value) -> String {
    let case = row.case;
    stack_with_fleet(emulator, case, row.stack, row.fleet).await;
    start_and_await_running(emulator, case, row.fleet).await;
    let mut request = json!({
        "StackName": row.stack,
        "FleetName": row.fleet,
        "UserId": row.user,
    });
    for (member, value) in extra.as_object().expect("the row carries members") {
        request[member] = value.clone();
    }
    let payload = emulator.ok(case, "CreateStreamingURL", request).await;
    payload
        .get("StreamingURL")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a StreamingURL, got {payload}"))
        .to_string()
}

/// The sessions a read of one stack's fleet reports.
async fn sessions(emulator: &Emulator, case: &str, stack: &str, fleet: &str) -> Vec<Value> {
    let payload = emulator
        .ok(
            case,
            "DescribeSessions",
            json!({ "StackName": stack, "FleetName": fleet }),
        )
        .await;
    payload
        .get("Sessions")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The body a session's own address answers with, parsed.
fn parsed(case: &str, body: &[u8]) -> Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|error| panic!("case {case:?}: the session must be JSON: {error}"))
}

#[tokio::test]
async fn fetching_an_issued_url_lands_on_a_session_endpoint_naming_its_fleet() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        let url = issued_url(&emulator, &row, json!({ "Validity": 600 })).await;

        let before = sessions(&emulator, case, row.stack, row.fleet).await;
        assert_eq!(
            before.len(),
            1,
            "case {case:?}: issuing the URL must have dedicated one session, got {before:?}"
        );
        let session_id = before[0]
            .get("Id")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: expected the session's Id, got {:?}", before[0]))
            .to_string();

        // Fetching the URL resolves the reservation: a redirect to the session's own
        // address, which names the fleet the session was allocated against.
        let (status, location, body) = emulator.fetch(&url).await;
        assert_eq!(
            status, 302,
            "case {case:?}: fetching the issued URL must redirect, got {status} {:?}",
            String::from_utf8_lossy(&body)
        );
        let location = location.unwrap_or_else(|| {
            panic!("case {case:?}: a redirect must carry a Location, got none")
        });
        let reference = format!("fleet%2F{}", row.fleet);
        assert!(
            location.contains(&reference),
            "case {case:?}: the session address must reference {reference:?}, got {location:?}"
        );
        assert!(
            location.contains(&session_id),
            "case {case:?}: the session address must name the session {session_id:?}, got \
             {location:?}"
        );

        // The address the redirect names serves that session, and serves it as the one the
        // control plane reports.
        let (status, next, body) = emulator.fetch(&location).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the session address must serve the session, got {status}"
        );
        assert_eq!(
            next, None,
            "case {case:?}: the session address is where the walk ends, got {next:?}"
        );
        let served = parsed(case, &body);
        assert_eq!(
            served,
            json!({
                "Id": session_id,
                "UserId": row.user,
                "StackName": row.stack,
                "FleetName": row.fleet,
                "State": before[0].get("State").cloned().unwrap_or(Value::Null),
                "Reference": format!("fleet/{}", row.fleet),
            }),
            "case {case:?}: the session served must be the session that was dedicated, got \
             {served}"
        );

        // Resolving the URL changed nothing: the same session, in the same state.
        let after = sessions(&emulator, case, row.stack, row.fleet).await;
        assert_eq!(
            after, before,
            "case {case:?}: resolving a URL must not change what a later caller sees, got \
             {after:?}"
        );
    }
}

#[tokio::test]
async fn an_address_that_names_no_session_of_this_account_serves_nothing() {
    let row = &rows()[0];
    let case = row.case;
    let emulator = Emulator::new();
    let url = issued_url(&emulator, row, json!({ "Validity": 600 })).await;
    let (_, location, _) = emulator.fetch(&url).await;
    let location =
        location.unwrap_or_else(|| panic!("case {case:?}: the issued URL must redirect"));

    // Rows: each makes one part of the session's address wrong — the id, and the fleet
    // the address says the session was allocated against.
    let (path, query) = location
        .split_once('?')
        .unwrap_or_else(|| panic!("case {case:?}: the session address carries a reference"));
    let other_id = format!("{path}00?{query}");
    let other_fleet = format!("{path}?reference=fleet%2Fsome-other-fleet");
    for (label, address) in [
        ("a session id nothing is held under", other_id),
        ("a reference naming another fleet", other_fleet),
        ("an address carrying no fleet reference", path.to_string()),
        (
            "a URL issued for no session",
            format!(
                "/streaming-sessions/{}/{}/nobody?X-Amz-Credential=x%2F20260101%2Fus-east-1\
                 %2Fappstream%2Faws4_request&X-Amz-Signature=00",
                row.stack, row.fleet
            ),
        ),
    ] {
        let (status, _, _) = emulator.fetch(&address).await;
        assert_eq!(
            status, 404,
            "case {label:?}: nothing is served at {address:?}, got {status}"
        );
    }

    // The positive control: the address the redirect named still serves the session.
    let (status, _, _) = emulator.fetch(&location).await;
    assert_eq!(
        status, 200,
        "case \"positive control\": the session's own address must still serve it"
    );
}

#[tokio::test]
async fn a_url_whose_session_the_clock_has_expired_resolves_to_nothing() {
    // The shortest validity the documentation admits, so the session the URL was issued
    // for is expired by the clock a moment later — nobody can stream from it, so there is
    // nothing to resolve.
    let row = &rows()[0];
    let case = "a session the clock expired";
    let emulator = Emulator::new();
    let url = issued_url(&emulator, row, json!({ "Validity": 1 })).await;

    let (status, _, _) = emulator.fetch(&url).await;
    assert_eq!(
        status, 302,
        "case {case:?}: while the session is live the URL must still resolve, got {status}"
    );

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (status, location, _) = emulator.fetch(&url).await;
    assert_eq!(
        status, 404,
        "case {case:?}: an expired session has no address to lead to, got {status}"
    );
    assert_eq!(
        location, None,
        "case {case:?}: nothing is redirected to, got {location:?}"
    );
}
