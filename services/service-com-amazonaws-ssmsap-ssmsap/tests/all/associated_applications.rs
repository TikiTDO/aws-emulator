// SPDX-License-Identifier: CC0-1.0

//! The applications an `Application` reports as ASSOCIATED, driven over the wire.
//!
//! RULE under test: two applications registered in the same account and Region are
//! associated with one another exactly when they share an SAP HANA database — a database
//! an application derives from its SAP HANA connection (`DatabaseArn`) or from a
//! credential's `DatabaseName` — so `Application$AssociatedApplicationArns` reports the
//! ARNs of those other applications, symmetrically, whichever call last set that
//! connection (`RegisterApplication$DatabaseArn`, `UpdateApplicationSettings$DatabaseArn`)
//! and only for as long as both registrations exist. A database is matched by its own ARN,
//! the way every call that takes a `DatabaseArn` resolves one, so two applications that
//! merely happen to hold a database of the same NAME share nothing; an application sharing
//! no database with any other reports the member absent.
//!
//! Every call goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: an association is only
//! believed when `GetApplication` or `RegisterApplication` reports it back.
//!
//! The table spans what the rule generalises over: which application holds the shared
//! connection, the application types involved, how many applications share one database,
//! the form of the shared ARN (one this service mints for a sub-resource database and one
//! of another service with a `:`-separated resource), applications holding a database of
//! the same name but not the same database, and an application with no database at all.
//! The sequence the issue records — a registration, then an `UpdateApplicationSettings`
//! that names another application's HANA database — is one case below.

use serde_json::{json, Map, Value};

use crate::application_lifecycle::{settle, string_list, valid_registration, Emulator, Reply, REGION};
use crate::list_databases::{credentials, expected_arn};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// An ARN of ANOTHER service whose resource part is `:`-separated: the modelled
/// `SsmSapArn` pattern (`^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$`) admits both forms,
/// so what a shared database ARN may look like is not one spelling.
const COLON_SEGMENT_DATABASE_ARN: &str = "arn:aws:rds:us-east-1:000000000000:db:sap-prd";

/// The ARN `RegisterApplication` mints for the application `id` in `region`.
fn application_arn_in(region: &str, id: &str) -> String {
    format!("arn:aws:ssm-sap:{region}:{ACCOUNT}:application/{id}")
}

/// The ARN `RegisterApplication` mints for the application `id` in the Region the harness
/// signs for.
fn application_arn(id: &str) -> String {
    application_arn_in(REGION, id)
}

/// One registration a case sets up.
struct Registration {
    /// The `ApplicationId` it carries, which is also how an assertion names it.
    id: &'static str,
    /// An `ApplicationType` value it carries.
    app_type: &'static str,
    /// The `DatabaseName` of every credential it carries, replacing the ones the harness
    /// builds; an empty list leaves the registration carrying the harness's own.
    database_names: &'static [&'static str],
    /// The `DatabaseArn` it carries, when it names one.
    database_arn: Option<String>,
}

