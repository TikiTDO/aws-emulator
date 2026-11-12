// SPDX-License-Identifier: CC0-1.0

//! `UpdateApplicationSettings`, driven over the wire.
//!
//! RULE: an update of a registered application's settings is asynchronous — the call
//! files ONE operation record, which its `OperationIds` names and `GetOperation` reads
//! back, and the settings the call named become the application's own settings when that
//! unit of work falls due, so a caller acting straight after the call still sees the
//! settings the application had. A setting the call did not name is left exactly as it
//! was; a credential is identified by the SAP HANA database it reaches and its
//! `CredentialType`, so a credential named for a database and type the application
//! already holds one for REPLACES it and one it holds none for is added; and the database
//! an update names replaces the application's SAP HANA connection. An update the
//! application cannot take is refused and changes nothing.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: a change is only believed
//! when `GetDatabase`, `ListDatabases` or `GetOperation` reports it back.
//!
//! The table spans what the rule generalises over: which settings members the call
//! carries (credentials added, a secret replaced, a credential removed, an add beside a
//! remove, the Backint configuration alone, the database connection alone, every setting
//! at once), how many credentials are involved, the form of the database name they reach,
//! whether the removal names the secret the application holds, and the application type.
//! The ceiling on how many credentials one application holds is driven on its published
//! bound and one credential past it.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::BACKINT_MODE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::{
    application_not_found_message, conditional_requirement_message,
    MAX_CREDENTIALS_PER_APPLICATION, MISSING_SETTINGS_MESSAGE, OPERATION_STATUS_INPROGRESS,
    OPERATION_STATUS_SUCCESS, OPERATION_TYPE_UPDATE_APPLICATION_SETTINGS,
};

use crate::application_lifecycle::{is_operation_id, settle, valid_registration, Emulator, Reply};
use crate::list_databases::{expected_arn, listed};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The wire error code of the modelled 409 (`smithy_query(query="operation",
/// operation="UpdateApplicationSettings")`: `ConflictException` `@httpError 409`).
const CONFLICT_EXCEPTION: &str = "ConflictException";

/// The wire error code of the modelled 404 (same query: `ResourceNotFoundException`
/// `@httpError 404`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The single `CredentialType` the model defines
/// (`service_com_amazonaws_ssmsap_ssmsap::ops::types::CREDENTIAL_TYPE_VALUES`).
const ADMIN: &str = "ADMIN";

/// One `(DatabaseName, SecretId)` pair as the `ApplicationCredential` the wire carries.
fn credential(database_name: &str, secret_id: &str) -> Value {
    json!({
        "DatabaseName": database_name,
        "CredentialType": ADMIN,
        "SecretId": secret_id,
    })
}

/// A `Credentials` / `CredentialsToAddOrUpdate` / `CredentialsToRemove` list.
fn credential_list(credentials: &[(&str, &str)]) -> Value {
    Value::Array(
        credentials
            .iter()
            .map(|(database_name, secret_id)| credential(database_name, secret_id))
            .collect(),
    )
}

/// One update of one application's settings, and the settings it must leave behind.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// The `(DatabaseName, SecretId)` of every credential the REGISTRATION carries.
    registered: &'static [(&'static str, &'static str)],
    /// The `CredentialsToAddOrUpdate` the update carries.
    add_or_update: &'static [(&'static str, &'static str)],
    /// The `CredentialsToRemove` it carries.
    remove: &'static [(&'static str, &'static str)],
    /// The `Backint$EnsureNoBackupInProcess` the update carries, when it carries a
    /// `Backint` configuration at all.
    backint: Option<bool>,
    /// The `DatabaseName` of the SAP HANA database the update points the application's
    /// connection at, when it names one. Its ARN is that database as a sub-resource of
    /// ANOTHER application, which is what a connection pointed elsewhere names.
    connect_database: Option<&'static str>,
    /// The `(DatabaseId, SecretId)` of every credential the application must hold once
    /// the update has fallen due, read back one database at a time.
    expected_credentials: &'static [(&'static str, &'static str)],
    /// The `DatabaseId` of every database the application must have then, in the order
    /// `ListDatabases` must report them.
    expected_databases: &'static [&'static str],
    /// The settings the answered `Message` must name.
    expected_settings_named: &'static [&'static str],
}

