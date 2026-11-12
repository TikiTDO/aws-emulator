// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the registered-application slice, driven over the wire.
//!
//! Every case here goes through the real router: an `http::Request` is dispatched to
//! the mounted service, so the restJson1 routing, the boundary constraint validation,
//! the handler and the response serialization all run exactly as they do in the
//! emulator binary. Nothing reaches into `crate::state` directly — a register is only
//! believed when a later `GetApplication` / `ListApplications` / `GetOperation` call
//! reports it back.
//!
//! The tables span the classes the rules generalise over (application type, the
//! optional members a registration carries, component count, page size, application
//! count, filter name and operator, stop switches), and the issue's
//! `HDB-TEST`/`HANA`/`env=dev` case is one row.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    APPLICATION_TYPE_VALUES, COMPONENT_TYPE_VALUES, FILTER_OPERATOR_VALUES,
};
use service_com_amazonaws_ssmsap_ssmsap::state::{
    conditionally_required_members, ConditionalMember, FILTER_NAMES, MAX_PAGE_SIZE,
    TRANSITION_SPAN,
};
use service_com_amazonaws_ssmsap_ssmsap::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The region the test signs for; it is the region every expected ARN carries.
///
/// `pub(crate)` because a suite that reaches the store of the endpoint it drives
/// (`tests/all/sub_check_results.rs`) has to name the SAME region this harness signs
/// for.
pub(crate) const REGION: &str = "us-east-1";

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// Comfortably longer than [`TRANSITION_SPAN`], so a status change the clock owes has
/// come due by the time the next call is made. The emulator never sleeps; this is the
/// test waiting for the clock, which is the only thing that can make the change due.
fn settle_wait() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(300)
}

/// One response, reduced to what the assertions read.
///
/// `pub(crate)` because the suites of this target share one harness: the
/// validation-message suites drive the same wire through the same
/// [`Emulator::send`].
#[derive(Debug)]
pub(crate) struct Reply {
    pub(crate) status: u16,
    /// The wire error code: restJson1 resolves an error shape from
    /// `x-amzn-errortype`, falling back to the body's `__type`.
    error_code: Option<String>,
    /// The `x-amzn-errortype` response header ALONE, with no fallback to the body —
    /// what a client that reads the header sees.
    pub(crate) error_type_header: Option<String>,
    pub(crate) body: Value,
}

impl Reply {
    pub(crate) fn code(&self) -> &str {
        self.error_code.as_deref().unwrap_or("")
    }

    /// A member of the response body, treating JSON `null` as absent the way an
    /// omitted optional member is.
    pub(crate) fn member(&self, name: &str) -> Option<&Value> {
        self.body.get(name).filter(|value| !value.is_null())
    }

    /// A member of the response body's `Application`.
    ///
    /// `pub(crate)` so every suite of this target reads an `Application` member the same
    /// way.
    pub(crate) fn app(&self, name: &str) -> Option<&Value> {
        self.member("Application")
            .and_then(|application| application.get(name))
            .filter(|value| !value.is_null())
    }

    /// A member of the response body's `Operation`.
    pub(crate) fn op(&self, name: &str) -> Option<&Value> {
        self.member("Operation")
            .and_then(|operation| operation.get(name))
            .filter(|value| !value.is_null())
    }

    pub(crate) fn app_str(&self, name: &str) -> &str {
        self.app(name).and_then(Value::as_str).unwrap_or("")
    }

    pub(crate) fn op_str(&self, name: &str) -> &str {
        self.op(name).and_then(Value::as_str).unwrap_or("")
    }
}

/// The emulator under test: one engine, one mounted service, one router.
///
/// `pub(crate)` so every suite of this test target drives ONE harness.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    /// The SAME engine the mounted service was built from, so a suite can read or place
    /// a record in the store the handlers reach. See [`Emulator::with_state`].
    engine: std::sync::Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine.clone()));
        Self { router, engine }
    }

    /// The SAME harness with EC2 mounted beside this service on the ONE engine, and the
    /// in-process callback that EC2 is driven over — the very seam a handler of this
    /// service reaches another service through.
    ///
    /// `pub(crate)` for the suite that pins what stopping an application does to its
    /// Amazon EC2 instances (`tests/all/ec2_instance_shutdown.rs`): the rule it drives
    /// spans both services, so it needs the owner of the instances on this engine — and
    /// the same dispatch path every other suite uses, which is why the second service is
    /// a constructor here rather than a harness of its own there.
    pub(crate) fn with_ec2() -> (Self, EmbeddedEmulator) {
        let engine: std::sync::Arc<engine::InMemoryEngine> =
            std::sync::Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine.clone()));
        router.register_service(
            service_com_amazonaws_ec2_amazonec2::EmulatedService::new(engine.clone()),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        (Self { router, engine }, callback)
    }

    /// Edit the store the mounted service reads.
    ///
    /// Every suite drives the wire alone wherever an API of this service can produce the
    /// record under test; this is for the one record no API produces
    /// (`tests/all/sub_check_results.rs` says which, and why).
    pub(crate) fn with_state(
        &self,
        edit: impl FnOnce(&mut service_com_amazonaws_ssmsap_ssmsap::state::ComAmazonawsSsmsapSsmsapState),
    ) {
        let state = service_com_amazonaws_ssmsap_ssmsap::state::service_state(&*self.engine)
            .expect("the mounted service's own store");
        let mut state = state.lock();
        edit(&mut state);
    }

    /// Dispatch to this service's [`REGION`] endpoint — what every suite that cares
    /// about one region's behaviour uses.
    pub(crate) async fn send(&self, path: &str, body: Value) -> Reply {
        self.send_in_region(REGION, path, body).await
    }

    /// Dispatch signed for `region`, which is the regional endpoint the call reaches:
    /// the region-scoping suite drives the SAME harness at more than one of them.
    pub(crate) async fn send_in_region(&self, region: &str, path: &str, body: Value) -> Reply {
        self.send_request("POST", region, path, Some(body)).await
    }

    /// Dispatch with an explicit HTTP method and an OPTIONAL body, signed for `region`.
    ///
    /// Every operation of this service is `POST` with a JSON body except the tagging
    /// reads and writes, which the model binds to `/tags/{resourceArn}` by method
    /// (`GET` lists, `POST` tags, `DELETE` untags) — so a suite that drives one of those
    /// addresses it here. `body: None` sends no bytes at all, which is what a client
    /// sends for an operation whose every member is bound to the URI.
    pub(crate) async fn send_request(
        &self,
        method: &str,
        region: &str,
        path: &str,
        body: Option<Value>,
    ) -> Reply {
        dispatch(&self.router, method, region, path, body).await
    }

    async fn register(&self, body: Value) -> Reply {
        self.send("/register-application", body).await
    }

    async fn get_application(&self, body: Value) -> Reply {
        self.send("/get-application", body).await
    }

    async fn list_applications(&self, body: Value) -> Reply {
        self.send("/list-applications", body).await
    }

    async fn start(&self, id: &str) -> Reply {
        self.send("/start-application", json!({"ApplicationId": id}))
            .await
    }

    async fn stop(&self, body: Value) -> Reply {
        self.send("/stop-application", body).await
    }

    async fn get_operation(&self, id: &str) -> Reply {
        self.send("/get-operation", json!({"OperationId": id})).await
    }

    async fn deregister(&self, id: &str) -> Reply {
        self.send(
            "/deregister-application",
            json!({"ApplicationId": id}),
        )
        .await
    }

    /// Register a minimal HANA application, without waiting for the clock.
    async fn register_minimal(&self, id: &str) -> Reply {
        let reply = self.register(valid_registration(id, "HANA")).await;
        assert_eq!(
            reply.status, 200,
            "case {id:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        reply
    }

    /// Register a minimal HANA application and return its `OperationId`, settled so the
    /// application is ACTIVATED and no change is outstanding.
    async fn register_settled(&self, id: &str) -> String {
        let reply = self.register_minimal(id).await;
        let operation_id = reply
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        settle().await;
        operation_id
    }
}

