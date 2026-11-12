// SPDX-License-Identifier: CC0-1.0

//! The identity of an SAP HANA database under the CASE of its name, driven over the wire.
//!
//! RULE: a database of a registered application is identified by its name, and an SAP HANA
//! database name is case-insensitive, so two names that differ only in case name ONE
//! database — exactly as two names differing only in characters the modelled `DatabaseId`
//! pattern rejects do. That one database is reported once, under one canonical
//! `DatabaseId` and one ARN, with one credential per `CredentialType`; a reference naming
//! that id in any case resolves to it; and a credential of it named for removal or for
//! update under any case of its name is the credential the application holds.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: what the application holds
//! is only believed when `ListDatabases` and `GetDatabase` report it back.
//!
//! The table spans what the rule generalises over: how many spellings of the one name a
//! registration declares (one, two, three), the case each spelling carries (lower, upper,
//! mixed, alternating), whether the spellings differ in characters the `DatabaseId` form
//! rewrites as well as in case, whether the database is the SAP HANA system database or a
//! tenant database, and the length of the name up to the modelled `DatabaseName` bound.

use serde_json::{json, Map, Value};

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::list_databases::{expected_arn, listed, NAME_AT_LENGTH_BOUND};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The single `CredentialType` the model defines
/// (`smithy_query(query="shape", shape="ApplicationCredential")`: `CredentialType` is an
/// enum whose one value is `ADMIN`).
const ADMIN: &str = "ADMIN";

/// The `DatabaseName` SAP HANA gives the system database of a multitenant database
/// system, written out here so a row that pins `DatabaseType` does not read the name off
/// the implementation.
const SYSTEM_DATABASE_NAME: &str = "SYSTEMDB";

/// The modelled `DatabaseType` values
/// (`smithy_query(query="shape", shape="DatabaseType")`: `SYSTEM`, `TENANT`).
const DATABASE_TYPE_SYSTEM: &str = "SYSTEM";
const DATABASE_TYPE_TENANT: &str = "TENANT";

/// The `DatabaseName` of a second credential every registration here carries, so a row can
/// take the credential under test away without asking an application of a type that
/// requires credentials to hold none.
const KEEPER_NAME: &str = "KEEP1";

/// The `DatabaseId` that name yields, and the `SecretId` of its credential.
const KEEPER_ID: &str = "KEEP1";
const KEEPER_SECRET: &str = "sap/keep/admin";

/// The secret an update re-declares a credential with.
const ROTATED_SECRET: &str = "sap/rotated/admin";

/// A `DatabaseName` no row registers, so a removal naming it names a credential no
/// application here holds.
const UNHELD_NAME: &str = "NOSUCHDB";

/// One registration and the one database its credentials must leave it with.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// The spellings of ONE SAP HANA database name the registration declares a credential
    /// for, in the order it declares them.
    spellings: Vec<String>,
    /// The one `DatabaseId` every one of those spellings must yield.
    expected_id: String,
    /// The `DatabaseType` that one database must carry.
    expected_type: &'static str,
}

