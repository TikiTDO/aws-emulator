// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WINDOW a call carrying this service's `@idempotencyToken`
//! reports as still in flight — the span that decides whether a repeat of that call is the
//! racer AWS refuses with `ConflictException` (HTTP 409) or the replay it answers with the
//! resource the first call minted.
//!
//! RULE under test: the span is one a caller acting the instant it holds the first answer
//! still observes, so a repeat issued at ANY instant inside it is refused with
//! `ConflictException` and a repeat issued once it has passed replays the first resource —
//! the same answer whichever of this service's token-carrying calls it is, and whether the
//! repeat arrives at once or late in the span. A window shorter than the gap between two of
//! a client's own calls would make which of those two answers a caller gets depend on how
//! loaded the host is rather than on when the call was made.
//!
//! The rows differ along what the rule generalises over: WHICH operation carries the token
//! (every one of the five that does — the four creates and the job start), and HOW FAR into
//! the window the repeat arrives (at once, shortly after, and well into it). The delays the
//! refusal rows use are stated as absolute spans rather than read off the window, so a
//! window shortened back past them fails here; only the replay rows wait the window itself
//! out.
//!
//! `tests/all/aws_worded_refusals.rs` owns the SENTENCE that refusal carries and
//! `tests/all/onboarding_loop.rs`, `tests/all/transformer_lifecycle.rs` and
//! `tests/all/transformer_jobs.rs` own what one token names; the axis here is the window
//! itself. The wire harness, the router and the fixtures are those suites', so a request is
//! built exactly one way across this crate.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::state::IDEMPOTENT_CREATE_IN_FLIGHT;
use smithy_server_runtime::MultiServiceRouter;

use crate::edi_parsing::{given_object, BUCKET, X12_850};
use crate::onboarding_loop::{
    b2bi_router, call, capability_configuration, error_code, given_capability,
    given_capability_buckets, given_profile, given_transformer, text,
};
use crate::transformer_jobs::set_status;

/// How long after the first answer each refusal row sends its repeat.
///
/// Absolute spans, not fractions of the window: at once (the retry a client wrapper sends
/// the moment it has an answer), shortly after, and well into the window. Every one of them
/// is inside [`IDEMPOTENT_CREATE_IN_FLIGHT`] with room left over for the call itself, so a
/// row is about where in the window the repeat landed and not about the window's edge.
const DELAYS_INSIDE_THE_WINDOW: &[(&str, Duration)] = &[
    ("at once", Duration::ZERO),
    ("a quarter of a second later", Duration::from_millis(250)),
    ("a second later", Duration::from_secs(1)),
];

/// One token-carrying operation: its name, the member its answer reports the minted id
/// under, and a request for it, minus the token each row adds.
struct Tokened {
    case: &'static str,
    operation: &'static str,
    id_member: &'static str,
    request: Value,
}

