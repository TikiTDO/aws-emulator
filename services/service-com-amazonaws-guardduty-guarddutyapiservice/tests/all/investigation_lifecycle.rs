// SPDX-License-Identifier: CC0-1.0

//! GuardDuty Investigation, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `CreateInvestigation` PERSISTS an investigation whose every reported member is
//!    read back by `GetInvestigation`: the prompt verbatim and the account that
//!    initiated it. `Investigation.Cloud` is NOT among them — it carries the
//!    environment the investigation itself resolves when its pass ends, which the
//!    `investigation_cloud_environment` suite is about.
//! 2. A created investigation starts `RUNNING` and reaches its terminal status only
//!    when the CLOCK owes it (`state::INVESTIGATION_TRANSITION_MILLIS`), after which
//!    it never moves again; a read never moves it early and never changes what a
//!    later read sees.
//! 3. `ClientToken` is `@idempotencyToken`: the same token returns the investigation
//!    the first call initiated, a different token initiates another.
//! 4. The operation is gated on the detector's AI_ANALYST feature — "To use this
//!    operation, the AI_ANALYST feature must be enabled on your detector" — so the
//!    stored feature decides the call, whichever call switched it.
//! 5. A `DetectorId` the account/Region does not hold is an AUTHORIZATION failure
//!    (HTTP 403 `AccessDeniedException`, disclosing nothing about whether it names
//!    anything), a value that cannot be a `DetectorId` at all is a parameter failure
//!    (HTTP 400), and an `InvestigationId` an authorized detector does not hold is
//!    `ResourceNotFoundException` (HTTP 404).
//! 6. The feature is served exactly through the Regions its documentation lists.
//! 7. The documented preview limit — 10 investigations per account per day — refuses
//!    the create past it and persists nothing.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, create_detector, enabled_detector, error_code, new_router, Router,
    ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    INVESTIGATIONS_PER_DAY, INVESTIGATION_REGIONS, INVESTIGATION_TRANSITION_MILLIS,
};

/// The status a fresh investigation reports, and the one it reaches once the clock
/// owes it. Both are `InvestigationStatus` members
/// (`smithy_query(shape="InvestigationStatus")`).
const RUNNING: &str = "RUNNING";
const FAILED: &str = "FAILED";

/// Long enough for the clock to owe the terminal status, plus slack.
fn past_the_transition() -> std::time::Duration {
    std::time::Duration::from_millis(INVESTIGATION_TRANSITION_MILLIS as u64 + 250)
}

async fn create_investigation(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation"),
        Some(body),
    )
    .await
}

async fn get_investigation(
    router: &Router,
    detector_id: &str,
    investigation_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/investigation/{investigation_id}"),
        None,
    )
    .await
}

