// SPDX-License-Identifier: CC0-1.0

//! The components of a registered application, and the `ComponentId`s its other
//! responses report for it.
//!
//! Every call goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state`: a component is only believed when a `GetComponent` /
//! `ListComponents` / `GetApplication` call reports it back out of what a
//! `RegisterApplication` persisted.
//!
//! The rule under test: a registration describes the components of its application — one
//! per `ComponentsInfo` entry, plus the SAP HANA component of the SAP system its own
//! `Sid` names, which is the component its SAP HANA databases belong to — and every
//! `ComponentId` any response reports for that application is one of them, so a client
//! that follows a `ComponentId` out of one response into `GetComponent` reads the
//! component that response named. A `ComponentId` nothing describes is still refused.
//!
//! The table spans what the rule generalises over: application type, whether the
//! registration named a System ID at all, how many components it declared (0, 1, 2 and
//! the 5 the model's `ComponentInfoList` admits), whether a declaration describes the SAP
//! HANA component of the application's own SAP system or of another, `ComponentType`s
//! taken from the model's own catalogue, and whether the databases were named by
//! credential, by `DatabaseArn` or not at all.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_COMPONENTS_PER_APPLICATION;

use crate::application_lifecycle::{settle, Emulator, Reply, REGION};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The single EC2 instance every registration here names (`InstanceList` is
/// `@length(min: 1, max: 1)`), which is the host the SAP HANA component of the
/// application's own SAP system runs on.
const APPLICATION_INSTANCE: &str = "i-0123456789abcdef0";

/// One registration, and the components and database references the API must then agree
/// on for it.
#[derive(Debug)]
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    sid: Option<&'static str>,
    sap_instance_number: Option<&'static str>,
    /// The `ComponentsInfo` entries, as (`ComponentType`, `Sid`, `Ec2InstanceId`).
    components_info: &'static [(&'static str, &'static str, &'static str)],
    /// The `DatabaseName` of each `ApplicationCredential` the registration carries.
    database_names: &'static [&'static str],
    /// The `DatabaseArn` the registration carries, when it names a database by ARN.
    database_arn: Option<&'static str>,
    /// The `ComponentId`s the application must have, in the order
    /// `Application$Components` must report them.
    expected_components: &'static [&'static str],
    /// The `ComponentId` every database of this application must report, or `None` when
    /// the application has no SAP HANA component for them to belong to.
    expected_database_component: Option<&'static str>,
    /// The `DatabaseId`s the application's databases must be reported under, in
    /// `DatabaseId` order.
    expected_databases: &'static [&'static str],
}

impl Case {
    /// The `RegisterApplication` body for this row.
    fn request(&self) -> Value {
        let mut body = Map::new();
        body.insert("ApplicationId".into(), json!(self.application_id));
        body.insert("ApplicationType".into(), json!(self.application_type));
        body.insert("Instances".into(), json!([APPLICATION_INSTANCE]));
        if let Some(sid) = self.sid {
            body.insert("Sid".into(), json!(sid));
        }
        if let Some(number) = self.sap_instance_number {
            body.insert("SapInstanceNumber".into(), json!(number));
        }
        if !self.components_info.is_empty() {
            body.insert(
                "ComponentsInfo".into(),
                Value::Array(
                    self.components_info
                        .iter()
                        .map(|(component_type, sid, instance)| {
                            json!({
                                "ComponentType": component_type,
                                "Sid": sid,
                                "Ec2InstanceId": instance,
                            })
                        })
                        .collect(),
                ),
            );
        }
        if !self.database_names.is_empty() {
            body.insert(
                "Credentials".into(),
                Value::Array(
                    self.database_names
                        .iter()
                        .map(|name| {
                            json!({
                                "DatabaseName": name,
                                "CredentialType": "ADMIN",
                                "SecretId": format!("sap/{name}/admin"),
                            })
                        })
                        .collect(),
                ),
            );
        }
        if let Some(arn) = self.database_arn {
            body.insert("DatabaseArn".into(), json!(arn));
        }
        Value::Object(body)
    }
}

