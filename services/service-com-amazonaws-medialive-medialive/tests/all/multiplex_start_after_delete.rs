// SPDX-License-Identifier: CC0-1.0

//! How a start of a multiplex whose delete this service has already ACCEPTED is
//! classified, end to end over the restJson1 wire.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! RULE: a multiplex's delete leaves its record RESOLVABLE — `DescribeMultiplex`
//! goes on reporting it while it is being torn down (`DELETING`) and in the
//! terminal `DELETED` state it then keeps — so a start aimed at it is judged on
//! the STATE that record is in and refused with the 409 `ConflictException` a
//! start's state precondition answers with, naming the multiplex and that state;
//! the 404 `NotFoundException` is the answer for a `MultiplexId` the account
//! holds no record for at all. Both forms of the start classify it the same way —
//! `StartMultiplex` and the `multiplexIds` arm of `BatchStart`, whose failed
//! entry carries that same code — and a refused start writes nothing.
//!
//! That classification is RECORDED, which is why it is pinned here: in the
//! `medialive.multiplex-start-without-programs-rejected` fixture the multiplex is
//! already absent from `ListMultiplexes` while a `StartMultiplex` sent straight
//! afterwards is answered 409 `ConflictException` on the `DELETING` state — the
//! record is still resolvable, so the refusal is a state conflict rather than the
//! not-found answer a record the account never held gets. The same recording is
//! where that refusal's wording comes from, so the message is asserted here too,
//! with its two variable parts filled as the service fills them: the id from the
//! request and the state from the stored record.
//!
//! The suites are TABLES: the recorded case (a start sent inside the `DELETING`
//! window of a multiplex that held no program) is ONE row of them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::multiplex_deletion::delete_multiplex;
use crate::multiplex_lifecycle::{describe_multiplex, settings};
use crate::multiplex_start::{add_programs, multiplex_with_settings, start_multiplex};
use crate::multiplex_stop::stop_multiplex;
use crate::wire::{call, error_code, error_message, medialive};

/// `BatchStart`'s binding (`smithy_query(query="operation",
/// operation="BatchStart")`: `POST /prod/batch/start`, success 200).
const BATCH_START: &str = "/prod/batch/start";

/// The error code a refusal on the resource's STATE carries, which on this
/// service is the modelled exception's shape name, in the error body of the
/// single-resource start and in the `Code` of a `BatchStart` failed entry alike.
const CONFLICT: &str = "ConflictException";

/// The error code a refusal on an id the account holds no record for carries,
/// in both of the same places.
const NOT_FOUND: &str = "NotFoundException";

/// The message a start refused on the multiplex's state carries, as the service
/// words that class (`crate::multiplex_start` asserts the same wording for the
/// states a live multiplex is refused in): the multiplex the request named and
/// the state its record is in.
fn not_startable(id: &str, state: &str) -> String {
    format!("Multiplex {id} cannot be started while in {state} state")
}

/// One `BatchStart` call: the status and the body it answered.
async fn batch_start(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, batched) = call(router, "POST", BATCH_START, Some(body)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the batch itself to answer 200, got {status}: {batched}"
    );
    batched
}

/// The one failed entry of a `BatchStart` that named one resource, with the call
/// asserted to have accepted nothing.
async fn only_failed_entry(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let batched = batch_start(router, case, json!({ "multiplexIds": [id] })).await;
    assert_eq!(
        batched["successful"],
        json!([]),
        "case {case:?}: expected no accepted entry, got {batched}"
    );
    let failed = batched["failed"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        failed.len(),
        1,
        "case {case:?}: expected exactly one failed entry, got {batched}"
    );
    failed[0].clone()
}

/// The multiplex `id`, read back through the reader a record a delete has been
/// accepted for stays visible in, asserted to report `state`.
async fn in_state(router: &MultiServiceRouter, case: &str, id: &str, state: &str) -> Value {
    let (status, described) = describe_multiplex(router, id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeMultiplex: expected the record to still be resolvable, got \
         {status}: {described}"
    );
    assert_eq!(
        described["state"],
        json!(state),
        "case {case:?}: the row did not reach {state}, it reached {:?}",
        described["state"]
    );
    described
}

/// A multiplex driven to the phase of an accepted delete the row asks for, as
/// its id and the record a reader still reports for it.
///
/// `ran` takes the multiplex up and back down before the delete, because a
/// delete is accepted from the stopped state only: it is what varies the history
/// behind the tombstone the start under test is aimed at.
async fn tombstone(
    router: &MultiServiceRouter,
    case: &str,
    phase: &str,
    programs: i64,
    ran: bool,
    multiplex_settings: Value,
) -> (String, Value) {
    let id = multiplex_with_settings(router, case, "doomed", multiplex_settings).await;
    add_programs(router, case, &id, programs).await;
    // A delete is accepted from the stopped state, which is the state the
    // create's own transition settles into.
    wait_for_the_clock();
    if ran {
        let (status, started) = start_multiplex(router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StartMultiplex: expected 202, got {status}: {started}"
        );
        wait_for_the_clock();
        let (status, stopping) = stop_multiplex(router, &id).await;
        assert_eq!(
            status, 202,
            "case {case:?}: StopMultiplex: expected 202, got {status}: {stopping}"
        );
        wait_for_the_clock();
    }

    let (status, deleting) = delete_multiplex(router, &id).await;
    assert_eq!(
        status, 202,
        "case {case:?}: DeleteMultiplex: expected 202, got {status}: {deleting}"
    );
    if phase == "DELETED" {
        wait_for_the_clock();
    }
    let described = in_state(router, case, &id, phase).await;
    (id, described)
}