/// Dispatch ONE request to `router`, signed for `region`, and reduce the response to a
/// [`Reply`].
///
/// Separate from [`Emulator`] because a suite can need a router this harness does not
/// build: the suite that drives the channel a handler answers on when it cannot reach
/// its own store (`tests/all/tagging_error_channel.rs`) mounts the service on an engine
/// that holds none, and must reach it over the SAME wire every other suite uses.
/// `body: None` sends no bytes at all, which is what a client sends for an operation
/// whose every member is bound to the URI.
pub(crate) async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    region: &str,
    path: &str,
    body: Option<Value>,
) -> Reply {
    let payload = match &body {
        Some(value) => serde_json::to_vec(value).expect("the request body serializes"),
        None => Vec::new(),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://ssm-sap.{region}.amazonaws.com{path}"))
        // The SigV4 credential scope is what routes the request to this service and
        // what names the region its ARNs carry. Signatures are not verified unless
        // the process turns authentication on.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/ssm-sap/aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let body: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    let error_type_header = error_code.clone();
    let error_code = error_code.or_else(|| {
        body.get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    Reply {
        status,
        error_code,
        error_type_header,
        body,
    }
}

/// One reply as a value — its status and its whole body — so what a reporting call
/// answers before and after something else happened compares as a value.
///
/// `pub(crate)` so a suite pinning that a read changes nothing a later caller sees
/// renders its witnesses the one way, rather than each re-deriving the snapshot.
pub(crate) fn rendered(reply: &Reply) -> String {
    format!("HTTP {} {}", reply.status, reply.body)
}

/// Wait out a change the clock owes an application — the next status an asynchronous
/// call moves it to, or the removal a deregistration ends it with.
///
/// `pub(crate)` so every suite of this target waits out the clock the same way.
pub(crate) async fn settle() {
    tokio::time::sleep(settle_wait()).await;
}

/// The smallest `RegisterApplication` body the service ACCEPTS for `app_type`: the
/// modelled `@required` members, plus every member that type requires conditionally
/// (`service_com_amazonaws_ssmsap_ssmsap::state::CONDITIONALLY_REQUIRED_MEMBERS` — a
/// HANA application's SID, instance number and credentials).
///
/// `pub(crate)` because every suite of this target registers through one builder.
pub(crate) fn valid_registration(id: &str, app_type: &str) -> Value {
    let mut body = Map::new();
    body.insert("ApplicationId".into(), json!(id));
    body.insert("ApplicationType".into(), json!(app_type));
    body.insert("Instances".into(), json!(["i-0123456789abcdef0"]));
    for member in conditionally_required_members(app_type) {
        body.insert(member.member_name().into(), conditional_value(*member));
    }
    Value::Object(body)
}

/// A value that satisfies `member`'s modelled constraints, so a body built by
/// [`valid_registration`] carries the member rather than merely naming it.
pub(crate) fn conditional_value(member: ConditionalMember) -> Value {
    match member {
        // `SID` is modelled `^[A-Z][A-Z0-9]{2}$`.
        ConditionalMember::Sid => json!("HDB"),
        // `SAPInstanceNumber` is modelled `^[0-9]{2}$`.
        ConditionalMember::SapInstanceNumber => json!("00"),
        // `ApplicationCredential` requires DatabaseName, CredentialType (an enum whose
        // value is `ADMIN`) and SecretId.
        ConditionalMember::Credentials => json!([{
            "DatabaseName": "HDB",
            "CredentialType": "ADMIN",
            "SecretId": "sap/hdb/admin",
        }]),
    }
}

/// The ARN `RegisterApplication` must mint for `id`.
fn expected_arn(id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:application/{id}")
}

/// Whether `value` is the UUID form the model's `OperationId` pattern describes:
/// `^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$`.
///
/// `pub(crate)` so every suite of this target reads one definition of the id form:
/// `tests/all/configuration_checks.rs` pins it on the ids a configuration check
/// operation is minted with.
pub(crate) fn is_operation_id(value: &str) -> bool {
    let core = value.strip_prefix('{').unwrap_or(value);
    let core = core.strip_suffix('}').unwrap_or(core);
    let groups: Vec<&str> = core.split('-').collect();
    let widths = [8usize, 4, 4, 4, 12];
    groups.len() == widths.len()
        && groups
            .iter()
            .zip(widths)
            .all(|(group, width)| {
                group.len() == width && group.chars().all(|c| c.is_ascii_hexdigit())
            })
}

/// The `Tags` map of a reply member, as comparable pairs.
///
/// `pub(crate)` because the suites of this target share one harness:
/// `tests/all/resource_tags.rs` reads the SAME map out of `ListTagsForResource`'s own
/// `tags` member.
pub(crate) fn tag_pairs(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `Components` list of an `Application`, as strings.
///
/// `pub(crate)` because the suites of this target share one harness:
/// `tests/all/registration_derived_state.rs` reads the SAME list out of its own replies.
pub(crate) fn string_list(value: Option<&Value>) -> Vec<String> {
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

/// The `Id` of every `ApplicationSummary` a listing reported, in the order reported.
fn listed_ids(reply: &Reply) -> Vec<String> {
    reply
        .member("Applications")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    item.get("Id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// One registration, and what the API must report about it.
#[derive(Debug)]
struct RegisterCase {
    id: &'static str,
    app_type: &'static str,
    sid: Option<&'static str>,
    sap_instance_number: Option<&'static str>,
    /// `ComponentsInfo` entries as (ComponentType, Sid, Ec2InstanceId).
    components_info: &'static [(&'static str, &'static str, &'static str)],
    tags: &'static [(&'static str, &'static str)],
    database_arn: Option<&'static str>,
    /// `Credentials` entries as (DatabaseName, CredentialType, SecretId).
    credentials: &'static [(&'static str, &'static str, &'static str)],
}

impl RegisterCase {
    fn request(&self) -> Value {
        let mut body = Map::new();
        body.insert("ApplicationId".into(), json!(self.id));
        body.insert("ApplicationType".into(), json!(self.app_type));
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
        if let Some(arn) = self.database_arn {
            body.insert("DatabaseArn".into(), json!(arn));
        }
        if !self.credentials.is_empty() {
            body.insert(
                "Credentials".into(),
                Value::Array(
                    self.credentials
                        .iter()
                        .map(|(database, credential_type, secret)| {
                            json!({
                                "DatabaseName": database,
                                "CredentialType": credential_type,
                                "SecretId": secret,
                            })
                        })
                        .collect(),
                ),
            );
        }
        Value::Object(body)
    }

    /// The component ids the registration DESCRIBES, which are the application's
    /// components once its discovery has completed: one per `ComponentsInfo` entry (a
    /// component is identified by the SID and type it was declared with, so duplicates
    /// collapse), plus the SAP HANA component of the SAP system the registration's own
    /// `Sid` names, when no declaration already describes it.
    fn expected_components(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for (component_type, sid, _) in self.components_info {
            let id = format!("{sid}-{component_type}");
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        if let Some(sid) = self.sid {
            let id = format!("{sid}-HANA");
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// The registrations the round-trip table spans: both application types, every
/// combination of the optional members, 0/1/2/5 components (the model's
/// `ComponentInfoList` is 0..5), and id forms across the `^[\w\d\.-]+$` pattern.
const REGISTER_CASES: &[RegisterCase] = &[
    // The case the issue records, with the members a HANA registration requires.
    RegisterCase {
        id: "HDB-TEST",
        app_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("00"),
        components_info: &[],
        tags: &[("env", "dev")],
        database_arn: None,
        credentials: &[("HDB", "ADMIN", "sap/hdb/admin")],
    },
    // The other application type, no optional member at all.
    RegisterCase {
        id: "A",
        app_type: "SAP_ABAP",
        sid: None,
        sap_instance_number: None,
        components_info: &[],
        tags: &[],
        database_arn: None,
        credentials: &[],
    },
    // One component, several tags, a database ARN and credentials.
    RegisterCase {
        id: "abap.prod_01",
        app_type: "HANA",
        sid: Some("PRD"),
        sap_instance_number: Some("01"),
        components_info: &[("ASCS", "PRD", "i-00000000000000001")],
        tags: &[("env", "prod"), ("owner", "sap-team"), ("tier", "1")],
        database_arn: Some("arn:aws:rds:us-east-1:000000000000:db:sap-prd"),
        credentials: &[("PRD", "ADMIN", "sap/prd/admin")],
    },
    // Two components, one of them a duplicate declaration of the first.
    RegisterCase {
        id: "hana_99",
        app_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("99"),
        components_info: &[
            ("HANA", "HDB", "i-00000000000000002"),
            ("HANA", "HDB", "i-00000000000000003"),
            ("HANA_NODE", "HDB", "i-00000000000000004"),
        ],
        tags: &[],
        database_arn: None,
        credentials: &[("HDB", "ADMIN", "sap/hdb/admin"), ("SYSTEMDB", "ADMIN", "sap/hdb/system")],
    },
    // The largest `ComponentsInfo` the model allows, and a 60-character id (the
    // `ApplicationId` length bound).
    RegisterCase {
        id: concat!(
            "a",
            "0123456789",
            "0123456789",
            "0123456789",
            "0123456789",
            "0123456789",
            "012345678"
        ),
        app_type: "SAP_ABAP",
        sid: Some("ABC"),
        sap_instance_number: Some("42"),
        components_info: &[
            ("ABAP", "ABC", "i-00000000000000005"),
            ("ASCS", "ABC", "i-00000000000000006"),
            ("ERS", "ABC", "i-00000000000000007"),
            ("WD", "ABC", "i-00000000000000008"),
            ("WEBDISP", "ABC", "i-00000000000000009"),
        ],
        tags: &[("k", "v")],
        database_arn: None,
        credentials: &[],
    },
];

#[tokio::test]
async fn register_persists_the_application_and_every_reader_reports_it() {
    for case in REGISTER_CASES {
        let emulator = Emulator::new();
        let arn = expected_arn(case.id);

        let registered = emulator.register(case.request()).await;
        assert_eq!(
            registered.status, 200,
            "case {case:?}: expected 200 from RegisterApplication, got {registered:?}"
        );
        assert_eq!(
            registered.app_str("Id"),
            case.id,
            "case {case:?}: expected Application.Id, got {:?}",
            registered.app("Id")
        );
        assert_eq!(
            registered.app_str("Type"),
            case.app_type,
            "case {case:?}: expected Application.Type, got {:?}",
            registered.app("Type")
        );
        assert_eq!(
            registered.app_str("Arn"),
            arn,
            "case {case:?}: expected Application.Arn {arn}, got {:?}",
            registered.app("Arn")
        );
        assert_eq!(
            registered.app_str("Status"),
            "REGISTERING",
            "case {case:?}: a registration still settling is REGISTERING, got {:?}",
            registered.app("Status")
        );
        assert_eq!(
            registered.app_str("DiscoveryStatus"),
            "REGISTERING",
            "case {case:?}: discovery is REGISTERING while it runs, got {:?}",
            registered.app("DiscoveryStatus")
        );
        assert_eq!(
            string_list(registered.app("Components")),
            Vec::<String>::new(),
            "case {case:?}: discovery is still running, so the application has no \
             components yet, got {:?}",
            registered.app("Components")
        );
        assert!(
            registered
                .app("LastUpdated")
                .is_some_and(|value| value.is_number()),
            "case {case:?}: LastUpdated is an epoch timestamp, got {:?}",
            registered.app("LastUpdated")
        );
        let operation_id = registered
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_operation_id(&operation_id),
            "case {case:?}: expected a UUID OperationId, got {operation_id:?}"
        );

        // The id and the ARN are two references to ONE record, so both must report it.
        for reference in [
            json!({"ApplicationId": case.id}),
            json!({"ApplicationArn": arn}),
        ] {
            let fetched = emulator.get_application(reference.clone()).await;
            assert_eq!(
                fetched.status, 200,
                "case {case:?} via {reference}: expected 200 from GetApplication, got {fetched:?}"
            );
            assert_eq!(
                fetched.app("Id"),
                registered.app("Id"),
                "case {case:?} via {reference}: GetApplication must report the registered id, got {fetched:?}"
            );
            assert_eq!(
                fetched.app_str("Arn"),
                arn,
                "case {case:?} via {reference}: expected Arn {arn}, got {:?}",
                fetched.app("Arn")
            );
            assert_eq!(
                fetched.app_str("Type"),
                case.app_type,
                "case {case:?} via {reference}: expected Type {}, got {:?}",
                case.app_type,
                fetched.app("Type")
            );
            assert_eq!(
                string_list(fetched.app("Components")),
                Vec::<String>::new(),
                "case {case:?} via {reference}: the discovery that produces the \
                 components is still running, got {:?}",
                fetched.app("Components")
            );
            assert_eq!(
                tag_pairs(fetched.member("Tags")),
                case.expected_tags(),
                "case {case:?} via {reference}: expected the registered tags, got {:?}",
                fetched.member("Tags")
            );
        }

        // An AppRegistry ARN names an application registered through AWS Service
        // Catalog; none is, so the reference resolves to nothing rather than to this
        // application.
        let by_app_registry = emulator
            .get_application(json!({
                "AppRegistryArn": "arn:aws:servicecatalog:us-east-1:000000000000:/applications/abc"
            }))
            .await;
        assert_eq!(
            by_app_registry.status, 400,
            "case {case:?}: an unmatched AppRegistryArn is refused, got {by_app_registry:?}"
        );

        // The operation the registration filed is retrievable, and reports the resource
        // it acted on.
        let operation = emulator.get_operation(&operation_id).await;
        assert_eq!(
            operation.status, 200,
            "case {case:?}: expected 200 from GetOperation, got {operation:?}"
        );
        assert_eq!(
            operation.op_str("Id"),
            operation_id,
            "case {case:?}: expected the operation's own id, got {:?}",
            operation.op("Id")
        );
        assert_eq!(
            operation.op_str("Type"),
            "REGISTER_APPLICATION",
            "case {case:?}: expected the register operation type, got {:?}",
            operation.op("Type")
        );
        assert_eq!(
            operation.op_str("Status"),
            "INPROGRESS",
            "case {case:?}: an operation that has not come due is INPROGRESS, got {:?}",
            operation.op("Status")
        );
        assert_eq!(
            operation.op_str("ResourceId"),
            case.id,
            "case {case:?}: expected ResourceId, got {:?}",
            operation.op("ResourceId")
        );
        assert_eq!(
            operation.op_str("ResourceArn"),
            arn,
            "case {case:?}: expected ResourceArn {arn}, got {:?}",
            operation.op("ResourceArn")
        );
        assert!(
            operation.op("ResourceType").is_some(),
            "case {case:?}: ResourceType is populated, got {operation:?}"
        );
        assert!(
            operation.op("StartTime").is_some_and(Value::is_number),
            "case {case:?}: StartTime is an epoch timestamp, got {:?}",
            operation.op("StartTime")
        );
        assert!(
            operation.op("EndTime").is_none(),
            "case {case:?}: an unfinished operation has no EndTime, got {:?}",
            operation.op("EndTime")
        );

        // The clock owes the registration its ACTIVATED status; once it is due, every
        // reader reports it, and the operation it filed has succeeded.
        settle().await;
        let settled = emulator
            .get_application(json!({"ApplicationId": case.id}))
            .await;
        assert_eq!(
            settled.app_str("Status"),
            "ACTIVATED",
            "case {case:?}: a settled registration is ACTIVATED, got {:?}",
            settled.app("Status")
        );
        assert_eq!(
            settled.app_str("DiscoveryStatus"),
            "SUCCESS",
            "case {case:?}: a settled discovery is SUCCESS, got {:?}",
            settled.app("DiscoveryStatus")
        );
        // A succeeded discovery is what produces the application's components.
        assert_eq!(
            string_list(settled.app("Components")),
            case.expected_components(),
            "case {case:?}: expected the components discovery produced, got {:?}",
            settled.app("Components")
        );
        let again = emulator
            .get_application(json!({"ApplicationId": case.id}))
            .await;
        assert_eq!(
            again.app("LastUpdated"),
            settled.app("LastUpdated"),
            "case {case:?}: LastUpdated is stable across reads once nothing is owed, got {:?} then {:?}",
            settled.app("LastUpdated"),
            again.app("LastUpdated")
        );

        let succeeded = emulator.get_operation(&operation_id).await;
        assert_eq!(
            succeeded.op_str("Status"),
            "SUCCESS",
            "case {case:?}: a due operation has succeeded, got {:?}",
            succeeded.op("Status")
        );
        assert!(
            succeeded.op("EndTime").is_some_and(Value::is_number),
            "case {case:?}: a finished operation has an EndTime, got {:?}",
            succeeded.op("EndTime")
        );
    }
}

#[tokio::test]
async fn every_modelled_application_type_registers() {
    // The catalogue is the model's own `ApplicationType` enum, so a value added to the
    // model shows up here rather than being silently untested.
    for app_type in APPLICATION_TYPE_VALUES {
        let emulator = Emulator::new();
        let id = format!("app-{}", app_type.to_ascii_lowercase());
        let reply = emulator.register(valid_registration(&id, app_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {app_type:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        assert_eq!(
            reply.app_str("Type"),
            *app_type,
            "case {app_type:?}: expected the registered type back, got {:?}",
            reply.app("Type")
        );
    }
}

#[tokio::test]
async fn every_modelled_component_type_registers_as_a_component_id() {
    // The catalogue is the model's own `ComponentType` enum. `WEBDISP`, which the
    // issue never cites, is one row like any other.
    for component_type in COMPONENT_TYPE_VALUES {
        let emulator = Emulator::new();
        let reply = emulator
            .register(json!({
                "ApplicationId": "comp-app",
                "ApplicationType": "SAP_ABAP",
                "Instances": ["i-0123456789abcdef0"],
                "ComponentsInfo": [{
                    "ComponentType": component_type,
                    "Sid": "ABC",
                    "Ec2InstanceId": "i-0123456789abcdef0",
                }],
            }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {component_type:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
        // The declared component is the application's once the discovery the
        // registration runs has produced it.
        settle().await;
        let fetched = emulator
            .get_application(json!({"ApplicationId": "comp-app"}))
            .await;
        assert_eq!(
            string_list(fetched.app("Components")),
            vec![format!("ABC-{component_type}")],
            "case {component_type:?}: expected one component id, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn re_registering_an_application_id_conflicts() {
    for id in ["HDB-TEST", "a", "dup.app-1"] {
        let emulator = Emulator::new();
        emulator.register_settled(id).await;
        let again = emulator
            .register(json!({
                "ApplicationId": id,
                "ApplicationType": "SAP_ABAP",
                "Instances": ["i-0123456789abcdef1"],
            }))
            .await;
        assert_eq!(
            again.status, 409,
            "case {id:?}: re-registering conflicts, got {again:?}"
        );
        assert_eq!(
            again.code(),
            "ConflictException",
            "case {id:?}: expected ConflictException, got {again:?}"
        );

        // The conflict changed nothing: the first registration is still the one stored.
        let fetched = emulator.get_application(json!({"ApplicationId": id})).await;
        assert_eq!(
            fetched.app_str("Type"),
            "HANA",
            "case {id:?}: a refused re-register must not overwrite the stored type, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn a_malformed_registration_is_refused_and_stores_nothing() {
    // Each row breaks ONE modelled constraint: the id pattern, the id length bound, the
    // type enum, and the `Instances` 1..1 length bound (empty and two).
    let cases: &[(&str, Value)] = &[
        (
            "id pattern",
            json!({"ApplicationId": "bad id!", "ApplicationType": "HANA", "Instances": ["i-0"]}),
        ),
        (
            "id too long",
            json!({"ApplicationId": "x".repeat(61), "ApplicationType": "HANA", "Instances": ["i-0"]}),
        ),
        (
            "id empty",
            json!({"ApplicationId": "", "ApplicationType": "HANA", "Instances": ["i-0"]}),
        ),
        (
            "unknown type",
            json!({"ApplicationId": "ok", "ApplicationType": "ORACLE", "Instances": ["i-0"]}),
        ),
        (
            "no instances",
            json!({"ApplicationId": "ok", "ApplicationType": "HANA", "Instances": []}),
        ),
        (
            "two instances",
            json!({"ApplicationId": "ok", "ApplicationType": "HANA", "Instances": ["i-0", "i-1"]}),
        ),
        (
            "sid pattern",
            json!({"ApplicationId": "ok", "ApplicationType": "HANA", "Instances": ["i-0"], "Sid": "hdb"}),
        ),
        (
            "instance number pattern",
            json!({"ApplicationId": "ok", "ApplicationType": "HANA", "Instances": ["i-0"], "SapInstanceNumber": "0"}),
        ),
    ];

    for (case, body) in cases {
        let emulator = Emulator::new();
        let reply = emulator.register(body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a malformed registration is refused with 400, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {reply:?}"
        );

        // Nothing was stored, so the listing is still empty.
        let listed = emulator.list_applications(json!({})).await;
        assert!(
            listed_ids(&listed).is_empty(),
            "case {case:?}: a refused registration stores nothing, got {listed:?}"
        );
    }
}

/// RULE: a lookup naming no registered application is refused with the modelled
/// `ValidationException`, stating that THIS account holds no application under the
/// reference the request carried — `GetApplication` models no
/// `ResourceNotFoundException`, so an absent application is a 400.
#[tokio::test]
async fn an_unresolvable_application_reference_is_refused_rather_than_invented() {
    let emulator = Emulator::new();
    emulator.register_settled("HDB-TEST").await;

    // Rows span the three members a reference can arrive in, and the request that
    // carries none. `reference` is the one the refusal must name.
    let app_registry_arn = "arn:aws:servicecatalog:us-east-1:000000000000:/applications/nope";
    let cases: Vec<(&str, Value, Option<String>)> = vec![
        (
            "unknown id",
            json!({"ApplicationId": "NO-SUCH"}),
            Some("NO-SUCH".to_string()),
        ),
        (
            "an id no registration used, in the recorded probe's form",
            json!({"ApplicationId": "sim2real-probe-ssm-sap-app-8d37e33f"}),
            Some("sim2real-probe-ssm-sap-app-8d37e33f".to_string()),
        ),
        (
            "unknown arn",
            json!({"ApplicationArn": expected_arn("NO-SUCH")}),
            Some(expected_arn("NO-SUCH")),
        ),
        (
            "unknown app registry arn",
            json!({"AppRegistryArn": app_registry_arn}),
            Some(app_registry_arn.to_string()),
        ),
        ("no reference at all", json!({}), None),
    ];

    for (case, body, reference) in cases {
        let reply = emulator.get_application(body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected a refusal, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
        assert_eq!(
            reply.error_type_header.as_deref(),
            Some("ValidationException"),
            "case {case:?}: the refusal must carry x-amzn-errortype, got {:?}",
            reply.error_type_header
        );
        assert!(
            reply.member("Application").is_none(),
            "case {case:?}: no application may be fabricated, got {reply:?}"
        );
        let actual = reply
            .member("Message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        match reference {
            Some(reference) => {
                let expected = format!(
                    "No registered application found for account: {ACCOUNT}, and application: {reference}"
                );
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {expected:?}, got {actual:?}"
                );
            }
            None => assert!(
                // The exact wording of THIS refusal is pinned by
                // `tests/all/application_reference_refusals.rs`, which owns the rule for
                // the identifier set; here it is enough that a request naming nothing is
                // refused for naming nothing.
                actual.contains("need to be specified"),
                "case {case:?}: a request naming nothing is refused for naming nothing, got {actual:?}"
            ),
        }
    }
}

#[tokio::test]
async fn an_operation_id_that_names_no_record_is_refused() {
    let emulator = Emulator::new();
    // Well-formed ids (the model's `OperationId` pattern) that this emulator never
    // issued: all-zero, mixed case, and the braced form the pattern allows.
    for id in [
        "00000000-0000-0000-0000-000000000000",
        "DEADBEEF-dead-BEEF-dead-beefDEADbeef",
        "{11111111-2222-3333-4444-555555555555}",
    ] {
        let reply = emulator.get_operation(id).await;
        assert_eq!(
            reply.status, 400,
            "case {id:?}: expected a refusal, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {id:?}: expected ValidationException, got {reply:?}"
        );
        assert!(
            reply.member("Operation").is_none(),
            "case {id:?}: no operation may be synthesized, got {reply:?}"
        );
    }

    // And one that is not the modelled form at all is refused at the boundary.
    let malformed = emulator.get_operation("not-a-uuid").await;
    assert_eq!(
        malformed.status, 400,
        "case \"not-a-uuid\": expected a refusal, got {malformed:?}"
    );
}

/// One start/stop step: the call to make, and the status the application takes while
/// it runs and once it is due.
#[derive(Debug)]
struct LifecycleStep {
    stop: bool,
    /// `StopApplication`'s two optional switches, recorded as the operation's
    /// `Properties`.
    stop_connected_entity: Option<&'static str>,
    include_ec2_instance_shutdown: Option<bool>,
    transient: &'static str,
    settled: &'static str,
    op_type: &'static str,
}

#[tokio::test]
async fn start_and_stop_move_the_stored_status_and_file_a_retrievable_operation() {
    // The sequence spans both calls, both switch members of `StopApplication` (absent,
    // the `DBMS` connected entity, and both booleans), and a start of an application
    // that is already ACTIVATED.
    let steps: &[LifecycleStep] = &[
        LifecycleStep {
            stop: true,
            stop_connected_entity: None,
            include_ec2_instance_shutdown: None,
            transient: "STOPPING",
            settled: "STOPPED",
            op_type: "STOP_APPLICATION",
        },
        LifecycleStep {
            stop: false,
            stop_connected_entity: None,
            include_ec2_instance_shutdown: None,
            transient: "STARTING",
            settled: "ACTIVATED",
            op_type: "START_APPLICATION",
        },
        LifecycleStep {
            stop: true,
            stop_connected_entity: Some("DBMS"),
            include_ec2_instance_shutdown: Some(true),
            transient: "STOPPING",
            settled: "STOPPED",
            op_type: "STOP_APPLICATION",
        },
        LifecycleStep {
            stop: false,
            stop_connected_entity: None,
            include_ec2_instance_shutdown: None,
            transient: "STARTING",
            settled: "ACTIVATED",
            op_type: "START_APPLICATION",
        },
        LifecycleStep {
            stop: true,
            stop_connected_entity: None,
            include_ec2_instance_shutdown: Some(false),
            transient: "STOPPING",
            settled: "STOPPED",
            op_type: "STOP_APPLICATION",
        },
    ];

    let emulator = Emulator::new();
    let id = "HDB-TEST";
    emulator.register_settled(id).await;

    for step in steps {
        let reply = if step.stop {
            let mut body = Map::new();
            body.insert("ApplicationId".into(), json!(id));
            if let Some(entity) = step.stop_connected_entity {
                body.insert("StopConnectedEntity".into(), json!(entity));
            }
            if let Some(shutdown) = step.include_ec2_instance_shutdown {
                body.insert("IncludeEc2InstanceShutdown".into(), json!(shutdown));
            }
            emulator.stop(Value::Object(body)).await
        } else {
            emulator.start(id).await
        };
        assert_eq!(
            reply.status, 200,
            "case {step:?}: expected 200, got {reply:?}"
        );
        let operation_id = reply
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            is_operation_id(&operation_id),
            "case {step:?}: expected a UUID OperationId, got {operation_id:?}"
        );

        // The transient status is observable: the change is never due at once.
        let during = emulator.get_application(json!({"ApplicationId": id})).await;
        assert_eq!(
            during.app_str("Status"),
            step.transient,
            "case {step:?}: expected the transient status, got {:?}",
            during.app("Status")
        );

        // And while it is outstanding the application is busy, which is what the
        // modelled conflict reports.
        let busy = if step.stop {
            emulator.start(id).await
        } else {
            emulator.stop(json!({"ApplicationId": id})).await
        };
        assert_eq!(
            busy.status, 409,
            "case {step:?}: a busy application conflicts, got {busy:?}"
        );
        assert_eq!(
            busy.code(),
            "ConflictException",
            "case {step:?}: expected ConflictException, got {busy:?}"
        );

        let running = emulator.get_operation(&operation_id).await;
        assert_eq!(
            running.op_str("Type"),
            step.op_type,
            "case {step:?}: expected the operation type, got {:?}",
            running.op("Type")
        );
        assert_eq!(
            running.op_str("Status"),
            "INPROGRESS",
            "case {step:?}: an outstanding operation is INPROGRESS, got {:?}",
            running.op("Status")
        );
        assert_eq!(
            running.op_str("ResourceId"),
            id,
            "case {step:?}: expected ResourceId {id}, got {:?}",
            running.op("ResourceId")
        );
        assert_eq!(
            running.op_str("ResourceArn"),
            expected_arn(id),
            "case {step:?}: expected ResourceArn, got {:?}",
            running.op("ResourceArn")
        );

        // The call's own parameters are what the record reports as `Properties`.
        let mut expected_properties: BTreeMap<String, String> = BTreeMap::new();
        if let Some(entity) = step.stop_connected_entity {
            expected_properties.insert("StopConnectedEntity".into(), entity.to_string());
        }
        if let Some(shutdown) = step.include_ec2_instance_shutdown {
            expected_properties.insert("IncludeEc2InstanceShutdown".into(), shutdown.to_string());
        }
        assert_eq!(
            tag_pairs(running.op("Properties")),
            expected_properties,
            "case {step:?}: expected the call's parameters as Properties, got {:?}",
            running.op("Properties")
        );

        settle().await;
        let after = emulator.get_application(json!({"ApplicationId": id})).await;
        assert_eq!(
            after.app_str("Status"),
            step.settled,
            "case {step:?}: expected the settled status, got {:?}",
            after.app("Status")
        );
        assert_eq!(
            after.app_str("DiscoveryStatus"),
            "SUCCESS",
            "case {step:?}: starting or stopping does not re-run discovery, got {:?}",
            after.app("DiscoveryStatus")
        );
        let done = emulator.get_operation(&operation_id).await;
        assert_eq!(
            done.op_str("Status"),
            "SUCCESS",
            "case {step:?}: a due operation has succeeded, got {:?}",
            done.op("Status")
        );
        assert!(
            done.op("EndTime").is_some_and(Value::is_number),
            "case {step:?}: a finished operation has an EndTime, got {:?}",
            done.op("EndTime")
        );
    }
}

#[tokio::test]
async fn starting_or_stopping_an_unregistered_application_is_not_found() {
    let emulator = Emulator::new();
    emulator.register_settled("HDB-TEST").await;

    for id in ["NO-SUCH", "hdb-test", "other.app"] {
        for (case, reply) in [
            ("start", emulator.start(id).await),
            ("stop", emulator.stop(json!({"ApplicationId": id})).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case} {id:?}: expected 404, got {reply:?}"
            );
            assert_eq!(
                reply.code(),
                "ResourceNotFoundException",
                "case {case} {id:?}: expected ResourceNotFoundException, got {reply:?}"
            );
            assert!(
                reply.member("OperationId").is_none(),
                "case {case} {id:?}: no OperationId may be minted, got {reply:?}"
            );
        }
    }
}

#[tokio::test]
async fn listing_pages_over_the_stored_set_and_terminates() {
    // The invariant: following `NextToken` to exhaustion reports every stored
    // application exactly once, in id order, with no page larger than `MaxResults`.
    // The table is exhaustive over the class rather than sampled: every application
    // count 0..=6 against every page size 1..=7 and the absent `MaxResults`, which
    // covers a page size below, equal to and above the stored count.
    for count in 0usize..=6 {
        let emulator = Emulator::new();
        let ids: Vec<String> = (0..count).map(|index| format!("app-{index:02}")).collect();
        for id in &ids {
            emulator.register_minimal(id).await;
        }
        // One wait covers every registration: each one's change is due at its own
        // instant, and all of them have passed by now.
        settle().await;

        for page_size in [None, Some(1), Some(2), Some(3), Some(5), Some(7)] {
            let case = format!("{count} applications, MaxResults {page_size:?}");
            let mut seen: Vec<String> = Vec::new();
            let mut next_token: Option<String> = None;
            // One page per application plus the terminating one is the most any correct
            // paging needs; more means the token is not advancing.
            let mut pages_left = count + 2;

            loop {
                let mut body = Map::new();
                if let Some(size) = page_size {
                    body.insert("MaxResults".into(), json!(size));
                }
                if let Some(token) = &next_token {
                    body.insert("NextToken".into(), json!(token));
                }
                let reply = emulator.list_applications(Value::Object(body)).await;
                assert_eq!(
                    reply.status, 200,
                    "case {case}: expected 200 from ListApplications, got {reply:?}"
                );
                let page: Vec<String> = listed_ids(&reply);
                let limit = page_size.unwrap_or(MAX_PAGE_SIZE) as usize;
                assert!(
                    page.len() <= limit,
                    "case {case}: a page holds at most {limit} items, got {}",
                    page.len()
                );
                seen.extend(page);

                next_token = reply
                    .member("NextToken")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if let Some(token) = &next_token {
                    // The model's `NextToken` pattern is `^.{16,2048}$`.
                    assert!(
                        token.len() >= 16 && token.len() <= 2048,
                        "case {case}: a token must satisfy the modelled length, got {} chars",
                        token.len()
                    );
                }
                if next_token.is_none() {
                    break;
                }
                pages_left = pages_left.saturating_sub(1);
                assert!(pages_left > 0, "case {case}: paging did not terminate");
            }

            assert_eq!(
                seen, ids,
                "case {case}: expected every stored id once in id order, got {seen:?}"
            );
        }
    }
}

/// One expected listing row: the application's id, its type, and its tags.
type SummaryRow = (&'static str, &'static str, &'static [(&'static str, &'static str)]);

#[tokio::test]
async fn a_listing_reports_each_applications_own_summary() {
    let emulator = Emulator::new();
    let expected: &[SummaryRow] = &[
        ("HDB-TEST", "HANA", &[("env", "dev")]),
        ("abap-1", "SAP_ABAP", &[]),
        ("hana-2", "HANA", &[("a", "1"), ("b", "2")]),
    ];
    for (id, app_type, tags) in expected {
        let mut body = valid_registration(id, app_type);
        let body_members = body.as_object_mut().expect("the body is a JSON object");
        if !tags.is_empty() {
            body_members.insert(
                "Tags".into(),
                Value::Object(
                    tags.iter()
                        .map(|(key, value)| ((*key).to_string(), json!(value)))
                        .collect(),
                ),
            );
        }
        let reply = emulator.register(body).await;
        assert_eq!(
            reply.status, 200,
            "case {id:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
    }
    settle().await;

    let listed = emulator.list_applications(json!({})).await;
    let summaries = listed
        .member("Applications")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        summaries.len(),
        expected.len(),
        "expected one summary per registered application, got {listed:?}"
    );
    for (id, app_type, tags) in expected {
        let summary = summaries
            .iter()
            .find(|summary| summary.get("Id").and_then(Value::as_str) == Some(id))
            .unwrap_or_else(|| panic!("case {id:?}: no summary for the application, got {listed:?}"));
        assert_eq!(
            summary.get("Type").and_then(Value::as_str),
            Some(*app_type),
            "case {id:?}: expected the registered type, got {summary:?}"
        );
        assert_eq!(
            summary.get("Arn").and_then(Value::as_str),
            Some(expected_arn(id).as_str()),
            "case {id:?}: expected the stored ARN, got {summary:?}"
        );
        assert_eq!(
            summary.get("DiscoveryStatus").and_then(Value::as_str),
            Some("SUCCESS"),
            "case {id:?}: a settled application has a SUCCESS discovery, got {summary:?}"
        );
        assert_eq!(
            tag_pairs(summary.get("Tags")),
            tags.iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect::<BTreeMap<String, String>>(),
            "case {id:?}: expected the registered tags, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    emulator.register_settled("HDB-TEST").await;
    // Each is at least 16 characters, so the model's `NextToken` pattern accepts it and
    // the handler is what refuses it.
    for token in [
        "not-a-token-at-all",
        "6162636465666768696a6b6c6d6e6f70",
        "AAAAAAAAAAAAAAAAAAAA",
    ] {
        let reply = emulator
            .list_applications(json!({"NextToken": token}))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {token:?}: expected a refusal, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {reply:?}"
        );
    }
}

#[tokio::test]
async fn filters_narrow_the_listing_by_the_summarys_own_members() {
    let emulator = Emulator::new();
    for (id, app_type) in [
        ("alpha", "HANA"),
        ("bravo", "SAP_ABAP"),
        ("charlie", "HANA"),
    ] {
        let reply = emulator.register(valid_registration(id, app_type)).await;
        assert_eq!(
            reply.status, 200,
            "case {id:?}: expected 200 from RegisterApplication, got {reply:?}"
        );
    }
    settle().await;

    // Every filter name the listing accepts, each operator, and a comma-separated
    // `Value` (which `Filter$Value`'s documentation joins with OR).
    let cases: &[(Value, &[&str])] = &[
        (
            json!([{"Name": "Type", "Value": "HANA", "Operator": "Equals"}]),
            &["alpha", "charlie"],
        ),
        (
            json!([{"Name": "Type", "Value": "SAP_ABAP", "Operator": "Equals"}]),
            &["bravo"],
        ),
        (
            json!([{"Name": "Id", "Value": "bravo", "Operator": "Equals"}]),
            &["bravo"],
        ),
        (
            json!([{"Name": "Id", "Value": "alpha,charlie", "Operator": "Equals"}]),
            &["alpha", "charlie"],
        ),
        // A value whose only non-empty member sits past the 32nd piece: every piece a
        // 64-character `Value` can split into is compared, so nothing is dropped.
        (
            json!([{"Name": "Id", "Value": format!("{}charlie", ",".repeat(40)), "Operator": "Equals"}]),
            &["charlie"],
        ),
        // And the last piece of a value that fills the length bound exactly.
        (
            json!([{"Name": "Id", "Value": format!("{}bravo", ",".repeat(59)), "Operator": "Equals"}]),
            &["bravo"],
        ),
        (
            json!([{"Name": "Id", "Value": "bravo", "Operator": "GreaterThanOrEquals"}]),
            &["bravo", "charlie"],
        ),
        (
            json!([{"Name": "Id", "Value": "bravo", "Operator": "LessThanOrEquals"}]),
            &["alpha", "bravo"],
        ),
        (
            json!([{"Name": "DiscoveryStatus", "Value": "SUCCESS", "Operator": "Equals"}]),
            &["alpha", "bravo", "charlie"],
        ),
        (
            json!([{"Name": "DiscoveryStatus", "Value": "REGISTERING", "Operator": "Equals"}]),
            &[],
        ),
        (
            json!([{"Name": "Arn", "Value": expected_arn("charlie"), "Operator": "Equals"}]),
            &["charlie"],
        ),
        // Two filters narrow together.
        (
            json!([
                {"Name": "Type", "Value": "HANA", "Operator": "Equals"},
                {"Name": "Id", "Value": "charlie", "Operator": "Equals"},
            ]),
            &["charlie"],
        ),
        (
            json!([
                {"Name": "Type", "Value": "HANA", "Operator": "Equals"},
                {"Name": "Id", "Value": "bravo", "Operator": "Equals"},
            ]),
            &[],
        ),
    ];

    for (filters, expected) in cases {
        let reply = emulator
            .list_applications(json!({"Filters": filters}))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {filters}: expected 200 from ListApplications, got {reply:?}"
        );
        let mut ids: Vec<String> = listed_ids(&reply);
        ids.sort();
        assert_eq!(
            ids,
            expected
                .iter()
                .map(|id| (*id).to_string())
                .collect::<Vec<String>>(),
            "case {filters}: expected the matching applications, got {reply:?}"
        );
    }

    // A filter the listing cannot answer is refused rather than silently ignored.
    for name in ["Sid", "Status", "id", "Tags"] {
        let reply = emulator
            .list_applications(json!({
                "Filters": [{"Name": name, "Value": "x", "Operator": "Equals"}]
            }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: an unsupported filter name is refused, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {name:?}: expected ValidationException, got {reply:?}"
        );
    }
    // Every supported name is accepted, so the refusal above is about the name and not
    // about filtering at all.
    for name in FILTER_NAMES {
        for operator in FILTER_OPERATOR_VALUES {
            let reply = emulator
                .list_applications(json!({
                    "Filters": [{"Name": name, "Value": "alpha", "Operator": operator}]
                }))
                .await;
            assert_eq!(
                reply.status, 200,
                "case {name:?}/{operator:?}: a modelled filter is accepted, got {reply:?}"
            );
        }
    }
    // And an operator outside the model's `FilterOperator` enum is refused.
    let bad_operator = emulator
        .list_applications(json!({
            "Filters": [{"Name": "Id", "Value": "alpha", "Operator": "NotEquals"}]
        }))
        .await;
    assert_eq!(
        bad_operator.status, 400,
        "case \"NotEquals\": an unmodelled operator is refused, got {bad_operator:?}"
    );

    // The bounds the model declares on the filter list and its members are enforced, so
    // the work one request can ask of the store is bounded: `FilterValue` is
    // `@length(1..64)`, `FilterName` `@length(1..32)` and `FilterList` `@length(1..10)`.
    // Each row is the first value PAST a published bound; the rows at the bound itself
    // are accepted above.
    let over_bounds: &[(&str, Value)] = &[
        (
            "Value one character over 64",
            json!({"Filters": [{"Name": "Id", "Value": "a".repeat(65), "Operator": "Equals"}]}),
        ),
        (
            "Value far over 64",
            json!({"Filters": [{"Name": "Id", "Value": "a,".repeat(4096), "Operator": "Equals"}]}),
        ),
        (
            "Name one character over 32",
            json!({"Filters": [{"Name": "N".repeat(33), "Value": "alpha", "Operator": "Equals"}]}),
        ),
        (
            "eleven filters",
            json!({"Filters": (0..11)
                .map(|index| json!({"Name": "Id", "Value": format!("app-{index}"), "Operator": "Equals"}))
                .collect::<Vec<Value>>()}),
        ),
    ];
    for (case, body) in over_bounds {
        let reply = emulator.list_applications(body.clone()).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: a filter past a published bound is refused, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {reply:?}"
        );
    }

    // At the bounds themselves the request is served: a 64-character `Value`, a
    // 32-character `Name` (which is not a filterable attribute, so it is refused for
    // being unknown rather than for its length) and exactly ten filters.
    let at_bound = emulator
        .list_applications(json!({
            "Filters": [{"Name": "Id", "Value": "a".repeat(64), "Operator": "Equals"}]
        }))
        .await;
    assert_eq!(
        at_bound.status, 200,
        "case \"Value of exactly 64\": the bound itself is accepted, got {at_bound:?}"
    );
    assert!(
        listed_ids(&at_bound).is_empty(),
        "case \"Value of exactly 64\": no application has that id, got {at_bound:?}"
    );
    let ten_filters = emulator
        .list_applications(json!({
            "Filters": (0..10)
                .map(|_| json!({"Name": "Type", "Value": "HANA", "Operator": "Equals"}))
                .collect::<Vec<Value>>()
        }))
        .await;
    assert_eq!(
        ten_filters.status, 200,
        "case \"ten filters\": the bound itself is accepted, got {ten_filters:?}"
    );
    assert_eq!(
        {
            let mut ids = listed_ids(&ten_filters);
            ids.sort();
            ids
        },
        vec!["alpha".to_string(), "charlie".to_string()],
        "case \"ten filters\": ten copies of one filter narrow the same way, got {ten_filters:?}"
    );
}

#[tokio::test]
async fn deregistering_removes_the_application_from_every_reader() {
    for id in ["HDB-TEST", "a", "abap.prod_01"] {
        let emulator = Emulator::new();
        let operation_id = emulator.register_settled(id).await;
        // A second application proves the removal is scoped to the one named.
        let survivor = format!("{id}-keep");
        emulator.register_settled(&survivor).await;

        let removed = emulator.deregister(id).await;
        assert_eq!(
            removed.status, 200,
            "case {id:?}: expected 200 from DeregisterApplication, got {removed:?}"
        );
        // The deregistration is asynchronous: the application reports DELETING until the
        // removal the clock owes it falls due (`tests/all/deregistration_window.rs` pins
        // that window), so this waits the clock out before reading it back.
        settle().await;

        let fetched = emulator.get_application(json!({"ApplicationId": id})).await;
        assert_eq!(
            fetched.status, 400,
            "case {id:?}: a deregistered application is gone, got {fetched:?}"
        );
        assert!(
            fetched.member("Application").is_none(),
            "case {id:?}: nothing may be reported for it, got {fetched:?}"
        );

        let listed = emulator.list_applications(json!({})).await;
        let ids: Vec<String> = listed_ids(&listed);
        assert_eq!(
            ids,
            vec![survivor.clone()],
            "case {id:?}: only the other application remains listed, got {listed:?}"
        );

        // The record of the call that handed out that `OperationId` is the history of a
        // call already made, so the removal does not destroy it.
        let operation = emulator.get_operation(&operation_id).await;
        assert_eq!(
            operation.status, 200,
            "case {id:?}: an OperationId handed out earlier still resolves, got {operation:?}"
        );
        assert_eq!(
            operation.op_str("ResourceId"),
            id,
            "case {id:?}: the surviving record still names the application it was filed \
             against, got {operation:?}"
        );

        // Starting and stopping it is now a not-found, and deregistering it again is
        // refused.
        let started = emulator.start(id).await;
        assert_eq!(
            started.status, 404,
            "case {id:?}: expected 404 after deregistration, got {started:?}"
        );
        let again = emulator.deregister(id).await;
        assert_eq!(
            again.status, 400,
            "case {id:?}: deregistering an unknown application is refused, got {again:?}"
        );

        // The id is free again, and the re-registration is a new record.
        let reregistered = emulator
            .register(json!({
                "ApplicationId": id,
                "ApplicationType": "SAP_ABAP",
                "Instances": ["i-0123456789abcdef0"],
            }))
            .await;
        assert_eq!(
            reregistered.status, 200,
            "case {id:?}: a deregistered id can be registered again, got {reregistered:?}"
        );
        assert_eq!(
            reregistered.app_str("Type"),
            "SAP_ABAP",
            "case {id:?}: the new registration's own type is stored, got {reregistered:?}"
        );
    }
}