/// The rows.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "the system database in upper and in lower case",
            application_id: "HDB-CASE",
            application_type: "HANA",
            spellings: spellings(&[SYSTEM_DATABASE_NAME, "systemdb"]),
            expected_id: SYSTEM_DATABASE_NAME.to_string(),
            expected_type: DATABASE_TYPE_SYSTEM,
        },
        Case {
            name: "the system database named in alternating case alone",
            application_id: "hdb-mixed-system",
            application_type: "HANA",
            spellings: spellings(&["SyStEmDb"]),
            expected_id: SYSTEM_DATABASE_NAME.to_string(),
            expected_type: DATABASE_TYPE_SYSTEM,
        },
        Case {
            name: "a tenant database named in three cases",
            application_id: "hdb-three-cases",
            application_type: "HANA",
            spellings: spellings(&["tnt1", "TNT1", "Tnt1"]),
            expected_id: "TNT1".to_string(),
            expected_type: DATABASE_TYPE_TENANT,
        },
        Case {
            name: "spellings differing in case and in characters the DatabaseId form rewrites",
            application_id: "hdb-punctuated-cases",
            application_type: "HANA",
            spellings: spellings(&["prod db.1", "PROD_DB_1", "Prod-Db 1"]),
            expected_id: "PROD_DB_1".to_string(),
            expected_type: DATABASE_TYPE_TENANT,
        },
        Case {
            name: "a name carrying digits and an underscore",
            application_id: "hdb-underscored",
            application_type: "HANA",
            spellings: spellings(&["hdb_01", "HDB_01"]),
            expected_id: "HDB_01".to_string(),
            expected_type: DATABASE_TYPE_TENANT,
        },
        Case {
            name: "a name at the top of the modelled DatabaseName length bound",
            application_id: "hdb-long-name-cases",
            application_type: "HANA",
            spellings: ::std::vec![
                NAME_AT_LENGTH_BOUND.to_lowercase(),
                NAME_AT_LENGTH_BOUND.to_string(),
            ],
            expected_id: NAME_AT_LENGTH_BOUND.to_uppercase(),
            expected_type: DATABASE_TYPE_TENANT,
        },
        // Every row is of the type whose registration CARRIES credentials: the other
        // modelled type runs on the SAP HANA database of such an application and names it
        // by ARN, so a registration of that type carries no credentials at all
        // (`service_com_amazonaws_ssmsap_ssmsap::state::CONDITIONALLY_REQUIRED_MEMBERS`)
        // and there is no database name on one for this rule to be about.
    ]
}

/// A row's spellings as owned strings.
fn spellings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// `name` with the case of every ASCII letter inverted: a spelling of the same SAP HANA
/// database name differing from it in nothing but case.
fn flipped_case(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_uppercase() {
                character.to_ascii_lowercase()
            } else {
                character.to_ascii_uppercase()
            }
        })
        .collect()
}

/// The `SecretId` the credential declared for the spelling at `index` carries, so the
/// declaration that last reached a database is visible in what the database reports.
fn secret_for(index: usize) -> String {
    format!("sap/case/{index}")
}

/// One `(DatabaseName, SecretId)` pair as the `ApplicationCredential` the wire carries.
fn credential(database_name: &str, secret_id: &str) -> Value {
    json!({
        "DatabaseName": database_name,
        "CredentialType": ADMIN,
        "SecretId": secret_id,
    })
}

/// A `Credentials` / `CredentialsToAddOrUpdate` / `CredentialsToRemove` list.
fn credential_list(credentials: &[(String, String)]) -> Value {
    Value::Array(
        credentials
            .iter()
            .map(|(database_name, secret_id)| credential(database_name, secret_id))
            .collect(),
    )
}

/// The credential declarations a registration carries for `spellings`: one per spelling,
/// each with its own secret, plus the keeper credential of another database.
fn declarations(spellings: &[String]) -> Vec<(String, String)> {
    let mut declared: Vec<(String, String)> = spellings
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), secret_for(index)))
        .collect();
    declared.push((KEEPER_NAME.to_string(), KEEPER_SECRET.to_string()));
    declared
}

/// The `RegisterApplication` body carrying `declared` as its `Credentials`.
fn registration(
    application_id: &str,
    application_type: &str,
    declared: &[(String, String)],
) -> Value {
    let mut body: Map<String, Value> = valid_registration(application_id, application_type)
        .as_object()
        .cloned()
        .expect("a registration body is a JSON object");
    body.insert("Credentials".into(), credential_list(declared));
    Value::Object(body)
}

