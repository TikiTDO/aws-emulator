// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for CloudWatchOmni's views.
//!
//! Every call goes through a real operation handler holding the SAME shared engine the
//! router hands every service, so a create is observable by the reader that runs next.
//! Failures are the operations' own modelled errors, so each assertion checks the wire
//! facts that are requirements: the HTTP status and the error code.
//!
//! The suite asserts the RULES rather than one recorded exchange:
//!
//! * a view IS the SQL query it is defined by, so `CreateView` admits a `definition` a
//!   SQL parser reads as ONE query — whatever form that query takes — and refuses one it
//!   does not with `ValidationException` (400) carrying the parser's own diagnosis of the
//!   submitted text, persisting nothing for the refusal (the diagnosis itself is
//!   asserted row by row in `view_definition_diagnosis`);
//! * a created view is persisted and `GetView` answers from that same store, reporting
//!   the definition, the description, the `USER` type, the ARN and the timestamps the
//!   create reported;
//! * a view's name is its identity within the account and region, so a second view under
//!   a live name is a `ConflictException` (409) and leaves the first one as it was;
//! * an unknown name resolves to nothing: `ResourceNotFoundException` (404);
//! * `ListViews` reports the views the store holds, in creation order, as summaries
//!   without their definitions, restricted to the ownership category the request names
//!   and paged at `maxResults` with a `nextToken` that resumes where the page stopped;
//! * `DeleteView` releases the view its name identifies, so the name resolves to nothing
//!   and is free again, and a name the store holds nothing under is a 404;
//! * `@idempotencyToken` replays return the original view rather than minting a second.
//!
//! Each assertion names its case and the actual value, so a failure identifies the row
//! of the table that broke.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use smithy_server_runtime::{
    EmbeddedEmulator, MultiServiceRouter, Operation, OperationContext, OperationError, Service,
};

use service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend as omni;

use omni::ops::create_view::{CreateView, CreateViewError};
use omni::ops::delete_view::{DeleteView, DeleteViewError};
use omni::ops::get_view::{GetView, GetViewError};
use omni::ops::list_views::{ListViews, ListViewsError};
use omni::ops::types::{
    CreateViewInput, CreateViewOutput, DeleteViewInput, DeleteViewOutput, GetViewInput,
    GetViewOutput, ListViewsInput, ListViewsOutput, CREATE_VIEW_INPUT_SCHEMA,
    GET_VIEW_INPUT_SCHEMA, LIST_VIEWS_INPUT_SCHEMA, VIEW_TYPE_VALUES,
};
use omni::state::{
    view_definition_rejection, INVALID_VIEW_DEFINITION, VIEW_ARN_RESOURCE_TYPE, VIEW_TYPE_USER,
};

/// The region the suite signs for; the handlers record the signed region on each
/// resource, so every ARN below is expected to carry it.
const REGION: &str = "us-west-2";

/// The access key id the suite signs with.
const ACCESS_KEY_ID: &str = "AKIDEMULATOR";

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (the router's routing key), the region the handlers record and `ACCESS_KEY_ID`.
fn signed_request_parts() -> http::request::Parts {
    let (parts, ()) = http::Request::builder()
        .method("POST")
        .uri("/service/CloudWatchOmniFrontend/operation/Unused")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={ACCESS_KEY_ID}/20260101/{REGION}/cloudwatch/aws4_request, \
                 SignedHeaders=host, Signature=emulator"
            ),
        )
        .body(())
        .expect("a well-formed request")
        .into_parts();
    parts
}

/// One emulator: one shared engine, the service mounted on a router, and the request
/// metadata a handler reads.
pub(crate) struct Omni {
    engine: Arc<engine::InMemoryEngine>,
    service: omni::EmulatedService<engine::InMemoryEngine>,
    router: MultiServiceRouter,
    parts: http::request::Parts,
}