/// The rows: one registration each.
const CASES: &[Case] = &[
    // The ordinary HANA registration: the members a HANA application requires and no
    // `ComponentsInfo` at all. Its databases belong to the SAP HANA component its System
    // ID names, so that component is one the application has.
    Case {
        name: "a HANA registration that declared no component",
        application_id: "hdb-single",
        application_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("00"),
        components_info: &[],
        database_names: &["HDB", "SYSTEMDB"],
        database_arn: None,
        expected_components: &["HDB-HANA"],
        expected_database_component: Some("HDB-HANA"),
        expected_databases: &["HDB", "SYSTEMDB"],
    },
    // The same component, DECLARED: one declaration of it and the System ID describing it
    // are one component, declared on the instance the declaration named.
    Case {
        name: "a HANA registration that also declared its own HANA component",
        application_id: "hdb-declared",
        application_type: "HANA",
        sid: Some("PRD"),
        sap_instance_number: Some("01"),
        components_info: &[("HANA", "PRD", "i-00000000000000001")],
        database_names: &["SYSTEMDB"],
        database_arn: None,
        expected_components: &["PRD-HANA"],
        expected_database_component: Some("PRD-HANA"),
        expected_databases: &["SYSTEMDB"],
    },
    // Declarations of OTHER types of the same SAP system, beside the SAP HANA component
    // nothing declared.
    Case {
        name: "a HANA registration declaring other component types of its own system",
        application_id: "hdb-nodes",
        application_type: "HANA",
        sid: Some("QAS"),
        sap_instance_number: Some("02"),
        components_info: &[
            ("HANA_NODE", "QAS", "i-00000000000000002"),
            ("ASCS", "QAS", "i-00000000000000003"),
        ],
        database_names: &["TNT1"],
        database_arn: None,
        expected_components: &["QAS-HANA_NODE", "QAS-ASCS", "QAS-HANA"],
        expected_database_component: Some("QAS-HANA"),
        expected_databases: &["TNT1"],
    },
    // A declared SAP HANA component of ANOTHER SAP system: it is a component of this
    // application, but the databases belong to the component of the application's OWN
    // system, which its System ID describes.
    Case {
        name: "a HANA component declared under another System ID",
        application_id: "hdb-foreign-component",
        application_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("03"),
        components_info: &[("HANA", "OTH", "i-00000000000000004")],
        database_names: &["HDB"],
        database_arn: None,
        expected_components: &["OTH-HANA", "HDB-HANA"],
        expected_database_component: Some("HDB-HANA"),
        expected_databases: &["HDB"],
    },
    // The other application type, with its database named by ARN alone rather than by a
    // credential.
    Case {
        name: "an ABAP registration naming a System ID and its database by ARN",
        application_id: "abap-by-arn",
        application_type: "SAP_ABAP",
        sid: Some("ABC"),
        sap_instance_number: Some("04"),
        components_info: &[],
        database_names: &[],
        database_arn: Some("arn:aws:ssm-sap:us-east-1:000000000000:database/ABC-HDB/SYSTEMDB"),
        expected_components: &["ABC-HANA"],
        expected_database_component: Some("ABC-HANA"),
        expected_databases: &["SYSTEMDB"],
    },
    // A registration that named NO System ID: there is nothing to identify a SAP HANA
    // component of its own system by, so it has only what it declared and its database
    // reports no component.
    Case {
        name: "an ABAP registration that named no System ID",
        application_id: "abap-no-sid",
        application_type: "SAP_ABAP",
        sid: None,
        sap_instance_number: None,
        components_info: &[("WD", "QAS", "i-00000000000000005")],
        database_names: &[],
        database_arn: Some("arn:aws:ssm-sap:us-east-1:000000000000:database/abap-no-sid/TNT9"),
        expected_components: &["QAS-WD"],
        expected_database_component: None,
        expected_databases: &["TNT9"],
    },
    // The largest `ComponentsInfo` the model admits (`ComponentInfoList` length 0..5),
    // beside the SAP HANA component of the application's own system: the most components
    // one registration can leave behind.
    Case {
        name: "the largest ComponentsInfo beside the HANA component of its own system",
        application_id: "abap-five-declared",
        application_type: "HANA",
        sid: Some("ZZ9"),
        sap_instance_number: Some("42"),
        components_info: &[
            ("ABAP", "ZZ9", "i-00000000000000006"),
            ("ASCS", "ZZ9", "i-00000000000000007"),
            ("ERS", "ZZ9", "i-00000000000000008"),
            ("DIALOG", "ZZ9", "i-00000000000000009"),
            ("WEBDISP", "ZZ9", "i-00000000000000010"),
        ],
        database_names: &["TNT7"],
        database_arn: None,
        expected_components: &[
            "ZZ9-ABAP",
            "ZZ9-ASCS",
            "ZZ9-ERS",
            "ZZ9-DIALOG",
            "ZZ9-WEBDISP",
            "ZZ9-HANA",
        ],
        expected_database_component: Some("ZZ9-HANA"),
        expected_databases: &["TNT7"],
    },
];

/// The ARN the component `component_id` of `application_id` carries in [`REGION`].
fn component_arn(application_id: &str, component_id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:component/{application_id}/{component_id}")
}

