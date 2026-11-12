// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the WINDOW a call carrying this service's `@idempotencyToken` is
//! IN FLIGHT for — the window that decides whether a repeat of that call is the racer AWS
//! refuses with `ConflictException` (HTTP 409) or the replay it answers with the resource
//! the first call minted.
//!
//! RULE under test: the window is the processing of the request that holds the token — the
//! token is claimed by that request in one atomic check-and-insert and is replaced by what
//! it minted when it answers — so a repeat arriving while the claim stands is refused with
//! `ConflictException`, a repeat arriving once the request has answered replays the
//! resource it minted however many times it is sent, and however many calls race under one
//! token exactly one resource is minted. Which of the two answers a repeat gets never
//! depends on how much time has passed, so it never depends on how loaded the host is.
//!
//! The rows differ along what the rule generalises over: WHICH operation carries the token
//! (every one of the five that does — the four creates and the job start), how many calls
//! race under one token, and how many repeats follow an answered request. The window is
//! held open and released through the store's own claim, the way the handler holds it while
//! it works, so no row waits on the clock: nothing here sleeps, and no assertion is about
//! elapsed time.
//!
//! `tests/all/aws_worded_refusals.rs` owns the SENTENCE that refusal carries and
//! `tests/all/onboarding_loop.rs`, `tests/all/transformer_lifecycle.rs` and
//! `tests/all/transformer_jobs.rs` own what one token names; the axis here is the window
//! itself. The wire harness, the router and the fixtures are those suites', so a request is
//! built exactly one way across this crate.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::state::{
    self, ComAmazonawsB2biB2biState, TokenReplay,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::edi_parsing::{given_object, BUCKET, X12_850};
use crate::onboarding_loop::{
    b2bi_router_with_engine, call, capability_configuration, error_code, given_capability,
    given_capability_buckets, given_profile, given_transformer, text,
};
use crate::transformer_jobs::set_status;

/// How many calls race under one token, per row of the racing table: a pair, a handful,
/// and more racers than the runtime has worker threads.
const RACER_COUNTS: &[usize] = &[2, 5, 17];

/// One token-carrying operation: its name, the resource type its token is scoped by (the
/// scope [`state::idempotency_key`] takes), the member its answer reports the minted id
/// under, a request for it minus the token each row adds, and the List operation that
/// counts what it minted (the job start has none — `GetTransformerJob` resolves one job by
/// id and this service lists no jobs).
struct Tokened {
    case: &'static str,
    operation: &'static str,
    resource_type: &'static str,
    id_member: &'static str,
    request: Value,
    list: Option<(&'static str, &'static str)>,
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
            resource_type: "profile",
            id_member: "profileId",
            request: json!({
                "name": "window profile",
                "phone": "555-555-0177",
                "businessName": "Window Corp",
                "logging": "DISABLED"
            }),
            list: Some(("ListProfiles", "profiles")),
        },
        Tokened {
            case: "a capability",
            operation: "CreateCapability",
            resource_type: "capability",
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
            list: Some(("ListCapabilities", "capabilities")),
        },
        Tokened {
            case: "a partnership",
            operation: "CreatePartnership",
            resource_type: "partnership",
            id_member: "partnershipId",
            request: json!({
                "profileId": profile_id,
                "name": "window partner",
                "email": "partner@example.com",
                "capabilities": [capability_id],
            }),
            list: Some(("ListPartnerships", "partnerships")),
        },
        Tokened {
            case: "a transformer",
            operation: "CreateTransformer",
            resource_type: "transformer",
            id_member: "transformerId",
            request: json!({
                "name": "window-transformer-row",
                "inputConversion": { "fromFormat": "X12" },
            }),
            list: Some(("ListTransformers", "transformers")),
        },
        Tokened {
            case: "a transformer job",
            operation: "StartTransformerJob",
            resource_type: "transformerJob",
            id_member: "transformerJobId",
            request: json!({
                "transformerId": job_transformer_id,
                "inputFile": { "bucketName": BUCKET, "key": "inbound/window.edi" },
                "outputLocation": { "bucketName": BUCKET, "key": "window-out/" },
            }),
            list: None,
        },
    ]
}

