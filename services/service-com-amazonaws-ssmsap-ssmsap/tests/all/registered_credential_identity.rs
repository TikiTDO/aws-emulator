// SPDX-License-Identifier: CC0-1.0

//! The credentials a registration declares, driven over the wire.
//!
//! RULE: a credential of an application is identified by the SAP HANA database it reaches
//! (`ApplicationCredential$DatabaseName`, by the `DatabaseId` that name yields) and its
//! `CredentialType`, while `SecretId` is the secret that reaches that database — so a
//! list of credential declarations is APPLIED by that identity: a declaration naming a
//! credential the list has already declared replaces its secret instead of adding a
//! second credential, and the application is left holding exactly one credential per
//! (database, type). `RegisterApplication$Credentials` and
//! `UpdateApplicationSettings$CredentialsToAddOrUpdate` are the same kind of list, so the
//! same declarations leave the same credentials whichever call carried them, and a later
//! update of such a credential replaces the ONE credential the application holds.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: what the application
//! holds is only believed when `ListDatabases` and `GetDatabase` report it back.
//!
//! The table spans what the rule generalises over: how many declarations name one
//! credential (two, three), whether the repeated declarations carry different secrets or
//! the same one, where the repetition sits among declarations of other credentials, the
//! FORM of the database name the repeated declarations spell (two spellings of one
//! `DatabaseId`, two spellings differing only in case), how many distinct credentials the
//! registration declares, and the `ApplicationType` carrying them.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};

use crate::application_lifecycle::{settle, valid_registration, Emulator};
use crate::list_databases::listed;

/// The single `CredentialType` the model defines
/// (`service_com_amazonaws_ssmsap_ssmsap::ops::types::CREDENTIAL_TYPE_VALUES`).
const ADMIN: &str = "ADMIN";

/// The secret a later update re-declares a credential with.
const ROTATED_SECRET: &str = "sap/rotated/admin";

/// One registration's credential declarations, and the credentials they must leave the
/// application holding.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// The `(DatabaseName, SecretId)` of every credential the registration DECLARES, in
    /// the order it declares them.
    declared: &'static [(&'static str, &'static str)],
    /// The `(DatabaseId, SecretId)` of every credential the application must then hold,
    /// one per database, in `DatabaseId` order.
    expected: &'static [(&'static str, &'static str)],
    /// The `(DatabaseName, DatabaseId)` of the credential a later update re-declares,
    /// for a row whose registration declared one credential more than once; `None` for a
    /// row that declared each credential once.
    rotated: Option<(&'static str, &'static str)>,
}