/// Register `case` and wait out the clock, so the application is up with its components.
async fn register_settled(emulator: &Emulator, case: &Case) {
    let reply = emulator.send("/register-application", case.request()).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
        case.name
    );
    settle().await;
}

/// The strings of a JSON array member, treating `null` as absent.
fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
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

/// `GetComponent` for the component `component_id` of `application_id`.
async fn get_component(emulator: &Emulator, application_id: &str, component_id: &str) -> Reply {
    emulator
        .send(
            "/get-component",
            json!({"ApplicationId": application_id, "ComponentId": component_id}),
        )
        .await
}

/// The `DatabaseSummary` list `ListDatabases` reports for `application_id`, as
/// (`DatabaseId`, `ComponentId`) pairs in the order reported.
async fn listed_databases(emulator: &Emulator, application_id: &str) -> Vec<(String, Option<String>)> {
    let reply = emulator
        .send("/list-databases", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "application {application_id:?}: expected 200 from ListDatabases, got {reply:?}"
    );
    reply
        .member("Databases")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        string_member(item, "DatabaseId").unwrap_or_default(),
                        string_member(item, "ComponentId"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: the components of an application are the ones its registration describes — each
/// `ComponentsInfo` entry, plus the SAP HANA component of the SAP system its own `Sid`
/// names — and `GetApplication`, `ListComponents` and `GetComponent` all report that one
/// set.
#[tokio::test]
async fn an_applications_components_are_the_ones_its_registration_describes() {
    for case in CASES {
        let emulator = Emulator::new();
        register_settled(&emulator, case).await;
        assert!(
            case.expected_components.len() <= MAX_COMPONENTS_PER_APPLICATION,
            "case {:?}: an application holds at most {MAX_COMPONENTS_PER_APPLICATION} \
             components, the row expects {}",
            case.name,
            case.expected_components.len()
        );

        let application = emulator
            .send(
                "/get-application",
                json!({"ApplicationId": case.application_id}),
            )
            .await;
        assert_eq!(
            application.status, 200,
            "case {:?}: expected 200 from GetApplication, got {application:?}",
            case.name
        );
        assert_eq!(
            string_list(application.app("Components")),
            case.expected_components
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the components the registration describes, got {:?}",
            case.name,
            application.app("Components")
        );

        // The listing reports the same set, and each of them resolves one by one.
        let listing = emulator
            .send(
                "/list-components",
                json!({"ApplicationId": case.application_id}),
            )
            .await;
        assert_eq!(
            listing.status, 200,
            "case {:?}: expected 200 from ListComponents, got {listing:?}",
            case.name
        );
        let listed: BTreeSet<String> = listing
            .member("Components")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| string_member(item, "ComponentId"))
                    .collect()
            })
            .unwrap_or_default();
        let expected: BTreeSet<String> = case
            .expected_components
            .iter()
            .map(|id| (*id).to_string())
            .collect();
        assert_eq!(
            listed, expected,
            "case {:?}: ListComponents must report the components GetApplication does, got \
             {listing:?}",
            case.name
        );

        for component_id in case.expected_components {
            let reply = get_component(&emulator, case.application_id, component_id).await;
            assert_eq!(
                reply.status, 200,
                "case {:?} at {component_id:?}: expected 200 from GetComponent, got {reply:?}",
                case.name
            );
            let reported = reply
                .member("Component")
                .and_then(|component| string_member(component, "ComponentId"));
            assert_eq!(
                reported.as_deref(),
                Some(*component_id),
                "case {:?} at {component_id:?}: expected that component back, got {reply:?}",
                case.name
            );
            let arn = reply
                .member("Component")
                .and_then(|component| string_member(component, "Arn"));
            assert_eq!(
                arn,
                Some(component_arn(case.application_id, component_id)),
                "case {:?} at {component_id:?}: expected the component's sub-resource ARN, got \
                 {reply:?}",
                case.name
            );
        }
    }
}

