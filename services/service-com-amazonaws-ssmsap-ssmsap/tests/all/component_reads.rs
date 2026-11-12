// SPDX-License-Identifier: CC0-1.0

//! `GetComponent` over the wire: the component of a registered application a request
//! names.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` directly: a component is only believed when a
//! `GetComponent` call reports it back out of what a `RegisterApplication` persisted.
//!
//! The rule under test: a component of an application is what that application's
//! registration described about it — its `ComponentType`, its `Sid` and the EC2 instance
//! it runs on, under the `ComponentId` those identify it by — reported with the
//! application it belongs to, the sub-resource ARN of that pair, the status the
//! application is in, and the SAP HANA databases of the application that belong to it;
//! a registration describes a component in each `ComponentsInfo` entry and, in its own
//! `Sid`, the SAP HANA component of its SAP system. A reference naming no such component
//! of this account and Region is refused, and nothing the registration did not state is
//! reported for it.
//!
//! The table spans what that rule generalises over: component count (0, 1, 2, and the 5
//! the model's `ComponentInfoList` admits), component type, whether the component's
//! System ID is the application's own, repeated declarations of one component,
//! application type, and how many databases the registration gave the component.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    COMPONENT_STATUS_VALUES, COMPONENT_TYPE_VALUES,
};

use crate::application_lifecycle::{settle, tag_pairs, Emulator, Reply, REGION};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// A second regional endpoint, so a component can be asked for at a Region its
/// application was not registered in.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `ComponentStatus` a component of an application that is up must report, and the
/// one every row of the table below expects once the clock has settled the registration.
/// One of the model's `ComponentStatus` values
/// (`smithy_query(query="shape", shape="ComponentStatus")`).
const RUNNING: &str = "RUNNING";

/// The members `Component` carries only from discovery on the instance, or from a
/// highly available topology nothing registers: none of them may be reported for a
/// component this emulator was merely told about. Source:
/// `smithy_query(query="shape", shape="Component")`.
const UNDISCOVERED_MEMBERS: &[&str] = &[
    "SapHostname",
    "SapFeature",
    "SapKernelVersion",
    "HdbVersion",
    "Resilience",
    "ParentComponent",
    "ChildComponents",
    "DatabaseConnection",
    // Deprecated in the model in favour of `AssociatedHost`.
    "Hosts",
    "PrimaryHost",
];

/// One component a registration described, and what `GetComponent` must report for it.
#[derive(Debug)]
struct Expected {
    component_id: &'static str,
    component_type: &'static str,
    sid: &'static str,
    /// The EC2 instance the component runs on: the one the `ComponentsInfo` entry that
    /// declared it named, or — for the SAP HANA component of the application's own SAP
    /// system — the instance the registration named for the application.
    ec2_instance_id: &'static str,
    /// The `SystemNumber` the component must report, or `None` when it must report none.
    system_number: Option<&'static str>,
    /// The `DatabaseId` of every SAP HANA database of the application that belongs to
    /// this component, in the order the component must report them.
    databases: &'static [&'static str],
}

/// One registration and the components `GetComponent` must report for it.
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
    /// The `Tags` the registration carries, which are the APPLICATION's tags.
    tags: &'static [(&'static str, &'static str)],
    expected: &'static [Expected],
}

impl Case {
    fn request(&self) -> Value {
        let mut body = Map::new();
        body.insert("ApplicationId".into(), json!(self.application_id));
        body.insert("ApplicationType".into(), json!(self.application_type));
        body.insert("Instances".into(), json!(["i-0123456789abcdef0"]));
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
        if !self.tags.is_empty() {
            body.insert(
                "Tags".into(),
                Value::Object(
                    self.tags
                        .iter()
                        .map(|(key, value)| ((*key).to_string(), json!(value)))
                        .collect(),
                ),
            );
        }
        Value::Object(body)
    }
}