/// The application whose database an update points a connection at: a database of this
/// application is a database of some OTHER application's registration.
const CONNECTION_SOURCE: &str = "hana-source";

/// The rows.
const CASES: &[Case] = &[
    // A credential for a database the application holds none for: the credential is
    // added, and the database it reaches is one of the application's databases.
    Case {
        name: "a new credential adds the database it reaches",
        application_id: "HDB-TEST",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin")],
        add_or_update: &[("TNT1", "sap/tnt1/admin")],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[("HDB", "sap/hdb/admin"), ("TNT1", "sap/tnt1/admin")],
        expected_databases: &["HDB", "TNT1"],
        expected_settings_named: &["credentials"],
    },
    // The same database and type as one already held: the secret that reaches it is
    // replaced rather than a second credential added.
    Case {
        name: "a credential for a database already held replaces its secret",
        application_id: "rotated-secret",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin")],
        add_or_update: &[("HDB", "sap/hdb/rotated")],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[("HDB", "sap/hdb/rotated")],
        expected_databases: &["HDB"],
        expected_settings_named: &["credentials"],
    },
    // A removal drops the credential of that database and type — and with it the
    // database the credential was the only mention of.
    Case {
        name: "a removed credential takes the database it was the only mention of",
        application_id: "removed-credential",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin"), ("TNT1", "sap/tnt1/admin")],
        add_or_update: &[],
        remove: &[("TNT1", "sap/tnt1/admin")],
        backint: None,
        connect_database: None,
        expected_credentials: &[("HDB", "sap/hdb/admin")],
        expected_databases: &["HDB"],
        expected_settings_named: &["credentials"],
    },
    // The secret a removal names is NOT what identifies the credential: the database and
    // the type are, so a removal naming another secret for that database still drops it.
    Case {
        name: "a removal naming another secret for the database still drops it",
        application_id: "removed-by-database",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin"), ("TNT1", "sap/tnt1/admin")],
        add_or_update: &[],
        remove: &[("TNT1", "sap/some/other/secret")],
        backint: None,
        connect_database: None,
        expected_credentials: &[("HDB", "sap/hdb/admin")],
        expected_databases: &["HDB"],
        expected_settings_named: &["credentials"],
    },
    // An add beside a remove in ONE call: both apply, and the databases follow.
    Case {
        name: "an add beside a remove in one call",
        application_id: "added-and-removed",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin"), ("TNT1", "sap/tnt1/admin")],
        add_or_update: &[("TNT2", "sap/tnt2/admin")],
        remove: &[("TNT1", "sap/tnt1/admin")],
        backint: None,
        connect_database: None,
        expected_credentials: &[("HDB", "sap/hdb/admin"), ("TNT2", "sap/tnt2/admin")],
        expected_databases: &["HDB", "TNT2"],
        expected_settings_named: &["credentials"],
    },
    // A database name of a form the modelled `DatabaseId` does not admit: the credential
    // reaches it all the same, under the id that name yields.
    Case {
        name: "a credential for a database whose name is not of the DatabaseId form",
        application_id: "punctuated-database",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin")],
        add_or_update: &[("prod db.1", "sap/prod/admin")],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[("HDB", "sap/hdb/admin"), ("PROD_DB_1", "sap/prod/admin")],
        expected_databases: &["HDB", "PROD_DB_1"],
        expected_settings_named: &["credentials"],
    },
    // The AWS Backint Agent configuration ALONE: the credentials and databases the
    // application has are left exactly as they were.
    Case {
        name: "the Backint agent configuration alone",
        application_id: "backint-only",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin")],
        add_or_update: &[],
        remove: &[],
        backint: Some(true),
        connect_database: None,
        expected_credentials: &[("HDB", "sap/hdb/admin")],
        expected_databases: &["HDB"],
        expected_settings_named: &["AWS Backint Agent for SAP HANA installation"],
    },
    // The database connection ALONE, on an application whose registration named no
    // database at all: the named database becomes the application's.
    Case {
        name: "the SAP HANA database connection alone",
        application_id: "abap-connection",
        application_type: "SAP_ABAP",
        registered: &[],
        add_or_update: &[],
        remove: &[],
        backint: None,
        connect_database: Some("SYSTEMDB"),
        expected_credentials: &[],
        expected_databases: &["SYSTEMDB"],
        expected_settings_named: &["SAP HANA database connection"],
    },
    // Every setting in one call.
    Case {
        name: "every setting in one call",
        application_id: "all-settings",
        application_type: "HANA",
        registered: &[("HDB", "sap/hdb/admin")],
        add_or_update: &[("TNT1", "sap/tnt1/admin")],
        remove: &[],
        backint: Some(false),
        connect_database: Some("AAA"),
        expected_credentials: &[("HDB", "sap/hdb/admin"), ("TNT1", "sap/tnt1/admin")],
        expected_databases: &["AAA", "HDB", "TNT1"],
        expected_settings_named: &[
            "credentials",
            "AWS Backint Agent for SAP HANA installation",
            "SAP HANA database connection",
        ],
    },
    // The same credential rule on the other application type, whose registration carries
    // no credentials at all.
    Case {
        name: "a credential added to an application of the other type",
        application_id: "abap-credential",
        application_type: "SAP_ABAP",
        registered: &[],
        add_or_update: &[("TNT9", "sap/tnt9/admin")],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[("TNT9", "sap/tnt9/admin")],
        expected_databases: &["TNT9"],
        expected_settings_named: &["credentials"],
    },
];