impl Omni {
    pub(crate) fn new() -> Self {
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
            parts: signed_request_parts(),
        }
    }

    fn ctx(&self) -> OperationContext<'_> {
        OperationContext::new(
            &self.parts,
            self.service.definition().identity(),
            EmbeddedEmulator::from_router(self.router.clone()),
        )
    }

    pub(crate) async fn create_view(
        &self,
        input: &CreateViewInput,
    ) -> Result<CreateViewOutput, CreateViewError> {
        CreateView::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    pub(crate) async fn get_view(&self, name: &str) -> Result<GetViewOutput, GetViewError> {
        GetView::new(Arc::clone(&self.engine))
            .handle(
                GetViewInput {
                    name: name.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    pub(crate) async fn list_views(
        &self,
        input: &ListViewsInput,
    ) -> Result<ListViewsOutput, ListViewsError> {
        ListViews::new(Arc::clone(&self.engine))
            .handle(input.clone(), &self.ctx())
            .await
    }

    pub(crate) async fn delete_view(
        &self,
        name: &str,
    ) -> Result<DeleteViewOutput, DeleteViewError> {
        DeleteView::new(Arc::clone(&self.engine))
            .handle(
                DeleteViewInput {
                    name: name.to_string(),
                },
                &self.ctx(),
            )
            .await
    }

    /// Dispatch one rpcv2Cbor request over HTTP and return the status and body.
    pub(crate) async fn over_the_wire<I: SerializableStruct>(
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
pub(crate) fn expect_modelled_error<O: std::fmt::Debug, E: OperationError + std::fmt::Debug>(
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

/// The smallest view request the model admits, for `name` and `definition`.
pub(crate) fn create_input(name: &str, definition: &str) -> CreateViewInput {
    CreateViewInput {
        name: name.to_string(),
        definition: definition.to_string(),
        description: None,
        tags: None,
        client_token: None,
    }
}

/// Definitions a SQL parser reads as ONE query, so `CreateView` persists them.
///
/// The rows differ along the FORM of the query — projection, predicate, grouping,
/// set operation, common table expression, join and ordering, a leading comment, a
/// trailing semicolon — because the rule is about the definition being one query, not
/// about the text it is written as.
const ONE_QUERY: &[(&str, &str)] = &[
    ("select all", "SELECT * FROM logs"),
    (
        "projection and predicate",
        "SELECT service, message FROM logs WHERE level = 'ERROR'",
    ),
    (
        "aggregate and grouping",
        "SELECT service, count(*) AS errors FROM logs GROUP BY service HAVING count(*) > 10",
    ),
    (
        "set operation",
        "SELECT id FROM logs UNION SELECT id FROM spans",
    ),
    (
        "common table expression",
        "WITH errors AS (SELECT * FROM logs WHERE level = 'ERROR') SELECT count(*) FROM errors",
    ),
    (
        "join, ordering and a bound",
        "SELECT l.id FROM logs l JOIN spans s ON s.trace_id = l.trace_id ORDER BY l.id LIMIT 10",
    ),
    (
        "a leading comment",
        "-- the day's errors\nSELECT count(*) FROM logs WHERE level = 'ERROR'",
    ),
    ("a trailing semicolon", "SELECT 1 AS one;"),
    (
        "a nested query",
        "SELECT id FROM logs WHERE service IN (SELECT service FROM spans)",
    ),
];

/// Definitions a SQL parser does NOT read as one query, so `CreateView` refuses them.
///
/// The rows differ along WHY: the text is not SQL at all, it carries no statement, it is
/// a statement that is not a query, or it is more than one statement.
const NOT_ONE_QUERY: &[(&str, &str)] = &[
    ("prose", "the daily error count"),
    ("a JSON document", "{\"query\": \"SELECT 1\"}"),
    ("a truncated query", "SELECT * FROM"),
    ("an unclosed string", "SELECT * FROM logs WHERE level = 'ER"),
    ("an unbalanced parenthesis", "SELECT count(* FROM logs"),
    ("whitespace only", "   "),
    ("a comment only", "-- nothing but a comment"),
    ("a delete", "DELETE FROM logs WHERE level = 'ERROR'"),
    ("an insert", "INSERT INTO logs (id) VALUES (1)"),
    ("an update", "UPDATE logs SET level = 'INFO'"),
    ("a DDL statement", "CREATE TABLE logs (id INT)"),
    ("two queries", "SELECT 1; SELECT 2"),
    ("a query followed by a mutation", "SELECT * FROM logs; DROP TABLE logs"),
    ("three statements", "SELECT 1; SELECT 2; SELECT 3"),
];

/// A view name satisfying `ViewName`'s `^view\.[a-z0-9][a-z0-9_-]{0,250}$` with its
/// `@length(6..256)`, from `smithy_query(query="shape",shape="ViewName")`.
pub(crate) fn view_name(suffix: &str) -> String {
    format!("view.{suffix}")
}

#[tokio::test]
async fn a_definition_that_is_one_sql_query_is_persisted_and_read_back() {
    let omni = Omni::new();
    for (case, definition) in ONE_QUERY {
        let name = view_name(&case.replace(' ', "-").replace(',', ""));
        assert_eq!(
            view_definition_rejection(definition),
            None,
            "case {case:?}: the definition rule must admit {definition:?}"
        );
        let created = omni
            .create_view(&CreateViewInput {
                description: Some(format!("the {case} view")),
                ..create_input(&name, definition)
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));

        assert_eq!(
            created.definition, *definition,
            "case {case:?}: the created view must carry the definition it was given, got {:?}",
            created.definition
        );
        assert_eq!(
            created.name, name,
            "case {case:?}: the created view must carry the name it was given, got {:?}",
            created.name
        );
        assert_eq!(
            created.r#type, VIEW_TYPE_USER,
            "case {case:?}: a view created through the API is a {VIEW_TYPE_USER} view, got {:?}",
            created.r#type
        );
        assert!(
            VIEW_TYPE_VALUES.contains(&created.r#type.as_str()),
            "case {case:?}: the reported type must be a modelled ViewType, got {:?}",
            created.r#type
        );
        assert!(
            created.arn.starts_with(&format!(
                "arn:aws:cloudwatch:{REGION}:{}:{VIEW_ARN_RESOURCE_TYPE}/{name}/",
                engine::emulator_account_id()
            )),
            "case {case:?}: the ARN must name this region, account, view and view id, got {:?}",
            created.arn
        );
        assert_eq!(
            created.created_at, created.updated_at,
            "case {case:?}: a view nothing has updated was last updated when it was created, got {:?} and {:?}",
            created.created_at, created.updated_at
        );

        // The round trip: the reader answers from the store the create wrote.
        let read = omni
            .get_view(&name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: GetView failed: {error:?}"));
        assert_eq!(
            (
                read.name.as_str(),
                read.definition.as_str(),
                read.description.as_deref(),
                read.r#type.as_str(),
                read.arn.as_str(),
                read.created_at,
                read.updated_at
            ),
            (
                created.name.as_str(),
                created.definition.as_str(),
                created.description.as_deref(),
                created.r#type.as_str(),
                created.arn.as_str(),
                created.created_at,
                created.updated_at
            ),
            "case {case:?}: GetView must answer with the persisted view, got {read:?}"
        );

        // A read reports: reading twice reports the same view.
        let read_again = omni
            .get_view(&name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the second GetView failed: {error:?}"));
        assert_eq!(
            read_again.updated_at, read.updated_at,
            "case {case:?}: reading a view must not change it, got {:?} after {:?}",
            read_again.updated_at, read.updated_at
        );
    }
}

#[tokio::test]
async fn a_definition_that_is_not_one_sql_query_is_refused_and_persists_nothing() {
    let omni = Omni::new();
    for (index, (case, definition)) in NOT_ONE_QUERY.iter().enumerate() {
        let name = view_name(&format!("refused-{index}"));
        let result = omni.create_view(&create_input(&name, definition)).await;
        // The refusal carries the parser's own reading of the submitted text under the
        // fixed prefix, so a caller learns what is wrong with THIS definition from the
        // message alone.
        let rendered = format!("{result:?}");
        let reason = view_definition_rejection(definition)
            .unwrap_or_else(|| panic!("case {case:?}: the definition rule must refuse this text"));
        assert!(
            reason.starts_with(INVALID_VIEW_DEFINITION),
            "case {case:?}: expected the refusal to open with {INVALID_VIEW_DEFINITION:?}, got {reason:?}"
        );
        assert!(
            reason.len() > INVALID_VIEW_DEFINITION.len(),
            "case {case:?}: expected the parser's diagnosis after the prefix, got {reason:?}"
        );
        assert!(
            rendered.contains(&reason),
            "case {case:?}: expected the refusal to carry {reason:?}, got {rendered}"
        );
        expect_modelled_error(case, result, 400, "ValidationException");
        // Nothing was persisted for the refused request, so the name resolves to
        // nothing.
        expect_modelled_error(
            case,
            omni.get_view(&name).await,
            404,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn a_views_name_is_its_identity_within_the_account_and_region() {
    let omni = Omni::new();
    let name = view_name("errors-by-service");
    let original = omni
        .create_view(&create_input(&name, "SELECT service FROM logs"))
        .await
        .expect("the first view is created");

    for (case, definition) in [
        ("the same definition", "SELECT service FROM logs"),
        ("another definition", "SELECT count(*) FROM spans"),
    ] {
        expect_modelled_error(
            case,
            omni.create_view(&create_input(&name, definition)).await,
            409,
            "ConflictException",
        );
    }

    // The conflict left the first view as it was rather than replacing it.
    let read = omni.get_view(&name).await.expect("the first view is read");
    assert_eq!(
        (read.definition.as_str(), read.updated_at),
        (original.definition.as_str(), original.updated_at),
        "a refused duplicate must leave the live view unchanged, got {read:?}"
    );

    // Another name is another view, definition and all.
    let other = view_name("spans-by-service");
    let second = omni
        .create_view(&create_input(&other, "SELECT count(*) FROM spans"))
        .await
        .expect("a second name is a second view");
    assert_ne!(
        second.arn, original.arn,
        "two views must carry two ARNs, got {:?} twice",
        second.arn
    );
}

#[tokio::test]
async fn an_unknown_view_name_resolves_to_nothing() {
    let omni = Omni::new();
    for (case, name) in [
        ("never created", view_name("no-such-view")),
        ("the shortest admitted name", view_name("a")),
    ] {
        expect_modelled_error(
            case,
            omni.get_view(&name).await,
            404,
            "ResourceNotFoundException",
        );
    }
}

#[tokio::test]
async fn an_idempotency_token_replay_returns_the_original_view() {
    let omni = Omni::new();
    let name = view_name("token-replayed");
    let request = CreateViewInput {
        client_token: Some("a-client-token".to_string()),
        ..create_input(&name, "SELECT * FROM logs")
    };
    let original = omni
        .create_view(&request)
        .await
        .expect("the first call creates the view");

    for (case, replay) in [
        ("the same request", request.clone()),
        (
            "the same token with another definition",
            CreateViewInput {
                definition: "SELECT count(*) FROM spans".to_string(),
                ..request.clone()
            },
        ),
    ] {
        let replayed = omni
            .create_view(&replay)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the replay failed: {error:?}"));
        assert_eq!(
            (
                replayed.name.as_str(),
                replayed.definition.as_str(),
                replayed.arn.as_str(),
                replayed.created_at
            ),
            (
                original.name.as_str(),
                original.definition.as_str(),
                original.arn.as_str(),
                original.created_at
            ),
            "case {case:?}: a token replay must return the original view, got {replayed:?}"
        );
    }

    // Another token under another name is another view.
    let fresh_name = view_name("token-fresh");
    let fresh = omni
        .create_view(&CreateViewInput {
            client_token: Some("another-client-token".to_string()),
            ..create_input(&fresh_name, "SELECT * FROM logs")
        })
        .await
        .expect("another token creates another view");
    assert_ne!(
        fresh.arn, original.arn,
        "a different token must create a different view, got {:?} twice",
        fresh.arn
    );
}

#[tokio::test]
async fn the_view_lifecycle_over_the_rpcv2cbor_wire() {
    let omni = Omni::new();
    let name = view_name("over-the-wire");

    let (status, body) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&name, "SELECT count(*) FROM logs"),
        )
        .await;
    assert_eq!(status, 200, "CreateView over the wire: expected 200");
    let codec = CborCodec::default();
    let mut deserializer = codec.create_deserializer(&body);
    let created = CreateViewOutput::deserialize(&mut deserializer)
        .expect("the CreateView response deserializes");

    let (status, body) = omni
        .over_the_wire(
            "GetView",
            GET_VIEW_INPUT_SCHEMA,
            &GetViewInput { name: name.clone() },
        )
        .await;
    assert_eq!(status, 200, "GetView over the wire: expected 200");
    let mut deserializer = codec.create_deserializer(&body);
    let read =
        GetViewOutput::deserialize(&mut deserializer).expect("the GetView response deserializes");
    assert_eq!(
        (read.name.as_str(), read.definition.as_str()),
        (created.name.as_str(), created.definition.as_str()),
        "GetView over the wire must answer with the persisted view, got {read:?}"
    );

    // The refusal travels as the modelled 400 over the same wire.
    let (status, _) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&view_name("refused-over-the-wire"), "not a sql query"),
        )
        .await;
    assert_eq!(
        status, 400,
        "a definition that is not one SQL query must be refused with 400"
    );

    // And the duplicate name travels as the modelled 409.
    let (status, _) = omni
        .over_the_wire(
            "CreateView",
            CREATE_VIEW_INPUT_SCHEMA,
            &create_input(&name, "SELECT count(*) FROM logs"),
        )
        .await;
    assert_eq!(
        status, 409,
        "a second view under a live name must be refused with 409"
    );

    // The listing travels over the same wire and reports the view the create persisted.
    let (status, body) = omni
        .over_the_wire(
            "ListViews",
            LIST_VIEWS_INPUT_SCHEMA,
            &ListViewsInput::default(),
        )
        .await;
    assert_eq!(status, 200, "ListViews over the wire: expected 200");
    let mut deserializer = codec.create_deserializer(&body);
    let listed = ListViewsOutput::deserialize(&mut deserializer)
        .expect("the ListViews response deserializes");
    assert!(
        listed.items.iter().any(|item| item.name == name),
        "ListViews over the wire must report the created view, got {listed:?}"
    );
}

/// `ViewType`'s value that is not [`VIEW_TYPE_USER`], read off the modelled enum
/// (`VIEW_TYPE_VALUES`, generated from `ViewType`) rather than written out here: AWS
/// provisions views of that category, so this store holds none of them.
fn the_other_view_type() -> &'static str {
    VIEW_TYPE_VALUES
        .iter()
        .copied()
        .find(|value| *value != VIEW_TYPE_USER)
        .expect("ViewType declares a category other than USER")
}

#[tokio::test]
async fn the_listing_reports_the_views_the_store_holds_in_creation_order() {
    let omni = Omni::new();
    // Created in a known order, with a description on some of them, so the listing's
    // order and its member set are both observable.
    let mut created: Vec<String> = Vec::new();
    for (index, (case, definition)) in ONE_QUERY.iter().enumerate() {
        let name = view_name(&format!("listed-{index}"));
        omni.create_view(&CreateViewInput {
            description: (index % 2 == 0).then(|| format!("the {case} view")),
            ..create_input(&name, definition)
        })
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));
        created.push(name);
    }

    let listed = omni
        .list_views(&ListViewsInput::default())
        .await
        .expect("ListViews answers");
    assert_eq!(
        listed
            .items
            .iter()
            .map(|item| item.name.clone())
            .collect::<Vec<String>>(),
        created,
        "the listing must report every view in creation order, got {listed:?}"
    );
    assert_eq!(
        listed.next_token, None,
        "a listing that reported every view must carry no nextToken, got {:?}",
        listed.next_token
    );

    // A summary is the view's member set WITHOUT the definition, read off the same
    // record `GetView` answers from.
    for item in &listed.items {
        let read = omni
            .get_view(&item.name)
            .await
            .unwrap_or_else(|error| panic!("case {:?}: GetView failed: {error:?}", item.name));
        assert_eq!(
            (
                item.r#type.as_str(),
                item.description.as_deref(),
                item.created_at,
                item.updated_at
            ),
            (
                read.r#type.as_str(),
                read.description.as_deref(),
                read.created_at,
                read.updated_at
            ),
            "case {:?}: the summary and the view must agree, got {item:?}",
            item.name
        );
    }

    // Listing is reporting: the second listing answers exactly what the first did.
    let again = omni
        .list_views(&ListViewsInput::default())
        .await
        .expect("the second ListViews answers");
    assert_eq!(
        again
            .items
            .iter()
            .map(|item| (item.name.clone(), item.updated_at))
            .collect::<Vec<_>>(),
        listed
            .items
            .iter()
            .map(|item| (item.name.clone(), item.updated_at))
            .collect::<Vec<_>>(),
        "listing views must not change them, got {again:?}"
    );
}

#[tokio::test]
async fn the_listing_is_restricted_to_the_ownership_category_the_request_names() {
    let omni = Omni::new();
    let names = ["categorised-a", "categorised-b", "categorised-c"];
    for name in names {
        omni.create_view(&create_input(&view_name(name), "SELECT 1"))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: CreateView failed: {error:?}"));
    }

    for (case, requested, expected) in [
        ("no filter", None, names.len()),
        ("the created category", Some(VIEW_TYPE_USER), names.len()),
        ("the other category", Some(the_other_view_type()), 0),
    ] {
        let listed = omni
            .list_views(&ListViewsInput {
                r#type: requested.map(str::to_string),
                ..ListViewsInput::default()
            })
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListViews failed: {error:?}"));
        assert_eq!(
            listed.items.len(),
            expected,
            "case {case:?}: expected {expected} views for type {requested:?}, got {listed:?}"
        );
        for item in &listed.items {
            assert!(
                requested.is_none_or(|category| item.r#type == category),
                "case {case:?}: a filtered listing must report only {requested:?}, got {item:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_listing_pages_at_max_results_and_resumes_where_the_page_stopped() {
    let omni = Omni::new();
    let total = 7;
    let mut created: Vec<String> = Vec::new();
    for index in 0..total {
        let name = view_name(&format!("paged-{index}"));
        omni.create_view(&create_input(&name, "SELECT 1"))
            .await
            .unwrap_or_else(|error| panic!("case {name:?}: CreateView failed: {error:?}"));
        created.push(name);
    }

    // The rows span the page sizes around the collection: smaller than it, a divisor of
    // it, one short of it, exactly it, larger than it, and the modelled maximum.
    for page_size in [1, 2, 3, total - 1, total, total + 1, 100] {
        let case = format!("maxResults {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let page = omni
                .list_views(&ListViewsInput {
                    max_results: Some(page_size as i32),
                    next_token: token.clone(),
                    ..ListViewsInput::default()
                })
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: ListViews failed: {error:?}"));
            pages += 1;
            assert!(
                page.items.len() <= page_size,
                "case {case:?}: a page must hold at most {page_size} views, got {page:?}"
            );
            assert!(
                pages <= total + 1,
                "case {case:?}: paging must terminate, got {pages} pages"
            );
            seen.extend(page.items.iter().map(|item| item.name.clone()));
            match page.next_token {
                None => break,
                Some(next) => token = Some(next),
            }
        }
        assert_eq!(
            seen, created,
            "case {case:?}: the pages together must be every view in creation order"
        );
        assert_eq!(
            pages,
            total.div_ceil(page_size),
            "case {case:?}: expected {} pages for {total} views, got {pages}",
            total.div_ceil(page_size)
        );
    }

    // A token this service did not mint names no resume point, so it is a bad request
    // rather than an empty page.
    for (case, token) in [
        ("not hex", "not-a-token"),
        ("hex that decodes to other text", "6e6f70652d31"),
    ] {
        expect_modelled_error(
            case,
            omni.list_views(&ListViewsInput {
                next_token: Some(token.to_string()),
                ..ListViewsInput::default()
            })
            .await,
            400,
            "ValidationException",
        );
    }
}

#[tokio::test]
async fn a_deleted_view_resolves_to_nothing_and_frees_its_name() {
    let omni = Omni::new();
    // The rows differ along what else the view carries, because what a delete releases
    // is the view its NAME identifies: a bare view, one with a description, and one a
    // client token minted.
    let cases: Vec<(&str, CreateViewInput)> = vec![
        (
            "a bare view",
            create_input(&view_name("deleted-bare"), "SELECT 1"),
        ),
        (
            "a described view",
            CreateViewInput {
                description: Some("the described view".to_string()),
                ..create_input(&view_name("deleted-described"), "SELECT 2")
            },
        ),
        (
            "a view a token minted",
            CreateViewInput {
                client_token: Some("deleted-token".to_string()),
                ..create_input(&view_name("deleted-tokened"), "SELECT 3")
            },
        ),
    ];

    for (case, request) in &cases {
        let created = omni
            .create_view(request)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: CreateView failed: {error:?}"));
        omni.delete_view(&created.name)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: DeleteView failed: {error:?}"));

        // The view is gone from both readers: the one that resolves a name and the one
        // that enumerates the store.
        expect_modelled_error(
            case,
            omni.get_view(&created.name).await,
            404,
            "ResourceNotFoundException",
        );
        let listed = omni
            .list_views(&ListViewsInput::default())
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: ListViews failed: {error:?}"));
        assert!(
            !listed.items.iter().any(|item| item.name == created.name),
            "case {case:?}: a deleted view must not be listed, got {listed:?}"
        );

        // Deleting it again is a miss: the name holds no view now.
        expect_modelled_error(
            case,
            omni.delete_view(&created.name).await,
            404,
            "ResourceNotFoundException",
        );

        // The name is free again, token and all: the same request creates a new view
        // rather than resurrecting the deleted one.
        let recreated = omni
            .create_view(request)
            .await
            .unwrap_or_else(|error| panic!("case {case:?}: the recreate failed: {error:?}"));
        assert_eq!(
            recreated.name, created.name,
            "case {case:?}: the freed name must be usable again, got {:?}",
            recreated.name
        );
        assert_eq!(
            omni.get_view(&created.name)
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: GetView failed: {error:?}"))
                .definition,
            recreated.definition,
            "case {case:?}: the recreated view must be the one the store now holds"
        );
    }

    // A name no view was ever created under is a miss too.
    expect_modelled_error(
        "never created",
        omni.delete_view(&view_name("never-created")).await,
        404,
        "ResourceNotFoundException",
    );
}