/// The rows: one registration each, spanning component count, type, System ID, repeated
/// declarations, application type and database count.
const CASES: &[Case] = &[
    // A registration that declared no component still describes one: the SAP HANA
    // component of the SAP system its own System ID names, which its databases belong to.
    Case {
        name: "no ComponentsInfo",
        application_id: "hdb-no-components",
        application_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("00"),
        components_info: &[],
        database_names: &["HDB"],
        tags: &[],
        expected: &[Expected {
            component_id: "HDB-HANA",
            component_type: "HANA",
            sid: "HDB",
            ec2_instance_id: "i-0123456789abcdef0",
            system_number: Some("00"),
            databases: &["HDB"],
        }],
    },
    // One HANA component, which is the component the registration's databases belong to.
    Case {
        name: "one HANA component owning two databases",
        application_id: "HDB-TEST",
        application_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("00"),
        components_info: &[("HANA", "HDB", "i-0123456789abcdef0")],
        database_names: &["HDB", "SYSTEMDB"],
        tags: &[("env", "dev")],
        expected: &[Expected {
            component_id: "HDB-HANA",
            component_type: "HANA",
            sid: "HDB",
            ec2_instance_id: "i-0123456789abcdef0",
            system_number: Some("00"),
            // In `DatabaseId` order, which is the order the databases are held in.
            databases: &["HDB", "SYSTEMDB"],
        }],
    },
    // Two components, one of them declared under a System ID that is not the
    // application's, beside the SAP HANA component of the application's own SAP system,
    // which its one database belongs to.
    Case {
        name: "a component declared under another System ID",
        application_id: "abap.prod_01",
        application_type: "HANA",
        sid: Some("PRD"),
        sap_instance_number: Some("01"),
        components_info: &[
            ("ASCS", "PRD", "i-00000000000000001"),
            ("WD", "QAS", "i-00000000000000002"),
        ],
        database_names: &["PRD"],
        tags: &[("owner", "sap-team"), ("tier", "1")],
        expected: &[
            Expected {
                component_id: "PRD-ASCS",
                component_type: "ASCS",
                sid: "PRD",
                ec2_instance_id: "i-00000000000000001",
                system_number: Some("01"),
                databases: &[],
            },
            Expected {
                component_id: "QAS-WD",
                component_type: "WD",
                sid: "QAS",
                ec2_instance_id: "i-00000000000000002",
                // The application's instance number is its own SAP system's, so a
                // component of another system reports none.
                system_number: None,
                databases: &[],
            },
            Expected {
                component_id: "PRD-HANA",
                component_type: "HANA",
                sid: "PRD",
                ec2_instance_id: "i-0123456789abcdef0",
                system_number: Some("01"),
                databases: &["PRD"],
            },
        ],
    },
    // Two declarations of one component are one component, and the first is what it was
    // declared as.
    Case {
        name: "a repeated declaration is one component",
        application_id: "hana_99",
        application_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("99"),
        components_info: &[
            ("HANA", "HDB", "i-00000000000000003"),
            ("HANA", "HDB", "i-00000000000000004"),
            ("HANA_NODE", "HDB", "i-00000000000000005"),
        ],
        database_names: &["HDB", "HDB"],
        tags: &[],
        expected: &[
            Expected {
                component_id: "HDB-HANA",
                component_type: "HANA",
                sid: "HDB",
                ec2_instance_id: "i-00000000000000003",
                system_number: Some("99"),
                databases: &["HDB"],
            },
            Expected {
                component_id: "HDB-HANA_NODE",
                component_type: "HANA_NODE",
                sid: "HDB",
                ec2_instance_id: "i-00000000000000005",
                system_number: Some("99"),
                databases: &[],
            },
        ],
    },
    // The largest `ComponentsInfo` the model allows
    // (`smithy_query(query="operation", operation="RegisterApplication")`: length 0..5),
    // on an application with no database at all — beside the SAP HANA component its own
    // System ID names, which no declaration describes.
    Case {
        name: "five components and no database",
        application_id: "abap-five-components",
        application_type: "SAP_ABAP",
        sid: Some("ABC"),
        sap_instance_number: Some("42"),
        components_info: &[
            ("ABAP", "ABC", "i-00000000000000006"),
            ("ASCS", "ABC", "i-00000000000000007"),
            ("ERS", "ABC", "i-00000000000000008"),
            ("WD", "ABC", "i-00000000000000009"),
            ("WEBDISP", "ABC", "i-00000000000000010"),
        ],
        database_names: &[],
        tags: &[],
        expected: &[
            Expected {
                component_id: "ABC-ABAP",
                component_type: "ABAP",
                sid: "ABC",
                ec2_instance_id: "i-00000000000000006",
                system_number: Some("42"),
                databases: &[],
            },
            Expected {
                component_id: "ABC-ASCS",
                component_type: "ASCS",
                sid: "ABC",
                ec2_instance_id: "i-00000000000000007",
                system_number: Some("42"),
                databases: &[],
            },
            Expected {
                component_id: "ABC-ERS",
                component_type: "ERS",
                sid: "ABC",
                ec2_instance_id: "i-00000000000000008",
                system_number: Some("42"),
                databases: &[],
            },
            Expected {
                component_id: "ABC-WD",
                component_type: "WD",
                sid: "ABC",
                ec2_instance_id: "i-00000000000000009",
                system_number: Some("42"),
                databases: &[],
            },
            Expected {
                component_id: "ABC-WEBDISP",
                component_type: "WEBDISP",
                sid: "ABC",
                ec2_instance_id: "i-00000000000000010",
                system_number: Some("42"),
                databases: &[],
            },
            Expected {
                component_id: "ABC-HANA",
                component_type: "HANA",
                sid: "ABC",
                ec2_instance_id: "i-0123456789abcdef0",
                system_number: Some("42"),
                databases: &[],
            },
        ],
    },
];