/// Initiate an investigation and return its id, failing the test if the call did not
/// succeed. `CreateInvestigation` answers the recorded `200` (see that operation's
/// `SUCCESS_STATUS`, which states why it is not the model's `202`).
async fn initiated(router: &Router, detector_id: &str, prompt: &str) -> String {
    let (status, body) =
        create_investigation(router, detector_id, json!({"triggerPrompt": prompt})).await;
    assert_eq!(
        status, 200,
        "create-investigation {prompt:?}: expected 200, got {status} {body:?}"
    );
    body["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("create-investigation {prompt:?}: expected an id, got {body:?}"))
        .to_string()
}

/// The `Investigation` a read reports.
async fn read(router: &Router, detector_id: &str, investigation_id: &str) -> Value {
    let (status, body) = get_investigation(router, detector_id, investigation_id).await;
    assert_eq!(
        status, 200,
        "get-investigation {investigation_id}: expected 200, got {status} {body:?}"
    );
    body["investigation"].clone()
}

// ---------------------------------------------------------------------------
// Rule 1: what a create stored is what a read reports.
// ---------------------------------------------------------------------------

/// The prompts span the whole class `TriggerPrompt` admits — `len 1..2048`, no
/// pattern (`smithy_query(operation="CreateInvestigation")`) — differing in SIZE
/// (both ends of the modelled range), ALPHABET (ASCII, non-ASCII, control
/// whitespace) and FORM (the shapes the member's own documentation gives as
/// examples).
fn prompt_rows() -> Vec<(String, &'static str)> {
    vec![
        ("I".to_string(), "the shortest prompt the model accepts"),
        (
            "Investigate finding 1ab2c3d4e5f6a7b8c9d0e1f2a3b4c5d6 in account 000000000000"
                .to_string(),
            "the documented finding-and-account form",
        ),
        (
            "Analyze findings in my organization".to_string(),
            "the documented organization-wide form",
        ),
        (
            "Анализировать подозрительную активность IAM — 調査".to_string(),
            "a non-ASCII prompt",
        ),
        (
            "first line\nsecond line\twith a tab".to_string(),
            "a prompt carrying control whitespace",
        ),
        (
            "x".repeat(2048),
            "the longest prompt the model accepts (2048)",
        ),
    ]
}

#[tokio::test]
async fn an_investigation_reports_back_everything_the_create_stored() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let account = engine::emulator_account_id();

    let mut seen: Vec<String> = Vec::new();
    for (prompt, case) in prompt_rows() {
        let id = initiated(&router, &detector_id, &prompt).await;

        // `InvestigationId` is `len 1..64, pattern ^[a-fA-F0-9\-]+$`.
        assert!(
            !id.is_empty() && id.len() <= 64,
            "case {case}: expected an InvestigationId of 1..64 characters, got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "case {case}: expected a hexadecimal-and-hyphen InvestigationId, got {id:?}"
        );
        assert!(
            !seen.contains(&id),
            "case {case}: expected a fresh InvestigationId, got the earlier {id:?}"
        );
        seen.push(id.clone());

        let got = read(&router, &detector_id, &id).await;
        assert_eq!(
            got["investigationId"],
            json!(id.clone()),
            "case {case}: expected the read to report the minted id, got {got:?}"
        );
        assert_eq!(
            got["triggerPrompt"],
            json!(prompt.clone()),
            "case {case}: expected the prompt stored verbatim, got {:?}",
            got["triggerPrompt"]
        );
        assert_eq!(
            got["triggeredBy"],
            json!(account),
            "case {case}: expected the initiating account, got {got:?}"
        );
        // `Investigation.Cloud` carries the environment the investigation itself
        // resolved, so an investigation whose pass is still RUNNING reports none; the
        // member is never rebuilt from the request's endpoint context. The
        // `investigation_cloud_environment` suite is where the whole rule, both
        // phases, is asserted over a table.
        if got["status"] == json!(RUNNING) {
            assert_eq!(
                got["cloud"],
                Value::Null,
                "case {case}: expected no cloud block on an investigation that resolved none, got {got:?}"
            );
        }
        assert!(
            got["startTime"].as_f64().unwrap_or_default() > 0.0,
            "case {case}: expected a startTime instant, got {:?}",
            got["startTime"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the status moves only when the clock owes it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_investigation_settles_only_once_the_clock_owes_it() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let id = initiated(&router, &detector_id, "summarize suspicious IAM activity").await;

    // The transient status is observable: the span is never zero, so a caller
    // reading straight after the create still sees RUNNING.
    let fresh = read(&router, &detector_id, &id).await;
    assert_eq!(
        fresh["status"],
        json!(RUNNING),
        "a just-created investigation: expected {RUNNING}, got {fresh:?}"
    );
    assert!(
        fresh["endTime"].is_null() && fresh["error"].is_null(),
        "a {RUNNING} investigation has not ended: got {fresh:?}"
    );

    // A read does not move it: the second read sees exactly what the first did.
    let again = read(&router, &detector_id, &id).await;
    assert_eq!(
        again, fresh,
        "reading an investigation must not change what a later read sees: first {fresh:?}, second {again:?}"
    );

    tokio::time::sleep(past_the_transition()).await;

    let settled = read(&router, &detector_id, &id).await;
    assert_eq!(
        settled["status"],
        json!(FAILED),
        "once the clock owes the transition: expected {FAILED}, got {settled:?}"
    );
    let reason = settled["error"].as_str().unwrap_or_default();
    assert!(
        !reason.is_empty() && reason.len() <= 2048,
        "a {FAILED} investigation carries the reason it failed, got {settled:?}"
    );
    assert!(
        settled["summary"].is_null()
            && settled["riskLevel"].is_null()
            && settled["confidence"].is_null(),
        "no analysis ran, so nothing may report an assessment: got {settled:?}"
    );
    let (start, end) = (
        settled["startTime"].as_f64().unwrap_or_default(),
        settled["endTime"].as_f64().unwrap_or_default(),
    );
    assert!(
        end >= start && start > 0.0,
        "expected startTime <= endTime, got start {start} end {end}"
    );
    assert_eq!(
        settled["triggerPrompt"], fresh["triggerPrompt"],
        "settling must not rewrite the stored prompt: got {settled:?}"
    );

    // Terminal is terminal: a later read reports the same instant and status.
    tokio::time::sleep(past_the_transition()).await;
    let terminal = read(&router, &detector_id, &id).await;
    assert_eq!(
        terminal, settled,
        "a terminal investigation never moves again: first {settled:?}, later {terminal:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: ClientToken idempotency.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_returns_the_first_investigation() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    // The rows differ in the SIZE and ALPHABET of the token: `ClientToken` is
    // `len 0..64` (`smithy_query(operation="CreateInvestigation")`), so both the
    // one-character token and the 64-character one are values of the member.
    let tokens: Vec<(String, &str)> = vec![
        ("a".to_string(), "the shortest token"),
        (
            "2f632e2c-94e0-4782-b293-bb0b8064ad93".to_string(),
            "a UUID token, the form an SDK generates",
        ),
        ("t".repeat(64), "the longest token the model accepts"),
    ];

    let mut minted: Vec<String> = Vec::new();
    for (token, case) in &tokens {
        let (status, first) = create_investigation(
            &router,
            &detector_id,
            json!({"triggerPrompt": "first attempt", "clientToken": token}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {first:?}"
        );
        let (status, replayed) = create_investigation(
            &router,
            &detector_id,
            json!({"triggerPrompt": "a replay carrying the same token", "clientToken": token}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected the replay to be accepted, got {status} {replayed:?}"
        );
        assert_eq!(
            replayed["investigationId"], first["investigationId"],
            "case {case}: expected the replay to return the first investigation, got {replayed:?}"
        );
        let id = first["investigationId"].as_str().unwrap_or_default();
        // The replay initiated nothing: the stored investigation still carries the
        // FIRST call's prompt.
        let got = read(&router, &detector_id, id).await;
        assert_eq!(
            got["triggerPrompt"],
            json!("first attempt"),
            "case {case}: a replay must not rewrite the investigation, got {got:?}"
        );
        assert!(
            !minted.contains(&id.to_string()),
            "case {case}: a different token must initiate a different investigation, got {id:?}"
        );
        minted.push(id.to_string());
    }

    // No token at all is not a replay of anything: each call initiates its own.
    let bare_one = initiated(&router, &detector_id, "no token").await;
    let bare_two = initiated(&router, &detector_id, "no token").await;
    assert_ne!(
        bare_one, bare_two,
        "two tokenless creates must initiate two investigations, got {bare_one:?} twice"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: the AI_ANALYST feature gates the operation.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_stored_ai_analyst_feature_decides_the_call() {
    // (how AI_ANALYST was switched off, the create body, the update body)
    let cases: [(&str, Value, Option<Value>); 2] = [
        (
            "switched off by the create",
            json!({"enable": true, "features": [{"name": "AI_ANALYST", "status": "DISABLED"}]}),
            None,
        ),
        (
            "switched off by a later update",
            json!({"enable": true}),
            Some(json!({"features": [{"name": "AI_ANALYST", "status": "DISABLED"}]})),
        ),
    ];

    for (case, create_body, update_body) in cases {
        let router = new_router();
        let (status, created) = create_detector(&router, create_body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case}: create-detector {create_body}: got {status} {created:?}"
        );
        let detector_id = created["detectorId"].as_str().unwrap_or_default().to_string();
        if let Some(update) = update_body {
            let (status, body) = call(
                &router,
                "POST",
                &format!("/detector/{detector_id}"),
                Some(update),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case}: update-detector: got {status} {body:?}"
            );
        }

        let (status, denied) = create_investigation(
            &router,
            &detector_id,
            json!({"triggerPrompt": "investigate without the feature"}),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {case}: expected 403 without AI_ANALYST, got {status} {denied:?}"
        );
        assert_eq!(
            error_code(&denied),
            "AccessDeniedException",
            "case {case}: expected AccessDeniedException, got {denied:?}"
        );

        // Enabling the feature is all that changes, and the same call is accepted.
        let (status, body) = call(
            &router,
            "POST",
            &format!("/detector/{detector_id}"),
            Some(json!({"features": [{"name": "AI_ANALYST", "status": "ENABLED"}]})),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: update-detector enabling AI_ANALYST: got {status} {body:?}"
        );
        let id = initiated(&router, &detector_id, "investigate with the feature").await;
        let got = read(&router, &detector_id, &id).await;
        assert_eq!(
            got["investigationId"],
            json!(id.clone()),
            "case {case}: expected the investigation to be readable, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: how an id the caller is not authorized on, a malformed one and an
// absent sub-resource are each refused.
// ---------------------------------------------------------------------------

/// Values that cannot be a `DetectorId` at all, each with why. A detector id is 32
/// hexadecimal characters, so the rows differ in LENGTH (both sides of 32) and in
/// ALPHABET; the model's own constraint is `len 1..300`, far wider, so every row
/// reaches the handler.
const MALFORMED_DETECTOR_IDS: [(&str, &str); 4] = [
    ("deadbeefdeadbeefdeadbeefdeadbee", "31 characters"),
    ("deadbeefdeadbeefdeadbeefdeadbeef0", "33 characters"),
    (
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        "32 characters outside the hexadecimal alphabet",
    ),
    ("not-a-detector", "a value that is not an id at all"),
];

#[tokio::test]
async fn an_unauthorized_detector_is_denied_and_a_malformed_one_is_refused() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    let investigation_id = initiated(&router, &detector_id, "a real investigation").await;

    // A well-formed id this account/Region does not hold: the caller's permission
    // on it is decided before it is resolved, so the answer is 403 and says nothing
    // about whether it names anything.
    let created_elsewhere = create_investigation(
        &router,
        ABSENT_DETECTOR_ID,
        json!({"triggerPrompt": "investigate someone else's detector"}),
    )
    .await;
    let read_elsewhere = get_investigation(&router, ABSENT_DETECTOR_ID, &investigation_id).await;
    for (operation, (status, body)) in [
        ("CreateInvestigation", created_elsewhere),
        ("GetInvestigation", read_elsewhere),
    ] {
        assert_eq!(
            status, 403,
            "{operation} on a detector the account does not hold: expected 403, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDeniedException",
            "{operation} on a detector the account does not hold: got {body:?}"
        );
        let message = body["message"].as_str().unwrap_or_default();
        assert!(
            !message.contains(ABSENT_DETECTOR_ID),
            "{operation}: a denial must not echo the caller's id, got {message:?}"
        );
    }

    // A value that cannot be a detector id at all never named a resource to be
    // authorized on: that is a parameter failure, HTTP 400.
    for (malformed, case) in MALFORMED_DETECTOR_IDS {
        let (status, body) = create_investigation(
            &router,
            malformed,
            json!({"triggerPrompt": "investigate a malformed id"}),
        )
        .await;
        assert_eq!(
            status, 400,
            "CreateInvestigation with {case}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "CreateInvestigation with {case}: got {body:?}"
        );

        let (status, body) = get_investigation(&router, malformed, &investigation_id).await;
        assert_eq!(
            status, 400,
            "GetInvestigation with {case}: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "GetInvestigation with {case}: got {body:?}"
        );
    }

    // An `InvestigationId` the authorized detector does not hold is the modelled
    // 404 — the resource, not the permission, is what is missing. The rows are
    // values the member's `^[a-fA-F0-9\-]+$` pattern admits, so each reaches the
    // handler.
    let longest_id = "f".repeat(64);
    for (absent, case) in [
        (
            "deadbeef-dead-beef-dead-beefdeadbeef",
            "a UUID nothing minted",
        ),
        ("0", "the shortest id the model accepts"),
        (
            longest_id.as_str(),
            "the longest id the model accepts (64)",
        ),
    ] {
        let (status, body) = get_investigation(&router, &detector_id, absent).await;
        assert_eq!(
            status, 404,
            "GetInvestigation with {case}: expected 404, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "GetInvestigation with {case}: got {body:?}"
        );
    }

    // The investigations hang off the detector, so dropping it drops them: the pair
    // that read a moment ago is now a detector the account does not hold.
    let (status, body) = call(&router, "DELETE", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(status, 200, "delete-detector: got {status} {body:?}");
    let (status, body) = get_investigation(&router, &detector_id, &investigation_id).await;
    assert_eq!(
        status, 403,
        "GetInvestigation after the detector was deleted: expected 403, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the Regions the feature is served through.
// ---------------------------------------------------------------------------

/// Regions GuardDuty itself serves that are NOT in the Investigation catalogue,
/// spanning four geographies so the rows are not one recorded endpoint.
const REGIONS_WITHOUT_INVESTIGATIONS: [&str; 4] =
    ["us-west-1", "eu-south-1", "ap-southeast-2", "sa-east-1"];

#[tokio::test]
async fn investigations_are_served_exactly_through_the_documented_regions() {
    // Every published Region serves the operation — including the ones no recorded
    // call ever used.
    for region in INVESTIGATION_REGIONS {
        let router = new_router();
        let (status, created) =
            call_in(&router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
        assert_eq!(
            status, 200,
            "create-detector in {region}: got {status} {created:?}"
        );
        let detector_id = created["detectorId"].as_str().unwrap_or_default().to_string();

        let (status, body) = call_in(
            &router,
            region,
            "POST",
            &format!("/detector/{detector_id}/investigation"),
            Some(json!({"triggerPrompt": "investigate in a published Region"})),
        )
        .await;
        assert_eq!(
            status, 200,
            "create-investigation in {region}: expected 200, got {status} {body:?}"
        );
        let id = body["investigationId"].as_str().unwrap_or_default().to_string();
        let (status, got) = call_in(
            &router,
            region,
            "GET",
            &format!("/detector/{detector_id}/investigation/{id}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "get-investigation in {region}: expected 200, got {status} {got:?}"
        );
        assert_eq!(
            got["investigation"]["investigationId"],
            json!(id),
            "get-investigation in {region}: expected the investigation it initiated, got {got:?}"
        );
    }

    // A Region outside the catalogue does not serve it, whatever the store holds.
    for region in REGIONS_WITHOUT_INVESTIGATIONS {
        let router = new_router();
        let (status, created) =
            call_in(&router, region, "POST", "/detector", Some(json!({"enable": true}))).await;
        assert_eq!(
            status, 200,
            "create-detector in {region}: got {status} {created:?}"
        );
        let detector_id = created["detectorId"].as_str().unwrap_or_default().to_string();

        for (operation, method, path, body) in [
            (
                "CreateInvestigation",
                "POST",
                format!("/detector/{detector_id}/investigation"),
                Some(json!({"triggerPrompt": "investigate in an unpublished Region"})),
            ),
            (
                "GetInvestigation",
                "GET",
                format!("/detector/{detector_id}/investigation/deadbeef-dead-beef-dead-beefdeadbeef"),
                None,
            ),
        ] {
            let (status, answered) = call_in(&router, region, method, &path, body).await;
            assert_eq!(
                status, 400,
                "{operation} in {region}: expected 400, got {status} {answered:?}"
            );
            assert_eq!(
                error_code(&answered),
                "InvalidInputException",
                "{operation} in {region}: got {answered:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 7: the documented preview limit.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_documented_daily_limit_refuses_the_create_past_it() {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;

    let mut ids: Vec<String> = Vec::new();
    for attempt in 1..=INVESTIGATIONS_PER_DAY {
        let id = initiated(&router, &detector_id, &format!("investigation {attempt}")).await;
        assert!(
            !ids.contains(&id),
            "attempt {attempt}: expected a fresh id, got the earlier {id:?}"
        );
        ids.push(id);
    }

    // The attempt past the limit is refused, and twice over: the refusal is
    // computed from what the store holds, not from a counter the first refusal
    // consumed.
    for attempt in [INVESTIGATIONS_PER_DAY + 1, INVESTIGATIONS_PER_DAY + 2] {
        let (status, body) = create_investigation(
            &router,
            &detector_id,
            json!({"triggerPrompt": format!("investigation {attempt}")}),
        )
        .await;
        assert_eq!(
            status, 400,
            "attempt {attempt} past the daily limit: expected 400, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "attempt {attempt} past the daily limit: got {body:?}"
        );
    }

    // A refused create persisted nothing: every investigation the limit allowed is
    // still readable, and nothing else was initiated.
    for id in &ids {
        let got = read(&router, &detector_id, id).await;
        assert_eq!(
            got["investigationId"],
            json!(id.clone()),
            "expected every accepted investigation to survive the refusals, got {got:?}"
        );
    }
}
