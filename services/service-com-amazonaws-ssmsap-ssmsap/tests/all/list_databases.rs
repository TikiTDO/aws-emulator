// SPDX-License-Identifier: CC0-1.0

//! `ListDatabases` over the wire: the SAP HANA databases of the registered applications
//! a request reaches.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` directly: a database is only believed when a
//! `ListDatabases` call reports it back out of what a `RegisterApplication` persisted.
//!
//! The rule under test: an application's SAP HANA databases are the ones its
//! registration named — one per distinct `ApplicationCredential$DatabaseName`, whose
//! `DatabaseId` is derived from that name, whose `DatabaseType` is `SYSTEM` for the HANA
//! system database and `TENANT` for every other, and whose component is the
//! application's HANA component — reported in (application id, database id) order,
//! narrowed by `ApplicationId`/`ComponentId`, paged by `MaxResults`/`NextToken`, and
//! scoped to the Region the request was signed for.
//!
//! The table spans what that rule generalises over: database count, name form (plain,
//! dotted, spaced, non-ASCII, mixed case, repeated, at the modelled length bound),
//! application type, and whether the registration named a System ID at all.

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::DATABASE_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_PAGE_SIZE;

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The region the suite signs for; it is the region every expected ARN carries.
const REGION: &str = "us-east-1";

/// A second regional endpoint, so a listing can be asked for a Region the application
/// was not registered in.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The modelled error code and status an absent resource is answered with
/// (`smithy_query(query="operation", operation="ListDatabases")`:
/// `ResourceNotFoundException` `@httpError 404`).
const RESOURCE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The `DatabaseName` AWS documents as naming the SAP HANA SYSTEM database, written out
/// here so the row that pins `DatabaseType` does not read it off the implementation.
const SYSTEM_DATABASE_NAME: &str = "SYSTEMDB";

/// One registration and the databases `ListDatabases` must report for it.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// The `Sid` the registration carries, or `None` for a registration that names none.
    sid: Option<&'static str>,
    /// The `DatabaseName` of each `ApplicationCredential` the registration carries.
    database_names: &'static [&'static str],
    /// The `ComponentId` every reported database must carry, or `None` when the listing
    /// must report none.
    expected_component: Option<&'static str>,
    /// The `(DatabaseId, DatabaseType)` of every database the listing must report, in
    /// the order it must report them.
    expected: &'static [(&'static str, &'static str)],
}

/// A `DatabaseName` at the top of the modelled `DatabaseName` length bound
/// (`smithy_query(query="shape", shape="ApplicationCredential")`: length 1..100).
///
/// `pub(crate)` so the suites of this target spell the bound once:
/// `tests/all/database_name_case.rs` drives the case rule on a name of that length.
pub(crate) const NAME_AT_LENGTH_BOUND: &str =
    "D000000001000000000200000000030000000004000000000500000000060000000007000000000800000000090000000";

/// The rows: one registration each, spanning database count, name form, application
/// type and whether a System ID was named.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "one HANA database",
            application_id: "hdb-single",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["HDB"],
            expected_component: Some("HDB-HANA"),
            expected: &[("HDB", "TENANT")],
        },
        Case {
            name: "the system database and two tenants, in database id order",
            application_id: "hdb-multi",
            application_type: "HANA",
            sid: Some("PRD"),
            database_names: &["TNT2", SYSTEM_DATABASE_NAME, "HDB"],
            expected_component: Some("PRD-HANA"),
            expected: &[
                ("HDB", "TENANT"),
                ("SYSTEMDB", "SYSTEM"),
                ("TNT2", "TENANT"),
            ],
        },
        Case {
            name: "the system database named in lower case",
            application_id: "hdb-lowercase-system",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["systemdb"],
            expected_component: Some("HDB-HANA"),
            expected: &[("SYSTEMDB", "SYSTEM")],
        },
        Case {
            name: "two credentials naming one database are one database",
            application_id: "hdb-repeated-credential",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["HDB", "HDB"],
            expected_component: Some("HDB-HANA"),
            expected: &[("HDB", "TENANT")],
        },
        Case {
            name: "a name carrying characters the DatabaseId pattern does not admit",
            application_id: "hdb-punctuated-name",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["prod db.1"],
            expected_component: Some("HDB-HANA"),
            expected: &[("PROD_DB_1", "TENANT")],
        },
        Case {
            name: "a name carrying characters outside ASCII",
            application_id: "hdb-non-ascii-name",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["datenbankü"],
            expected_component: Some("HDB-HANA"),
            expected: &[("DATENBANK_", "TENANT")],
        },
        Case {
            name: "a name at the top of the modelled length bound",
            application_id: "hdb-long-name",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &[NAME_AT_LENGTH_BOUND],
            expected_component: Some("HDB-HANA"),
            expected: &[(NAME_AT_LENGTH_BOUND, "TENANT")],
        },
        Case {
            name: "a HANA application whose registration named the system database",
            application_id: "hdb-with-system-database",
            application_type: "HANA",
            sid: Some("ABC"),
            database_names: &[SYSTEM_DATABASE_NAME],
            expected_component: Some("ABC-HANA"),
            expected: &[("SYSTEMDB", "SYSTEM")],
        },
        // The other modelled type runs on the SAP HANA database of such an application
        // and names it by ARN, so its registration carries no credentials at all
        // (`service_com_amazonaws_ssmsap_ssmsap::state::CONDITIONALLY_REQUIRED_MEMBERS`):
        // it therefore derives no database from credentials, which is this row.
        Case {
            name: "an ABAP application registered with no credentials has no databases",
            application_id: "abap-no-credentials",
            application_type: "SAP_ABAP",
            sid: None,
            database_names: &[],
            expected_component: None,
            expected: &[],
        },
    ]
}