/// `request` carrying `token` as its `clientToken`.
fn with_token(request: &Value, token: &str) -> Value {
    let mut request = request.clone();
    request["clientToken"] = json!(token);
    request
}

/// This service's store inside the engine the router under test serves from — the one the
/// handlers claim their tokens in.
pub(crate) fn b2bi_store(
    engine: &engine::InMemoryEngine,
) -> engine::ServiceState<ComAmazonawsB2biB2biState> {
    state::service_state(engine).expect("the b2bi service store is reachable")
}

/// Hold `token` of `resource_type` IN FLIGHT, exactly as the request carrying it holds it
/// while it is being processed, and assert the claim was free to take.
///
/// This is what keeps the window open for a row that asserts the refusal: the claim is the
/// state the handler itself puts the token in, and [`release_token`] ends it the way a
/// request that minted nothing does.
pub(crate) fn hold_token_in_flight(
    engine: &engine::InMemoryEngine,
    resource_type: &str,
    token: &str,
    case: &str,
) {
    let key = state::idempotency_key(resource_type, token);
    let held = b2bi_store(engine).lock().claim_idempotency(&key);
    assert!(
        held.is_none(),
        "case {case:?}: the token must be free to claim, got {held:?}"
    );
}

/// Release the claim on `token`, as the request holding it does when it ends without
/// minting anything.
pub(crate) fn release_token(engine: &engine::InMemoryEngine, resource_type: &str, token: &str) {
    let key = state::idempotency_key(resource_type, token);
    b2bi_store(engine)
        .lock()
        .release_idempotency(&key);
}