/// The `RegisterApplication` body for `case`.
fn registration(case: &Case) -> Value {
    let mut body: Map<String, Value> = valid_registration(case.application_id, case.application_type)
        .as_object()
        .cloned()
        .expect("a registration body is a JSON object");
    if case.registered.is_empty() {
        body.remove("Credentials");
    } else {
        body.insert("Credentials".into(), credential_list(case.registered));
    }
    Value::Object(body)
}

/// The `UpdateApplicationSettings` body for `case`.
fn update_request(case: &Case) -> Value {
    let mut body = Map::new();
    body.insert("ApplicationId".into(), json!(case.application_id));
    if !case.add_or_update.is_empty() {
        body.insert(
            "CredentialsToAddOrUpdate".into(),
            credential_list(case.add_or_update),
        );
    }
    if !case.remove.is_empty() {
        body.insert("CredentialsToRemove".into(), credential_list(case.remove));
    }
    if let Some(ensure_no_backup_in_process) = case.backint {
        body.insert(
            "Backint".into(),
            json!({
                // The model's single `BackintMode` value.
                "BackintMode": BACKINT_MODE_VALUES[0],
                "EnsureNoBackupInProcess": ensure_no_backup_in_process,
            }),
        );
    }
    if let Some(database_name) = case.connect_database {
        body.insert(
            "DatabaseArn".into(),
            json!(expected_arn(CONNECTION_SOURCE, database_name)),
        );
    }
    Value::Object(body)
}

/// `UpdateApplicationSettings` with `body`.
async fn update(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("/update-application-settings", body).await
}

/// Register `case`'s application and wait out the clock, so it is up and owed nothing.
async fn register_settled(emulator: &Emulator, case: &Case) {
    let reply = emulator
        .send("/register-application", registration(case))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
        case.name
    );
    settle().await;
}

/// The `DatabaseId` of every database `ListDatabases` reports for `application_id`, in
/// the order it reported them.
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

