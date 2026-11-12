// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's dashboards.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * a `body` is a dashboard DEFINITION — a JSON object, closed at the top level, whose
//!   `panels` member is a list of panel objects. A body that violates that schema is
//!   refused with the modelled `ValidationException` (400) and nothing is persisted,
//!   whatever shape it has, and the refusal names EVERY constraint the document
//!   violates, one clause per violation;
//! * a body that IS one is persisted verbatim and read back verbatim;
//! * `CreateOmniDashboard` mints the dashboard in an EXISTING space and
//!   `GetOmniDashboard` answers from that same store;
//! * every derived member (`dashboardId`, `arn`, `createdBy`, `createdAt`/`updatedAt`)
//!   is computed from the request and the store rather than fixed;
//! * a dashboard is addressed by its space AND its id, so neither an unknown id nor a
//!   known id under another space resolves;
//! * a name already identifying a dashboard of that space cannot identify a second one;
//! * `@idempotencyToken` replays return the original dashboard.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::collections::HashMap;
use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_domain::CreateDomain;
use omni::ops::create_omni_dashboard::{
    CreateOmniDashboard, CreateOmniDashboardError, CreateOmniDashboardErrorModeled,
};
use omni::ops::create_space::CreateSpace;
use omni::ops::delete_omni_dashboard::{DeleteOmniDashboard, DeleteOmniDashboardError};
use omni::ops::list_omni_dashboards::{ListOmniDashboards, ListOmniDashboardsError};
use omni::ops::get_omni_dashboard::{GetOmniDashboard, GetOmniDashboardError};
use omni::scaffolded::Scaffolded;
use omni::ops::types::{
    CreateDomainInput, CreateOmniDashboardInput, CreateOmniDashboardOutput, CreateSpaceInput,
    DeleteOmniDashboardInput, DeleteOmniDashboardOutput, GetOmniDashboardInput,
    GetOmniDashboardOutput, ListOmniDashboardsInput, ListOmniDashboardsOutput, OmniDashboard,
    CREATE_OMNI_DASHBOARD_INPUT_SCHEMA, GET_OMNI_DASHBOARD_INPUT_SCHEMA,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// The access key id the suite signs with, which `createdBy` is expected to name.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A role ARN satisfying `Arn`'s `^arn:.+$`, which `CreateSpace` requires.
const DATA_ACCESS_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OmniData";

/// `DashboardId`'s modelled `@pattern` with its `@length(36..36)`, from
/// `smithy_query(query="shape",shape="DashboardId")`.
const DASHBOARD_ID_PATTERN: &str =
    r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ (length 36..36)";

/// A well-formed `SpaceId` (its modelled pattern) that names nothing.
const MISSING_SPACE_ID: &str = "00000000-0000-4000-8000-000000000000";

/// A well-formed `DashboardId` that names nothing.
const MISSING_DASHBOARD_ID: &str = "11111111-2222-4333-8444-555555555555";

/// Whether `value` satisfies [`DASHBOARD_ID_PATTERN`].
///
/// Checked by hand rather than through a regex engine, so this crate declares no
/// dependency beyond the ones it already has. The predicate is exercised on values that
/// must FAIL it as well as pass ([`the_dashboard_id_pattern_is_checked_as_written`]).
fn is_dashboard_id(value: &str) -> bool {
    let groups: Vec<&str> = value.split('-').collect();
    groups.len() == 5
        && [8, 4, 4, 4, 12]
            .iter()
            .zip(&groups)
            .all(|(width, group)| {
                group.len() == *width
                    && group
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            })
}

/// Whether `value` satisfies `Arn`'s modelled `^arn:.+$`.
fn is_arn(value: &str) -> bool {
    value
        .strip_prefix("arn:")
        .is_some_and(|rest| !rest.is_empty())
}

/// A timestamp at millisecond resolution — the resolution rpcv2Cbor encodes a
/// `Timestamp` at, so a value compared across a wire round trip is compared there.
fn millis(at: aws_smithy_types::DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos() / 1_000_000)
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key), the region the handlers record and `access_key_id`.
fn signed_request_parts(access_key_id: Option<&str>) -> http::request::Parts {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused");
    if let Some(access_key_id) = access_key_id {
        builder = builder.header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        );
    }
    let (parts, ()) = builder
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    fn new() -> Self {
        Self::signed_as(Some(ACCESS_KEY_ID))
    }

    /// An emulator whose requests carry `access_key_id`, or no credential at all.
    fn signed_as(access_key_id: Option<&str>) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        // The router's service and the operations constructed below all hold the SAME
        // engine, so both front doors read and write one store.
        router.register_service(omni::EmulatedService::new(Arc::clone(&engine)));
        Self {
            service: omni::EmulatedService::new(Arc::clone(&engine)),
            engine,
            router,
            parts: signed_request_parts(access_key_id),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    /// A space to create dashboards in, through the handlers that own those resources.
    async fn space(&self, case: &str, name: &str) -> String {
        let domain = CreateDomain::new(Arc::clone(&self.engine))
            .handle(
                CreateDomainInput {
                    name: format!("omni-{name}"),
                    identity_providers: vec!["IAM".to_string()],
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateDomain failed: {error:?}"))
            .domain;
        CreateSpace::new(Arc::clone(&self.engine))
            .handle(
                CreateSpaceInput {
                    name: name.to_string(),
                    domain_id: domain.domain_id,
                    data_access_role_arn: DATA_ACCESS_ROLE_ARN.to_string(),
                    ..Default::default()
                },
                &self.ctx(),
            )
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateSpace failed: {error:?}"))
            .space
            .space_id
    }

    async fn create_dashboard(
        &self,
        input: &CreateOmniDashboardInput,
    ) -> Result<CreateOmniDashboardOutput, CreateOmniDashboardError> {
        CreateOmniDashboard::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn get_dashboard(
        &self,
        space_id: &str,
        dashboard_id: &str,
    ) -> Result<GetOmniDashboardOutput, GetOmniDashboardError> {
        GetOmniDashboard::new(Arc::clone(&self.engine))
            .handle(
                GetOmniDashboardInput {
                    space_id: space_id.to_string(),
                    dashboard_id: dashboard_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    async fn list_dashboards(
        &self,
        input: &ListOmniDashboardsInput,
    ) -> Result<ListOmniDashboardsOutput, ListOmniDashboardsError> {
        ListOmniDashboards::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    async fn delete_dashboard(
        &self,
        space_id: &str,
        dashboard_id: &str,
    ) -> Result<DeleteOmniDashboardOutput, DeleteOmniDashboardError> {
        DeleteOmniDashboard::new(Arc::clone(&self.engine))
            .handle(
                DeleteOmniDashboardInput {
                    space_id: space_id.to_string(),
                    dashboard_id: dashboard_id.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    async fn over_the_wire<I: SerializableStruct>(
        &self,
        operation: &str,
        input_schema: &Schema,
        input: &I,
    ) -> (u16, bytes::Bytes) {
        let codec = CborCodec::default();
        let mut serializer = codec.create_serializer();
        serializer
            .write_struct(input_schema, input)
            .expect("the input shape serializes");
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(format!(
                "/service/CloudWatchOmniFrontend/operation/{operation}"
            ))
            .header("content-type", "application/cbor")
            .header("smithy-protocol", "rpc-v2-cbor");
        for (name, value) in &self.parts.headers {
            builder = builder.header(name, value);
        }
        let request = builder
            .body(bytes::Bytes::from(serializer.finish()))
            .expect("a well-formed rpcv2Cbor request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().clone())
    }
}

/// Require a modelled failure with the status and error code the model declares for it.
fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
    case: &str,
    result: Result<O, E>,
    expected_status: u16,
    expected_code: &str,
) {
    match result {
        Ok(output) => panic!(
            "case {case:?}: expected {expected_code} ({expected_status}), got Ok({output:?})"
        ),
        Err(error) => {
            assert_eq!(
                error.shape_name(),
                expected_code,
                "case {case:?}: expected error code {expected_code}, got {error:?}"
            );
            assert_eq!(
                error.http_status(),
                expected_status,
                "case {case:?}: expected HTTP {expected_status} for {expected_code}, got {}",
                error.http_status()
            );
        }
    }
}

/// The smallest body the rule admits: a definition that lays out no panel yet.
const EMPTY_PANELS_BODY: &str = r#"{"panels":[]}"#;

fn create_input(space_id: &str, name: &str, body: &str) -> CreateOmniDashboardInput {
    CreateOmniDashboardInput {
        space_id: space_id.to_string(),
        name: name.to_string(),
        body: body.to_string(),
        ..Default::default()
    }
}

/// One row of the `CreateOmniDashboard` table: the request members that vary.
struct DashboardCase {
    case: &'static str,
    name: String,
    body: String,
    description: Option<String>,
    tags: Option<HashMap<String, String>>,
}

/// The rows span what the rule generalises over: a `panels` list that is empty, has one
/// panel and has many; whitespace and
/// non-ASCII text inside the document; `name` over the whole
/// `@length(1..256)`/`^[a-zA-Z0-9_.@~()-]+$` class; a `description` absent and at
/// `@length(max: 1024)`; and `tags` absent, one and at `@length(max: 50)`.
fn dashboard_cases() -> Vec<DashboardCase> {
    vec![
        DashboardCase {
            case: "an empty panels list, nothing else",
            name: "a".to_string(),
            body: EMPTY_PANELS_BODY.to_string(),
            description: None,
            tags: None,
        },
        DashboardCase {
            case: "one panel, pretty-printed with newlines",
            name: "Checkout_latency.p99@~(prod)-view".to_string(),
            body: "{\n  \"panels\": [\n    {\"type\": \"timeSeries\", \"title\": \"p99\"}\n  ]\n}"
                .to_string(),
            description: Some("an optional description".to_string()),
            tags: Some(HashMap::from([("team".to_string(), "sre".to_string())])),
        },
        DashboardCase {
            case: "many panels, non-ASCII text",
            name: "many-panels".to_string(),
            body: format!(
                r#"{{"panels":[{}]}}"#,
                (0..24)
                    .map(|index| format!(
                        r#"{{"id":{index},"title":"café ☕ {index}","query":{{"language":"sql","expression":"SELECT 1"}}}}"#
                    ))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            description: Some("d".repeat(1024)),
            tags: Some(
                (0..50)
                    .map(|index| (format!("key{index}"), format!("value{index}")))
                    .collect(),
            ),
        },
        DashboardCase {
            case: "the longest name the model admits, a deeply nested panel",
            name: "n".repeat(256),
            body: r#"{"panels":[{"rows":[{"cells":[{"widget":{"kind":"logs","filters":["a","b"]}}]}]}]}"#
                .to_string(),
            description: Some("~".to_string()),
            tags: Some(HashMap::new()),
        },
    ]
}

/// Bodies that are NOT a dashboard definition, spanning the ways a body can fail the
/// rule: text that is not JSON at all, a JSON value that is not an object, an object
/// with no `panels`, a `panels` member that is not a list, a list whose entries are
/// not panel objects, and a document carrying a top-level member the closed schema does
/// not declare.
fn rejected_bodies() -> Vec<(&'static str, &'static str)> {
    vec![
        ("not JSON at all", "panels"),
        ("truncated JSON", r#"{"panels":[{"type":"logs"}"#),
        ("JSON with a trailing comma", r#"{"panels":[],}"#),
        ("a JSON string", r#""{\"panels\":[]}""#),
        ("a JSON number", "7"),
        ("a JSON boolean", "true"),
        ("JSON null", "null"),
        ("a JSON array of panels", r#"[{"type":"logs"}]"#),
        ("an empty JSON object", "{}"),
        ("a document with no panels member", r#"{"widgets":[]}"#),
        ("panels spelled with another case", r#"{"Panels":[]}"#),
        ("panels as an object", r#"{"panels":{"0":{"type":"logs"}}}"#),
        ("panels as a string", r#"{"panels":"none"}"#),
        ("panels as a number", r#"{"panels":0}"#),
        ("panels as null", r#"{"panels":null}"#),
        ("a panel that is a number", r#"{"panels":[1]}"#),
        ("a panel that is a string", r#"{"panels":["timeSeries"]}"#),
        (
            "a panel that is a list, after a valid one",
            r#"{"panels":[{"type":"logs"},[]]}"#,
        ),
        (
            "a conforming panels list beside an undeclared top-level member",
            r#"{"panels":[],"widgets":[]}"#,
        ),
        (
            "a conforming panels list beside several undeclared members",
            r#"{"version":"2026-01-01","panels":[],"variables":{"env":"prod"}}"#,
        ),
    ]
}

/// Every member of a dashboard but its timestamps, as a comparable tuple.
type DashboardFields = (
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    Option<HashMap<String, String>>,
);

fn fields(dashboard: &OmniDashboard) -> DashboardFields {
    (
        dashboard.dashboard_id.clone(),
        dashboard.arn.clone(),
        dashboard.name.clone(),
        dashboard.body.clone(),
        dashboard.created_by.clone(),
        dashboard.description.clone(),
        dashboard.tags.clone(),
    )
}

#[tokio::test]
async fn the_dashboard_id_pattern_is_checked_as_written() {
    // Each row: the value, and whether the modelled pattern and length admit it.
    for (value, expected) in [
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6d", true),
        ("00000000-0000-0000-0000-000000000000", true),
        ("ffffffff-ffff-ffff-ffff-ffffffffffff", true),
        ("FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF", false),
        ("8a4f1b3c0e2d4a5b9c7d1f2e3a4b5c6d", false),
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6", false),
        ("8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6dd", false),
        ("8a4f1b3g-0e2d-4a5b-9c7d-1f2e3a4b5c6d", false),
        ("", false),
    ] {
        assert_eq!(
            is_dashboard_id(value),
            expected,
            "case {value:?}: {DASHBOARD_ID_PATTERN} should {} it",
            if expected { "admit" } else { "reject" }
        );
    }

    for (value, expected) in [
        (
            "arn:aws:cloudwatch:us-west-2:000000000000:omni-dashboard/8a4f1b3c-0e2d-4a5b-9c7d-1f2e3a4b5c6d",
            true,
        ),
        ("arn:x", true),
        ("arn:", false),
        ("aws:arn:thing", false),
    ] {
        assert_eq!(
            is_arn(value),
            expected,
            "case {value:?}: ^arn:.+$ should {} it",
            if expected { "admit" } else { "reject" }
        );
    }
}

#[tokio::test]
async fn create_omni_dashboard_persists_a_definition_get_omni_dashboard_returns_it() {
    let omni = Omni::new();
    let space_id = omni.space("table", "dashboard-table").await;
    let mut minted: Vec<String> = Vec::new();

    for row in dashboard_cases() {
        let case = row.case;
        let created = omni
            .create_dashboard(&CreateOmniDashboardInput {
                space_id: space_id.clone(),
                name: row.name.clone(),
                body: row.body.clone(),
                description: row.description.clone(),
                tags: row.tags.clone(),
                client_token: None,
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateOmniDashboard failed: {error:?}"))
            .omni_dashboard;

        // The id is minted by the service, inside what `DashboardId` models.
        assert!(
            is_dashboard_id(&created.dashboard_id),
            "case {case:?}: dashboardId {:?} does not satisfy {DASHBOARD_ID_PATTERN}",
            created.dashboard_id
        );
        assert!(
            !minted.contains(&created.dashboard_id),
            "case {case:?}: dashboardId {:?} repeats an earlier dashboard's id",
            created.dashboard_id
        );
        minted.push(created.dashboard_id.clone());

        // The ARN is derived from this dashboard's id in the signed region, in the form
        // the model's own example documents, and satisfies `Arn`'s pattern.
        assert!(
            is_arn(&created.arn),
            "case {case:?}: arn {:?} does not satisfy ^arn:.+$",
            created.arn
        );
        assert!(
            created
                .arn
                .ends_with(&format!(":omni-dashboard/{}", created.dashboard_id)),
            "case {case:?}: arn {:?} does not carry the modelled omni-dashboard/<id> resource",
            created.arn
        );
        assert!(
            created.arn.contains(&format!(":{REGION}:")),
            "case {case:?}: arn {:?} does not carry the signed region {REGION}",
            created.arn
        );

        // The request's own members come back as sent — the body BYTE for byte, since a
        // definition is stored rather than reformatted — and `tags` is reported as the
        // dashboard's tag map, empty when the request named none.
        assert_eq!(
            (
                created.name.as_str(),
                created.body.as_str(),
                created.description.as_deref(),
                created.tags.clone(),
            ),
            (
                row.name.as_str(),
                row.body.as_str(),
                row.description.as_deref(),
                Some(row.tags.clone().unwrap_or_default()),
            ),
            "case {case:?}: the created dashboard is {created:?}"
        );
        assert!(
            created.created_by.ends_with(&format!(":user/{ACCESS_KEY_ID}")),
            "case {case:?}: createdBy {:?} does not name the signing principal {ACCESS_KEY_ID}",
            created.created_by
        );
        assert_eq!(
            millis(created.created_at),
            millis(created.updated_at),
            "case {case:?}: a dashboard nothing has updated reports createdAt {:?} and updatedAt {:?}",
            created.created_at,
            created.updated_at
        );

        // The reader answers from the store, verbatim and stably.
        for read in ["first read", "second read"] {
            let fetched = omni
                .get_dashboard(&space_id, &created.dashboard_id)
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?} ({read}): GetOmniDashboard failed: {error:?}")
                })
                .omni_dashboard;
            assert_eq!(
                fields(&fetched),
                fields(&created),
                "case {case:?} ({read}): the stored dashboard is {fetched:?}, the created one {created:?}"
            );
            assert_eq!(
                (millis(fetched.created_at), millis(fetched.updated_at)),
                (millis(created.created_at), millis(created.updated_at)),
                "case {case:?} ({read}): the stored timestamps differ from the created ones"
            );
        }
    }
}

#[tokio::test]
async fn a_body_that_is_not_a_panels_document_is_refused_and_nothing_is_persisted() {
    let omni = Omni::new();
    let space_id = omni.space("bodies", "dashboard-bodies").await;

    for (index, (case, body)) in rejected_bodies().into_iter().enumerate() {
        let name = format!("rejected-{index}");
        let refused = omni
            .create_dashboard(&create_input(&space_id, &name, body))
            .await;
        // The refusal says what is wrong with the body, naming the member a caller has
        // to fix — so the text is a diagnosis rather than a bare code.
        let reported = format!("{refused:?}");
        assert!(
            reported.contains("dashboard body"),
            "case {case:?}: the refusal does not name the dashboard body: {reported}"
        );
        expect_modelled_error(case, refused, 400, "ValidationException");

        // Nothing was persisted for the refused request: the name it asked for is still
        // free, so a definition under the same name is created rather than conflicting.
        let created = omni
            .create_dashboard(&create_input(&space_id, &name, EMPTY_PANELS_BODY))
            .await
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the refusal persisted the name: {error:?}")
            })
            .omni_dashboard;
        assert_eq!(
            created.body, EMPTY_PANELS_BODY,
            "case {case:?}: expected the stored body {EMPTY_PANELS_BODY:?}, got {:?}",
            created.body
        );
    }
}

#[tokio::test]
async fn a_dashboard_is_addressed_by_its_space_and_its_id() {
    let omni = Omni::new();
    let home = omni.space("home", "dashboard-home").await;
    let other = omni.space("other", "dashboard-other").await;
    let created = omni
        .create_dashboard(&create_input(&home, "addressed", EMPTY_PANELS_BODY))
        .await
        .expect("CreateOmniDashboard in an existing space")
        .omni_dashboard;

    // Rows: the id under its own space resolves; everything else is the modelled 404.
    omni.get_dashboard(&home, &created.dashboard_id)
        .await
        .expect("case \"its own space\": the dashboard resolves");
    for (case, space_id, dashboard_id) in [
        (
            "a known id under another space",
            other.as_str(),
            created.dashboard_id.as_str(),
        ),
        ("an unknown id", home.as_str(), MISSING_DASHBOARD_ID),
        (
            "a space the store does not hold",
            MISSING_SPACE_ID,
            created.dashboard_id.as_str(),
        ),
    ] {
        expect_modelled_error(
            case,
            omni.get_dashboard(space_id, dashboard_id).await,
            404,
            "ResourceNotFoundException",
        );
    }

    // And a create names its parent: a space the store does not hold is the same 404.
    expect_modelled_error(
        "a create in a space the store does not hold",
        omni.create_dashboard(&create_input(MISSING_SPACE_ID, "orphan", EMPTY_PANELS_BODY))
            .await,
        404,
        "ResourceNotFoundException",
    );
}

#[tokio::test]
async fn a_name_identifies_one_dashboard_per_space() {
    let omni = Omni::new();
    let first = omni.space("first", "dashboard-names-a").await;
    let second = omni.space("second", "dashboard-names-b").await;

    // Rows span the name class: each is created once, refused the second time in the
    // same space, and accepted in another space.
    for name in ["a", "Checkout_latency.p99@~(prod)-view", &"n".repeat(256)] {
        let created = omni
            .create_dashboard(&create_input(&first, name, EMPTY_PANELS_BODY))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: the first create failed: {error:?}"))
            .omni_dashboard;
        expect_modelled_error(
            name,
            omni.create_dashboard(&create_input(&first, name, EMPTY_PANELS_BODY))
                .await,
            409,
            "ConflictException",
        );
        let elsewhere = omni
            .create_dashboard(&create_input(&second, name, EMPTY_PANELS_BODY))
            .await
            .unwrap_or_else(|error| {
                panic!("case {name:?}: the same name in another space failed: {error:?}")
            })
            .omni_dashboard;
        assert_ne!(
            elsewhere.dashboard_id, created.dashboard_id,
            "case {name:?}: the same name in another space is a different dashboard, got {:?} twice",
            elsewhere.dashboard_id
        );
        assert!(
            elsewhere.arn.ends_with(&format!(
                ":omni-dashboard/{}",
                elsewhere.dashboard_id
            )),
            "case {name:?}: arn {:?} does not carry the modelled omni-dashboard/<id> resource",
            elsewhere.arn
        );
        omni.get_dashboard(&second, &elsewhere.dashboard_id)
            .await
            .unwrap_or_else(|error| {
                panic!("case {name:?}: the second space does not hold its dashboard: {error:?}")
            });
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_dashboard() {
    let omni = Omni::new();
    let space_id = omni.space("tokens", "dashboard-tokens").await;

    // Rows span `ClientToken`'s `@length(1..64)` and its `^[!-~]+$` character class.
    for (index, token) in ["!", "a-token", "~{}[]|\\^`", &"t".repeat(64)]
        .into_iter()
        .enumerate()
    {
        let input = CreateOmniDashboardInput {
            client_token: Some(token.to_string()),
            ..create_input(
                &space_id,
                &format!("token-dashboard-{index}"),
                EMPTY_PANELS_BODY,
            )
        };
        let created = omni
            .create_dashboard(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the first create failed: {error:?}"))
            .omni_dashboard;
        let replayed = omni
            .create_dashboard(&input)
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: the replay failed: {error:?}"))
            .omni_dashboard;
        assert_eq!(
            fields(&replayed),
            fields(&created),
            "case {token:?}: the replay returned {replayed:?} rather than the original {created:?}"
        );

        // A DIFFERENT token for the same request is a different dashboard, so the replay
        // is the token's doing rather than the name's.
        let fresh = omni
            .create_dashboard(&CreateOmniDashboardInput {
                client_token: Some(format!("{token}x")),
                name: format!("token-dashboard-{index}-fresh"),
                ..input.clone()
            })
            .await
            .unwrap_or_else(|error| panic!("case {token:?}: a fresh token failed: {error:?}"))
            .omni_dashboard;
        assert_ne!(
            fresh.dashboard_id, created.dashboard_id,
            "case {token:?}: a fresh token must mint a new dashboard, got {:?} twice",
            fresh.dashboard_id
        );
    }
}

#[tokio::test]
async fn created_by_names_the_principal_that_signed_the_request() {
    // Rows: two different access key ids, and a request carrying no credential at all.
    // `createdBy` is read from the request, so it differs per row rather than being one
    // fixed principal.
    let mut reported: Vec<String> = Vec::new();
    for (case, access_key_id) in [
        ("the suite's key", Some(ACCESS_KEY_ID)),
        ("another key", Some("AKIDSECONDCALLER")),
        ("an unsigned, in-process call", None),
    ] {
        let omni = Omni::signed_as(access_key_id);
        let space_id = omni.space(case, "dashboard-principal").await;
        let created = omni
            .create_dashboard(&create_input(&space_id, "who-made-me", EMPTY_PANELS_BODY))
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateOmniDashboard failed: {error:?}"))
            .omni_dashboard;
        assert!(
            is_arn(&created.created_by),
            "case {case:?}: createdBy {:?} does not satisfy ^arn:.+$",
            created.created_by
        );
        if let Some(access_key_id) = access_key_id {
            assert!(
                created
                    .created_by
                    .ends_with(&format!(":user/{access_key_id}")),
                "case {case:?}: expected createdBy to name {access_key_id}, got {:?}",
                created.created_by
            );
        }
        assert!(
            !reported.contains(&created.created_by),
            "case {case:?}: createdBy {:?} repeats another caller's principal",
            created.created_by
        );
        reported.push(created.created_by.clone());
    }
}

#[tokio::test]
async fn the_dashboard_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    // The parent space is created through the handlers holding the same engine the
    // router's service holds, which is what makes the wire calls below see it.
    let space_id = omni.space("wire", "dashboard-wire").await;

    let codec = CborCodec::default();
    let (status, body) = omni
        .over_the_wire(
            "CreateOmniDashboard",
            CREATE_OMNI_DASHBOARD_INPUT_SCHEMA,
            &create_input(
                &space_id,
                "wire-dashboard",
                r#"{"panels":[{"type":"timeSeries"}]}"#,
            ),
        )
        .await;
    assert_eq!(
        status,
        200,
        "CreateOmniDashboard over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let created = {
        let mut deserializer = codec.create_deserializer(&body);
        CreateOmniDashboardOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .omni_dashboard
    };
    assert!(
        is_dashboard_id(&created.dashboard_id),
        "dashboardId {:?} does not satisfy {DASHBOARD_ID_PATTERN}",
        created.dashboard_id
    );

    // The dashboard the wire call created is readable over the wire too, as persisted.
    let (status, body) = omni
        .over_the_wire(
            "GetOmniDashboard",
            GET_OMNI_DASHBOARD_INPUT_SCHEMA,
            &GetOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: created.dashboard_id.clone(),
            },
        )
        .await;
    assert_eq!(
        status,
        200,
        "GetOmniDashboard over the wire answered {status}: {:?}",
        String::from_utf8_lossy(&body)
    );
    let fetched = {
        let mut deserializer = codec.create_deserializer(&body);
        GetOmniDashboardOutput::deserialize(&mut deserializer)
            .expect("the output deserializes")
            .omni_dashboard
    };
    assert_eq!(
        fields(&fetched),
        fields(&created),
        "the wire read answered {fetched:?} for the created {created:?}"
    );
    assert_eq!(
        (millis(fetched.created_at), millis(fetched.updated_at)),
        (millis(created.created_at), millis(created.updated_at)),
        "the wire read and the create disagree on the timestamps"
    );

    // A miss over the wire is the modelled 404 with its modelled error code.
    let (status, body) = omni
        .over_the_wire(
            "GetOmniDashboard",
            GET_OMNI_DASHBOARD_INPUT_SCHEMA,
            &GetOmniDashboardInput {
                space_id: space_id.clone(),
                dashboard_id: MISSING_DASHBOARD_ID.to_string(),
            },
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 404,
        "GetOmniDashboard over the wire answered {status} for a missing dashboard: {rendered:?}"
    );
    assert!(
        rendered.contains("ResourceNotFoundException"),
        "the wire error body does not name ResourceNotFoundException: {rendered:?}"
    );

    // A body that is not a dashboard definition is the modelled 400 over the wire too.
    for (case, bad_body) in rejected_bodies() {
        let (status, body) = omni
            .over_the_wire(
                "CreateOmniDashboard",
                CREATE_OMNI_DASHBOARD_INPUT_SCHEMA,
                &create_input(&space_id, "wire-rejected", bad_body),
            )
            .await;
        let rendered = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 400,
            "case {case:?}: the wire answered {status} rather than 400: {rendered:?}"
        );
        assert!(
            rendered.contains("ValidationException"),
            "case {case:?}: the wire refusal does not name ValidationException: {rendered:?}"
        );
    }
}

/// One clause of a dashboard-body refusal: the location of the violation, then the
/// schema constraint violated there, in the fixed wording the refusal uses.
fn clause(location: &str, constraint: &str) -> String {
    format!("{location}: does not satisfy the '{constraint}' constraint of the dashboard body schema")
}

/// What joins one clause to the next.
const CLAUSE_SEPARATOR: &str = "; ";

/// The location a constraint on the whole document is reported at.
const DOCUMENT_ROOT: &str = "$";

/// The message the operation's modelled `ValidationException` carries.
fn validation_message(
    case: &str,
    result: Result<CreateOmniDashboardOutput, CreateOmniDashboardError>,
) -> String {
    match result {
        Ok(output) => {
            panic!("case {case:?}: expected a ValidationException, got Ok({output:?})")
        }
        Err(Scaffolded::Modeled(CreateOmniDashboardErrorModeled::ValidationException(error))) => {
            error.message
        }
        Err(other) => panic!(
            "case {case:?}: expected the modelled ValidationException, got {other:?}"
        ),
    }
}

#[tokio::test]
async fn the_refusal_names_every_schema_constraint_the_body_violates() {
    let omni = Omni::new();
    let space_id = omni.space("schema", "dashboard-schema").await;

    // Rows span the schema's constraints and the number of them one document can
    // violate at once: the document's own type (text that is not JSON, and every JSON
    // type that is not an object), the required `panels` member, its type, its entries,
    // and the closedness of the top level — each alone and in combination. The clause
    // list is the schema's own order: the root's type, then `panels`, then closedness.
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "no panels member, one undeclared member",
            r#"{"widgets":[]}"#.to_string(),
            vec![
                clause("panels", "required"),
                clause(DOCUMENT_ROOT, "additionalProperties"),
            ],
        ),
        (
            "an empty document",
            "{}".to_string(),
            vec![clause("panels", "required")],
        ),
        (
            "panels spelled with another case, so it is an undeclared member",
            r#"{"Panels":[]}"#.to_string(),
            vec![
                clause("panels", "required"),
                clause(DOCUMENT_ROOT, "additionalProperties"),
            ],
        ),
        (
            "a conforming panels list beside one undeclared member",
            r#"{"panels":[],"widgets":[]}"#.to_string(),
            vec![clause(DOCUMENT_ROOT, "additionalProperties")],
        ),
        (
            "a conforming panels list beside several undeclared members",
            r#"{"version":"2026-01-01","panels":[{"type":"logs"}],"variables":{"env":"prod"}}"#
                .to_string(),
            vec![clause(DOCUMENT_ROOT, "additionalProperties")],
        ),
        (
            "panels as an object",
            r#"{"panels":{"0":{"type":"logs"}}}"#.to_string(),
            vec![clause("panels", "type")],
        ),
        (
            "panels as a string, beside an undeclared member",
            r#"{"panels":"none","widgets":[]}"#.to_string(),
            vec![
                clause("panels", "type"),
                clause(DOCUMENT_ROOT, "additionalProperties"),
            ],
        ),
        (
            "panels as null",
            r#"{"panels":null}"#.to_string(),
            vec![clause("panels", "type")],
        ),
        (
            "a panel that is a number, after a conforming one",
            r#"{"panels":[{"type":"logs"},1]}"#.to_string(),
            vec![clause("panels", "items")],
        ),
        (
            "several panels that are not objects, beside an undeclared member",
            r#"{"panels":["a",2],"layout":{}}"#.to_string(),
            vec![
                clause("panels", "items"),
                clause(DOCUMENT_ROOT, "additionalProperties"),
            ],
        ),
        (
            "a JSON array",
            r#"[{"type":"logs"}]"#.to_string(),
            vec![clause(DOCUMENT_ROOT, "type")],
        ),
        (
            "a JSON string",
            r#""{\"panels\":[]}""#.to_string(),
            vec![clause(DOCUMENT_ROOT, "type")],
        ),
        (
            "a JSON number",
            "7".to_string(),
            vec![clause(DOCUMENT_ROOT, "type")],
        ),
        (
            "JSON null",
            "null".to_string(),
            vec![clause(DOCUMENT_ROOT, "type")],
        ),
        (
            "not JSON at all",
            "panels".to_string(),
            vec![clause(DOCUMENT_ROOT, "type")],
        ),
        (
            "truncated JSON",
            r#"{"panels":[{"type":"logs"}"#.to_string(),
            vec![clause(DOCUMENT_ROOT, "type")],
        ),
    ];

    for (index, (case, body, clauses)) in cases.into_iter().enumerate() {
        let refused = omni
            .create_dashboard(&create_input(
                &space_id,
                &format!("schema-{index}"),
                &body,
            ))
            .await;
        if let Err(error) = &refused {
            assert_eq!(
                (error.shape_name(), error.http_status()),
                ("ValidationException", 400),
                "case {case:?}: expected ValidationException (400), got {error:?}"
            );
        }
        let actual = validation_message(case, refused);
        let expected = clauses.join(CLAUSE_SEPARATOR);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the message {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_conforming_body_is_not_refused_however_its_panels_are_laid_out() {
    let omni = Omni::new();
    let space_id = omni.space("conforming", "dashboard-conforming").await;

    // Rows span what the schema admits: `panels` alone, empty and holding one, many and
    // deeply nested panel objects, with whitespace and non-ASCII text in the document.
    let bodies = [
        ("no panel yet", EMPTY_PANELS_BODY.to_string()),
        (
            "one panel, pretty-printed",
            "{\n  \"panels\": [\n    {}\n  ]\n}".to_string(),
        ),
        (
            "a panel carrying any members it likes",
            r#"{"panels":[{"type":"timeSeries","title":"café ☕","query":{"sql":"SELECT 1"}}]}"#
                .to_string(),
        ),
        (
            "many panels",
            format!(
                r#"{{"panels":[{}]}}"#,
                (0..16)
                    .map(|index| format!(r#"{{"id":{index}}}"#))
                    .collect::<Vec<String>>()
                    .join(",")
            ),
        ),
    ];

    for (index, (case, body)) in bodies.into_iter().enumerate() {
        let created = omni
            .create_dashboard(&create_input(
                &space_id,
                &format!("conforming-{index}"),
                &body,
            ))
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the body was refused: {error:?}"))
            .omni_dashboard;
        // The definition round-trips verbatim through the reader that answers from the
        // same store, so acceptance is backed by persisted state.
        let fetched = omni
            .get_dashboard(&space_id, &created.dashboard_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetOmniDashboard failed: {error:?}"))
            .omni_dashboard;
        assert_eq!(
            (created.body.as_str(), fetched.body.as_str()),
            (body.as_str(), body.as_str()),
            "case {case:?}: expected the body {body:?} back, got {:?} and {:?}",
            created.body,
            fetched.body
        );
    }
}

#[tokio::test]
async fn the_schema_refusal_carries_its_clauses_over_the_wire() {
    let omni = Omni::new();
    let space_id = omni.space("wire-schema", "dashboard-wire-schema").await;

    // One body violating two constraints: the message a client reads off the wire is
    // the same clause list the handler rendered.
    let (status, body) = omni
        .over_the_wire(
            "CreateOmniDashboard",
            CREATE_OMNI_DASHBOARD_INPUT_SCHEMA,
            &create_input(&space_id, "wire-schema", r#"{"widgets":[]}"#),
        )
        .await;
    let rendered = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 400,
        "the wire answered {status} rather than 400: {rendered:?}"
    );
    let expected = [
        clause("panels", "required"),
        clause(DOCUMENT_ROOT, "additionalProperties"),
    ]
    .join(CLAUSE_SEPARATOR);
    assert!(
        rendered.contains(&expected),
        "the wire refusal does not carry the message {expected:?}: {rendered:?}"
    );
}

#[tokio::test]
async fn list_omni_dashboards_pages_one_space_in_creation_order() {
    let omni = Omni::new();
    let space_id = omni.space("list", "dashboard-list").await;
    let other_space = omni.space("list-other", "dashboard-list-other").await;

    // Rows: the names created in this space, in creation order, plus one dashboard in
    // ANOTHER space that no page of this space may carry.
    let names = ["alpha", "alpine", "beta", "gamma", "delta"];
    let mut created: Vec<String> = Vec::new();
    for name in names {
        created.push(
            omni.create_dashboard(&create_input(&space_id, name, EMPTY_PANELS_BODY))
                .await
                .unwrap_or_else(|error| panic!("creating {name:?} failed: {error:?}"))
                .omni_dashboard
                .dashboard_id,
        );
    }
    let elsewhere = omni
        .create_dashboard(&create_input(&other_space, "alpha", EMPTY_PANELS_BODY))
        .await
        .expect("the other space's dashboard is created")
        .omni_dashboard
        .dashboard_id;

    // One page holding everything: the listing is the store's creation order, and the
    // summary projection carries every member of the dashboard but its definition.
    let page = omni
        .list_dashboards(&ListOmniDashboardsInput {
            space_id: space_id.clone(),
            ..Default::default()
        })
        .await
        .expect("the space lists");
    assert_eq!(
        (
            page.items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<&str>>(),
            page.next_token.clone(),
        ),
        (names.to_vec(), None),
        "expected the space's dashboards in creation order, got {:?}",
        page.items
    );
    for (index, item) in page.items.iter().enumerate() {
        let full = omni
            .get_dashboard(&space_id, &created[index])
            .await
            .expect("the dashboard reads back")
            .omni_dashboard;
        assert_eq!(
            (
                item.dashboard_id.as_str(),
                item.arn.as_str(),
                item.name.as_str(),
                item.created_by.as_str(),
                millis(item.created_at),
                millis(item.updated_at),
            ),
            (
                full.dashboard_id.as_str(),
                full.arn.as_str(),
                full.name.as_str(),
                full.created_by.as_str(),
                millis(full.created_at),
                millis(full.updated_at),
            ),
            "case {:?}: the summary disagrees with the dashboard {full:?}",
            item.name
        );
    }

    // Paging: every page size from 1 up to past the collection walks the SAME order and
    // yields every dashboard exactly once, following `nextToken` while it is present.
    for page_size in [1, 2, 4, 5, 100] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let page = omni
                .list_dashboards(&ListOmniDashboardsInput {
                    space_id: space_id.clone(),
                    max_results: Some(page_size),
                    next_token: token.clone(),
                    ..Default::default()
                })
                .await
                .unwrap_or_else(|error| panic!("case {page_size}: the page failed: {error:?}"));
            assert!(
                page.items.len() <= page_size as usize,
                "case {page_size}: a page carried {} items",
                page.items.len()
            );
            walked.extend(page.items.iter().map(|item| item.name.clone()));
            token = page.next_token;
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, names,
            "case {page_size}: walking the pages gave {walked:?}"
        );
    }

    // `namePrefix`'s model doc: "Filter to dashboards whose name starts with this
    // prefix." Rows span a prefix matching several names, one name, and none.
    for (prefix, expected) in [
        ("alp", vec!["alpha", "alpine"]),
        ("alpha", vec!["alpha"]),
        ("beta", vec!["beta"]),
        ("zeta", vec![]),
    ] {
        let page = omni
            .list_dashboards(&ListOmniDashboardsInput {
                space_id: space_id.clone(),
                name_prefix: Some(prefix.to_string()),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|error| panic!("case {prefix:?}: the filtered page failed: {error:?}"));
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.name.as_str())
                .collect::<Vec<&str>>(),
            expected,
            "case {prefix:?}: the filtered page is {:?}",
            page.items
        );
    }

    // A listing is scoped to the space it names: the other space lists its own
    // dashboard and nothing of this one's.
    let other = omni
        .list_dashboards(&ListOmniDashboardsInput {
            space_id: other_space.clone(),
            ..Default::default()
        })
        .await
        .expect("the other space lists");
    assert_eq!(
        other
            .items
            .iter()
            .map(|item| item.dashboard_id.as_str())
            .collect::<Vec<&str>>(),
        vec![elsewhere.as_str()],
        "the other space listed {:?}",
        other.items
    );

    // A space the store does not hold is a miss rather than an empty page, and a token
    // this service did not mint is a bad request.
    expect_modelled_error(
        "an unknown space",
        omni.list_dashboards(&ListOmniDashboardsInput {
            space_id: MISSING_SPACE_ID.to_string(),
            ..Default::default()
        })
        .await,
        404,
        "ResourceNotFoundException",
    );
    for token in ["not-a-token", "6e6f7065"] {
        expect_modelled_error(
            token,
            omni.list_dashboards(&ListOmniDashboardsInput {
                space_id: space_id.clone(),
                next_token: Some(token.to_string()),
                ..Default::default()
            })
            .await,
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn a_deleted_dashboard_is_gone_from_get_and_from_the_listing() {
    let omni = Omni::new();
    let space_id = omni.space("delete", "dashboard-delete").await;
    let other_space = omni.space("delete-other", "dashboard-delete-other").await;

    let mut ids: Vec<String> = Vec::new();
    for name in ["kept", "removed"] {
        ids.push(
            omni.create_dashboard(&create_input(&space_id, name, EMPTY_PANELS_BODY))
                .await
                .unwrap_or_else(|error| panic!("creating {name:?} failed: {error:?}"))
                .omni_dashboard
                .dashboard_id,
        );
    }
    let (kept, removed) = (ids[0].clone(), ids[1].clone());

    // A dashboard is addressed by its space AND its id, so a delete naming another
    // space removes nothing: the record is still readable.
    omni.delete_dashboard(&other_space, &removed)
        .await
        .expect("a delete under another space succeeds without removing anything");
    assert!(
        omni.get_dashboard(&space_id, &removed).await.is_ok(),
        "the delete under another space removed the dashboard"
    );

    omni.delete_dashboard(&space_id, &removed)
        .await
        .expect("the delete succeeds");
    expect_modelled_error(
        "a deleted dashboard",
        omni.get_dashboard(&space_id, &removed).await,
        404,
        "ResourceNotFoundException",
    );

    // @idempotent, and the model declares no not-found error for the delete: deleting
    // an id that is already gone, and one that never existed, both succeed.
    for (case, dashboard_id) in [
        ("an already deleted dashboard", removed.as_str()),
        ("a dashboard that never existed", MISSING_DASHBOARD_ID),
    ] {
        omni.delete_dashboard(&space_id, dashboard_id)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the delete failed: {error:?}"));
    }

    // The listing answers from the same store, so the removed dashboard is gone from it
    // and the one left behind still lists.
    let page = omni
        .list_dashboards(&ListOmniDashboardsInput {
            space_id: space_id.clone(),
            ..Default::default()
        })
        .await
        .expect("the space lists");
    assert_eq!(
        page.items
            .iter()
            .map(|item| item.dashboard_id.as_str())
            .collect::<Vec<&str>>(),
        vec![kept.as_str()],
        "the listing after the delete is {:?}",
        page.items
    );

    // The name the deleted dashboard held is free again, which is observable only
    // because the record really is gone from the store.
    omni.create_dashboard(&create_input(&space_id, "removed", EMPTY_PANELS_BODY))
        .await
        .expect("the freed name can identify a new dashboard");
}
