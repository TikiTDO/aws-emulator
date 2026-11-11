// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the sentence a Glue blueprint LOOKUP MISS is reported with once the
//! blueprint has been REMOVED — the axis `tests/all/not_found_wording.rs` does not cover,
//! which pins the same sentence over a name nothing ever registered.
//!
//! RULE: every operation that addresses one blueprint by name answers a name this account
//! holds no blueprint under with the service's single FIXED sentence for that miss — the same
//! text whichever of those operations asked and however the name came to hold nothing — so
//! the sentence carries no part of the request and never varies with it. A name whose whole
//! registration `DeleteBlueprint` removed is therefore the same miss as a name nothing ever
//! registered.
//!
//! The rows differ along what the rule generalises over: the form, character set and length of
//! the requested name, the state the registration was in when it went (still in flight,
//! settled, carrying a finished run of itself), how many times the name went through
//! registration and removal, which operation of the family asks, and whether the run the
//! request addresses is one the removed blueprint had really recorded or an id nothing minted.
//!
//! The expected sentence is written out as a literal rather than read from the service's own
//! helper, so this suite pins the wording instead of restating it; a template interpolating
//! the requested name — or any other request-derived prose — fails here.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    BLUEPRINT_RUN_SETTLES_AFTER, BLUEPRINT_STATUS_CREATING,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::blueprint_deletion::{delete, ROLE_ARN};
use crate::blueprint_runs::{emulator_with_engine, register_active, rewind, start, stored_runs};
use crate::not_found_wording::assert_fixed_miss;
use crate::wire::ok;

/// The whole sentence a blueprint lookup miss is reported with. Fixed text: it names the
/// member the request addressed the blueprint by and discloses no value. The status (400) and
/// the error code (`EntityNotFoundException`, shared by every entity this service holds —
/// `smithy_query operation=GetBlueprint,StartBlueprintRun,GetBlueprintRun,GetBlueprintRuns`)
/// come with it, which is why the sentence is what tells a client WHICH entity was missing.
const BLUEPRINT_MISS: &str = "Blueprint with given name not found.";

/// The longest name the family's strictest member admits: `OrchestrationNameString`,
/// "len 1..128", pattern `^[\.\-_A-Za-z0-9]+$` (`smithy_query operation=StartBlueprintRun`).
/// Every name below stays inside that member, so one axis serves every row and no row is
/// refused at the request boundary instead of by the handler.
const MAX_NAME_LENGTH: usize = 128;

/// A run id no start ever minted, for the rows whose request addresses a run of a blueprint
/// that recorded none. Inside `IdString`, "len 1..255" (`smithy_query
/// operation=GetBlueprintRun`).
const UNMINTED_RUN_ID: &str = "br_0000000000000000";

/// The state the registration a row removes was in when it went.
#[derive(Clone, Copy, Debug)]
enum Removed {
    /// A registration the clock had not settled yet.
    WhileCreating,
    /// A settled registration.
    WhenSettled,
    /// A settled registration that had recorded a finished run of itself.
    WithARun,
    /// The name went through registration and removal twice.
    Twice,
}

/// Remove the blueprint `name` addresses through the delete `tests/all/blueprint_deletion.rs`
/// owns, asserting it answered with the name it removed — so a row that means "this
/// registration is gone" really removed one.
async fn remove(router: &MultiServiceRouter, case: &str, name: &str) {
    let reported = delete(router, name).await;
    assert_eq!(
        reported.as_deref(),
        Some(name),
        "case {case:?}: DeleteBlueprint must answer with the name it removed, got {reported:?}"
    );
}

/// Register `name` and leave it in flight, asserting it is in the status a registration the
/// clock has not settled yet carries.
async fn register_in_flight(router: &MultiServiceRouter, case: &str, name: &str) {
    ok(
        router,
        "CreateBlueprint",
        &json!({
            "Name": name,
            "BlueprintLocation": "s3://blueprint-bucket/in-flight.zip",
        }),
    )
    .await;
    let read = ok(router, "GetBlueprint", &json!({ "Name": name })).await;
    assert_eq!(
        read["Blueprint"]["Status"].as_str(),
        Some(BLUEPRINT_STATUS_CREATING),
        "case {case:?}: the registration must still be in flight when it is removed, got {read}"
    );
}