/// What the store says `token` of `resource_type` is holding, if anything.
pub(crate) fn token_state(
    engine: &engine::InMemoryEngine,
    resource_type: &str,
    token: &str,
) -> Option<TokenReplay> {
    let key = state::idempotency_key(resource_type, token);
    b2bi_store(engine).lock().replay(&key)
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

/// How many resources `row`'s List operation reports.
async fn listed(router: &MultiServiceRouter, row: &Tokened) -> usize {
    let Some((list_op, items)) = row.list else {
        return 0;
    };
    let (status, body) = call(router, list_op, json!({})).await;
    assert_eq!(status, 200, "{list_op} failed: {status} {body}");
    body[items].as_array().map(Vec::len).unwrap_or_default()
}

/// RULE: a repeat that arrives while the request holding the token is still in flight is
/// the racer that lost, refused with `ConflictException` (409) and minting nothing — and
/// once that claim ends with nothing minted the very same request is served. Asserted for
/// every operation of this service that carries an `@idempotencyToken`.
#[tokio::test]
async fn a_repeat_of_a_request_still_in_flight_is_refused_and_mints_nothing() {
    let (router, engine) = b2bi_router_with_engine();
    for row in tokened_operations(&router).await {
        let case = row.case;
        let token = format!("in-flight-{}", row.operation);

        let before = listed(&router, &row).await;
        hold_token_in_flight(&engine, row.resource_type, &token, case);

        let (status, raced) = call(&router, row.operation, with_token(&row.request, &token)).await;
        assert_eq!(
            status, 409,
            "case {case:?}: a repeat of a request still in flight must be refused, got \
             {status} {raced}"
        );
        let code = error_code(&raced);
        assert_eq!(
            code, "ConflictException",
            "case {case:?}: expected a ConflictException, got {code:?} in {raced}"
        );
        let after_refusal = listed(&router, &row).await;
        assert_eq!(
            after_refusal, before,
            "case {case:?}: the refused repeat must mint nothing, got {after_refusal} \
             records against {before}"
        );
        assert_eq!(
            token_state(&engine, row.resource_type, &token),
            Some(TokenReplay::InFlight),
            "case {case:?}: the refusal must leave the claim with the request that holds it"
        );

        // The claim ends without a resource — the request holding it was refused, or never
        // reached its store — so the token is free and the same call is served.
        release_token(&engine, row.resource_type, &token);
        let minted = served_under(&router, &row, &token, case).await;
        assert_eq!(
            token_state(&engine, row.resource_type, &token),
            Some(TokenReplay::Completed(minted.clone())),
            "case {case:?}: the served call must leave the token naming the {} it minted \
             ({minted:?})",
            row.id_member
        );
    }
}

/// RULE: the window is the request's own processing, and it is OVER as soon as that request
/// has answered — so a repeat sent straight afterwards replays the resource the first call
/// minted, however many repeats are sent, and mints nothing second.
#[tokio::test]
async fn a_repeat_after_the_request_answered_replays_it_however_often_it_is_sent() {
    let (router, engine) = b2bi_router_with_engine();
    for row in tokened_operations(&router).await {
        let case = row.case;
        let token = format!("answered-{}", row.operation);

        let before = listed(&router, &row).await;
        let first = served_under(&router, &row, &token, case).await;
        assert_eq!(
            token_state(&engine, row.resource_type, &token),
            Some(TokenReplay::Completed(first.clone())),
            "case {case:?}: the answered call must leave the token naming what it minted, \
             not a claim still in flight"
        );

        // Three repeats, sent one after another with nothing waited for in between: the
        // first answer having been received is the whole of what ends the window.
        for repeat in 1..=3 {
            let case = format!("{case} repeat {repeat}");
            let (status, replay) =
                call(&router, row.operation, with_token(&row.request, &token)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a repeat of an answered request must replay it, got \
                 {status} {replay}"
            );
            let replayed = text(&replay, row.id_member);
            assert_eq!(
                replayed, first,
                "case {case:?}: the replay must answer with the first {}, got {replayed:?} \
                 vs {first:?}",
                row.id_member
            );
        }

        let after = listed(&router, &row).await;
        assert_eq!(
            after,
            before + row.list.map(|_| 1).unwrap_or_default(),
            "case {case:?}: one token must leave exactly one record behind, got {after} \
             against {before}"
        );
    }
}

/// RULE: however many calls race under ONE token, exactly one resource is minted: the
/// claim is a single atomic check-and-insert, so every racer either wins it, replays what
/// the winner minted, or is refused with `ConflictException` (409) — and never mints a
/// second resource.
///
/// The rows differ along the racer COUNT (a pair, a handful, and more racers than the
/// runtime's worker threads) and along which create they race, and the racers are SPAWNED
/// so they genuinely contend for the store rather than being polled one after another.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn racing_calls_under_one_token_mint_exactly_one_resource() {
    let (router, engine) = b2bi_router_with_engine();
    let rows = tokened_operations(&router).await;
    let router = Arc::new(router);

    for row in &rows {
        // The job start has no List operation to count with, and its racers are covered by
        // the claim rows above; the four creates are what this table races.
        let Some(_) = row.list else { continue };
        for racers in RACER_COUNTS {
            let case = format!("{} raced by {racers} calls", row.case);
            let token = format!("raced-{}-{racers}", row.operation);
            let before = listed(&router, row).await;

            let mut running = Vec::new();
            for _ in 0..*racers {
                let router = Arc::clone(&router);
                let operation = row.operation;
                let request = with_token(&row.request, &token);
                running.push(tokio::spawn(async move {
                    call(&router, operation, request).await
                }));
            }

            let mut served: Vec<String> = Vec::new();
            for (index, racer) in running.into_iter().enumerate() {
                let (status, body) = racer.await.expect("the racer task runs to completion");
                match status {
                    200 => served.push(text(&body, row.id_member)),
                    409 => assert_eq!(
                        error_code(&body),
                        "ConflictException",
                        "case {case:?}: racer {index} lost the race, so it must be refused \
                         with ConflictException, got {body}"
                    ),
                    other => panic!(
                        "case {case:?}: racer {index} must be served or refused with a \
                         conflict, got {other} {body}"
                    ),
                }
            }

            assert!(
                !served.is_empty(),
                "case {case:?}: at least one racer must be served"
            );
            let one: Vec<&String> = served
                .iter()
                .filter(|id| **id != *served[0])
                .collect();
            assert!(
                one.is_empty(),
                "case {case:?}: every served racer must report the one resource the token \
                 minted, got {served:?}"
            );
            let after = listed(&router, row).await;
            assert_eq!(
                after,
                before + 1,
                "case {case:?}: {racers} racers under one token must add exactly one \
                 record, got {after} against {before}"
            );
            assert_eq!(
                token_state(&engine, row.resource_type, &token),
                Some(TokenReplay::Completed(served[0].clone())),
                "case {case:?}: the token must end up naming the one resource it minted"
            );
        }
    }
}
