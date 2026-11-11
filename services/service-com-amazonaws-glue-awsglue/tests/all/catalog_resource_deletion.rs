// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteCatalog`: the delete of a catalog RESOURCE a
//! `CreateCatalog` registered, read back through the `GetCatalog` that served it.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test is that a delete removes the record the requested `CatalogId`
//! answers to, with everything stored under it; an id no such record answers to is
//! refused as a missing entity, an id of another account as unauthorized, and the
//! account's own Data Catalog — the container those records live in — as invalid input,
//! because it is never deletable. The suites are tables: each row is one input along the
//! axis the rule generalises over (how the id addresses the catalog, what definition the
//! catalog carried, how a requested id misses, how many catalogs the store holds), and
//! the recorded lifecycle is one row among them.

use std::sync::Arc;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::DatabaseInput;
use service_com_amazonaws_glue_awsglue::state::{ComAmazonawsGlueAwsglueState, DatabaseEntry};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// A `FederatedCatalog` naming the source outside the Glue Data Catalog that the catalog
/// `name` points at: `CreateCatalog` registers only a catalog naming such a source.
fn federation_source(name: &str) -> Value {
    json!({
        "Identifier": format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}"),
        "ConnectionName": "aws:s3tables",
    })
}

/// A `CreateCatalog` request body for `name`, carrying `extra` on top of the federation
/// every create needs.
fn create_request(name: &str, extra: Value) -> Value {
    let mut catalog_input = json!({ "FederatedCatalog": federation_source(name) });
    if let Some(members) = extra.as_object() {
        for (member, value) in members {
            catalog_input[member] = value.clone();
        }
    }
    json!({ "Name": name, "CatalogInput": catalog_input })
}

/// Create one catalog, which must succeed.
async fn create(router: &MultiServiceRouter, body: &Value) {
    ok(router, "CreateCatalog", body).await;
}

/// The status and body of a `DeleteCatalog` for `catalog_id`.
async fn delete(router: &MultiServiceRouter, catalog_id: &str) -> (u16, Value) {
    call(router, "DeleteCatalog", &json!({ "CatalogId": catalog_id })).await
}

/// The status and body of a `GetCatalog` for `catalog_id`.
async fn get(router: &MultiServiceRouter, catalog_id: &str) -> (u16, Value) {
    call(router, "GetCatalog", &json!({ "CatalogId": catalog_id })).await
}

// ---------------------------------------------------------------------------
// The lifecycle: create, read, delete, read again
// ---------------------------------------------------------------------------

/// A delete removes the record the requested id answers to: the create's catalog reads
/// back before it and is a miss after it, and the delete's own answer carries no member
/// (`DeleteCatalogResponse` models none).
///
/// The rows differ along how the delete addresses the catalog (bare name, the name in
/// another casing, the account and the name, the account and the name lowercased, a name
/// carrying a colon of its own) and along what definition the catalog carried.
#[tokio::test]
async fn deleting_a_catalog_removes_what_the_create_stored() {
    let router = emulator();

    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "the name as created",
            "lifecycle-catalog",
            json!({ "Description": "the recorded lifecycle" }),
            "lifecycle-catalog".to_string(),
        ),
        (
            "the name in another casing",
            "MixedCaseDeleted",
            json!({}),
            "mixedcasedeleted".to_string(),
        ),
        (
            "the account and the name",
            "qualified-catalog",
            json!({ "Parameters": { "owner": "analytics", "tier": "gold" } }),
            format!("{ACCOUNT_ID}:qualified-catalog"),
        ),
        (
            "the account and the name lowercased",
            "QualifiedMixed",
            json!({ "Description": "addressed by its full id" }),
            format!("{ACCOUNT_ID}:qualifiedmixed"),
        ),
        (
            "a name carrying a colon of its own",
            "team:catalog",
            json!({ "CatalogProperties": { "CustomProperties": { "a": "b" } } }),
            "team:catalog".to_string(),
        ),
        (
            "a catalog carrying default permissions",
            "permissioned-catalog",
            json!({
                "CreateTableDefaultPermissions": [],
                "CreateDatabaseDefaultPermissions": [],
                "AllowFullTableExternalDataAccess": "False",
            }),
            "permissioned-catalog".to_string(),
        ),
    ];

    for (case, name, extra, catalog_id) in &cases {
        create(&router, &create_request(name, extra.clone())).await;
        let (status, body) = get(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case}: the created catalog must read back before the delete, got {status} \
             with {body}"
        );

        let (status, body) = delete(&router, catalog_id).await;
        assert_eq!(
            status, 200,
            "case {case}: deleting {catalog_id:?} must answer 200, got {status} with {body}"
        );
        assert_eq!(
            body,
            json!({}),
            "case {case}: DeleteCatalog models no response member, got {body}"
        );

        // The record is gone: the read that served it is now the modelled miss.
        let (status, body) = get(&router, name).await;
        assert_eq!(
            status, 400,
            "case {case}: the deleted catalog must not read back, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException after the delete, got {body}"
        );
    }
}