/// RULE: every `ComponentId` a response reports for an application names a component that
/// application has, so following it into `GetComponent` reads the component the response
/// named — and the databases that component reports are the databases that name it.
#[tokio::test]
async fn every_component_id_a_response_reports_resolves_through_get_component() {
    for case in CASES {
        let emulator = Emulator::new();
        register_settled(&emulator, case).await;

        let databases = listed_databases(&emulator, case.application_id).await;
        assert_eq!(
            databases
                .iter()
                .map(|(database_id, _)| database_id.clone())
                .collect::<Vec<String>>(),
            case.expected_databases
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {:?}: expected the databases the registration named, got {databases:?}",
            case.name
        );

        // Every `ComponentId` the responses of this application carry: the component ids
        // of the application itself, and the one each of its databases reports in a
        // listing and read one by one.
        let mut referenced: BTreeSet<String> = string_list(
            emulator
                .send(
                    "/get-application",
                    json!({"ApplicationId": case.application_id}),
                )
                .await
                .app("Components"),
        )
        .into_iter()
        .collect();
        for (database_id, component_id) in &databases {
            assert_eq!(
                component_id.as_deref(),
                case.expected_database_component,
                "case {:?} at database {database_id:?}: expected ComponentId {:?}, got \
                 {component_id:?}",
                case.name,
                case.expected_database_component
            );
            if let Some(component_id) = component_id {
                referenced.insert(component_id.clone());
            }

            let read = emulator
                .send(
                    "/get-database",
                    json!({
                        "ApplicationId": case.application_id,
                        "DatabaseId": database_id,
                    }),
                )
                .await;
            assert_eq!(
                read.status, 200,
                "case {:?} at database {database_id:?}: expected 200 from GetDatabase, got \
                 {read:?}",
                case.name
            );
            let database = read.member("Database").cloned().unwrap_or(Value::Null);
            let from_read = string_member(&database, "ComponentId");
            assert_eq!(
                from_read, *component_id,
                "case {:?} at database {database_id:?}: the listing and the read must report one \
                 ComponentId, got {component_id:?} and {from_read:?}",
                case.name
            );
            if let Some(component_id) = &from_read {
                referenced.insert(component_id.clone());
                assert_eq!(
                    string_list(database.get("ConnectedComponentArns")),
                    vec![component_arn(case.application_id, component_id)],
                    "case {:?} at database {database_id:?}: expected the ARN of the component it \
                     belongs to, got {read:?}",
                    case.name
                );
            }
        }

        for component_id in &referenced {
            let reply = get_component(&emulator, case.application_id, component_id).await;
            assert_eq!(
                reply.status, 200,
                "case {:?}: a response named component {component_id:?}, so GetComponent must \
                 report it, got {reply:?}",
                case.name
            );
            // The databases the component reports are exactly the databases that name it.
            let reported = string_list(
                reply
                    .member("Component")
                    .and_then(|component| component.get("Databases")),
            );
            let naming_it: Vec<String> = databases
                .iter()
                .filter(|(_, component)| component.as_deref() == Some(component_id.as_str()))
                .map(|(database_id, _)| database_id.clone())
                .collect();
            assert_eq!(
                reported, naming_it,
                "case {:?} at {component_id:?}: expected the databases that name this component, \
                 got {reply:?}",
                case.name
            );
        }
    }
}

/// A `ComponentId` no registration describes, and the application it is asked of.
#[derive(Debug)]
struct Refusal {
    label: &'static str,
    application_id: &'static str,
    component_id: &'static str,
}

/// RULE: a `ComponentId` of the modelled form that no registration describes resolves to
/// nothing, so it is refused with the `ValidationException` the operation models rather
/// than answered with a component.
#[tokio::test]
async fn a_component_no_registration_describes_is_still_refused() {
    let emulator = Emulator::new();
    for case in CASES {
        register_settled(&emulator, case).await;
    }

    let refusals = [
        Refusal {
            label: "a HANA component of a System ID the registration never named",
            application_id: "hdb-single",
            component_id: "ZZZ-HANA",
        },
        Refusal {
            label: "a component type nothing declared for this application",
            application_id: "hdb-single",
            component_id: "HDB-ASCS",
        },
        Refusal {
            label: "the HANA component of an application that named no System ID",
            application_id: "abap-no-sid",
            component_id: "QAS-HANA",
        },
        Refusal {
            label: "another application's HANA component",
            application_id: "hdb-single",
            component_id: "PRD-HANA",
        },
    ];

    for refusal in &refusals {
        let reply = get_component(&emulator, refusal.application_id, refusal.component_id).await;
        assert_eq!(
            reply.status, 400,
            "case {:?} ({} / {}): GetComponent models no 404, so an unresolvable reference is a \
             400, got {reply:?}",
            refusal.label, refusal.application_id, refusal.component_id
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}",
            refusal.label
        );
        let stated = message(&reply);
        assert!(
            stated.contains(refusal.component_id) && stated.contains(refusal.application_id),
            "case {:?}: the refusal names what resolved to nothing, got {stated:?}",
            refusal.label
        );
        assert_eq!(
            reply.member("Component"),
            None,
            "case {:?}: a refused read reports no component, got {reply:?}",
            refusal.label
        );
    }
}