/// The ARN the component `component_id` of `application_id` must carry in [`REGION`]:
/// a sub-resource ARN naming both ids, as a database's does.
fn expected_arn(application_id: &str, component_id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:component/{application_id}/{component_id}")
}

/// A member of the response body's `Component`, treating JSON `null` as absent the way
/// an omitted optional member is.
fn component(reply: &Reply, name: &str) -> Option<Value> {
    reply
        .member("Component")
        .and_then(|value| value.get(name))
        .filter(|value| !value.is_null())
        .cloned()
}

/// A string member of the response body's `Component`.
fn component_str(reply: &Reply, name: &str) -> String {
    component(reply, name)
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The `Databases` the reported component carries, as strings.
fn reported_databases(reply: &Reply) -> Vec<String> {
    component(reply, "Databases")
        .and_then(|value| value.as_array().cloned())
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Register `case` and wait out the clock, so the application is up and its components
/// with it.
async fn register_settled(emulator: &Emulator, case: &Case) {
    let reply = emulator.send("/register-application", case.request()).await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
        case.name
    );
    settle().await;
}

async fn get_component(emulator: &Emulator, application_id: &str, component_id: &str) -> Reply {
    emulator
        .send(
            "/get-component",
            json!({"ApplicationId": application_id, "ComponentId": component_id}),
        )
        .await
}

/// RULE: `GetComponent` reports every component a registration described — each
/// `ComponentsInfo` entry and the SAP HANA component of the application's own SAP system
/// — out of what that registration persisted.
#[tokio::test]
async fn get_component_reports_every_component_a_registration_declared() {
    for case in CASES {
        let emulator = Emulator::new();
        register_settled(&emulator, case).await;

        for expected in case.expected {
            let reply = get_component(&emulator, case.application_id, expected.component_id).await;
            assert_eq!(
                reply.status, 200,
                "case {:?} at {:?}: expected 200 from GetComponent, got {reply:?}",
                case.name, expected.component_id
            );
            assert_eq!(
                component_str(&reply, "ComponentId"),
                expected.component_id,
                "case {:?} at {expected:?}: expected the described ComponentId, got {reply:?}",
                case.name
            );
            assert_eq!(
                component_str(&reply, "ApplicationId"),
                case.application_id,
                "case {:?} at {expected:?}: the component belongs to the application asked for, got {reply:?}",
                case.name
            );
            assert_eq!(
                component_str(&reply, "ComponentType"),
                expected.component_type,
                "case {:?} at {expected:?}: expected the declared ComponentType, got {reply:?}",
                case.name
            );
            assert_eq!(
                component_str(&reply, "Sid"),
                expected.sid,
                "case {:?} at {expected:?}: expected the declared Sid, got {reply:?}",
                case.name
            );
            assert_eq!(
                component(&reply, "SystemNumber").and_then(|value| value
                    .as_str()
                    .map(str::to_string)),
                expected.system_number.map(str::to_string),
                "case {:?} at {expected:?}: expected the SAP system number of the component's own system, got {reply:?}",
                case.name
            );
            assert_eq!(
                component_str(&reply, "Arn"),
                expected_arn(case.application_id, expected.component_id),
                "case {:?} at {expected:?}: expected the component's sub-resource ARN, got {reply:?}",
                case.name
            );
            assert_eq!(
                component_str(&reply, "Status"),
                RUNNING,
                "case {:?} at {expected:?}: a settled application's components are up, got {reply:?}",
                case.name
            );
            assert_eq!(
                component(&reply, "AssociatedHost")
                    .and_then(|host| host.get("Ec2InstanceId").cloned())
                    .and_then(|value| value.as_str().map(str::to_string)),
                Some(expected.ec2_instance_id.to_string()),
                "case {:?} at {expected:?}: expected the declared EC2 instance as the associated host, got {reply:?}",
                case.name
            );
            assert_eq!(
                reported_databases(&reply),
                expected
                    .databases
                    .iter()
                    .map(|id| (*id).to_string())
                    .collect::<Vec<String>>(),
                "case {:?} at {expected:?}: expected the databases the registration gave this component, got {reply:?}",
                case.name
            );
            assert!(
                component(&reply, "LastUpdated").is_some_and(|value| value.is_number()),
                "case {:?} at {expected:?}: LastUpdated is an epoch timestamp, got {reply:?}",
                case.name
            );
            for member in UNDISCOVERED_MEMBERS {
                assert_eq!(
                    component(&reply, member),
                    None,
                    "case {:?} at {expected:?}: nothing is reported in {member}, got {reply:?}",
                    case.name
                );
            }
            // The tags of the component, which nothing has tagged: the application's own
            // tags are not reported in their place.
            assert_eq!(
                tag_pairs(reply.member("Tags")),
                BTreeMap::new(),
                "case {:?} at {expected:?}: a component nobody tagged carries no tags, got {reply:?}",
                case.name
            );

            // A read reports, so a second one reports exactly the same component.
            let again = get_component(&emulator, case.application_id, expected.component_id).await;
            assert_eq!(
                again.body, reply.body,
                "case {:?} at {expected:?}: two reads of an unchanged component report the same thing, got {again:?}",
                case.name
            );
        }

        // The application's own tags ARE reported for the application, so the empty tag
        // set above is the component's and not a tag store this registration never wrote.
        let application = emulator
            .send(
                "/get-application",
                json!({"ApplicationId": case.application_id}),
            )
            .await;
        assert_eq!(
            tag_pairs(application.member("Tags")),
            case.tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
            "case {:?}: expected the registered tags on the application, got {application:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn every_modelled_component_type_is_readable_as_a_component() {
    // The catalogue is the model's own `ComponentType` enum
    // (`smithy_query(query="shape", shape="ComponentType")`), so a type added to the
    // model is covered here without this test changing.
    for component_type in COMPONENT_TYPE_VALUES {
        let emulator = Emulator::new();
        let case = Case {
            name: "one component of each modelled type",
            application_id: "abap-one-of-each",
            application_type: "SAP_ABAP",
            sid: Some("ABC"),
            sap_instance_number: Some("07"),
            components_info: &[],
            database_names: &[],
            tags: &[],
            expected: &[],
        };
        let mut body = case.request();
        body.as_object_mut()
            .expect("the body is a JSON object")
            .insert(
                "ComponentsInfo".into(),
                json!([{
                    "ComponentType": component_type,
                    "Sid": "ABC",
                    "Ec2InstanceId": "i-0123456789abcdef0",
                }]),
            );
        let registered = emulator.send("/register-application", body).await;
        assert_eq!(
            registered.status, 200,
            "case {component_type:?}: expected 200 from RegisterApplication, got {registered:?}"
        );
        // The declared component is the application's once the discovery the
        // registration runs has produced it.
        settle().await;

        let id = format!("ABC-{component_type}");
        let reply = get_component(&emulator, case.application_id, &id).await;
        assert_eq!(
            reply.status, 200,
            "case {component_type:?}: expected 200 from GetComponent, got {reply:?}"
        );
        assert_eq!(
            component_str(&reply, "ComponentId"),
            id,
            "case {component_type:?}: expected ComponentId {id}, got {reply:?}"
        );
        assert_eq!(
            component_str(&reply, "ComponentType"),
            *component_type,
            "case {component_type:?}: expected the declared ComponentType, got {reply:?}"
        );
    }
}

#[tokio::test]
async fn a_components_status_is_the_status_its_application_is_in() {
    let emulator = Emulator::new();
    let case = &CASES[1];
    let id = case.expected[0].component_id;

    let registered = emulator.send("/register-application", case.request()).await;
    assert_eq!(
        registered.status, 200,
        "expected 200 from RegisterApplication, got {registered:?}"
    );

    // Discovery runs as part of the registration, so while the registration is still
    // under way the application has no component at all: the reference is refused rather
    // than answered with a component whose status nothing established.
    let before = get_component(&emulator, case.application_id, id).await;
    assert_eq!(
        before.status, 400,
        "step \"the registration is still running\": a component discovery has not \
         produced resolves to nothing, got {before:?}"
    );

    // Each step: what happens to the application, and the `ComponentStatus` its
    // components are in afterwards. A stop or start still under way has not established
    // what the component is running, which is what the transient statuses state.
    let steps: &[(&str, Step, &str)] = &[
        ("the registration has completed", Step::Settle, RUNNING),
        ("a stop is still running", Step::Call("/stop-application"), "STOPPING"),
        ("the stop has completed", Step::Settle, "STOPPED"),
        ("a start is still running", Step::Call("/start-application"), "STARTING"),
        ("the start has completed", Step::Settle, RUNNING),
    ];

    for (label, step, expected) in steps {
        match step {
            Step::Settle => settle().await,
            Step::Call(path) => {
                let reply = emulator
                    .send(path, json!({"ApplicationId": case.application_id}))
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "step {label:?}: expected 200 from {path}, got {reply:?}"
                );
            }
        }

        let reply = get_component(&emulator, case.application_id, id).await;
        assert_eq!(
            reply.status, 200,
            "step {label:?}: expected 200 from GetComponent, got {reply:?}"
        );
        let status = component_str(&reply, "Status");
        assert!(
            COMPONENT_STATUS_VALUES.contains(&status.as_str()),
            "step {label:?}: {status:?} is not a modelled ComponentStatus"
        );
        assert_eq!(
            status, *expected,
            "step {label:?}: expected ComponentStatus {expected}, got {reply:?}"
        );
    }
}

/// What happens to the application between two readings of its component's status.
#[derive(Debug)]
enum Step {
    /// Wait out the clock, so the change it owed the application has fallen due.
    Settle,
    /// Call the operation at this path on the application.
    Call(&'static str),
}

/// One reference that names no component of the Region it is asked at.
#[derive(Debug)]
struct Refusal {
    label: &'static str,
    application_id: &'static str,
    component_id: &'static str,
    /// The regional endpoint the call is signed for.
    region: &'static str,
}

#[tokio::test]
async fn a_reference_naming_no_component_is_refused() {
    let emulator = Emulator::new();
    // Two applications, so a component of one is a reference the other does not answer.
    for case in [&CASES[1], &CASES[2]] {
        register_settled(&emulator, case).await;
    }

    let refusals = [
        Refusal {
            label: "an application nothing registered",
            application_id: "no-such-application",
            component_id: "HDB-HANA",
            region: REGION,
        },
        Refusal {
            label: "a well-formed component id the registration never declared",
            application_id: "HDB-TEST",
            component_id: "ZZZ-HANA",
            region: REGION,
        },
        Refusal {
            label: "a component type this application did not declare",
            application_id: "HDB-TEST",
            component_id: "HDB-HANA_NODE",
            region: REGION,
        },
        Refusal {
            label: "another application's component",
            application_id: "HDB-TEST",
            component_id: "PRD-ASCS",
            region: REGION,
        },
        Refusal {
            label: "a component of this application at another regional endpoint",
            application_id: "HDB-TEST",
            component_id: "HDB-HANA",
            region: OTHER_REGION,
        },
    ];

    for refusal in &refusals {
        let case = format!(
            "{} ({} / {} at {})",
            refusal.label, refusal.application_id, refusal.component_id, refusal.region
        );
        let reply = emulator
            .send_in_region(
                refusal.region,
                "/get-component",
                json!({
                    "ApplicationId": refusal.application_id,
                    "ComponentId": refusal.component_id,
                }),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: GetComponent models no 404, so an unresolvable reference is a 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {case:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        let message = message(&reply);
        assert!(
            message.contains(refusal.component_id) || message.contains(refusal.application_id),
            "case {case:?}: the refusal names what resolved to nothing, got {message:?}"
        );
        assert_eq!(
            reply.member("Component"),
            None,
            "case {case:?}: a refused read reports no component, got {reply:?}"
        );
    }
}

#[tokio::test]
async fn a_component_is_gone_when_its_application_is_deregistered() {
    let emulator = Emulator::new();
    let case = &CASES[1];
    let id = case.expected[0].component_id;
    register_settled(&emulator, case).await;

    let deregistered = emulator
        .send(
            "/deregister-application",
            json!({"ApplicationId": case.application_id}),
        )
        .await;
    assert_eq!(
        deregistered.status, 200,
        "expected 200 from DeregisterApplication, got {deregistered:?}"
    );

    // While the deregistration runs the application still exists, so its components do
    // too — and nothing has established what they are running.
    let during = get_component(&emulator, case.application_id, id).await;
    assert_eq!(
        during.status, 200,
        "a deregistration under way leaves the component readable, got {during:?}"
    );
    assert_eq!(
        component_str(&during, "Status"),
        "UNDEFINED",
        "a component of an application being deregistered has no established status, got {during:?}"
    );

    settle().await;
    let after = get_component(&emulator, case.application_id, id).await;
    assert_eq!(
        after.status, 400,
        "the component of a deregistered application resolves to nothing, got {after:?}"
    );
    assert_eq!(
        after.code(),
        VALIDATION_EXCEPTION,
        "expected {VALIDATION_EXCEPTION}, got {after:?}"
    );
}