/// The rows.
const CASES: &[Case] = &[
    // Two declarations of one database, with different secrets: the secret declared last
    // is the one that reaches it.
    Case {
        name: "one database declared twice with different secrets",
        application_id: "HDB-DUP",
        application_type: "HANA",
        declared: &[("HDB", "secret-one"), ("HDB", "secret-two")],
        expected: &[("HDB", "secret-two")],
        rotated: Some(("HDB", "HDB")),
    },
    // Three declarations of one database: still one credential, still the last secret.
    Case {
        name: "one database declared three times",
        application_id: "declared-thrice",
        application_type: "HANA",
        declared: &[
            ("HDB", "sap/hdb/first"),
            ("HDB", "sap/hdb/second"),
            ("HDB", "sap/hdb/third"),
        ],
        expected: &[("HDB", "sap/hdb/third")],
        rotated: Some(("HDB", "HDB")),
    },
    // The very same declaration twice, secret included: one credential.
    Case {
        name: "the same declaration repeated verbatim",
        application_id: "declared-verbatim-twice",
        application_type: "HANA",
        declared: &[("HDB", "sap/hdb/admin"), ("HDB", "sap/hdb/admin")],
        expected: &[("HDB", "sap/hdb/admin")],
        rotated: Some(("HDB", "HDB")),
    },
    // A repeated declaration among declarations of OTHER credentials: only the repeated
    // one collapses, and the others are each their own credential.
    Case {
        name: "a repeated declaration among distinct ones",
        application_id: "duplicate-among-distinct",
        application_type: "HANA",
        declared: &[
            ("HDB", "sap/hdb/first"),
            ("TNT1", "sap/tnt1/admin"),
            ("HDB", "sap/hdb/last"),
        ],
        expected: &[("HDB", "sap/hdb/last"), ("TNT1", "sap/tnt1/admin")],
        rotated: Some(("HDB", "HDB")),
    },
    // Two SPELLINGS of one database: the `DatabaseId` a name yields is what identifies
    // the database (every character the modelled `DatabaseId` pattern `[\w\d]+$` does not
    // admit becomes `_`, and every ASCII letter is folded to upper case), so these two
    // name ONE credential.
    Case {
        name: "two spellings of one DatabaseId",
        application_id: "punctuated-duplicate",
        application_type: "HANA",
        declared: &[("prod db.1", "sap/prod/first"), ("prod_db_1", "sap/prod/last")],
        expected: &[("PROD_DB_1", "sap/prod/last")],
        rotated: Some(("prod db.1", "PROD_DB_1")),
    },
    // Two spellings differing only in CASE: an SAP HANA database name is
    // case-insensitive, so these name one database and one credential of it.
    Case {
        name: "names differing only in case are one database",
        application_id: "case-distinct",
        application_type: "HANA",
        declared: &[("HDB", "sap/upper/admin"), ("hdb", "sap/lower/admin")],
        expected: &[("HDB", "sap/lower/admin")],
        rotated: Some(("hDb", "HDB")),
    },
    // Distinct databases throughout: nothing collapses, and each is its own credential.
    Case {
        name: "three distinct databases",
        application_id: "distinct-databases",
        application_type: "HANA",
        declared: &[
            ("SYSTEMDB", "sap/systemdb/admin"),
            ("HDB", "sap/hdb/admin"),
            ("TNT1", "sap/tnt1/admin"),
        ],
        expected: &[
            ("HDB", "sap/hdb/admin"),
            ("SYSTEMDB", "sap/systemdb/admin"),
            ("TNT1", "sap/tnt1/admin"),
        ],
        rotated: None,
    },
    // Every row is of the type whose registration CARRIES credentials: the other modelled
    // type runs on the SAP HANA database of such an application and names it by ARN, so a
    // registration of that type carries no credentials at all
    // (`service_com_amazonaws_ssmsap_ssmsap::state::CONDITIONALLY_REQUIRED_MEMBERS`) and
    // the declarations this rule is about never reach one through a registration. The same
    // declarations reaching one through `UpdateApplicationSettings` are what
    // `tests/all/update_application_settings.rs` drives.
];

/// One `(DatabaseName, SecretId)` pair as the `ApplicationCredential` the wire carries.
fn credential(database_name: &str, secret_id: &str) -> Value {
    json!({
        "DatabaseName": database_name,
        "CredentialType": ADMIN,
        "SecretId": secret_id,
    })
}

/// A `Credentials` / `CredentialsToAddOrUpdate` list.
fn credential_list(credentials: &[(&str, &str)]) -> Value {
    Value::Array(
        credentials
            .iter()
            .map(|(database_name, secret_id)| credential(database_name, secret_id))
            .collect(),
    )
}

/// The `RegisterApplication` body for `application_id`, carrying `declared` as its
/// `Credentials`.
fn registration(
    application_id: &str,
    application_type: &str,
    declared: &[(&str, &str)],
) -> Value {
    let mut body: Map<String, Value> = valid_registration(application_id, application_type)
        .as_object()
        .cloned()
        .expect("a registration body is a JSON object");
    body.insert("Credentials".into(), credential_list(declared));
    Value::Object(body)
}