/// A set of registrations, and the applications each one must then report as associated.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    registrations: Vec<Registration>,
    /// The `ApplicationId`s of the applications each application must report as
    /// associated, in the order it must report them.
    expected: Vec<(&'static str, &'static [&'static str])>,
}

/// The rows.
fn cases() -> Vec<Case> {
    vec![
        // The pair the service is built around: an SAP_ABAP application whose SAP HANA
        // connection names the database of a registered HANA application. A third
        // application sharing nothing is registered beside them.
        Case {
            name: "an ABAP application registered with the HANA application's database ARN",
            registrations: vec![
                Registration {
                    id: "assoc-hdb",
                    app_type: "HANA",
                    database_names: &["SYSTEMDB"],
                    database_arn: None,
                },
                Registration {
                    id: "assoc-abap",
                    app_type: "SAP_ABAP",
                    database_names: &[],
                    database_arn: Some(expected_arn("assoc-hdb", "SYSTEMDB")),
                },
                Registration {
                    id: "assoc-bystander",
                    app_type: "SAP_ABAP",
                    database_names: &[],
                    database_arn: None,
                },
            ],
            expected: vec![
                ("assoc-hdb", &["assoc-abap"]),
                ("assoc-abap", &["assoc-hdb"]),
                ("assoc-bystander", &[]),
            ],
        },
        // The shared connection the other way round: one application's registration names
        // a database of the other's, which is the same sharing seen from the other record.
        Case {
            name: "a HANA application registered with the other application's database ARN",
            registrations: vec![
                Registration {
                    id: "reverse-abap",
                    app_type: "HANA",
                    database_names: &["TNT1"],
                    database_arn: None,
                },
                Registration {
                    id: "reverse-hdb",
                    app_type: "HANA",
                    database_names: &["SYSTEMDB"],
                    database_arn: Some(expected_arn("reverse-abap", "TNT1")),
                },
            ],
            expected: vec![
                ("reverse-abap", &["reverse-hdb"]),
                ("reverse-hdb", &["reverse-abap"]),
            ],
        },
        // Two applications holding a database of the same NAME hold two different
        // databases: each is a sub-resource of its own application, so neither is shared
        // and neither application is associated with anything.
        Case {
            name: "two applications holding a database of the same name share no database",
            registrations: vec![
                Registration {
                    id: "same-name-one",
                    app_type: "HANA",
                    database_names: &["SYSTEMDB"],
                    database_arn: None,
                },
                Registration {
                    id: "same-name-two",
                    app_type: "HANA",
                    database_names: &["SYSTEMDB"],
                    database_arn: None,
                },
            ],
            expected: vec![("same-name-one", &[]), ("same-name-two", &[])],
        },
        // THREE applications on one database: each reports the other two, in application
        // id order.
        Case {
            name: "two ABAP applications naming one HANA database",
            registrations: vec![
                Registration {
                    id: "shared-hdb",
                    app_type: "HANA",
                    database_names: &["TNT1"],
                    database_arn: None,
                },
                Registration {
                    id: "shared-abap-1",
                    app_type: "SAP_ABAP",
                    database_names: &[],
                    database_arn: Some(expected_arn("shared-hdb", "TNT1")),
                },
                Registration {
                    id: "shared-abap-2",
                    app_type: "SAP_ABAP",
                    database_names: &[],
                    database_arn: Some(expected_arn("shared-hdb", "TNT1")),
                },
            ],
            expected: vec![
                ("shared-hdb", &["shared-abap-1", "shared-abap-2"]),
                ("shared-abap-1", &["shared-abap-2", "shared-hdb"]),
                ("shared-abap-2", &["shared-abap-1", "shared-hdb"]),
            ],
        },
        // The shared ARN is one this service never mints, with a `:`-separated resource:
        // the applications that name it still share the database it names.
        Case {
            name: "a shared database ARN of another service",
            registrations: vec![
                Registration {
                    id: "colon-abap",
                    app_type: "SAP_ABAP",
                    database_names: &[],
                    database_arn: Some(COLON_SEGMENT_DATABASE_ARN.to_string()),
                },
                Registration {
                    id: "colon-hdb",
                    app_type: "HANA",
                    database_names: &["SYSTEMDB"],
                    database_arn: Some(COLON_SEGMENT_DATABASE_ARN.to_string()),
                },
            ],
            expected: vec![
                ("colon-abap", &["colon-hdb"]),
                ("colon-hdb", &["colon-abap"]),
            ],
        },
        // An application with no database at all, beside one that has databases of its
        // own: nothing is shared, so neither reports an association.
        Case {
            name: "an application with no database at all",
            registrations: vec![
                Registration {
                    id: "no-database-abap",
                    app_type: "SAP_ABAP",
                    database_names: &[],
                    database_arn: None,
                },
                Registration {
                    id: "databases-of-its-own",
                    app_type: "HANA",
                    database_names: &["SYSTEMDB", "TNT1"],
                    database_arn: None,
                },
            ],
            expected: vec![("no-database-abap", &[]), ("databases-of-its-own", &[])],
        },
    ]
}

/// The `RegisterApplication` body for `registration`.
fn registration_body(registration: &Registration) -> Value {
    let mut body: Map<String, Value> = valid_registration(registration.id, registration.app_type)
        .as_object()
        .cloned()
        .expect("a registration body is a JSON object");
    if !registration.database_names.is_empty() {
        body.insert("Credentials".into(), credentials(registration.database_names));
    }
    if let Some(database_arn) = registration.database_arn.as_ref() {
        body.insert("DatabaseArn".into(), json!(database_arn));
    }
    Value::Object(body)
}

/// Register `registration` in `region`, answering the reply so a caller can read the
/// `Application` the registration itself reported.
async fn register_in(emulator: &Emulator, region: &str, registration: &Registration) -> Reply {
    let reply = emulator
        .send_in_region(region, "/register-application", registration_body(registration))
        .await;
    assert_eq!(
        reply.status, 200,
        "registration {:?}: expected 200 from RegisterApplication, got {reply:?}",
        registration.id
    );
    reply
}

/// `GetApplication` for `id` in the Region the harness signs for.
async fn get(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .send("/get-application", json!({"ApplicationId": id}))
        .await
}

/// `UpdateApplicationSettings` naming `database_arn` as the application's SAP HANA
/// connection, asserting the update itself was accepted.
async fn connect(emulator: &Emulator, id: &str, database_arn: &str) {
    let reply = emulator
        .send(
            "/update-application-settings",
            json!({"ApplicationId": id, "DatabaseArn": database_arn}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "application {id:?}: expected 200 from UpdateApplicationSettings, got {reply:?}"
    );
}

/// The `AssociatedApplicationArns` of the `Application` a reply carries, and `None` when
/// the member is ABSENT — which is what an application associated with none reports.
fn associated(reply: &Reply) -> Option<Vec<String>> {
    reply
        .app("AssociatedApplicationArns")
        .map(|arns| string_list(Some(arns)))
}

/// The value `AssociatedApplicationArns` must carry for the applications `ids` names.
fn expected_arns(ids: &[&str]) -> Option<Vec<String>> {
    (!ids.is_empty()).then(|| ids.iter().map(|id| application_arn(id)).collect())
}

/// Assert `GetApplication` reports exactly the applications `ids` names as associated with
/// `id`.
async fn assert_associated(emulator: &Emulator, case: &str, id: &str, ids: &[&str]) {
    let reply = get(emulator, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetApplication for {id:?}, got {reply:?}"
    );
    let reported = associated(&reply);
    let expected = expected_arns(ids);
    assert_eq!(
        reported, expected,
        "case {case:?}: application {id:?}: expected AssociatedApplicationArns {expected:?}, got \
         {reported:?}"
    );
}

/// RULE: applications that share an SAP HANA database are associated with one another, and
/// each reports the others' ARNs in `AssociatedApplicationArns`; one sharing no database
/// reports the member absent.
#[tokio::test]
async fn applications_that_share_a_hana_database_report_one_another() {
    for case in cases() {
        // A fresh emulator per row, so what one row registers cannot associate anything in
        // another.
        let emulator = Emulator::new();
        for registration in &case.registrations {
            register_in(&emulator, REGION, registration).await;
        }

        for (id, expected) in &case.expected {
            assert_associated(&emulator, case.name, id, expected).await;
        }

        // Reading is a read: the same call answers the same associations again.
        for (id, expected) in &case.expected {
            assert_associated(&emulator, case.name, id, expected).await;
        }
    }
}

/// RULE: the association is what the store holds at the instant the `Application` is
/// projected, so the response of the registration that creates the sharing already reports
/// it — the same set a later `GetApplication` reports.
#[tokio::test]
async fn the_registration_that_creates_the_sharing_reports_it_in_its_own_response() {
    for case in cases() {
        let emulator = Emulator::new();
        let mut last: Option<Reply> = None;
        for registration in &case.registrations {
            last = Some(register_in(&emulator, REGION, registration).await);
        }
        let registration = case
            .registrations
            .last()
            .expect("every case registers at least one application");
        let reply = last.expect("every case registers at least one application");
        let reported = associated(&reply);
        let expected = case
            .expected
            .iter()
            .find(|(id, _)| *id == registration.id)
            .map(|(_, expected)| expected_arns(expected))
            .expect("every registration of a case has an expectation");
        assert_eq!(
            reported, expected,
            "case {:?}: the registration of {:?} must report AssociatedApplicationArns \
             {expected:?}, got {reported:?}",
            case.name, registration.id
        );
    }
}

/// RULE: `UpdateApplicationSettings$DatabaseArn` REPLACES the application's SAP HANA
/// connection, so once the change the clock owes falls due the application shares the
/// database the update named — and no longer the one it named before — which is what both
/// applications then report as their association.
#[tokio::test]
async fn an_update_that_replaces_the_hana_connection_moves_the_association() {
    let emulator = Emulator::new();
    let case = "an update that replaces the SAP HANA connection";
    for registration in [
        Registration {
            id: "move-hdb-a",
            app_type: "HANA",
            database_names: &["SYSTEMDB"],
            database_arn: None,
        },
        Registration {
            id: "move-hdb-b",
            app_type: "HANA",
            database_names: &["SYSTEMDB"],
            database_arn: None,
        },
        Registration {
            id: "move-abap",
            app_type: "SAP_ABAP",
            database_names: &[],
            database_arn: None,
        },
    ] {
        register_in(&emulator, REGION, &registration).await;
    }
    // The registrations themselves have to be settled before an update is accepted: a
    // change the clock still owes an application is an operation in progress on it.
    settle().await;

    // Nothing is shared yet.
    for id in ["move-abap", "move-hdb-a", "move-hdb-b"] {
        assert_associated(&emulator, case, id, &[]).await;
    }

    connect(&emulator, "move-abap", &expected_arn("move-hdb-a", "SYSTEMDB")).await;
    // The settings are owed, not applied: until they fall due the application's SAP HANA
    // connection is the one it had, so nothing is associated yet.
    for id in ["move-abap", "move-hdb-a"] {
        assert_associated(&emulator, "the update is still outstanding", id, &[]).await;
    }

    settle().await;
    assert_associated(&emulator, case, "move-abap", &["move-hdb-a"]).await;
    assert_associated(&emulator, case, "move-hdb-a", &["move-abap"]).await;
    assert_associated(&emulator, case, "move-hdb-b", &[]).await;

    // The second update names the OTHER HANA application's database, which replaces the
    // connection: the association moves with it.
    connect(&emulator, "move-abap", &expected_arn("move-hdb-b", "SYSTEMDB")).await;
    settle().await;
    assert_associated(&emulator, case, "move-abap", &["move-hdb-b"]).await;
    assert_associated(&emulator, case, "move-hdb-b", &["move-abap"]).await;
    assert_associated(&emulator, case, "move-hdb-a", &[]).await;
}

/// RULE: an association is a relation between two REGISTRATIONS, so it ends when either of
/// them does — a deregistered application is associated with nothing and is nobody's
/// associated application.
#[tokio::test]
async fn the_association_ends_with_the_other_registration() {
    let emulator = Emulator::new();
    let case = "a deregistered partner";
    for registration in [
        Registration {
            id: "gone-hdb",
            app_type: "HANA",
            database_names: &["SYSTEMDB"],
            database_arn: None,
        },
        Registration {
            id: "left-abap",
            app_type: "SAP_ABAP",
            database_names: &[],
            database_arn: Some(expected_arn("gone-hdb", "SYSTEMDB")),
        },
    ] {
        register_in(&emulator, REGION, &registration).await;
    }
    settle().await;
    assert_associated(&emulator, case, "left-abap", &["gone-hdb"]).await;

    let reply = emulator
        .send("/deregister-application", json!({"ApplicationId": "gone-hdb"}))
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from DeregisterApplication, got {reply:?}"
    );
    // The removal is asynchronous: until it falls due the application is still registered,
    // so it is still the application the other one shares its database with.
    assert_associated(&emulator, "the deregistration is outstanding", "left-abap", &["gone-hdb"])
        .await;

    settle().await;
    assert_associated(&emulator, case, "left-abap", &[]).await;
    let gone = get(&emulator, "gone-hdb").await;
    assert!(
        gone.status >= 400 && gone.member("Application").is_none(),
        "case {case:?}: a deregistered application must no longer be read, got {gone:?}"
    );
}

/// RULE: the relation holds between applications of ONE account and Region, which is the
/// partition a call reaches, so an application of another Region naming the same database
/// ARN is associated with nothing.
#[tokio::test]
async fn an_application_of_another_region_is_not_an_associated_application() {
    let emulator = Emulator::new();
    let other_region = "us-west-2";
    let shared = expected_arn("cross-hdb", "SYSTEMDB");
    register_in(
        &emulator,
        REGION,
        &Registration {
            id: "cross-hdb",
            app_type: "HANA",
            database_names: &["SYSTEMDB"],
            database_arn: None,
        },
    )
    .await;
    let elsewhere = register_in(
        &emulator,
        other_region,
        &Registration {
            id: "cross-abap",
            app_type: "SAP_ABAP",
            database_names: &[],
            database_arn: Some(shared.clone()),
        },
    )
    .await;
    assert_eq!(
        associated(&elsewhere),
        None,
        "an application of {other_region:?} must report no association with one of {REGION:?}, \
         got {elsewhere:?}"
    );

    assert_associated(
        &emulator,
        "an application of another Region",
        "cross-hdb",
        &[],
    )
    .await;
    let reply = emulator
        .send_in_region(
            other_region,
            "/get-application",
            json!({"ApplicationId": "cross-abap"}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from GetApplication in {other_region:?}, got {reply:?}"
    );
    assert_eq!(
        associated(&reply),
        None,
        "the application of {other_region:?} must report no associated application, got {reply:?}"
    );
    // The ARN the association would have carried is the other Region's, which is never
    // what this one reports.
    assert_ne!(
        application_arn_in(other_region, "cross-abap"),
        application_arn("cross-abap"),
        "an application ARN carries the Region it was registered in"
    );
}