/// A delete removes ONLY the catalog it names: every other catalog the store holds still
/// reads back, and the account's own Data Catalog — the container the created catalogs
/// live in — is untouched.
#[tokio::test]
async fn deleting_one_catalog_leaves_the_others_and_the_account_catalog() {
    let router = emulator();

    let names = ["keep-one", "delete-me", "keep-two", "keep-three"];
    for name in names {
        create(&router, &create_request(name, json!({}))).await;
    }

    let (status, body) = delete(&router, "delete-me").await;
    assert_eq!(
        status, 200,
        "case the middle catalog: expected 200, got {status} with {body}"
    );

    for name in names {
        let expected = if name == "delete-me" { 400 } else { 200 };
        let (status, body) = get(&router, name).await;
        assert_eq!(
            status, expected,
            "case {name}: expected status {expected} after deleting \"delete-me\", got {status} \
             with {body}"
        );
    }

    // The account's own Data Catalog is a container nothing created, so no catalog delete
    // can remove it.
    let (status, body) = get(&router, ACCOUNT_ID).await;
    assert_eq!(
        status, 200,
        "case the account's own Data Catalog: it must still be reported, got {status} with {body}"
    );
}

/// A delete frees the name: a create under it succeeds again and stores the NEW
/// definition, so the name carries nothing of the catalog the delete removed.
#[tokio::test]
async fn a_deleted_name_is_free_for_a_new_catalog() {
    let router = emulator();

    for (case, first, second) in [
        (
            "a different description",
            json!({ "Description": "first" }),
            json!({ "Description": "second" }),
        ),
        (
            "parameters dropped by the second create",
            json!({ "Parameters": { "owner": "analytics" } }),
            json!({ "Description": "no parameters this time" }),
        ),
    ] {
        let name = "recreated-catalog";
        create(&router, &create_request(name, first)).await;
        let (status, _) = delete(&router, name).await;
        assert_eq!(status, 200, "case {case}: the first delete must succeed");

        create(&router, &create_request(name, second.clone())).await;
        let (status, body) = get(&router, name).await;
        assert_eq!(
            status, 200,
            "case {case}: the re-created catalog must read back, got {status} with {body}"
        );
        let catalog = body
            .get("Catalog")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: GetCatalog must report a Catalog, got {body}"));
        assert_eq!(
            catalog.get("Description").and_then(Value::as_str),
            second.get("Description").and_then(Value::as_str),
            "case {case}: the name must carry the new definition, got {catalog:?}"
        );
        assert_eq!(
            catalog.get("Parameters").and_then(Value::as_object),
            second.get("Parameters").and_then(Value::as_object),
            "case {case}: the deleted catalog's parameters must not survive, got {catalog:?}"
        );

        // Leave the store as the next row expects it.
        let (status, _) = delete(&router, name).await;
        assert_eq!(status, 200, "case {case}: the second delete must succeed");
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A delete for an id this account holds no catalog under is refused with the modelled
/// `EntityNotFoundException` and changes nothing.
///
/// The rows differ along how the requested id misses: never created, a prefix of a stored
/// name, a suffix of one, a name qualified by this account that names nothing, a catalog
/// already deleted, and the account's own id as a catalog name — a name no create can
/// take.
#[tokio::test]
async fn deleting_an_unknown_catalog_is_refused() {
    let router = emulator();
    create(&router, &create_request("stored-catalog", json!({}))).await;
    create(&router, &create_request("already-deleted", json!({}))).await;
    let (status, _) = delete(&router, "already-deleted").await;
    assert_eq!(status, 200, "the catalog to delete twice must delete once");

    for (case, catalog_id) in [
        ("never created", "no-such-catalog".to_string()),
        ("a prefix of a stored name", "stored".to_string()),
        ("a suffix of a stored name", "catalog".to_string()),
        (
            "this account and a name nothing created",
            format!("{ACCOUNT_ID}:absent"),
        ),
        ("a catalog already deleted", "already-deleted".to_string()),
        (
            "the account's own id as a catalog name",
            format!("{ACCOUNT_ID}:{ACCOUNT_ID}"),
        ),
    ] {
        let (status, body) = delete(&router, &catalog_id).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {catalog_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {catalog_id:?}, got {body}"
        );
        // The refusal's wording is the one the read path reports for the same miss.
        assert_eq!(
            error_message(&body),
            "Catalog not found.",
            "case {case}: expected the catalog miss wording for {catalog_id:?}, got {body}"
        );
    }

    // The refused deletes removed nothing: the stored catalog and the account's own Data
    // Catalog both still read back.
    for (case, catalog_id) in [
        ("the stored catalog", "stored-catalog"),
        ("the account's own Data Catalog", ACCOUNT_ID),
    ] {
        let (status, body) = get(&router, catalog_id).await;
        assert_eq!(
            status, 200,
            "case {case}: the refused deletes must have removed nothing, got {status} with {body}"
        );
    }
}

/// An id ADDRESSES a catalog, so it is resolved against the store and answered with the
/// modelled `EntityNotFoundException` when nothing is stored under it — on both the delete
/// and the read — even for an id outside the grammar a create's `Name` has to satisfy
/// (`CatalogNameString`'s pattern, which forbids the three path separators and the `aws:`
/// prefix). Such an id is an address like any other: `CatalogIdString` admits it, and the
/// nested ids the Data Catalog itself publishes (`s3tablescatalog/<bucket>`) are spelled
/// with exactly those characters.
///
/// The rows differ along which character takes the id outside the name grammar and where
/// the id carries it, and along whether the id names this account explicitly.
#[tokio::test]
async fn an_id_outside_the_name_grammar_is_a_miss_and_not_a_malformed_request() {
    let router = emulator();
    create(&router, &create_request("stored-catalog", json!({}))).await;

    for (case, catalog_id) in [
        ("a dot inside the id", "my.catalog".to_string()),
        ("a dot at the end", "catalog.".to_string()),
        ("a dot at the start", ".catalog".to_string()),
        (
            "the nested form the Data Catalog publishes",
            "s3tablescatalog/analytics".to_string(),
        ),
        ("a backslash", "my\\catalog".to_string()),
        ("the reserved prefix", "aws:catalog".to_string()),
        (
            "this account and a nested name",
            format!("{ACCOUNT_ID}:s3tablescatalog/analytics"),
        ),
    ] {
        for (surface, (status, body)) in [
            ("DeleteCatalog", delete(&router, &catalog_id).await),
            ("GetCatalog", get(&router, &catalog_id).await),
        ] {
            assert_eq!(
                status, 400,
                "case {case} on {surface}: expected 400 for {catalog_id:?}, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case} on {surface}: expected EntityNotFoundException for \
                 {catalog_id:?}, got {body}"
            );
            assert_eq!(
                error_message(&body),
                "Catalog not found.",
                "case {case} on {surface}: expected the catalog miss wording for \
                 {catalog_id:?}, got {body}"
            );
        }
    }

    // Nothing the refusals touched was removed: the stored catalog still reads back.
    let (status, body) = get(&router, "stored-catalog").await;
    assert_eq!(
        status, 200,
        "the refused calls must have removed nothing, got {status} with {body}"
    );
}

/// A catalog in ANOTHER account's Data Catalog is refused as unauthorized rather than as
/// a miss, before any lookup: it is reachable on the real service only through that
/// catalog's own resource-based policy, and this store can hold no such grant. The rows
/// differ along the account named and whether this account holds a catalog of that name.
#[tokio::test]
async fn deleting_another_accounts_catalog_is_refused_as_unauthorized() {
    let router = emulator();
    create(&router, &create_request("shared", json!({}))).await;

    for (case, catalog_id) in [
        (
            "another account, a name it does not hold",
            "111122223333:absent",
        ),
        ("another account, a name this one holds", "111122223333:shared"),
        // An id that is an account number and nothing else names that account's own Data
        // Catalog, which holds catalogs but is not one of them.
        ("another account's Data Catalog itself", "111122223333"),
        ("a third account", "444455556666:anything"),
    ] {
        let (status, body) = delete(&router, catalog_id).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {catalog_id:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AccessDeniedException"),
            "case {case}: expected AccessDeniedException for {catalog_id:?}, got {body}"
        );
        assert!(
            error_message(&body).contains("glue:DeleteCatalog"),
            "case {case}: the refusal must name the action, got {body}"
        );
    }

    // This account's same-named catalog was not touched by the cross-account requests.
    let (status, body) = get(&router, "shared").await;
    assert_eq!(
        status, 200,
        "case the local catalog: a cross-account refusal must remove nothing, got {status} with \
         {body}"
    );
}

// ---------------------------------------------------------------------------
// One delete wins
// ---------------------------------------------------------------------------

/// The account's own Data Catalog is never deletable: a delete addressing it is refused
/// with the modelled `InvalidInputException`, stating the rule a deletable catalog's name
/// satisfies, and the catalog is left intact — a following read of it still reports it.
///
/// The rows differ along how many catalogs the account holds when the delete arrives, so
/// what is refused is the catalog's own protection and not an artefact of an empty store.
#[tokio::test]
async fn the_accounts_own_data_catalog_cannot_be_deleted() {
    let router = emulator();

    for (case, catalogs) in [
        ("an account holding no catalog", Vec::new()),
        ("an account holding one catalog", vec!["protected-one"]),
        (
            "an account holding several",
            vec!["protected-two", "protected-three"],
        ),
    ] {
        for name in &catalogs {
            create(&router, &create_request(name, json!({}))).await;
        }

        let (status, body) = delete(&router, ACCOUNT_ID).await;
        assert_eq!(
            status, 400,
            "case {case}: deleting the account's own Data Catalog must be refused, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {case}: expected InvalidInputException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Only lowercase letters, numbers, underscores and hyphens are allowed for catalog \
             name",
            "case {case}: expected the catalog-name rule as the refusal, got {body}"
        );

        // The refusal left the catalog intact: the read that reports it still does, and
        // every catalog inside it is still there.
        let (status, body) = get(&router, ACCOUNT_ID).await;
        assert_eq!(
            status, 200,
            "case {case}: the refused delete must leave the catalog readable, got {status} with \
             {body}"
        );
        let reported = body
            .get("Catalog")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: GetCatalog must report a Catalog, got {body}"));
        assert_eq!(
            reported.get("CatalogId").and_then(Value::as_str),
            Some(ACCOUNT_ID),
            "case {case}: the catalog must still report the account's own id, got {reported}"
        );
        for name in &catalogs {
            let (status, body) = get(&router, name).await;
            assert_eq!(
                status, 200,
                "case {case}: {name} must survive the refused delete, got {status} with {body}"
            );
        }
    }
}

/// Concurrent deletes of one catalog resolve to exactly one deletion: one answers 200 and
/// every other is refused with the modelled `EntityNotFoundException`, because the removal
/// and the miss it reports are one step over the store. The rows differ along how many
/// racers address the catalog and along how each spells its id.
#[tokio::test]
async fn concurrent_deletes_of_one_catalog_resolve_to_one_deletion() {
    for (case, ids) in [
        (
            "two racers, the same spelling",
            vec!["raced-catalog".to_string(), "raced-catalog".to_string()],
        ),
        (
            "two racers, one by the full id",
            vec![
                "raced-catalog".to_string(),
                format!("{ACCOUNT_ID}:raced-catalog"),
            ],
        ),
        (
            "four racers, mixed spellings",
            vec![
                "raced-catalog".to_string(),
                "RACED-CATALOG".to_string(),
                format!("{ACCOUNT_ID}:raced-catalog"),
                format!("{ACCOUNT_ID}:RACED-CATALOG"),
            ],
        ),
    ] {
        let router = Arc::new(emulator());
        create(&router, &create_request("raced-catalog", json!({}))).await;

        let mut racers = Vec::new();
        for id in ids.clone() {
            let router = Arc::clone(&router);
            racers.push(tokio::spawn(async move { delete(&router, &id).await }));
        }

        let mut deleted = 0usize;
        for (racer, outcome) in racers.into_iter().enumerate() {
            let (status, body) = outcome.await.expect("the racer completed");
            if status == 200 {
                deleted += 1;
                continue;
            }
            assert_eq!(
                status, 400,
                "case {case}: racer {racer} must answer 200 or 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case}: racer {racer} lost the race, so it must be refused as a miss, got \
                 {body}"
            );
        }
        assert_eq!(
            deleted, 1,
            "case {case}: exactly one of {} deletes may succeed, got {deleted}",
            ids.len()
        );

        let (status, body) = get(&router, "raced-catalog").await;
        assert_eq!(
            status, 400,
            "case {case}: the catalog must be gone after the race, got {status} with {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// The cascade
// ---------------------------------------------------------------------------

/// Everything stored UNDER a catalog goes with it: the store holds a catalog's databases
/// and connections inside the catalog's own entry, and the delete removes that entry, so
/// no record of the deleted catalog is left behind for a later create of the same name to
/// inherit.
///
/// The databases are seeded through the store because a database create addresses the
/// account's Data Catalog and not a catalog resource inside it; what is asserted is that
/// the delete left no entry under the catalog's key.
#[tokio::test]
async fn a_catalog_delete_removes_what_was_stored_under_it() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    create(&router, &create_request("cascading-catalog", json!({}))).await;
    let key = format!("{ACCOUNT_ID}:cascading-catalog");

    // Seed records inside the catalog's own entry, as a client of that catalog's
    // databases would leave them.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let catalog = store
            .catalog_mut_for_create(&key)
            .expect("the created catalog is inside the ceiling");
        for database in ["sales", "marketing"] {
            let input = DatabaseInput {
                name: database.to_string(),
                ..DatabaseInput::default()
            };
            catalog.databases.insert(
                database.to_string(),
                DatabaseEntry::from_input(&input, &key, None, SystemTime::now()),
            );
        }
        assert_eq!(
            catalog.databases.len(),
            2,
            "case the seeded catalog: the databases must be stored before the delete"
        );
    }

    let (status, body) = delete(&router, "cascading-catalog").await;
    assert_eq!(
        status, 200,
        "case the seeded catalog: expected 200, got {status} with {body}"
    );

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    assert!(
        !store.catalogs.contains_key(&key),
        "case the seeded catalog: the delete must leave no entry under {key:?}"
    );
}
