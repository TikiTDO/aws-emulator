// SPDX-License-Identifier: CC0-1.0

//! The FORM an Event API's `name` must have, and the refusal a value outside it takes, end
//! to end over the restJson1 wire.
//!
//! The rule under test, one level above the two requests that filed the issue:
//!
//! > A `name` outside the form `ApiName` declares — its `@length` 1..50 or its `@pattern`
//! > `^[A-Za-z0-9_\-\ ]+$` (`smithy_query(query="shape", shape="ApiName")`) — is refused by
//! > EITHER operation that carries that member (`CreateApi`, `UpdateApi`) with the
//! > FRAMEWORK-level constraint error: HTTP 400 with the error code `ValidationException`,
//! > carrying the framework's own report — one clause per constraint the value broke, each
//! > naming the member and the requirement it broke, with a `@pattern` stated as the
//! > expression the model wraps rather than the whole-value wrapping, and with no part of
//! > the submitted value quoted back. A name INSIDE that form is served and round-trips.
//!
//! The rows differ along what the rule generalises over: the OPERATION (every row is run
//! through both), the CONSTRAINT CLASS broken (the maximum length, the minimum length, the
//! expression, and both at once), WHICH character outside the expression's class the value
//! carries (punctuation, a separator, a non-ASCII letter, an emoji, a control character)
//! and WHERE it sits, and the LENGTH (empty, one past the bound, far past it). The served
//! rows sit at each published bound of the length and span the whole character class.
//!
//! Two further rows hold the rule's EDGES, so the refusal stays the one the recordings show
//! per operation rather than becoming the service's answer to every malformed member: a
//! `name` the request omits entirely keeps AppSync's own `BadRequestException`, and another
//! operation's `name` outside ITS target shape's expression keeps that code too.
//!
//! The harness (`call`, `router`, `Answer`) is the error-contract suite's; the `eventConfig`
//! builder is the Event-API suite's.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::error_contract::{call, router, Answer};
use crate::event_api::event_config;

/// `ApiName`'s published length bounds (`@length` 1..50,
/// `smithy_query(query="shape", shape="ApiName")`).
const NAME_MIN_LEN: usize = 1;
const NAME_MAX_LEN: usize = 50;

/// The EXPRESSION a clause about this member states: `ApiName`'s modelled `@pattern`
/// `^[A-Za-z0-9_\-\ ]+$` without the whole-value anchors the model wraps it in.
/// Transcribed here rather than computed, so the suite states the published expression
/// independently of the code that strips those anchors.
const STATED_EXPRESSION: &str = r"[A-Za-z0-9_\-\ ]+";

/// The error code the framework names a constraint refusal with, which is what this
/// surface's recordings show for this member.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// AppSync's OWN modelled client error for a request that is not well formed
/// (`smithy_query(query="shape", shape="BadRequestException")`, `@httpError` 400) — the
/// code every other refusal of this service answers under.
const BAD_REQUEST: &str = "BadRequestException";

/// The status both codes are answered at.
const REFUSED_STATUS: u16 = 400;

/// An `eventConfig` set up through one `API_KEY` provider, which every authorization mode
/// list then draws on — the one member besides `name` both operations require.
fn api_key_config() -> Value {
    event_config(&["API_KEY"], &["API_KEY"], &["API_KEY"], &["API_KEY"], None)
}

/// A `CreateApi`/`UpdateApi` body carrying `name` and that configuration.
fn body(name: &str) -> Value {
    json!({"name": name, "eventConfig": api_key_config()})
}

/// The report this surface states over `requirements`, one clause per constraint broken:
/// the framework's envelope, its member path, and no value at all.
fn report(requirements: &[String]) -> String {
    let clauses: Vec<String> = requirements
        .iter()
        .map(|requirement| {
            format!("Value at 'name' failed to satisfy constraint: {requirement}")
        })
        .collect();
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        n => format!("{n} validation errors detected: {}", clauses.join("; ")),
    }
}

/// The requirement a clause states for a value outside the expression.
fn pattern_requirement() -> String {
    format!("Member must satisfy regular expression pattern: {STATED_EXPRESSION}")
}

/// The requirement a clause states for a value past the declared maximum length.
fn max_length_requirement() -> String {
    format!("Member must have length less than or equal to {NAME_MAX_LEN}")
}

/// The requirement a clause states for a value short of the declared minimum length.
fn min_length_requirement() -> String {
    format!("Member must have length greater than or equal to {NAME_MIN_LEN}")
}

/// One refused row: the name the request carries, and the requirements its report states.
struct Refused {
    case: &'static str,
    name: String,
    requirements: Vec<String>,
}