/// `RegisterApplication` with `body`, asserting it was accepted.
async fn register(emulator: &Emulator, case_name: &str, body: Value) {
    let reply = emulator.send("/register-application", body).await;
    assert_eq!(
        reply.status, 200,
        "case {case_name:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
}

/// `UpdateApplicationSettings` with `body`.
async fn update(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("/update-application-settings", body).await
}

/// The `DatabaseId` of every database `ListDatabases` reports for `application_id`, in the
/// order it reported them.
async fn database_ids(emulator: &Emulator, application_id: &str) -> Vec<String> {
    let reply = emulator
        .send("/list-databases", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListDatabases for {application_id:?}, got {reply:?}"
    );
    listed(&reply)
        .iter()
        .map(|summary| summary.id().to_string())
        .collect()
}

/// `GetDatabase` for the database `database_id` of `application_id`.
async fn get_database(emulator: &Emulator, application_id: &str, database_id: &str) -> Reply {
    emulator
        .send(
            "/get-database",
            json!({"ApplicationId": application_id, "DatabaseId": database_id}),
        )
        .await
}

/// A string member of the `Database` a reply reported.
fn database_member(reply: &Reply, name: &str) -> String {
    reply
        .member("Database")
        .and_then(|database| database.get(name))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `(DatabaseName, SecretId)` of every credential the reported `Database` carries, in
/// the order it reported them.
fn reported_credentials(reply: &Reply) -> Vec<(String, String)> {
    reply
        .member("Database")
        .and_then(|database| database.get("Credentials"))
        .and_then(Value::as_array)
        .map(|credentials| {
            credentials
                .iter()
                .map(|credential| {
                    let member = |name: &str| {
                        credential
                            .get(name)
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    (member("DatabaseName"), member("SecretId"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The ids the listing must report for a row, in `DatabaseId` order: the one database the
/// spellings name, and the keeper.
fn expected_ids(case: &Case) -> Vec<String> {
    let mut ids = ::std::vec![case.expected_id.clone(), KEEPER_ID.to_string()];
    ids.sort();
    ids
}

/// RULE: names differing only in case name ONE SAP HANA database, so a registration
/// declaring a credential under several cases of one name leaves the application with a
/// single database, reported once under the canonical `DatabaseId` that name yields, with
/// the `DatabaseType` that name decides and one credential — the one the last declaration
/// of it named.
#[tokio::test]
async fn names_differing_only_in_case_are_one_database() {
    let emulator = Emulator::new();

    for case in cases() {
        let declared = declarations(&case.spellings);
        register(
            &emulator,
            case.name,
            registration(case.application_id, case.application_type, &declared),
        )
        .await;

        let reported = database_ids(&emulator, case.application_id).await;
        let expected = expected_ids(&case);
        assert_eq!(
            reported, expected,
            "case {:?}: expected the spellings {:?} to leave the databases {expected:?}, got \
             {reported:?}",
            case.name, case.spellings
        );

        let read = get_database(&emulator, case.application_id, &case.expected_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetDatabase for {:?}, got {read:?}",
            case.name, case.expected_id
        );
        assert_eq!(
            database_member(&read, "DatabaseId"),
            case.expected_id,
            "case {:?}: expected DatabaseId {:?}, got {read:?}",
            case.name,
            case.expected_id
        );
        assert_eq!(
            database_member(&read, "DatabaseType"),
            case.expected_type,
            "case {:?}: expected DatabaseType {:?}, got {read:?}",
            case.name,
            case.expected_type
        );
        assert_eq!(
            database_member(&read, "Arn"),
            expected_arn(case.application_id, &case.expected_id),
            "case {:?}: expected the ARN of the one database, got {read:?}",
            case.name
        );

        // One credential, carrying the name and the secret of the declaration that last
        // named that database — the spellings before it declared the same credential.
        let last = case.spellings.len() - 1;
        let held = reported_credentials(&read);
        assert_eq!(
            held,
            ::std::vec![(case.spellings[last].clone(), secret_for(last))],
            "case {:?}: expected the one credential the last declaration left, got {held:?}",
            case.name
        );
    }
}

/// RULE: the case of a database's name is no part of its identity, so a `DatabaseId`
/// reference spelling that id in any case names the one database, and every such
/// reference is answered with the same record.
#[tokio::test]
async fn every_case_of_a_database_id_names_the_one_database() {
    let emulator = Emulator::new();

    for case in cases() {
        let declared = declarations(&case.spellings);
        register(
            &emulator,
            case.name,
            registration(case.application_id, case.application_type, &declared),
        )
        .await;

        let canonical = get_database(&emulator, case.application_id, &case.expected_id).await;
        assert_eq!(
            canonical.status, 200,
            "case {:?}: expected 200 from GetDatabase for {:?}, got {canonical:?}",
            case.name, case.expected_id
        );

        for reference in [
            case.expected_id.clone(),
            case.expected_id.to_lowercase(),
            flipped_case(&case.expected_id),
        ] {
            let read = get_database(&emulator, case.application_id, &reference).await;
            assert_eq!(
                read.status, 200,
                "case {:?}: expected 200 from GetDatabase for {reference:?}, got {read:?}",
                case.name
            );
            assert_eq!(
                read.body, canonical.body,
                "case {:?}: expected the reference {reference:?} to name the one database \
                 {canonical:?}, got {read:?}",
                case.name
            );
        }
    }
}

/// RULE: a credential is named for removal by the database it reaches, under that same
/// identity — so a removal naming the database in another case takes away the credential
/// the application holds, and the database it was the only credential of goes with it.
#[tokio::test]
async fn a_credential_is_removed_under_any_case_of_its_database_name() {
    let emulator = Emulator::new();

    for case in cases() {
        // Registered under the FIRST spelling alone, so the removal reaches it under a
        // spelling nothing wrote.
        let declared = declarations(&case.spellings[..1]);
        register(
            &emulator,
            case.name,
            registration(case.application_id, case.application_type, &declared),
        )
        .await;
        settle().await;

        let named = flipped_case(&case.spellings[0]);
        let reply = update(
            &emulator,
            json!({
                "ApplicationId": case.application_id,
                "CredentialsToRemove": credential_list(&[(named.clone(), secret_for(0))]),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from UpdateApplicationSettings removing {named:?}, got \
             {reply:?}",
            case.name
        );
        settle().await;

        let reported = database_ids(&emulator, case.application_id).await;
        assert_eq!(
            reported,
            ::std::vec![KEEPER_ID.to_string()],
            "case {:?}: expected the removal of {named:?} to leave only {KEEPER_ID:?}, got \
             {reported:?}",
            case.name
        );

        // The identity is the NAME, not merely its letters: a credential for a database
        // the application holds none of is still refused, in the words the service states
        // that refusal in.
        let refused = update(
            &emulator,
            json!({
                "ApplicationId": case.application_id,
                "CredentialsToRemove": credential_list(&[(
                    UNHELD_NAME.to_string(),
                    KEEPER_SECRET.to_string(),
                )]),
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {:?}: expected HTTP 400 for a credential the application does not hold, got \
             {refused:?}",
            case.name
        );
        assert_eq!(
            refused.code(),
            VALIDATION_EXCEPTION,
            "case {:?}: expected {VALIDATION_EXCEPTION}, got {refused:?}",
            case.name
        );
        let stated = message(&refused);
        let expected = format!(
            "Application {} has no {ADMIN} credential for database {UNHELD_NAME}",
            case.application_id
        );
        assert!(
            stated.contains(&expected),
            "case {:?}: expected the refusal to state {expected:?}, got {stated:?}",
            case.name
        );
    }
}

/// RULE: a credential declared for update is applied by that same identity, so a
/// declaration naming the database in another case replaces the credential the
/// application holds rather than adding a second one — and the application still has the
/// one database.
#[tokio::test]
async fn an_update_in_another_case_replaces_the_credential_of_the_one_database() {
    let emulator = Emulator::new();

    for case in cases() {
        let declared = declarations(&case.spellings[..1]);
        register(
            &emulator,
            case.name,
            registration(case.application_id, case.application_type, &declared),
        )
        .await;
        settle().await;

        let named = flipped_case(&case.spellings[0]);
        let reply = update(
            &emulator,
            json!({
                "ApplicationId": case.application_id,
                "CredentialsToAddOrUpdate": credential_list(&[(
                    named.clone(),
                    ROTATED_SECRET.to_string(),
                )]),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from UpdateApplicationSettings declaring {named:?}, got \
             {reply:?}",
            case.name
        );
        settle().await;

        let reported = database_ids(&emulator, case.application_id).await;
        let expected = expected_ids(&case);
        assert_eq!(
            reported, expected,
            "case {:?}: expected the declaration of {named:?} to leave the databases \
             {expected:?}, got {reported:?}",
            case.name
        );

        let read = get_database(&emulator, case.application_id, &case.expected_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetDatabase for {:?}, got {read:?}",
            case.name, case.expected_id
        );
        let held = reported_credentials(&read);
        assert_eq!(
            held,
            ::std::vec![(named.clone(), ROTATED_SECRET.to_string())],
            "case {:?}: expected the one credential the update left, got {held:?}",
            case.name
        );
    }
}