/// The `SecretId` of the `ADMIN` credential `GetDatabase` reports for every database of
/// `application_id`, keyed by `DatabaseId`.
async fn held_secrets(emulator: &Emulator, application_id: &str) -> BTreeMap<String, String> {
    let mut held = BTreeMap::new();
    for database_id in database_ids(emulator, application_id).await {
        let reply = emulator
            .send(
                "/get-database",
                json!({"ApplicationId": application_id, "DatabaseId": database_id}),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "expected 200 from GetDatabase for {application_id:?}/{database_id:?}, got {reply:?}"
        );
        let credentials = reply
            .member("Database")
            .and_then(|database| database.get("Credentials"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for credential in credentials {
            let secret_id = credential
                .get("SecretId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            held.insert(database_id.clone(), secret_id);
        }
    }
    held
}

/// The `(DatabaseId, SecretId)` pairs `case` expects, as the map [`held_secrets`] reads.
fn expected_secrets(case: &Case) -> BTreeMap<String, String> {
    case.expected_credentials
        .iter()
        .map(|(database_id, secret_id)| ((*database_id).to_string(), (*secret_id).to_string()))
        .collect()
}

/// The `OperationIds` a reply reported, in the order it reported them.
fn operation_ids(reply: &Reply) -> Vec<String> {
    reply
        .member("OperationIds")
        .and_then(Value::as_array)
        .map(|ids| {
            ids.iter()
                .map(|id| id.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `GetOperation` for `operation_id`.
async fn get_operation(emulator: &Emulator, operation_id: &str) -> Reply {
    emulator
        .send("/get-operation", json!({"OperationId": operation_id}))
        .await
}

/// RULE: the settings an update names become the application's own settings when the
/// update falls due, and every other setting is left as it was — read back through
/// `GetDatabase` and `ListDatabases`.
#[tokio::test]
async fn the_settings_an_update_names_become_the_applications_settings() {
    let emulator = Emulator::new();

    for case in CASES {
        register_settled(&emulator, case).await;
        let registered_secrets = held_secrets(&emulator, case.application_id).await;
        let registered_databases = database_ids(&emulator, case.application_id).await;

        let reply = update(&emulator, update_request(case)).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from UpdateApplicationSettings, got {reply:?}",
            case.name
        );

        // The call answers the ONE record it filed, whose id is of the modelled
        // `OperationId` form.
        let ids = operation_ids(&reply);
        assert_eq!(
            ids.len(),
            1,
            "case {:?}: expected one OperationId, got {ids:?}",
            case.name
        );
        assert!(
            is_operation_id(&ids[0]),
            "case {:?}: OperationId {:?} is not of the modelled form",
            case.name,
            ids[0]
        );

        // The `Message` names the application and the settings the update changes.
        let stated = reply
            .member("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            stated.contains(case.application_id),
            "case {:?}: expected the message to name the application, got {stated:?}",
            case.name
        );
        for named in case.expected_settings_named {
            assert!(
                stated.contains(named),
                "case {:?}: expected the message to name {named:?}, got {stated:?}",
                case.name
            );
        }

        // The update is asynchronous, so straight after the call the application still
        // has the settings it had and the record it filed is still running.
        let pending_secrets = held_secrets(&emulator, case.application_id).await;
        assert_eq!(
            pending_secrets, registered_secrets,
            "case {:?}: expected the credentials to be unchanged until the update falls \
             due, got {pending_secrets:?}",
            case.name
        );
        let pending_databases = database_ids(&emulator, case.application_id).await;
        assert_eq!(
            pending_databases, registered_databases,
            "case {:?}: expected the databases to be unchanged until the update falls due, \
             got {pending_databases:?}",
            case.name
        );
        let running = get_operation(&emulator, &ids[0]).await;
        assert_eq!(
            running.op_str("Status"),
            OPERATION_STATUS_INPROGRESS,
            "case {:?}: expected the filed record to be running, got {running:?}",
            case.name
        );

        settle().await;

        let settled_secrets = held_secrets(&emulator, case.application_id).await;
        let expected = expected_secrets(case);
        assert_eq!(
            settled_secrets, expected,
            "case {:?}: expected the credentials {expected:?}, got {settled_secrets:?}",
            case.name
        );
        let settled_databases = database_ids(&emulator, case.application_id).await;
        assert_eq!(
            settled_databases, case.expected_databases,
            "case {:?}: expected the databases {:?}, got {settled_databases:?}",
            case.name, case.expected_databases
        );

        // The database an update pointed the connection at is reported under the ARN the
        // update named, which is its own.
        if let Some(database_name) = case.connect_database {
            let expected_database_arn = expected_arn(CONNECTION_SOURCE, database_name);
            let reply = emulator
                .send(
                    "/get-database",
                    json!({"DatabaseArn": expected_database_arn.clone()}),
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: expected 200 from GetDatabase for the connected database, got \
                 {reply:?}",
                case.name
            );
            let reported_application = reply
                .member("Database")
                .and_then(|database| database.get("ApplicationId"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                reported_application, case.application_id,
                "case {:?}: expected the connected database to belong to the updated \
                 application, got {reported_application:?}",
                case.name
            );
        }
    }
}

/// RULE: the record an update files is the unit of work the call asked for — of the
/// update type, against the application the call named, carrying what the update asked
/// for and never a credential's secret — and it reaches its terminal status on the clock.
#[tokio::test]
async fn the_record_an_update_files_reports_the_update_it_was_asked_for() {
    let emulator = Emulator::new();

    for case in CASES {
        register_settled(&emulator, case).await;
        let reply = update(&emulator, update_request(case)).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from UpdateApplicationSettings, got {reply:?}",
            case.name
        );
        let ids = operation_ids(&reply);
        let operation_id = ids.first().cloned().unwrap_or_default();

        settle().await;

        let record = get_operation(&emulator, &operation_id).await;
        assert_eq!(
            record.status, 200,
            "case {:?}: expected 200 from GetOperation, got {record:?}",
            case.name
        );
        assert_eq!(
            record.op_str("Type"),
            OPERATION_TYPE_UPDATE_APPLICATION_SETTINGS,
            "case {:?}: expected the record's Type, got {record:?}",
            case.name
        );
        assert_eq!(
            record.op_str("ResourceId"),
            case.application_id,
            "case {:?}: expected the record to name the application, got {record:?}",
            case.name
        );
        assert_eq!(
            record.op_str("Status"),
            OPERATION_STATUS_SUCCESS,
            "case {:?}: expected the record to have succeeded on the clock, got {record:?}",
            case.name
        );

        // The secret a credential names is `@sensitive`, so nothing the record reports
        // carries one.
        let rendered = record.body.to_string();
        for (_, secret_id) in case.add_or_update.iter().chain(case.remove.iter()) {
            assert!(
                !rendered.contains(secret_id),
                "case {:?}: expected the record to carry no secret id, got {rendered:?}",
                case.name
            );
        }

        // What the record carries of the update is the setting the call asked to change.
        let properties = record
            .op("Properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let expected_keys: Vec<&str> = [
            (!case.add_or_update.is_empty()).then_some("CredentialsToAddOrUpdate"),
            (!case.remove.is_empty()).then_some("CredentialsToRemove"),
            case.backint.map(|_| "BackintMode"),
            case.connect_database.map(|_| "DatabaseArn"),
        ]
        .into_iter()
        .flatten()
        .collect();
        for key in expected_keys {
            assert!(
                properties.contains_key(key),
                "case {:?}: expected the record's Properties to carry {key:?}, got \
                 {properties:?}",
                case.name
            );
        }
    }
}

/// One update the service must refuse, and how.
struct Refusal {
    name: &'static str,
    /// The `UpdateApplicationSettings` body, built against the application
    /// [`REFUSAL_APPLICATION`] registered below.
    body: fn(&str) -> Value,
    expected_status: u16,
    expected_code: &'static str,
    /// What the refusal's message must state.
    expected_message: fn(&str) -> String,
}

/// The application every refusal row is aimed at: a HANA application holding ONE
/// credential, so a row can try to take its last one away.
const REFUSAL_APPLICATION: &str = "refused-update";

/// The `(DatabaseName, SecretId)` of that application's single credential.
const REFUSAL_CREDENTIAL: (&str, &str) = ("HDB", "sap/hdb/admin");

/// The rows: every update this service refuses, one per condition.
fn refusals() -> Vec<Refusal> {
    vec![
        Refusal {
            name: "an update naming no setting at all",
            body: |id| json!({"ApplicationId": id}),
            expected_status: 400,
            expected_code: VALIDATION_EXCEPTION,
            expected_message: |_| MISSING_SETTINGS_MESSAGE.to_string(),
        },
        Refusal {
            name: "an update of an application that is not registered",
            body: |_| {
                json!({
                    "ApplicationId": "not-registered",
                    "CredentialsToAddOrUpdate": credential_list(&[("HDB", "sap/hdb/admin")]),
                })
            },
            expected_status: 404,
            expected_code: RESOURCE_NOT_FOUND_EXCEPTION,
            expected_message: |_| application_not_found_message("not-registered"),
        },
        Refusal {
            name: "a removal of a credential the application does not hold",
            body: |id| {
                json!({
                    "ApplicationId": id,
                    "CredentialsToRemove": credential_list(&[("TNT4", "sap/tnt4/admin")]),
                })
            },
            expected_status: 400,
            expected_code: VALIDATION_EXCEPTION,
            expected_message: |id| format!("Application {id} has no ADMIN credential for database TNT4"),
        },
        Refusal {
            name: "a credential named both for update and for removal",
            body: |id| {
                json!({
                    "ApplicationId": id,
                    "CredentialsToAddOrUpdate": credential_list(&[REFUSAL_CREDENTIAL]),
                    "CredentialsToRemove": credential_list(&[REFUSAL_CREDENTIAL]),
                })
            },
            expected_status: 400,
            expected_code: VALIDATION_EXCEPTION,
            expected_message: |_| {
                "The ADMIN credential for database HDB is named both for update and for \
                 removal in the same request"
                    .to_string()
            },
        },
        Refusal {
            name: "a removal of the last credential of an application that requires some",
            body: |id| {
                json!({
                    "ApplicationId": id,
                    "CredentialsToRemove": credential_list(&[REFUSAL_CREDENTIAL]),
                })
            },
            expected_status: 400,
            expected_code: VALIDATION_EXCEPTION,
            // The requirement an application of this type carries, stated in the words
            // its registration states it in.
            expected_message: |_| conditional_requirement_message("HANA"),
        },
        Refusal {
            name: "a Backint mode outside the modelled catalogue",
            body: |id| {
                json!({
                    "ApplicationId": id,
                    "Backint": {"BackintMode": "NotAMode", "EnsureNoBackupInProcess": true},
                })
            },
            expected_status: 400,
            expected_code: VALIDATION_EXCEPTION,
            // Refused at the boundary against the model's own enum, so the refusal names
            // the member rather than the catalogue.
            expected_message: |_| "backint".to_string(),
        },
    ]
}

/// RULE: an update the application cannot take is refused with the error the operation
/// models for that condition, and changes nothing a later caller sees.
#[tokio::test]
async fn an_update_the_application_cannot_take_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let case = Case {
        name: "the application every refusal row is aimed at",
        application_id: REFUSAL_APPLICATION,
        application_type: "HANA",
        registered: &[REFUSAL_CREDENTIAL],
        add_or_update: &[],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[],
        expected_databases: &[],
        expected_settings_named: &[],
    };
    register_settled(&emulator, &case).await;
    let before = held_secrets(&emulator, REFUSAL_APPLICATION).await;

    for refusal in refusals() {
        let reply = update(&emulator, (refusal.body)(REFUSAL_APPLICATION)).await;
        assert_eq!(
            reply.status, refusal.expected_status,
            "case {:?}: expected HTTP {}, got {reply:?}",
            refusal.name, refusal.expected_status
        );
        assert_eq!(
            reply.code(),
            refusal.expected_code,
            "case {:?}: expected {}, got {reply:?}",
            refusal.name,
            refusal.expected_code
        );
        let stated = message(&reply);
        let expected = (refusal.expected_message)(REFUSAL_APPLICATION);
        assert!(
            stated.contains(&expected),
            "case {:?}: expected the refusal to state {expected:?}, got {stated:?}",
            refusal.name
        );
        assert!(
            reply.member("OperationIds").is_none(),
            "case {:?}: a refused update files no operation, got {reply:?}",
            refusal.name
        );

        // Nothing was written: the credential the application holds is the one it was
        // registered with, now and once the clock has had its chance.
        settle().await;
        let after = held_secrets(&emulator, REFUSAL_APPLICATION).await;
        assert_eq!(
            after, before,
            "case {:?}: expected the credentials to be unchanged, got {after:?}",
            refusal.name
        );
    }
}

/// RULE: a change the clock still owes an application is work already under way on it, so
/// an update of its settings is refused with the modelled `ConflictException` (HTTP 409)
/// until that work completes — and the update is accepted once it has.
#[tokio::test]
async fn an_update_is_refused_while_a_change_is_already_owed() {
    let emulator = Emulator::new();
    let case = Case {
        name: "an application with work under way",
        application_id: "busy-application",
        application_type: "HANA",
        registered: &[REFUSAL_CREDENTIAL],
        add_or_update: &[("TNT1", "sap/tnt1/admin")],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[],
        expected_databases: &[],
        expected_settings_named: &[],
    };
    register_settled(&emulator, &case).await;

    // Starting the application leaves the next status the clock owes it outstanding.
    let started = emulator
        .send(
            "/start-application",
            json!({"ApplicationId": case.application_id}),
        )
        .await;
    assert_eq!(
        started.status, 200,
        "expected 200 from StartApplication, got {started:?}"
    );

    let refused = update(&emulator, update_request(&case)).await;
    assert_eq!(
        refused.status, 409,
        "expected 409 while a change is owed, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        CONFLICT_EXCEPTION,
        "expected {CONFLICT_EXCEPTION}, got {refused:?}"
    );
    assert!(
        message(&refused).contains(case.application_id),
        "expected the refusal to name the application, got {:?}",
        message(&refused)
    );

    settle().await;

    let accepted = update(&emulator, update_request(&case)).await;
    assert_eq!(
        accepted.status, 200,
        "expected the update to be accepted once the owed change fell due, got {accepted:?}"
    );

    // An update of its own is work under way too, so a second one is refused until it
    // falls due.
    let again = update(&emulator, update_request(&case)).await;
    assert_eq!(
        again.status, 409,
        "expected 409 while the update is still running, got {again:?}"
    );

    settle().await;

    let secrets = held_secrets(&emulator, case.application_id).await;
    let expected: BTreeMap<String, String> = [
        (REFUSAL_CREDENTIAL.0.to_string(), REFUSAL_CREDENTIAL.1.to_string()),
        ("TNT1".to_string(), "sap/tnt1/admin".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        secrets, expected,
        "expected the accepted update to have been applied, got {secrets:?}"
    );
}

/// RULE: the credentials ONE application holds are bounded by this emulator's own
/// ceiling, so an update that would leave it holding more is refused and writes nothing,
/// while one that stays within the ceiling is accepted.
#[tokio::test]
async fn an_update_cannot_grow_one_applications_credentials_past_the_ceiling() {
    let emulator = Emulator::new();
    let case = Case {
        name: "an application filled to the credential ceiling",
        application_id: "filled-credentials",
        application_type: "HANA",
        registered: &[REFUSAL_CREDENTIAL],
        add_or_update: &[],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[],
        expected_databases: &[],
        expected_settings_named: &[],
    };
    register_settled(&emulator, &case).await;

    // Fill the application to the ceiling, at most `MAX_PER_CALL` at a time — the
    // modelled `ApplicationCredentialList` bound of 0..20 is how many one call may carry.
    let mut held = case.registered.len();
    let mut minted = 0usize;
    while held < MAX_CREDENTIALS_PER_APPLICATION {
        let batch = (MAX_CREDENTIALS_PER_APPLICATION - held).min(MAX_PER_CALL);
        let credentials = fresh_credentials(minted, batch);
        minted += batch;
        let reply = update(
            &emulator,
            json!({
                "ApplicationId": case.application_id,
                "CredentialsToAddOrUpdate": owned_credential_list(&credentials),
            }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "expected 200 while filling to {MAX_CREDENTIALS_PER_APPLICATION} credentials \
             (holding {held}), got {reply:?}"
        );
        settle().await;
        held += batch;
    }

    // One past the ceiling.
    let past = fresh_credentials(minted, 1);
    let refused = update(
        &emulator,
        json!({
            "ApplicationId": case.application_id,
            "CredentialsToAddOrUpdate": owned_credential_list(&past),
        }),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "expected 400 past the credential ceiling, got {refused:?}"
    );
    assert_eq!(
        refused.code(),
        VALIDATION_EXCEPTION,
        "expected {VALIDATION_EXCEPTION}, got {refused:?}"
    );
    let stated = message(&refused);
    assert!(
        stated.contains(&MAX_CREDENTIALS_PER_APPLICATION.to_string())
            && stated.contains(case.application_id),
        "expected the refusal to state the ceiling and the application, got {stated:?}"
    );

    settle().await;

    // The refused call wrote nothing: the database its credential would have reached is
    // not one of the application's.
    let unwritten = emulator
        .send(
            "/get-database",
            json!({"ApplicationId": case.application_id, "DatabaseId": past[0].0.clone()}),
        )
        .await;
    assert!(
        unwritten.status >= 400,
        "expected the refused credential to have reached no database, got {unwritten:?}"
    );

    // An update that leaves the application AT the ceiling is still accepted: replacing
    // the secret of a credential it holds adds nothing.
    let at_ceiling = update(
        &emulator,
        json!({
            "ApplicationId": case.application_id,
            "CredentialsToAddOrUpdate": credential_list(&[(REFUSAL_CREDENTIAL.0, "sap/hdb/rotated")]),
        }),
    )
    .await;
    assert_eq!(
        at_ceiling.status, 200,
        "expected an update that stays at the ceiling to be accepted, got {at_ceiling:?}"
    );
}

/// How many credentials ONE call may carry: the modelled `ApplicationCredentialList`
/// bound (`smithy_query(query="shape", shape="ApplicationCredentialList")`: length
/// 0..20).
const MAX_PER_CALL: usize = 20;

/// `count` credentials for database names nothing holds yet, numbered from `from`.
fn fresh_credentials(from: usize, count: usize) -> Vec<(String, String)> {
    (from..from + count)
        .map(|index| (format!("TNT{index}"), format!("sap/tnt{index}/admin")))
        .collect()
}

/// A credential list built from owned `(DatabaseName, SecretId)` pairs.
fn owned_credential_list(credentials: &[(String, String)]) -> Value {
    Value::Array(
        credentials
            .iter()
            .map(|(database_name, secret_id)| credential(database_name, secret_id))
            .collect(),
    )
}

/// RULE: a second Backint configuration updates the one the application holds, which is
/// what the answered message states — the first one installs the agent.
#[tokio::test]
async fn a_second_backint_configuration_updates_the_one_held() {
    let emulator = Emulator::new();
    let case = Case {
        name: "an application configured for the Backint agent twice",
        application_id: "backint-twice",
        application_type: "HANA",
        registered: &[REFUSAL_CREDENTIAL],
        add_or_update: &[],
        remove: &[],
        backint: Some(true),
        connect_database: None,
        expected_credentials: &[],
        expected_databases: &[],
        expected_settings_named: &[],
    };
    register_settled(&emulator, &case).await;

    let installed = update(&emulator, update_request(&case)).await;
    assert_eq!(
        installed.status, 200,
        "expected 200 from the first Backint configuration, got {installed:?}"
    );
    let first = installed
        .member("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        first.contains("AWS Backint Agent for SAP HANA installation"),
        "expected the first configuration to install the agent, got {first:?}"
    );

    settle().await;

    let reconfigured = update(&emulator, update_request(&case)).await;
    assert_eq!(
        reconfigured.status, 200,
        "expected 200 from the second Backint configuration, got {reconfigured:?}"
    );
    let second = reconfigured
        .member("Message")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        second.contains("AWS Backint Agent for SAP HANA configuration"),
        "expected the second configuration to update the one held, got {second:?}"
    );
}

/// RULE: an application is registered in ONE account and Region, so an update signed for
/// another Region reaches no such application and changes nothing in the Region that
/// holds it.
#[tokio::test]
async fn an_update_signed_for_another_region_reaches_no_application() {
    let emulator = Emulator::new();
    let case = Case {
        name: "an application of one region updated through another",
        application_id: "region-scoped-update",
        application_type: "HANA",
        registered: &[REFUSAL_CREDENTIAL],
        add_or_update: &[("TNT1", "sap/tnt1/admin")],
        remove: &[],
        backint: None,
        connect_database: None,
        expected_credentials: &[],
        expected_databases: &[],
        expected_settings_named: &[],
    };
    register_settled(&emulator, &case).await;
    let before = held_secrets(&emulator, case.application_id).await;

    let reply = emulator
        .send_in_region(
            "eu-west-1",
            "/update-application-settings",
            update_request(&case),
        )
        .await;
    assert_eq!(
        reply.status, 404,
        "expected 404 at another Region's endpoint, got {reply:?}"
    );
    assert_eq!(
        reply.code(),
        RESOURCE_NOT_FOUND_EXCEPTION,
        "expected {RESOURCE_NOT_FOUND_EXCEPTION}, got {reply:?}"
    );

    settle().await;

    let after = held_secrets(&emulator, case.application_id).await;
    assert_eq!(
        after, before,
        "expected the application of the other Region to be untouched, got {after:?}"
    );
}