/// The refused table.
fn refused_rows() -> Vec<Refused> {
    let pattern = pattern_requirement();
    let mut rows: Vec<Refused> = vec![
        ("an at sign inside the name", "bad@name".to_string()),
        ("a hash inside the name", "name#1".to_string()),
        ("an exclamation mark at the end", "Events!".to_string()),
        ("a dot, which the class does not admit", "Events.v2".to_string()),
        ("a slash at the start", "/Events".to_string()),
        ("a colon, admitted by tag keys but not here", "Events:v2".to_string()),
        ("a non-ASCII letter", "Évènements".to_string()),
        ("an emoji", "Events 🎉".to_string()),
        ("a newline inside the name", "Events\nv2".to_string()),
        ("a tab inside the name", "Events\tv2".to_string()),
        ("a name of only characters outside the class", "@@@".to_string()),
    ]
    .into_iter()
    .map(|(case, name)| Refused {
        case,
        name,
        requirements: vec![pattern.clone()],
    })
    .collect();

    rows.push(Refused {
        case: "a name one character past the modelled maximum length",
        name: "E".repeat(NAME_MAX_LEN + 1),
        requirements: vec![max_length_requirement()],
    });
    rows.push(Refused {
        case: "a name far past the modelled maximum length",
        name: "E".repeat(500),
        requirements: vec![max_length_requirement()],
    });
    rows.push(Refused {
        // Past the ceiling a clause would quote a value under at all, so a report that
        // leaked any of it would be visible as a truncation marker.
        case: "a name past every ceiling, at 20,000 characters",
        name: "E".repeat(20_000),
        requirements: vec![max_length_requirement()],
    });
    rows.push(Refused {
        case: "a name past the maximum length AND outside the expression",
        name: format!("{}!", "E".repeat(NAME_MAX_LEN)),
        requirements: vec![max_length_requirement(), pattern_requirement()],
    });
    rows.push(Refused {
        case: "an empty name, which is short of the minimum and matches no expression",
        name: String::new(),
        requirements: vec![min_length_requirement(), pattern_requirement()],
    });
    rows
}

/// The names INSIDE the declared form: each published length bound, every character class
/// the expression admits, and a value drawn only from the separators it admits.
fn served_names() -> Vec<(&'static str, String)> {
    vec![
        ("a name at the modelled minimum length", "E".repeat(NAME_MIN_LEN)),
        ("a name at the modelled maximum length", "E".repeat(NAME_MAX_LEN)),
        (
            "every character class the expression admits",
            "Ev3nt_api-name with spaces".to_string(),
        ),
        ("only the separators the class admits", "_- ".to_string()),
        ("digits alone", "2026".to_string()),
    ]
}