/// Every operation of this service that carries an `@idempotencyToken`, with a request the
/// service serves and the fixtures that request needs already in place.
async fn tokened_operations(router: &MultiServiceRouter) -> Vec<Tokened> {
    let profile_id = given_profile(router, "window").await;
    let capability_id = given_capability(router, "window-capability").await;
    let transformer_id = given_transformer(router, "window-transformer").await;
    given_capability_buckets(router).await;

    // The job start needs an ACTIVE transformer of its own and an object to read.
    let job_transformer_id = given_transformer(router, "window-job-transformer").await;
    set_status(router, &job_transformer_id, "active").await;
    given_object(router, "inbound/window.edi", X12_850).await;

    vec![
        Tokened {
            case: "a profile",
            operation: "CreateProfile",
            id_member: "profileId",
            request: json!({
                "name": "window profile",
                "phone": "555-555-0177",
                "businessName": "Window Corp",
                "logging": "DISABLED"
            }),
        },
        Tokened {
            case: "a capability",
            operation: "CreateCapability",
            id_member: "capabilityId",
            request: json!({
                "name": "window-capability-row",
                "type": "edi",
                "configuration": capability_configuration(
                    &transformer_id,
                    "X12_850",
                    "VERSION_4010",
                ),
            }),
        },
        Tokened {
            case: "a partnership",
            operation: "CreatePartnership",
            id_member: "partnershipId",
            request: json!({
                "profileId": profile_id,
                "name": "window partner",
                "email": "partner@example.com",
                "capabilities": [capability_id],
            }),
        },
        Tokened {
            case: "a transformer",
            operation: "CreateTransformer",
            id_member: "transformerId",
            request: json!({
                "name": "window-transformer-row",
                "inputConversion": { "fromFormat": "X12" },
            }),
        },
        Tokened {
            case: "a transformer job",
            operation: "StartTransformerJob",
            id_member: "transformerJobId",
            request: json!({
                "transformerId": job_transformer_id,
                "inputFile": { "bucketName": BUCKET, "key": "inbound/window.edi" },
                "outputLocation": { "bucketName": BUCKET, "key": "window-out/" },
            }),
        },
    ]
}

/// `request` carrying `token` as its `clientToken`.
fn with_token(request: &Value, token: &str) -> Value {
    let mut request = request.clone();
    request["clientToken"] = json!(token);
    request
}

/// Send `request` under `token` and assert it is served, answering the id it minted.
async fn served_under(
    router: &MultiServiceRouter,
    row: &Tokened,
    token: &str,
    case: &str,
) -> String {
    let (status, body) = call(router, row.operation, with_token(&row.request, token)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first call under a fresh token must be served, got {status} {body}"
    );
    let id = text(&body, row.id_member);
    assert!(
        !id.is_empty(),
        "case {case:?}: the first call must answer with a {}, got {body}",
        row.id_member
    );
    id
}

/// RULE: a repeat issued at any instant inside the in-flight window is the racer that lost,
/// refused with `ConflictException` (409) — however far into the window it arrives, and
/// whichever token-carrying operation it repeats.
#[tokio::test]
async fn a_repeat_inside_the_in_flight_window_is_refused_however_late_it_arrives() {
    let router = b2bi_router();
    for row in tokened_operations(&router).await {
        for (delay_case, delay) in DELAYS_INSIDE_THE_WINDOW {
            let case = format!("{} repeated {delay_case}", row.case);
            let token = format!("window-{}-{}", row.operation, delay.as_millis());

            served_under(&router, &row, &token, &case).await;

            tokio::time::sleep(*delay).await;
            let (status, raced) =
                call(&router, row.operation, with_token(&row.request, &token)).await;
            assert_eq!(
                status, 409,
                "case {case:?}: a repeat inside the in-flight window must be refused, got \
                 {status} {raced}"
            );
            let code = error_code(&raced);
            assert_eq!(
                code, "ConflictException",
                "case {case:?}: expected a ConflictException, got {code:?} in {raced}"
            );
        }
    }
}

/// RULE: the window is BOUNDED — once it has passed the same token answers with the
/// resource the first call minted, for every token-carrying operation, and mints nothing
/// second.
#[tokio::test]
async fn the_same_token_replays_the_first_resource_once_the_window_has_passed() {
    let router = b2bi_router();
    for row in tokened_operations(&router).await {
        let case = row.case;
        let token = format!("settled-{}", row.operation);
        let first = served_under(&router, &row, &token, case).await;

        tokio::time::sleep(IDEMPOTENT_CREATE_IN_FLIGHT).await;
        let (status, replay) = call(&router, row.operation, with_token(&row.request, &token)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: once the window has passed the same token must replay, got \
             {status} {replay}"
        );
        let replayed = text(&replay, row.id_member);
        assert_eq!(
            replayed, first,
            "case {case:?}: the replay must answer with the first {}, got {replayed:?} vs \
             {first:?}",
            row.id_member
        );
    }
}