/// Bring `name` to the state the row means and remove it, answering with the run id the row's
/// `GetBlueprintRun` should address: the id a run of the removed blueprint really carried
/// where the row started one, and an id nothing minted otherwise.
async fn register_then_remove(
    router: &MultiServiceRouter,
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    name: &str,
    removed: Removed,
) -> String {
    match removed {
        Removed::WhileCreating => {
            register_in_flight(router, case, name).await;
            remove(router, case, name).await;
            UNMINTED_RUN_ID.to_string()
        }
        Removed::WhenSettled => {
            register_active(router, engine, name).await;
            remove(router, case, name).await;
            UNMINTED_RUN_ID.to_string()
        }
        Removed::WithARun => {
            register_active(router, engine, name).await;
            let run_id = start(
                router,
                &json!({ "BlueprintName": name, "RoleArn": ROLE_ARN }),
            )
            .await;
            // Past the span the clock owed the run, so the row removes a blueprint whose run
            // history is complete rather than one with work still in flight.
            rewind(engine, name, BLUEPRINT_RUN_SETTLES_AFTER);
            let recorded = stored_runs(engine, name);
            assert_eq!(
                recorded, 1,
                "case {case:?}: the start must have recorded one run before the delete, got \
                 {recorded}"
            );
            remove(router, case, name).await;
            run_id
        }
        Removed::Twice => {
            for round in 1..=2 {
                let label = format!("{case} (round {round})");
                register_active(router, engine, name).await;
                remove(router, &label, name).await;
            }
            UNMINTED_RUN_ID.to_string()
        }
    }
}

/// A name whose whole registration was removed is reported with the one fixed sentence by
/// each of the four reads that address a blueprint by name — whatever state the registration
/// was in when it went, and even where the run id the request addresses is one that blueprint
/// really carried.
#[tokio::test]
async fn a_removed_blueprint_is_the_one_fixed_miss_sentence() {
    let (router, engine) = emulator_with_engine();
    // A registration that stays throughout, so every refusal below is about the name the row
    // addressed and not a lookup that stopped resolving.
    register_active(&router, &engine, "SentinelBlueprint").await;

    let rows: Vec<(&str, String, Removed)> = vec![
        (
            "removed while still in flight",
            "in-flight.blueprint".to_string(),
            Removed::WhileCreating,
        ),
        (
            "removed once settled",
            "SettledBlueprint".to_string(),
            Removed::WhenSettled,
        ),
        (
            "punctuation and digits, removed once settled",
            "team.data_lake-2026".to_string(),
            Removed::WhenSettled,
        ),
        (
            "at the 128 ceiling, removed once settled",
            "c".repeat(MAX_NAME_LENGTH),
            Removed::WhenSettled,
        ),
        (
            "removed with a finished run of it",
            "ran-then-removed".to_string(),
            Removed::WithARun,
        ),
        (
            "registered and removed twice",
            "twice-around".to_string(),
            Removed::Twice,
        ),
    ];

    for (case, name, removed) in &rows {
        let run_id = register_then_remove(&router, &engine, case, name, *removed).await;

        let requests: Vec<(&str, Value)> = vec![
            ("GetBlueprint", json!({ "Name": name })),
            (
                "StartBlueprintRun",
                json!({ "BlueprintName": name, "RoleArn": ROLE_ARN }),
            ),
            (
                // The blueprint half of the address is resolved first, so a run id the
                // removed blueprint really carried is reported as the blueprint miss and not
                // as the run-id miss, which is a sentence of its own.
                "GetBlueprintRun",
                json!({ "BlueprintName": name, "RunId": run_id }),
            ),
            ("GetBlueprintRuns", json!({ "BlueprintName": name })),
        ];

        for (operation, request) in &requests {
            // The status, the code and the whole sentence, through the assertion
            // `tests/all/not_found_wording.rs` owns for a fixed refusal of this service.
            assert_fixed_miss(&router, case, operation, request, name, BLUEPRINT_MISS).await;
        }

        // The refused start above named a blueprint that is gone, so it must not have
        // recorded its run against the registration that remains.
        let recorded = stored_runs(&engine, "SentinelBlueprint");
        assert_eq!(
            recorded, 0,
            "case {case:?}: a refused start must record no run on another blueprint, got \
             {recorded}"
        );

        // The registration that was never touched is still served, so every refusal above is
        // about the name the row addressed.
        let read = ok(
            &router,
            "GetBlueprint",
            &json!({ "Name": "SentinelBlueprint" }),
        )
        .await;
        assert_eq!(
            read["Blueprint"]["Name"].as_str(),
            Some("SentinelBlueprint"),
            "case {case:?}: the untouched registration must still be reported, got {read}"
        );
    }

    // The sentinel is the one blueprint this account still holds, so each row's registration
    // really went and the misses above were misses of an empty namespace.
    let page = ok(&router, "ListBlueprints", &json!({})).await;
    let listed: Vec<&str> = page["Blueprints"]
        .as_array()
        .unwrap_or_else(|| panic!("ListBlueprints must report a list, got {page}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        listed,
        vec!["SentinelBlueprint"],
        "only the untouched registration may remain, got {page}"
    );
}
