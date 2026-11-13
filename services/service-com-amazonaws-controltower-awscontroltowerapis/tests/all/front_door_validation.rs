// SPDX-License-Identifier: CC0-1.0

//! How Control Tower answers a request that fails input validation, over HTTP.
//!
//! The rule under test: a request whose members do not satisfy the constraints the
//! Smithy model declares is refused by the service's REST front door with HTTP 400 and
//! the generic error code `BadRequestException` — NOT the per-member
//! `ValidationException` the operations model — for every operation and every kind of
//! violated constraint. The three cases the issue report cited (a `maxResults` above
//! its ceiling, and a non-ARN identifier on each of two operations) are three rows of
//! the table below.
//!
//! Two narrower rules keep the first one from over-reaching, and are asserted here too:
//!
//! * a request that satisfies every modeled constraint is NOT refused by the front
//!   door, including values sitting exactly ON each published bound;
//! * a request the front door passed and the HANDLER then refuses keeps the
//!   operation's own modeled `ValidationException`.
//!
//! The one parameter this service words a refusal of differently — the tag plane's
//! `resourceArn` path label, which takes a sentence about that parameter rather than the
//! generic one — is the subject of [`crate::tag_plane_arn_syntax`]; no row below carries
//! it.
//!
//! Every row names the model member and the constraint it violates; the bounds come
//! from `smithy_query(query="shape", shape="ListControlOperationsMaxResults,Arn,
//! ListEnabledBaselinesMaxResults,ListControlOperationsNextToken")` on this crate's
//! `smithy-model/`.

use crate::landing_zone_lifecycle::{call, emulator, set_up_account, simple_manifest};

/// The error code the front door names an input-validation refusal with.
pub(crate) const FRONT_DOOR_CODE: &str = "BadRequestException";

/// The message that refusal carries. Constant: the front door refuses before the
/// request is dispatched, so it names neither the member nor the constraint.
pub(crate) const FRONT_DOOR_MESSAGE: &str = "Invalid request body";

/// An ARN-shaped value of exactly `length` characters that satisfies `Arn`'s
/// `@pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$`, so the only constraint a row varies is
/// `@length` (min 20, max 2048).
pub(crate) fn arn_of_length(length: usize) -> String {
    let prefix = "arn:aws";
    assert!(
        length > prefix.len(),
        "an ARN-shaped value needs at least {} characters, asked for {length}",
        prefix.len() + 1
    );
    format!("{prefix}{}", "a".repeat(length - prefix.len()))
}

/// RULE: a request that violates a modeled input constraint is refused with HTTP 400
/// and error code `BadRequestException`, whichever operation, member and constraint
/// kind it is.
#[tokio::test]
async fn a_modeled_constraint_violation_is_a_front_door_bad_request() {
    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        // ListControlOperationsMaxResults: @range min 1, max 100.
        (
            "maxResults one above the range max of 100",
            "/list-control-operations",
            serde_json::json!({ "maxResults": 101 }),
        ),
        (
            "maxResults one below the range min of 1",
            "/list-control-operations",
            serde_json::json!({ "maxResults": 0 }),
        ),
        (
            "maxResults negative",
            "/list-control-operations",
            serde_json::json!({ "maxResults": -7 }),
        ),
        // ListControlOperationsNextToken: @pattern \S+, which no all-whitespace
        // (or empty) string satisfies.
        (
            "nextToken that holds no non-whitespace character",
            "/list-control-operations",
            serde_json::json!({ "nextToken": "" }),
        ),
        // ControlOperationStatuses inside ControlOperationFilter: @length max 1, so
        // the violation is on a NESTED member path rather than a top-level one.
        (
            "filter.statuses longer than its max of 1",
            "/list-control-operations",
            serde_json::json!({ "filter": { "statuses": ["IN_PROGRESS", "SUCCEEDED"] } }),
        ),
        // Arn: @length min 20, max 2048, @pattern ^arn:aws[0-9a-zA-Z_\-:\/]+$.
        (
            "enabledBaselineIdentifier that is not an ARN at all",
            "/get-enabled-baseline",
            serde_json::json!({ "enabledBaselineIdentifier": "totally-not-an-arn" }),
        ),
        (
            "enabledControlIdentifier that is not an ARN at all",
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": "totally-not-an-arn" }),
        ),
        (
            "enabledControlIdentifier one character below the length min of 20",
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": arn_of_length(19) }),
        ),
        (
            "enabledControlIdentifier one character above the length max of 2048",
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": arn_of_length(2049) }),
        ),
        // @required members, absent: found while reading the request rather than while
        // walking it, and the same refusal to a caller.
        (
            "get-enabled-baseline without its required identifier",
            "/get-enabled-baseline",
            serde_json::json!({}),
        ),
        (
            "get-enabled-control without its required identifier",
            "/get-enabled-control",
            serde_json::json!({}),
        ),
        (
            "enable-control without its required members",
            "/enable-control",
            serde_json::json!({}),
        ),
        // An operation the issue did not cite, with a DIFFERENT published bound:
        // ListEnabledBaselinesMaxResults is @range min 5, max 100.
        (
            "list-enabled-baselines maxResults one below its own range min of 5",
            "/list-enabled-baselines",
            serde_json::json!({ "maxResults": 4 }),
        ),
        // BaselineArn: @pattern ...:baseline/[A-Z0-9]{16}$ — a well-formed ARN that
        // does not match the resource part.
        (
            "baselineIdentifier whose resource part fails BaselineArn's pattern",
            "/get-baseline",
            serde_json::json!({
                "baselineIdentifier":
                    "arn:aws:controltower:us-west-2:123456789012:baseline/notsixteenchars"
            }),
        ),
        // An IMPLEMENTED operation: the refusal is the front door's there too, so this
        // is not a property of scaffolded handlers. LandingZoneVersion: @length min 3.
        (
            "create-landingzone version below the length min of 3",
            "/create-landingzone",
            serde_json::json!({
                "version": "3",
                "manifest": simple_manifest()
            }),
        ),
    ];

    for (case, path, body) in cases {
        let router = emulator();
        let refused = call(&router, path, body).await;
        let message = refused
            .body
            .get("message")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();
        let wire_type = refused
            .body
            .get("__type")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string();

        assert_eq!(
            (refused.status, refused.error_code.as_deref()),
            (400, Some(FRONT_DOOR_CODE)),
            "case {case:?}: expected 400 {FRONT_DOOR_CODE}, got {:?} body {:?}",
            (refused.status, refused.error_code.as_deref()),
            refused.body
        );
        assert_eq!(
            wire_type, FRONT_DOOR_CODE,
            "case {case:?}: expected __type {FRONT_DOOR_CODE}, got {wire_type:?}"
        );
        assert_eq!(
            message, FRONT_DOOR_MESSAGE,
            "case {case:?}: expected the front door's message, got {message:?}"
        );
    }
}