/// Create one Event API and answer its `apiId`.
async fn seed(router: &MultiServiceRouter, name: &str) -> String {
    let created = call(router, "POST", "/v2/apis", Some(body(name))).await;
    assert_eq!(
        created.status, 200,
        "case seed {name:?}: expected CreateApi 200, got {created:?}"
    );
    created.body["api"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The `apiId` of every `Api` the service holds, in listing order.
async fn listed_api_ids(router: &MultiServiceRouter) -> Vec<String> {
    let listed = call(router, "GET", "/v2/apis", None).await;
    assert_eq!(
        listed.status, 200,
        "case list the stored APIs: expected 200, got {listed:?}"
    );
    listed.body["apis"]
        .as_array()
        .map(|apis| {
            apis.iter()
                .map(|api| api["apiId"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Assert one refusal: the framework's code and status, the report over `requirements`, and
/// nothing of the submitted value anywhere in it.
fn assert_refusal(case: &str, name: &str, requirements: &[String], answer: &Answer) {
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (REFUSED_STATUS, VALIDATION_EXCEPTION),
        "case {case:?}: expected {REFUSED_STATUS} {VALIDATION_EXCEPTION}, got {answer:?}"
    );
    let expected = report(requirements);
    assert_eq!(
        answer.message, expected,
        "case {case:?}: expected the report {expected:?}, got {answer:?}"
    );
    assert!(
        !answer.message.contains(&format!("Value '{name}'")),
        "case {case:?}: expected no part of the submitted value quoted, got {answer:?}"
    );
    assert!(
        !answer.message.contains('^') && !answer.message.contains("+$"),
        "case {case:?}: expected the expression stated without its anchors, got {answer:?}"
    );
    assert!(
        answer.body["api"].is_null(),
        "case {case:?}: a refused call answers no API, got {answer:?}"
    );
}

/// RULE, on the create: a name outside the declared form is the framework's constraint
/// error, and the refused create persists nothing.
#[tokio::test]
async fn a_create_whose_name_is_outside_the_declared_form_is_a_validation_refusal() {
    let router = router();
    // One stored API, so "persists nothing" is asserted against a store that is not empty
    // either side of the refusal.
    let seeded = seed(&router, "Events_seed").await;
    let before = listed_api_ids(&router).await;

    for row in refused_rows() {
        let answer = call(&router, "POST", "/v2/apis", Some(body(&row.name))).await;
        assert_refusal(row.case, &row.name, &row.requirements, &answer);
        assert_eq!(
            listed_api_ids(&router).await,
            before,
            "case {:?}: a refused create persists nothing",
            row.case
        );
    }

    // The refusals changed nothing: the seeded API is still readable and a well-formed
    // create still succeeds.
    let read = call(&router, "GET", &format!("/v2/apis/{seeded}"), None).await;
    assert_eq!(
        read.body["api"]["name"].as_str(),
        Some("Events_seed"),
        "case \"the seeded API after the refusals\": expected its name intact, got {read:?}"
    );
}

/// RULE, on the update: the same refusal, from the other operation that carries the member,
/// and the stored API keeps the name it had.
#[tokio::test]
async fn an_update_whose_name_is_outside_the_declared_form_is_a_validation_refusal() {
    let router = router();
    let api_id = seed(&router, "Events_update").await;
    let before = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
    let stored = before.body["api"].clone();

    for row in refused_rows() {
        let answer = call(
            &router,
            "POST",
            &format!("/v2/apis/{api_id}"),
            Some(body(&row.name)),
        )
        .await;
        assert_refusal(row.case, &row.name, &row.requirements, &answer);

        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.body["api"], stored,
            "case {:?}: expected the stored API untouched as {stored:?}, got {read:?}",
            row.case
        );
    }

    // An id that names NO API is still refused for its name first, exactly as the boundary
    // would have refused it before the API was ever resolved.
    let answer = call(
        &router,
        "POST",
        "/v2/apis/zzzzzzzzzzzzzzzzzzzzzzzzzz",
        Some(body("bad@name")),
    )
    .await;
    assert_refusal(
        "a malformed name on an id that names no API",
        "bad@name",
        &[pattern_requirement()],
        &answer,
    );
}

/// RULE: a name INSIDE the declared form is served by both operations and round-trips
/// through the reader.
#[tokio::test]
async fn a_name_inside_the_declared_form_round_trips_through_both_operations() {
    let router = router();

    for (case, name) in served_names() {
        let created = call(&router, "POST", "/v2/apis", Some(body(&name))).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected the create to be served, got {created:?}"
        );
        let api_id = created.body["api"]["apiId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.body["api"]["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the created name back, got {read:?}"
        );

        // The same value through the update path, onto an API created under another name.
        let renamed = call(
            &router,
            "POST",
            &format!("/v2/apis/{api_id}"),
            Some(body(&name)),
        )
        .await;
        assert_eq!(
            renamed.status, 200,
            "case {case:?}: expected the update to be served, got {renamed:?}"
        );
        let read = call(&router, "GET", &format!("/v2/apis/{api_id}"), None).await;
        assert_eq!(
            read.body["api"]["name"].as_str(),
            Some(name.as_str()),
            "case {case:?}: expected the updated name back, got {read:?}"
        );
    }
}

/// RULE's EDGE: the framework's code answers the FORM of this member's value only. A
/// `name` the request omits altogether, and another operation's `name` outside its own
/// target shape's expression, keep AppSync's own `BadRequestException`.
#[tokio::test]
async fn the_refusals_this_surface_does_not_restate_keep_the_services_own_error() {
    let router = router();
    let api_id = seed(&router, "Events_edges").await;

    // The required member absent, on each operation that carries it.
    let paths = [
        ("CreateApi", "/v2/apis".to_string()),
        ("UpdateApi", format!("/v2/apis/{api_id}")),
    ];
    for (case, path) in paths {
        let answer = call(
            &router,
            "POST",
            &path,
            Some(json!({"eventConfig": api_key_config()})),
        )
        .await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (REFUSED_STATUS, BAD_REQUEST),
            "case {case:?} with no name at all: expected {REFUSED_STATUS} {BAD_REQUEST}, \
             got {answer:?}"
        );
        assert_eq!(
            answer.message, "Invalid name given.",
            "case {case:?} with no name at all: expected the service's own sentence, got \
             {answer:?}"
        );
    }

    // Another operation's `name`, outside the expression ITS target shape declares
    // (`Namespace`, `^([A-Za-z0-9](?:[A-Za-z0-9\-]{0,48}[A-Za-z0-9])?)$`,
    // `smithy_query(query="operation", operation="CreateChannelNamespace")`): the same
    // constraint class on a member of the same name, answered under this service's own
    // code because that is what its own recordings show.
    let answer = call(
        &router,
        "POST",
        &format!("/v2/apis/{api_id}/channelNamespaces"),
        Some(json!({"name": "bad@namespace"})),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (REFUSED_STATUS, BAD_REQUEST),
        "case \"a channel namespace outside its own expression\": expected \
         {REFUSED_STATUS} {BAD_REQUEST}, got {answer:?}"
    );
}