/// The `SecretId`s `GetDatabase` reports for every database `ListDatabases` reports for
/// `application_id`, keyed by `DatabaseId` — every credential, so a database holding more
/// than one is visible here.
async fn held_credentials(
    emulator: &Emulator,
    application_id: &str,
) -> BTreeMap<String, Vec<String>> {
    let reply = emulator
        .send("/list-databases", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from ListDatabases for {application_id:?}, got {reply:?}"
    );
    let mut held: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for summary in listed(&reply) {
        let database_id = summary.id().to_string();
        let database = emulator
            .send(
                "/get-database",
                json!({"ApplicationId": application_id, "DatabaseId": database_id}),
            )
            .await;
        assert_eq!(
            database.status, 200,
            "expected 200 from GetDatabase for {application_id:?}/{database_id:?}, got {database:?}"
        );
        let secrets = database
            .member("Database")
            .and_then(|database| database.get("Credentials"))
            .and_then(Value::as_array)
            .map(|credentials| {
                credentials
                    .iter()
                    .map(|credential| {
                        credential
                            .get("SecretId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();
        held.insert(database_id, secrets);
    }
    held
}

/// The `(DatabaseId, SecretId)` pairs `expected` describes, as the map
/// [`held_credentials`] reads: one credential per database.
fn expected_credentials(expected: &[(&str, &str)]) -> BTreeMap<String, Vec<String>> {
    expected
        .iter()
        .map(|(database_id, secret_id)| {
            (
                (*database_id).to_string(),
                ::std::vec![(*secret_id).to_string()],
            )
        })
        .collect()
}

/// `RegisterApplication` with `body`, asserting it was accepted.
async fn register(emulator: &Emulator, case: &Case, body: Value) {
    let reply = emulator.send("/register-application", body).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
        case.name
    );
}

/// RULE: the credential declarations a registration carries are applied by credential
/// identity, so the application is left holding ONE credential per (SAP HANA database,
/// `CredentialType`), carrying the secret the last declaration of that credential named.
#[tokio::test]
async fn a_registration_holds_one_credential_per_database_and_type() {
    for case in CASES {
        // One emulator per row: an account is allotted only so many registered
        // applications per Region
        // (`service_com_amazonaws_ssmsap_ssmsap::state::APPLICATIONS_QUOTA`), and each row
        // is a registration of its own, so the rows must not compete for that allotment.
        let emulator = Emulator::new();
        register(
            &emulator,
            case,
            registration(case.application_id, case.application_type, case.declared),
        )
        .await;

        let held = held_credentials(&emulator, case.application_id).await;
        assert_eq!(
            held,
            expected_credentials(case.expected),
            "case {:?}: expected the declarations {:?} to leave {:?}, got {held:?}",
            case.name,
            case.declared,
            case.expected
        );
    }
}

/// RULE: a registration's `Credentials` and an update's `CredentialsToAddOrUpdate` are
/// the same list of declarations, so the same declarations leave the application holding
/// the same credentials whichever call carried them.
#[tokio::test]
async fn the_registering_and_updating_paths_agree_on_the_same_declarations() {
    for case in CASES {
        // One emulator per row, for the reason the suite's first case states: each row
        // registers two applications of its own.
        let emulator = Emulator::new();

        // The registering path: every declaration in the registration.
        register(
            &emulator,
            case,
            registration(case.application_id, case.application_type, case.declared),
        )
        .await;
        let registered = held_credentials(&emulator, case.application_id).await;

        // The updating path: the same application, registered with the FIRST declaration
        // alone (an application of a type that requires credentials cannot be registered
        // with none), then sent every declaration through the update.
        let updated_id = format!("{}-via-update", case.application_id);
        register(
            &emulator,
            case,
            registration(&updated_id, case.application_type, &case.declared[..1]),
        )
        .await;
        settle().await;
        let reply = emulator
            .send(
                "/update-application-settings",
                json!({
                    "ApplicationId": updated_id,
                    "CredentialsToAddOrUpdate": credential_list(case.declared),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from UpdateApplicationSettings, got {reply:?}",
            case.name
        );
        settle().await;
        let updated = held_credentials(&emulator, &updated_id).await;

        assert_eq!(
            registered, updated,
            "case {:?}: expected both write paths to leave the same credentials for {:?}, \
             registering left {registered:?} and updating left {updated:?}",
            case.name, case.declared
        );
        assert_eq!(
            registered,
            expected_credentials(case.expected),
            "case {:?}: expected {:?}, got {registered:?}",
            case.name,
            case.expected
        );
    }
}

/// RULE: because the registration left ONE credential for that database and type, a later
/// update re-declaring it replaces that credential — the application holds no other secret
/// for the same database afterwards.
#[tokio::test]
async fn an_update_replaces_the_one_credential_a_registration_left() {
    for case in CASES {
        // One emulator per row, for the reason the suite's first case states.
        let emulator = Emulator::new();
        let Some((database_name, database_id)) = case.rotated else {
            continue;
        };
        register(
            &emulator,
            case,
            registration(case.application_id, case.application_type, case.declared),
        )
        .await;
        settle().await;

        let reply = emulator
            .send(
                "/update-application-settings",
                json!({
                    "ApplicationId": case.application_id,
                    "CredentialsToAddOrUpdate": credential_list(&[(database_name, ROTATED_SECRET)]),
                }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from UpdateApplicationSettings, got {reply:?}",
            case.name
        );
        settle().await;

        let expected: BTreeMap<String, Vec<String>> = expected_credentials(case.expected)
            .into_iter()
            .map(|(id, secrets)| {
                if id == database_id {
                    (id, ::std::vec![ROTATED_SECRET.to_string()])
                } else {
                    (id, secrets)
                }
            })
            .collect();
        let held = held_credentials(&emulator, case.application_id).await;
        assert_eq!(
            held, expected,
            "case {:?}: expected the update of database {database_name:?} to leave \
             {expected:?}, got {held:?}",
            case.name
        );
    }
}