/// RULE (the other side of it): a request that satisfies every modeled constraint is
/// never refused by the front door — including a value sitting exactly ON a published
/// bound.
#[tokio::test]
async fn a_request_within_every_modeled_bound_is_not_a_front_door_bad_request() {
    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "maxResults exactly the range min of 1",
            "/list-control-operations",
            serde_json::json!({ "maxResults": 1 }),
        ),
        (
            "maxResults exactly the range max of 100",
            "/list-control-operations",
            serde_json::json!({ "maxResults": 100 }),
        ),
        (
            "no optional member at all",
            "/list-control-operations",
            serde_json::json!({}),
        ),
        (
            "filter.statuses exactly its max length of 1",
            "/list-control-operations",
            serde_json::json!({ "filter": { "statuses": ["IN_PROGRESS"] } }),
        ),
        (
            "enabledControlIdentifier exactly the length min of 20",
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": arn_of_length(20) }),
        ),
        (
            "enabledControlIdentifier exactly the length max of 2048",
            "/get-enabled-control",
            serde_json::json!({ "enabledControlIdentifier": arn_of_length(2048) }),
        ),
        (
            "enabledBaselineIdentifier a well-formed ARN",
            "/get-enabled-baseline",
            serde_json::json!({
                "enabledBaselineIdentifier":
                    "arn:aws:controltower:us-west-2:123456789012:enabledbaseline/absent"
            }),
        ),
        (
            "list-enabled-baselines maxResults exactly its range min of 5",
            "/list-enabled-baselines",
            serde_json::json!({ "maxResults": 5 }),
        ),
    ];

    for (case, path, body) in cases {
        let router = emulator();
        // The account is set up first: the enablement reads Control Tower serves only for a
        // set-up account refuse an un-set-up one with their own modelled error, and what
        // this case is about is which refusals the FRONT DOOR owns.
        set_up_account(&router).await;
        let answer = call(&router, path, body).await;
        assert!(
            answer.status != 400 && answer.error_code.as_deref() != Some(FRONT_DOOR_CODE),
            "case {case:?}: a request inside every modeled bound must not be refused as a \
             bad request, got {:?} body {:?}",
            (answer.status, answer.error_code.as_deref()),
            answer.body
        );
    }
}

/// RULE: only the PRE-HANDLER refusal is the front door's. A request that satisfied
/// every modeled constraint and is then refused by the handler keeps the operation's
/// own modeled error — so the rewording above does not swallow the service's own
/// `ValidationException`.
#[tokio::test]
async fn a_handler_refusal_keeps_its_modeled_error() {
    let router = emulator();

    // `ListLandingZonesNextToken`'s pattern is \S+, which this satisfies, so the front
    // door passes the request and ListLandingZones itself refuses the token.
    let refused = call(
        &router,
        "/list-landingzones",
        serde_json::json!({ "nextToken": "not-a-token" }),
    )
    .await;
    assert_eq!(
        (refused.status, refused.error_code.as_deref()),
        (400, Some("ValidationException")),
        "a token the service never handed out: expected 400 ValidationException, got {:?} \
         body {:?}",
        (refused.status, refused.error_code.as_deref()),
        refused.body
    );

    // `manifest` carries no @required trait in this model, so the front door passes a
    // request without it and CreateLandingZone is the one that refuses it.
    let no_manifest = call(
        &router,
        "/create-landingzone",
        serde_json::json!({ "version": "3.3" }),
    )
    .await;
    assert_eq!(
        (no_manifest.status, no_manifest.error_code.as_deref()),
        (400, Some("ValidationException")),
        "a create with no manifest: expected 400 ValidationException, got {:?} body {:?}",
        (no_manifest.status, no_manifest.error_code.as_deref()),
        no_manifest.body
    );

    // And a well-formed request still succeeds and round-trips through the store.
    let created = call(
        &router,
        "/create-landingzone",
        serde_json::json!({
            "version": "3.3",
            "manifest": simple_manifest()
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "a well-formed create: expected 200, got {} body {:?}",
        created.status, created.body
    );

    let listed = call(&router, "/list-landingzones", serde_json::json!({})).await;
    let count = listed
        .body
        .get("landingZones")
        .and_then(|value| value.as_array())
        .map(|entries| entries.len())
        .unwrap_or_default();
    assert_eq!(
        count, 1,
        "the created landing zone must be readable back, got {:?}",
        listed.body
    );
}