/// Rule: the record a multiplex's accepted delete leaves behind is still
/// resolvable, so a start aimed at it is judged on the state that record is in
/// and refused with the modelled 409 `ConflictException` naming the multiplex and
/// that state — never with the not-found answer an id the account holds no record
/// for gets — and it writes nothing: the record reads back exactly as it did an
/// instant before the refusal.
///
/// The rows span what the rule generalises over: both phases of an accepted
/// delete (the transient `DELETING` the recorded fixture sends its start in, and
/// the terminal `DELETED` the clock then owes), how many programs the multiplex
/// held, its `MultiplexSettings` values, and whether it had been run and stopped
/// before the delete.
#[tokio::test]
async fn a_start_of_a_multiplex_whose_delete_was_accepted_conflicts_with_its_state() {
    // (case, the phase of the accepted delete, programs held, whether the
    //  multiplex was run and stopped before the delete, its settings)
    let rows: [(&str, &str, i64, bool, Value); 5] = [
        (
            "inside the DELETING window, holding no program",
            "DELETING",
            0,
            false,
            settings(2_000_000, 1),
        ),
        (
            "once the delete settled, holding no program",
            "DELETED",
            0,
            false,
            settings(2_000_000, 1),
        ),
        (
            "inside the DELETING window, holding two programs",
            "DELETING",
            2,
            false,
            settings(20_000_000, 42),
        ),
        (
            "once the delete settled, holding three programs",
            "DELETED",
            3,
            false,
            settings(100_000_000, 65_535),
        ),
        (
            "run and stopped before the delete",
            "DELETED",
            1,
            true,
            settings(1_000_000, 0),
        ),
    ];

    for (case, phase, programs, ran, multiplex_settings) in rows {
        let router = medialive();
        let (id, before) =
            tombstone(&router, case, phase, programs, ran, multiplex_settings).await;

        // The single-resource start: the state conflict, naming the multiplex and
        // the state its record is in.
        let (status, refusal) = start_multiplex(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected starting a {phase} multiplex to conflict, got {status}: \
             {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            CONFLICT,
            "case {case:?}: expected {CONFLICT}, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            not_startable(&id, phase),
            "case {case:?}: expected the refusal to name the multiplex and the {phase} state it \
             is in, got {refusal}"
        );

        // The batch form reports the same classification per entry, with the
        // record's own ARN beside it.
        let entry = only_failed_entry(&router, case, &id).await;
        assert_eq!(
            entry["code"],
            json!(CONFLICT),
            "case {case:?}: expected the batch entry to carry {CONFLICT}, got {entry}"
        );
        assert_eq!(
            entry["id"],
            json!(id),
            "case {case:?}: expected the entry to name the multiplex, got {entry}"
        );
        assert_eq!(
            entry["arn"], before["arn"],
            "case {case:?}: expected the entry to carry the record's ARN, got {entry}"
        );
        assert_eq!(
            entry["message"],
            json!(not_startable(&id, phase)),
            "case {case:?}: expected the entry to carry the same refusal the single-resource \
             start answers, got {entry}"
        );

        // Neither refused start moved anything: the record a reader still reports
        // is the one both were judged against.
        let after = in_state(&router, case, &id, phase).await;
        assert_eq!(
            after, before,
            "case {case:?}: expected the refused starts to leave the record alone, got {after}"
        );
    }
}

/// Rule: the not-found answer belongs to a `MultiplexId` the account holds no
/// record for — the one half of the start's refusals the resolution really
/// decides — so it is 404 `NotFoundException` in the single-resource form and the
/// same code, with no ARN to report, in the batch form, whatever the id is
/// spelled like.
///
/// The rows span the spellings an unheld id takes: one that looks like a minted
/// multiplex id, one that does not look like an id of this service at all, and
/// one that is numeric but far outside what this account has minted.
#[tokio::test]
async fn a_start_of_an_id_the_account_holds_no_record_for_is_not_found() {
    // (case, the id the start names)
    let rows: [(&str, &str); 3] = [
        ("never minted", "9999999"),
        ("not a number at all", "no-such-multiplex"),
        ("numeric and far out of range", "1234567890123"),
    ];

    for (case, id) in rows {
        let router = medialive();
        // One multiplex the account DOES hold, so the refusal is the resolution's
        // and not an empty store's.
        let held = multiplex_with_settings(&router, case, "held", settings(2_000_000, 1)).await;
        wait_for_the_clock();

        let (status, refusal) = start_multiplex(&router, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 for an id the account holds no record for, got {status}: \
             {refusal}"
        );
        assert_eq!(
            error_code(status, &refusal),
            NOT_FOUND,
            "case {case:?}: expected {NOT_FOUND}, got {refusal}"
        );
        assert_eq!(
            error_message(&refusal),
            format!("Multiplex {id} not found"),
            "case {case:?}: expected the refusal to name the id the request asked for, got \
             {refusal}"
        );

        let entry = only_failed_entry(&router, case, id).await;
        assert_eq!(
            entry["code"],
            json!(NOT_FOUND),
            "case {case:?}: expected the batch entry to carry {NOT_FOUND}, got {entry}"
        );
        assert!(
            entry["arn"].is_null(),
            "case {case:?}: expected no ARN for an id nothing holds, got {entry}"
        );

        // And the multiplex the account does hold was left where it was: the
        // refused start named another id.
        let described = in_state(&router, case, &held, "IDLE").await;
        assert_eq!(
            described["pipelinesRunningCount"],
            json!(0),
            "case {case:?}: expected no pipeline healthy on the multiplex the start did not name, \
             got {described}"
        );
    }
}