/// One `DatabaseSummary` of a listing, reduced to what the assertions read.
///
/// `pub(crate)` because the suites of this target read ONE reduction of a listing:
/// `tests/all/registered_database_arn.rs` pins the databases a registration named by
/// ARN out of the same `ListDatabases` reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Summary {
    pub(crate) application_id: Option<String>,
    pub(crate) component_id: Option<String>,
    pub(crate) database_id: Option<String>,
    pub(crate) database_type: Option<String>,
    pub(crate) arn: Option<String>,
    /// Whether the summary carried a `Tags` member at all.
    pub(crate) tagged: bool,
}

impl Summary {
    pub(crate) fn id(&self) -> &str {
        self.database_id.as_deref().unwrap_or_default()
    }
}

/// The `DatabaseSummary` list a reply reported, in the order it reported them.
pub(crate) fn listed(reply: &Reply) -> Vec<Summary> {
    reply
        .member("Databases")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| Summary {
                    application_id: string_member(item, "ApplicationId"),
                    component_id: string_member(item, "ComponentId"),
                    database_id: string_member(item, "DatabaseId"),
                    database_type: string_member(item, "DatabaseType"),
                    arn: string_member(item, "Arn"),
                    tagged: item.get("Tags").is_some_and(|tags| !tags.is_null()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A string member of a JSON object, treating `null` as absent.
fn string_member(value: &Value, name: &str) -> Option<String> {
    value
        .get(name)
        .filter(|member| !member.is_null())
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The `NextToken` a listing handed out, absent when it reported the last page.
pub(crate) fn next_token(reply: &Reply) -> Option<String> {
    reply
        .member("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The ARN the database `database_id` of `application_id` must carry in [`REGION`] when
/// it is a sub-resource of that application.
///
/// `pub(crate)` so the suites of this target read ONE spelling of a database ARN:
/// `tests/all/registered_database_arn.rs` builds the `DatabaseArn` a registration names
/// with it and expects it back.
pub(crate) fn expected_arn(application_id: &str, database_id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:database/{application_id}/{database_id}")
}

/// The `Credentials` list of a registration naming each of `database_names`.
///
/// `CredentialType` is the model's single `CredentialType` value and `SecretId` is a
/// Secrets Manager id; only the `DatabaseName` varies between rows.
pub(crate) fn credentials(database_names: &[&str]) -> Value {
    Value::Array(
        database_names
            .iter()
            .enumerate()
            .map(|(index, name)| {
                json!({
                    "DatabaseName": name,
                    "CredentialType": "ADMIN",
                    "SecretId": format!("sap/hana/admin/{index}"),
                })
            })
            .collect(),
    )
}

/// The `RegisterApplication` body for `case`.
fn registration(case: &Case) -> Value {
    let mut body: Map<String, Value> = valid_registration(case.application_id, case.application_type)
        .as_object()
        .cloned()
        .expect("a registration body is a JSON object");
    match case.sid {
        Some(sid) => {
            body.insert("Sid".into(), json!(sid));
        }
        None => {
            body.remove("Sid");
        }
    }
    body.insert("Credentials".into(), credentials(case.database_names));
    Value::Object(body)
}

/// Register `body`, asserting the registration itself was accepted, and wait out the
/// clock so the discovery that registration runs has produced the application's
/// components — the components its databases are reported as connected to.
async fn register(emulator: &Emulator, name: &str, body: Value) {
    let reply = emulator.send("/register-application", body).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    settle().await;
}

/// `ListDatabases` in [`REGION`].
async fn list(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("/list-databases", body).await
}

/// Follow every page of a listing to exhaustion, at `max_results` per page.
///
/// Returns every database reported, in the order the pages reported them. The loop is
/// bounded, so a listing that failed to terminate fails the assertion rather than
/// hanging.
async fn all_pages(emulator: &Emulator, body: &Value, max_results: Option<i32>) -> Vec<Summary> {
    let mut reported: Vec<Summary> = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..64 {
        let mut request = body.as_object().cloned().expect("a listing body is an object");
        if let Some(max_results) = max_results {
            request.insert("MaxResults".into(), json!(max_results));
        }
        if let Some(token) = token.as_deref() {
            request.insert("NextToken".into(), json!(token));
        }
        let reply = list(emulator, Value::Object(request)).await;
        assert_eq!(
            reply.status, 200,
            "page at {token:?} of {max_results:?}: expected 200 from ListDatabases, got {reply:?}"
        );
        let page = listed(&reply);
        if let Some(limit) = max_results {
            assert!(
                page.len() <= limit as usize,
                "page at {token:?}: expected at most {limit} databases, got {page:?}"
            );
        }
        reported.extend(page);
        token = next_token(&reply);
        if token.is_none() {
            return reported;
        }
    }
    panic!("the listing of {body} at {max_results:?} per page never reported a last page");
}

#[tokio::test]
async fn databases_are_the_sap_hana_databases_the_registration_named() {
    let emulator = Emulator::new();
    for case in cases() {
        register(&emulator, case.name, registration(&case)).await;

        let reply = list(&emulator, json!({"ApplicationId": case.application_id})).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from ListDatabases, got {reply:?}",
            case.name
        );
        let reported = listed(&reply);
        let expected: Vec<Summary> = case
            .expected
            .iter()
            .map(|(database_id, database_type)| Summary {
                application_id: Some(case.application_id.to_string()),
                component_id: case.expected_component.map(str::to_string),
                database_id: Some((*database_id).to_string()),
                database_type: Some((*database_type).to_string()),
                arn: Some(expected_arn(case.application_id, database_id)),
                tagged: false,
            })
            .collect();
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?}",
            case.name
        );
        assert!(
            next_token(&reply).is_none(),
            "case {:?}: a listing that reported every database must carry no NextToken, got {:?}",
            case.name,
            next_token(&reply)
        );
        for database in &reported {
            // The modelled `DatabaseId` pattern (`[\w\d]+$`) and `DatabaseType` enum,
            // pinned on every row however the name was spelled.
            assert!(
                !database.id().is_empty()
                    && database
                        .id()
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_'),
                "case {:?}: DatabaseId {:?} is not of the modelled form",
                case.name,
                database.id()
            );
            let database_type = database.database_type.clone().unwrap_or_default();
            assert!(
                DATABASE_TYPE_VALUES.contains(&database_type.as_str()),
                "case {:?}: DatabaseType {database_type:?} is not one of {DATABASE_TYPE_VALUES:?}",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_component_narrows_the_listing_to_its_own_databases() {
    let emulator = Emulator::new();
    for case in cases() {
        register(&emulator, case.name, registration(&case)).await;
    }

    for case in cases() {
        let body = json!({"ApplicationId": case.application_id});
        let every = listed(&list(&emulator, body.clone()).await);

        if let Some(component_id) = case.expected_component {
            let reply = list(
                &emulator,
                json!({"ApplicationId": case.application_id, "ComponentId": component_id}),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: expected 200 from ListDatabases, got {reply:?}",
                case.name
            );
            assert_eq!(
                listed(&reply),
                every,
                "case {:?}: the application's own component must report every database of {every:?}",
                case.name
            );
        }

        // A component this application does not have: the WD component form a
        // registration's `ComponentsInfo` declares, and a well-formed id nothing uses.
        for component_id in ["HDB-WD", "ZZZ-HANA"] {
            if case.expected_component == Some(component_id) {
                continue;
            }
            let reply = list(
                &emulator,
                json!({"ApplicationId": case.application_id, "ComponentId": component_id}),
            )
            .await;
            assert_eq!(
                reply.status, 200,
                "case {:?} at {component_id:?}: expected 200 from ListDatabases, got {reply:?}",
                case.name
            );
            assert_eq!(
                listed(&reply),
                Vec::new(),
                "case {:?}: component {component_id:?} has no database of this application",
                case.name
            );
        }
    }
}

#[tokio::test]
async fn a_listing_names_every_application_of_the_region_when_the_request_names_none() {
    let emulator = Emulator::new();
    // Two application ids that differ in a character lower than a letter, so the order
    // of the listing is the order of (application id, database id) rather than of some
    // concatenation of the two.
    for (application_id, names) in [("app", &["SYSTEMDB", "HDB"][..]), ("app.2", &["TNT1"][..])] {
        register(
            &emulator,
            application_id,
            registration(&Case {
                name: application_id,
                application_id,
                application_type: "HANA",
                sid: Some("HDB"),
                database_names: names,
                expected_component: Some("HDB-HANA"),
                expected: &[],
            }),
        )
        .await;
    }

    let expected: Vec<(&str, &str)> = vec![
        ("app", "HDB"),
        ("app", "SYSTEMDB"),
        ("app.2", "TNT1"),
    ];

    // Every page size from one database per page to more than the listing holds, plus
    // the request that names none at all: each must report the same databases, once
    // each, in the same order.
    for max_results in [Some(1), Some(2), Some(3), Some(MAX_PAGE_SIZE), None] {
        let reported = all_pages(&emulator, &json!({}), max_results).await;
        let pairs: Vec<(String, String)> = reported
            .iter()
            .map(|database| {
                (
                    database.application_id.clone().unwrap_or_default(),
                    database.id().to_string(),
                )
            })
            .collect();
        let expected_pairs: Vec<(String, String)> = expected
            .iter()
            .map(|(application_id, database_id)| {
                ((*application_id).to_string(), (*database_id).to_string())
            })
            .collect();
        assert_eq!(
            pairs, expected_pairs,
            "case {max_results:?} per page: expected {expected_pairs:?}, got {pairs:?}"
        );
    }
}

#[tokio::test]
async fn a_page_token_resumes_at_the_database_the_previous_page_stopped_at() {
    let emulator = Emulator::new();
    let names: Vec<String> = (0..7).map(|index| format!("TNT{index}")).collect();
    let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();
    let body = json!({"ApplicationId": "hdb-paged"});
    register(
        &emulator,
        "hdb-paged",
        json!({
            "ApplicationId": "hdb-paged",
            "ApplicationType": "HANA",
            "Instances": ["i-0123456789abcdef0"],
            "Sid": "HDB",
            "SapInstanceNumber": "00",
            "Credentials": credentials(&borrowed),
        }),
    )
    .await;

    let whole = all_pages(&emulator, &body, None).await;
    assert_eq!(
        whole.len(),
        names.len(),
        "case \"one page\": expected {} databases, got {whole:?}",
        names.len()
    );

    for max_results in [1, 2, 3, 6, 7, MAX_PAGE_SIZE] {
        let paged = all_pages(&emulator, &body, Some(max_results)).await;
        assert_eq!(
            paged, whole,
            "case {max_results:?} per page: expected {whole:?}, got {paged:?}"
        );
    }

    // The first page of two carries a token; following it reports the rest and no more.
    let first = list(&emulator, json!({"ApplicationId": "hdb-paged", "MaxResults": 2})).await;
    let token = next_token(&first).unwrap_or_else(|| {
        panic!("case \"a page with databases left over\": expected a NextToken, got {first:?}")
    });
    let second = list(
        &emulator,
        json!({"ApplicationId": "hdb-paged", "MaxResults": MAX_PAGE_SIZE, "NextToken": token}),
    )
    .await;
    let resumed = listed(&second);
    assert_eq!(
        resumed,
        whole[2..].to_vec(),
        "case \"resumed page\": expected {:?}, got {resumed:?}",
        &whole[2..]
    );
    assert!(
        next_token(&second).is_none(),
        "case \"resumed page\": expected no NextToken, got {:?}",
        next_token(&second)
    );
}

#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    register(
        &emulator,
        "hdb-token",
        registration(&Case {
            name: "hdb-token",
            application_id: "hdb-token",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["HDB"],
            expected_component: Some("HDB-HANA"),
            expected: &[],
        }),
    )
    .await;

    // Each satisfies the modelled `NextToken` pattern (`^.{16,2048}$`), so the refusal
    // under test is the handler's rather than the boundary validator's.
    for token in [
        "0123456789abcdef",
        "not-a-token-this-service-issued",
        "6e6f742d612d746f6b656e2d6174",
    ] {
        let reply = list(
            &emulator,
            json!({"ApplicationId": "hdb-token", "NextToken": token}),
        )
        .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected 400 from ListDatabases, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {token:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        let message = message(&reply);
        assert!(
            message.contains(token),
            "case {token:?}: the refusal must name the token it refused, got {message:?}"
        );
    }
}

#[tokio::test]
async fn an_application_not_registered_here_is_not_found() {
    let emulator = Emulator::new();
    register(
        &emulator,
        "hdb-present",
        registration(&Case {
            name: "hdb-present",
            application_id: "hdb-present",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["HDB"],
            expected_component: Some("HDB-HANA"),
            expected: &[],
        }),
    )
    .await;

    // A reference nothing registered, one registered in ANOTHER Region, and one that was
    // registered here and deregistered: the same condition, however it arose.
    let never_registered = list(&emulator, json!({"ApplicationId": "hdb-absent"})).await;
    let other_region = emulator
        .send_in_region(
            OTHER_REGION,
            "/list-databases",
            json!({"ApplicationId": "hdb-present"}),
        )
        .await;
    let deregistered = {
        emulator
            .send(
                "/deregister-application",
                json!({"ApplicationId": "hdb-present"}),
            )
            .await;
        // Deregistration is asynchronous: the application (and so its databases) stops
        // resolving when the removal the clock owes it falls due.
        settle().await;
        list(&emulator, json!({"ApplicationId": "hdb-present"})).await
    };

    for (case, reply, reference) in [
        ("never registered", &never_registered, "hdb-absent"),
        ("registered in another Region", &other_region, "hdb-present"),
        ("deregistered", &deregistered, "hdb-present"),
    ] {
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404 from ListDatabases, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            RESOURCE_NOT_FOUND_EXCEPTION,
            "case {case:?}: expected {RESOURCE_NOT_FOUND_EXCEPTION}, got {reply:?}"
        );
        let message = message(reply);
        assert!(
            message.contains(reference),
            "case {case:?}: the refusal must name the application it could not find, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_listing_reports_only_the_signed_region_and_changes_nothing() {
    let emulator = Emulator::new();
    register(
        &emulator,
        "hdb-regional",
        registration(&Case {
            name: "hdb-regional",
            application_id: "hdb-regional",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["SYSTEMDB", "HDB"],
            expected_component: Some("HDB-HANA"),
            expected: &[],
        }),
    )
    .await;

    let here = list(&emulator, json!({})).await;
    let reported = listed(&here);
    assert_eq!(
        reported.len(),
        2,
        "case \"the signed Region\": expected both databases, got {reported:?}"
    );
    for database in &reported {
        let arn = database.arn.clone().unwrap_or_default();
        assert!(
            arn == expected_arn("hdb-regional", database.id()),
            "case \"the signed Region\": expected {:?}, got {arn:?}",
            expected_arn("hdb-regional", database.id())
        );
    }

    let elsewhere = emulator
        .send_in_region(OTHER_REGION, "/list-databases", json!({}))
        .await;
    assert_eq!(
        elsewhere.status, 200,
        "case \"another Region\": expected 200 from ListDatabases, got {elsewhere:?}"
    );
    assert_eq!(
        listed(&elsewhere),
        Vec::new(),
        "case \"another Region\": a Region nothing was registered in holds no database"
    );

    // A read changes nothing a later caller sees: the second listing reports what the
    // first did, and so does the one after the other Region was asked.
    let again = list(&emulator, json!({})).await;
    assert_eq!(
        listed(&again),
        reported,
        "case \"repeated listing\": expected {reported:?}, got {:?}",
        listed(&again)
    );
}

#[tokio::test]
async fn the_tags_of_an_application_are_not_the_tags_of_its_databases() {
    let emulator = Emulator::new();
    let mut body = registration(&Case {
        name: "hdb-tagged",
        application_id: "hdb-tagged",
        application_type: "HANA",
        sid: Some("HDB"),
        database_names: &["HDB"],
        expected_component: Some("HDB-HANA"),
        expected: &[],
    });
    body.as_object_mut()
        .expect("a registration body is a JSON object")
        .insert("Tags".into(), json!({"env": "dev"}));
    register(&emulator, "hdb-tagged", body).await;

    let application = emulator
        .send("/get-application", json!({"ApplicationId": "hdb-tagged"}))
        .await;
    assert_eq!(
        application
            .member("Tags")
            .and_then(|tags| tags.get("env"))
            .and_then(Value::as_str),
        Some("dev"),
        "case \"a tagged application\": expected the registration's tag, got {application:?}"
    );

    let reported = listed(&list(&emulator, json!({"ApplicationId": "hdb-tagged"})).await);
    assert_eq!(
        reported.len(),
        1,
        "case \"a tagged application\": expected its one database, got {reported:?}"
    );
    assert!(
        !reported[0].tagged,
        "case \"a tagged application\": nothing tagged the database, got {:?}",
        reported[0]
    );
}
