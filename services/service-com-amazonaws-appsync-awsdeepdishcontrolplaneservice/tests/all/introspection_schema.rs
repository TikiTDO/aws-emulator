// SPDX-License-Identifier: CC0-1.0

//! The introspection schema of a GraphQL API, end to end over HTTP:
//! `GetIntrospectionSchema` and the `StartSchemaCreation`/`CreateType` calls that put a
//! type system on the API for it to report.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. the call reports the type system the API HOLDS, in the representation the `format`
//!    it names chooses: `SDL` answers the GraphQL schema language the schema's types are
//!    defined with — root operation types included where the schema binds them — and
//!    `JSON` answers that same type system as GraphQL introspection, every type with the
//!    kind its keyword declares and every built-in scalar the schema references beside
//!    them. Asserted over a table of documents spanning every kind of type SDL declares,
//!    a schema whose root operation types carry names of its own, a schema with no
//!    mutation root and a document that defines a directive;
//! 2. the SDL the call answers is itself a schema document: creating it on another API
//!    installs the same type system, which reports the same introspection schema back —
//!    so the two representations and the stored types cannot drift apart;
//! 3. the type system is the CURRENT one rather than the document that was created: a
//!    type a later `CreateType` added is reported too;
//! 4. `includeDirectives` chooses whether the directives the schema defines are reported,
//!    in either representation;
//! 5. an API that holds no type system has no schema to introspect — none ever created,
//!    a creation that FAILED and a creation still in progress alike — and the call is
//!    refused 400 `GraphQLSchemaException` carrying the parser's diagnostic for the
//!    document the API holds;
//! 6. an `apiId` that cannot name an API is a 400 `BadRequestException` and one that
//!    names no API this account holds a 404 `NotFoundException`, whichever format is
//!    asked for.
//!
//! The harness (`router`, `call`, `create_api`) is the one the schema-creation suite
//! already carries, so every assertion goes through the router the production emulator
//! mounts: routing, restJson1 (de)serialization, the handlers and the shared engine
//! together. The schema itself is an `@httpPayload` blob rather than JSON, so this suite
//! adds a call that keeps the RAW response body.

use std::time::Duration;

use base64::Engine as _;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::source_api_association::{call, create_api, router, associate_path, ACCOUNT, REGION};

/// How long a case that must observe the far side of the scheduled creation waits: the
/// service's own span plus slack. Only the TEST waits — the handler stores the due
/// instant.
const PAST_THE_CREATION: Duration = Duration::from_millis(250);

/// Every value of the `OutputType` enum — the whole set the required `format` member can
/// take.
///
/// Source: `smithy_query(query="shape", shape="OutputType")`.
const OUTPUT_TYPES: &[&str] = &["SDL", "JSON"];

/// One answer to `GetIntrospectionSchema`: the status, the error shape and the RAW
/// response body, because the schema is an `@httpPayload` blob rather than a JSON
/// document.
#[derive(Debug, Clone)]
struct SchemaAnswer {
    status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    error_type: String,
    payload: Vec<u8>,
}

impl SchemaAnswer {
    /// The payload as the text the schema was rendered as.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.payload).into_owned()
    }

    /// The payload as the introspection document it carries.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.payload).unwrap_or(Value::Null)
    }

    /// The `message` of the refusal the payload carries.
    fn message(&self) -> String {
        let body = self.json();
        body["message"]
            .as_str()
            .or_else(|| body["Message"].as_str())
            .unwrap_or_default()
            .to_string()
    }
}

/// Read the introspection schema of `api_id`, with `query` as the request's query string.
async fn get_schema(router: &MultiServiceRouter, api_id: &str, query: &str) -> SchemaAnswer {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!(
            "https://appsync.{REGION}.amazonaws.com/v1/apis/{api_id}/schema?{query}"
        ))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    SchemaAnswer {
        status,
        error_type,
        payload: response.into_body().to_vec(),
    }
}

/// Read the introspection schema of `api_id` in `format`, asking for the schema's
/// directives.
async fn schema_in(router: &MultiServiceRouter, api_id: &str, format: &str) -> SchemaAnswer {
    get_schema(
        router,
        api_id,
        &format!("format={format}&includeDirectives=true"),
    )
    .await
}

/// Start the creation of `definition` on `api_id`.
///
/// The definition is a blob, which restJson1 carries base64-encoded in the JSON body.
async fn start_creation(router: &MultiServiceRouter, api_id: &str, definition: &str) -> u16 {
    let encoded = base64::engine::general_purpose::STANDARD.encode(definition.as_bytes());
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": encoded})),
    )
    .await
    .status
}

/// The `SchemaStatus` the creation of `api_id` is in.
async fn creation_status(router: &MultiServiceRouter, api_id: &str) -> String {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/schemacreation"),
        None,
    )
    .await
    .body["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// One row of the schema table: a document, and what the type system it defines must
/// report.
#[derive(Debug)]
struct Case {
    /// The row's label, used both in the API name and in every assertion message.
    label: &'static str,
    /// The schema document the creation carries.
    document: &'static str,
    /// Every type the document declares, with the `__TypeKind` its keyword declares.
    types: &'static [(&'static str, &'static str)],
    /// The type the schema's query root resolves to, or `None` for a schema that has no
    /// query root.
    query_root: Option<&'static str>,
    /// The type the schema's mutation root resolves to.
    mutation_root: Option<&'static str>,
    /// The built-in scalars the type system references, which introspection reports
    /// beside the types the document defines.
    built_in_scalars: &'static [&'static str],
    /// The directives the document defines.
    directives: &'static [&'static str],
}

/// The schema documents the rules above are asserted over: one kind of type at a time up
/// to a document carrying every kind SDL declares, a schema that binds root operation
/// types of its own, and a document that defines a directive.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "query-only",
            document: "type Query {\n  hello: String\n}\n",
            types: &[("Query", "OBJECT")],
            query_root: Some("Query"),
            mutation_root: None,
            built_in_scalars: &["String"],
            directives: &[],
        },
        Case {
            label: "every-kind",
            document: concat!(
                "schema {\n  query: Query\n  mutation: Mutation\n}\n\n",
                "type Query {\n  node(id: ID!): Node\n  posts: [Post]\n}\n\n",
                "type Mutation {\n  addPost(input: PostInput!): Post\n}\n\n",
                "interface Node {\n  id: ID!\n}\n\n",
                "type Post implements Node {\n  id: ID!\n  title: String\n",
                "  status: Status\n  price: Money\n  author: Author\n}\n\n",
                "union Author = Person | Organisation\n\n",
                "type Person {\n  id: ID!\n  name: String\n}\n\n",
                "type Organisation {\n  id: ID!\n  name: String\n}\n\n",
                "enum Status {\n  DRAFT\n  PUBLISHED\n}\n\n",
                "input PostInput {\n  title: String!\n  status: Status\n}\n\n",
                "scalar Money\n",
            ),
            types: &[
                ("Query", "OBJECT"),
                ("Mutation", "OBJECT"),
                ("Node", "INTERFACE"),
                ("Post", "OBJECT"),
                ("Author", "UNION"),
                ("Person", "OBJECT"),
                ("Organisation", "OBJECT"),
                ("Status", "ENUM"),
                ("PostInput", "INPUT_OBJECT"),
                ("Money", "SCALAR"),
            ],
            query_root: Some("Query"),
            mutation_root: Some("Mutation"),
            built_in_scalars: &["ID", "String"],
            directives: &[],
        },
        Case {
            label: "own-root-names",
            document: concat!(
                "schema {\n  query: RootQuery\n  mutation: RootMutation\n}\n\n",
                "type RootQuery {\n  ping: String\n}\n\n",
                "type RootMutation {\n  pong(times: Int = 1): Int\n}\n",
            ),
            types: &[("RootQuery", "OBJECT"), ("RootMutation", "OBJECT")],
            query_root: Some("RootQuery"),
            mutation_root: Some("RootMutation"),
            built_in_scalars: &["Int", "String"],
            directives: &[],
        },
        Case {
            label: "described-and-deprecated",
            document: concat!(
                "\"The queries this API serves.\"\n",
                "type Query {\n  \"The greeting.\"\n  hello: String\n",
                "  legacy: String @deprecated(reason: \"use hello\")\n}\n",
            ),
            types: &[("Query", "OBJECT")],
            query_root: Some("Query"),
            mutation_root: None,
            built_in_scalars: &["String"],
            directives: &[],
        },
        Case {
            label: "defines-a-directive",
            document: concat!(
                "directive @auth(level: String! = \"owner\") repeatable on FIELD_DEFINITION\n\n",
                "type Query {\n  secret: String\n}\n",
            ),
            types: &[("Query", "OBJECT")],
            query_root: Some("Query"),
            mutation_root: None,
            built_in_scalars: &["String"],
            directives: &["auth"],
        },
    ]
}

/// Assert `answer` is the SDL of `case`'s type system.
fn assert_sdl(case: &Case, answer: &SchemaAnswer) {
    assert_eq!(
        answer.status, 200,
        "case {:?} SDL: expected 200, got {} {:?}",
        case.label,
        answer.status,
        answer.text()
    );
    let sdl = answer.text();
    for (name, kind) in case.types {
        let keyword = match *kind {
            "OBJECT" => format!("type {name}"),
            "INTERFACE" => format!("interface {name}"),
            "UNION" => format!("union {name}"),
            "ENUM" => format!("enum {name}"),
            "INPUT_OBJECT" => format!("input {name}"),
            _ => format!("scalar {name}"),
        };
        assert!(
            sdl.contains(&keyword),
            "case {:?} SDL: expected the definition {keyword:?}, got {sdl:?}",
            case.label
        );
    }
    // A schema that binds root operation types of its own says so in its SDL; one whose
    // roots carry GraphQL's default names needs no `schema` definition to be read back.
    if case.document.contains("schema {") {
        assert!(
            sdl.contains("schema {"),
            "case {:?} SDL: expected the schema's root operation types, got {sdl:?}",
            case.label
        );
    }
    for directive in case.directives {
        assert!(
            sdl.contains(&format!("directive @{directive}")),
            "case {:?} SDL: expected the directive {directive:?} the schema defines, got {sdl:?}",
            case.label
        );
    }
}

/// Assert `answer` is the introspection JSON of `case`'s type system.
fn assert_json(case: &Case, answer: &SchemaAnswer) {
    assert_eq!(
        answer.status, 200,
        "case {:?} JSON: expected 200, got {} {:?}",
        case.label,
        answer.status,
        answer.text()
    );
    let document = answer.json();
    let schema = &document["data"]["__schema"];
    assert_eq!(
        schema["queryType"]["name"].as_str(),
        case.query_root,
        "case {:?} JSON: expected the query root {:?}, got {:?}",
        case.label,
        case.query_root,
        schema["queryType"]
    );
    assert_eq!(
        schema["mutationType"]["name"].as_str(),
        case.mutation_root,
        "case {:?} JSON: expected the mutation root {:?}, got {:?}",
        case.label,
        case.mutation_root,
        schema["mutationType"]
    );
    let types = schema["types"].as_array().cloned().unwrap_or_default();
    for (name, kind) in case.types {
        let reported = types
            .iter()
            .find(|entry| entry["name"].as_str() == Some(name));
        assert_eq!(
            reported.map(|entry| entry["kind"].clone()),
            Some(Value::String((*kind).to_string())),
            "case {:?} JSON: expected the type {name:?} as a {kind}, got {reported:?}",
            case.label
        );
    }
    for scalar in case.built_in_scalars {
        let reported = types
            .iter()
            .find(|entry| entry["name"].as_str() == Some(scalar));
        assert_eq!(
            reported.map(|entry| entry["kind"].clone()),
            Some(Value::String("SCALAR".to_string())),
            "case {:?} JSON: expected the built-in scalar {scalar:?} the schema \
             references, got {reported:?}",
            case.label
        );
    }
    let reported: Vec<&str> = schema["directives"]
        .as_array()
        .map(|directives| {
            directives
                .iter()
                .filter_map(|directive| directive["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        reported, case.directives,
        "case {:?} JSON: expected the directives {:?} the schema defines, got {:?}",
        case.label, case.directives, schema["directives"]
    );
}

#[tokio::test]
async fn the_introspection_schema_is_the_type_system_the_api_holds() {
    let router = router();
    let cases = cases();

    // Every creation is started first and the clock is let run once, so the suite waits
    // one span however many rows the table holds.
    let mut apis = Vec::new();
    for case in &cases {
        let api = create_api(&router, case.label, state::GRAPHQL_API_TYPE).await;
        let started = start_creation(&router, &api.id, case.document).await;
        assert_eq!(
            started, 200,
            "case {:?}: expected StartSchemaCreation 200, got {started}",
            case.label
        );
        apis.push(api.id);
    }
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    let mut rendered = Vec::new();
    for (case, api_id) in cases.iter().zip(&apis) {
        let status = creation_status(&router, api_id).await;
        assert_eq!(
            status,
            state::SCHEMA_SUCCESS,
            "case {:?}: expected the creation to have been performed, got {status:?}",
            case.label
        );
        let sdl = schema_in(&router, api_id, "SDL").await;
        assert_sdl(case, &sdl);
        assert_json(case, &schema_in(&router, api_id, "JSON").await);
        rendered.push(sdl.text());
    }

    // The SDL the call answers is a schema document: creating it on another API installs
    // the same type system, which reports the same schema back.
    let mut copies = Vec::new();
    for (case, sdl) in cases.iter().zip(&rendered) {
        let api = create_api(&router, &format!("{}-copy", case.label), state::GRAPHQL_API_TYPE).await;
        let started = start_creation(&router, &api.id, sdl).await;
        assert_eq!(
            started, 200,
            "case {:?} round trip: expected StartSchemaCreation 200, got {started}",
            case.label
        );
        copies.push(api.id);
    }
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    for ((case, sdl), api_id) in cases.iter().zip(&rendered).zip(&copies) {
        let status = creation_status(&router, api_id).await;
        assert_eq!(
            status,
            state::SCHEMA_SUCCESS,
            "case {:?} round trip: expected the answered SDL to be a schema document, \
             got {status:?}",
            case.label
        );
        let answer = schema_in(&router, api_id, "SDL").await;
        assert_sdl(case, &answer);
        assert_eq!(
            &answer.text(),
            sdl,
            "case {:?} round trip: expected the same schema back, got {:?}",
            case.label,
            answer.text()
        );
        assert_json(case, &schema_in(&router, api_id, "JSON").await);
    }
}

#[tokio::test]
async fn the_schema_reported_is_the_api_s_current_type_system() {
    let router = router();
    let api = create_api(&router, "current-type-system", state::GRAPHQL_API_TYPE).await;
    let started = start_creation(&router, &api.id, "type Query {\n  hello: String\n}\n").await;
    assert_eq!(
        started, 200,
        "case current type system: expected StartSchemaCreation 200, got {started}"
    );
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    // A type created on its own is part of the API's type system too, so the schema
    // reports it — the answer is rendered from the types the API HOLDS, not from the
    // document that was created.
    let created = call(
        &router,
        "POST",
        &format!("/v1/apis/{}/types", api.id),
        Some(json!({
            "definition": "type Extra {\n  count: Int\n}\n",
            "format": state::SDL_FORMAT,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case current type system: expected CreateType 200, got {} {:?}",
        created.status, created.body
    );

    let sdl = schema_in(&router, &api.id, "SDL").await;
    assert!(
        sdl.text().contains("type Extra"),
        "case current type system: expected the created type in the SDL, got {:?}",
        sdl.text()
    );
    let reported = schema_in(&router, &api.id, "JSON").await.json();
    let names: Vec<String> = reported["data"]["__schema"]["types"]
        .as_array()
        .map(|types| {
            types
                .iter()
                .filter_map(|entry| entry["name"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        names.contains(&"Extra".to_string()) && names.contains(&"Query".to_string()),
        "case current type system: expected both types in the introspection JSON, got {names:?}"
    );
}

#[tokio::test]
async fn directives_are_reported_only_when_the_call_asks_for_them() {
    let router = router();
    let api = create_api(&router, "directive-flag", state::GRAPHQL_API_TYPE).await;
    let document = concat!(
        "directive @auth(level: String! = \"owner\") repeatable on FIELD_DEFINITION\n\n",
        "type Query {\n  secret: String\n}\n",
    );
    let started = start_creation(&router, &api.id, document).await;
    assert_eq!(
        started, 200,
        "case directive flag: expected StartSchemaCreation 200, got {started}"
    );
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    // The flag is read the same way whichever representation is asked for, and a request
    // that names none asks for no directives.
    for format in OUTPUT_TYPES {
        for (flag, wanted) in [
            ("&includeDirectives=true", true),
            ("&includeDirectives=false", false),
            ("", false),
        ] {
            let answer = get_schema(&router, &api.id, &format!("format={format}{flag}")).await;
            assert_eq!(
                answer.status, 200,
                "case directive flag {format} {flag:?}: expected 200, got {} {:?}",
                answer.status,
                answer.text()
            );
            let reports = if *format == "JSON" {
                answer.json()["data"]["__schema"]["directives"]
                    .as_array()
                    .is_some_and(|directives| {
                        directives
                            .iter()
                            .any(|directive| directive["name"].as_str() == Some("auth"))
                    })
            } else {
                answer.text().contains("directive @auth")
            };
            assert_eq!(
                reports, wanted,
                "case directive flag {format} {flag:?}: expected the directive reported \
                 {wanted}, got {:?}",
                answer.text()
            );
            // The schema's own type is reported whatever the flag says.
            assert!(
                answer.text().contains("Query"),
                "case directive flag {format} {flag:?}: expected the schema's types, got {:?}",
                answer.text()
            );
        }
    }

    // The directive is reported as GraphQL introspection defines a `__Directive`: what
    // the definition declares, nothing of the request.
    let reported = schema_in(&router, &api.id, "JSON").await.json();
    let directive = reported["data"]["__schema"]["directives"][0].clone();
    assert_eq!(
        (
            directive["name"].as_str(),
            directive["isRepeatable"].as_bool(),
            directive["locations"][0].as_str(),
            directive["args"][0]["name"].as_str(),
            directive["args"][0]["defaultValue"].as_str(),
        ),
        (
            Some("auth"),
            Some(true),
            Some("FIELD_DEFINITION"),
            Some("level"),
            Some("\"owner\""),
        ),
        "case directive flag: expected the directive the document defines, got {directive:?}"
    );
}

/// One API that holds no type system, and how it came to hold none.
#[derive(Debug)]
struct NoSchema {
    label: &'static str,
    /// The document a creation carried, or `None` for an API no creation ever ran for.
    document: Option<&'static str>,
}

/// The diagnostic the refusal carries for an API that holds no schema: the parse of the
/// EMPTY document such a call reads as its schema, which stops at that document's origin.
///
/// Written out here rather than read off the service's own code, so a change of wording
/// is caught rather than followed.
const NO_SCHEMA_MESSAGE: &str = "errors=[InvalidSyntaxError{ message=Invalid Syntax ,locations=\
                                 [SourceLocation{line=1, column=0}]}]";

#[tokio::test]
async fn an_api_that_holds_no_schema_has_no_introspection_schema() {
    let router = router();
    let rows = [
        // However the API came to hold no type system — no creation ever run, a creation
        // that failed on a document that parses, one that failed on a document that does
        // not, one that failed on a type system that does not resolve — it holds the same
        // nothing, so the same diagnostic is owed.
        NoSchema {
            label: "no creation ever ran",
            document: None,
        },
        NoSchema {
            label: "the document declares no type",
            document: Some("query Everything {\n  hello\n}\n"),
        },
        NoSchema {
            label: "the document does not parse",
            document: Some("type Query {\n  hello: String\n"),
        },
        NoSchema {
            label: "the document references a type nothing defines",
            document: Some("type Query {\n  post: Post\n}\n"),
        },
    ];

    let mut apis = Vec::new();
    for row in &rows {
        let api = create_api(&router, row.label, state::GRAPHQL_API_TYPE).await;
        if let Some(document) = row.document {
            let started = start_creation(&router, &api.id, document).await;
            assert_eq!(
                started, 200,
                "case {:?}: expected StartSchemaCreation 200, got {started}",
                row.label
            );
        }
        apis.push(api.id);
    }
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    for (row, api_id) in rows.iter().zip(&apis) {
        for format in OUTPUT_TYPES {
            let answer = schema_in(&router, api_id, format).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str()),
                (400, "GraphQLSchemaException"),
                "case {:?} {format}: expected 400 GraphQLSchemaException, got {} {:?} {:?}",
                row.label,
                answer.status,
                answer.error_type,
                answer.text()
            );
            let message = answer.message();
            assert_eq!(
                message, NO_SCHEMA_MESSAGE,
                "case {:?} {format}: expected the diagnostic {NO_SCHEMA_MESSAGE:?}, got \
                 {message:?}",
                row.label
            );
        }
    }

    // A creation the clock has not performed yet has installed no type system either, so
    // the API still holds no schema while it is in progress.
    let api = create_api(&router, "creation in progress", state::GRAPHQL_API_TYPE).await;
    let started = start_creation(&router, &api.id, "type Query {\n  hello: String\n}\n").await;
    assert_eq!(
        started, 200,
        "case creation in progress: expected StartSchemaCreation 200, got {started}"
    );
    let answer = schema_in(&router, &api.id, "SDL").await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "GraphQLSchemaException"),
        "case creation in progress: expected 400 GraphQLSchemaException, got {} {:?} {:?}",
        answer.status,
        answer.error_type,
        answer.text()
    );

    // Once the clock has performed it, the identical call answers the created schema.
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;
    let answer = schema_in(&router, &api.id, "SDL").await;
    assert_eq!(
        answer.status, 200,
        "case creation in progress: expected 200 once the creation was performed, got {} {:?}",
        answer.status,
        answer.text()
    );
    assert!(
        answer.text().contains("type Query"),
        "case creation in progress: expected the created schema, got {:?}",
        answer.text()
    );
}

#[tokio::test]
async fn a_merged_api_reports_the_schemas_its_source_apis_were_merged_from() {
    let router = router();
    let source = create_api(&router, "merge-source", state::GRAPHQL_API_TYPE).await;
    let other = create_api(&router, "merge-source-other", state::GRAPHQL_API_TYPE).await;
    let merged = create_api(&router, "merge-target", state::MERGED_API_TYPE).await;

    let started = start_creation(
        &router,
        &source.id,
        "type Query {\n  hello: String\n}\n\ntype Widget {\n  id: ID!\n}\n",
    )
    .await;
    assert_eq!(
        started, 200,
        "case merge: expected StartSchemaCreation 200 on the source API, got {started}"
    );
    let started = start_creation(&router, &other.id, "type Gadget {\n  id: ID!\n}\n").await;
    assert_eq!(
        started, 200,
        "case merge: expected StartSchemaCreation 200 on the second source API, got {started}"
    );

    // Associating a source API SCHEDULES the merge; both associations are created while
    // the creations above are still in progress, so the merge cannot carry a schema yet.
    for (label, api_id) in [("first", &source.id), ("second", &other.id)] {
        let answer = call(
            &router,
            "POST",
            &associate_path(api_id),
            Some(json!({"mergedApiIdentifier": merged.id})),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case merge {label}: expected AssociateMergedGraphqlApi 200, got {} {:?}",
            answer.status, answer.body
        );
    }

    // The merge is due one merge span after the association, which is BEFORE the source
    // API's own creation: it carries that creation's types, so it is held until the
    // creation is due and the Merged API still has no schema in between.
    tokio::time::sleep(state::MERGE_SPAN + PAST_THE_CREATION).await;
    let answer = schema_in(&router, &merged.id, "SDL").await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "GraphQLSchemaException"),
        "case merge: expected the Merged API to hold no schema until the source API's \
         creation is due, got {} {:?} {:?}",
        answer.status,
        answer.error_type,
        answer.text()
    );

    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    // Once both merges have been performed, the Merged API's schema is the schemas of
    // its source APIs: every type they hold, reported under the Merged API's own path.
    let answer = schema_in(&router, &merged.id, "SDL").await;
    assert_eq!(
        answer.status, 200,
        "case merge: expected the merged schema, got {} {:?}",
        answer.status,
        answer.text()
    );
    for definition in ["type Query", "type Widget", "type Gadget"] {
        assert!(
            answer.text().contains(definition),
            "case merge: expected the merged schema to carry {definition:?}, got {:?}",
            answer.text()
        );
    }
    let reported = schema_in(&router, &merged.id, "JSON").await.json();
    assert_eq!(
        reported["data"]["__schema"]["queryType"]["name"].as_str(),
        Some("Query"),
        "case merge: expected the merged schema's query root, got {:?}",
        reported["data"]["__schema"]["queryType"]
    );

    // A merged type is a type OF the Merged API: addressed by its own name there and
    // named under the Merged API's path, so a read of it agrees with the schema.
    let answer = call(
        &router,
        "GET",
        &format!(
            "/v1/apis/{}/types/Widget?format={}",
            merged.id,
            state::SDL_FORMAT
        ),
        None,
    )
    .await;
    assert_eq!(
        (
            answer.status,
            answer.body["type"]["arn"].as_str().unwrap_or_default()
        ),
        (
            200,
            format!(
                "arn:aws:appsync:{REGION}:{ACCOUNT}:apis/{}/types/Widget",
                merged.id
            )
            .as_str()
        ),
        "case merge: expected the merged type under the Merged API's own path, got {} {:?}",
        answer.status,
        answer.body
    );
}

#[tokio::test]
async fn a_format_outside_the_value_set_is_refused_naming_the_value() {
    let router = router();
    let api = create_api(&router, "format-refusal", state::GRAPHQL_API_TYPE).await;

    // Values outside the enumerated set of either format member: a format name the
    // service does not report in, and the right value in the wrong case.
    for value in ["YAML", "GRAPHQL", "sdl"] {
        let answer = get_schema(&router, &api.id, &format!("format={value}")).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str(), answer.message()),
            (
                400,
                "BadRequestException",
                format!("Invalid format {value} given")
            ),
            "case format {value:?}: expected 400 BadRequestException naming the value, \
             got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.text()
        );

        // The same refusal wherever a report's format is named: a type is reported in a
        // format out of the same two-value set.
        let answer = call(
            &router,
            "GET",
            &format!("/v1/apis/{}/types/Query?format={value}", api.id),
            None,
        )
        .await;
        assert_eq!(
            (
                answer.status,
                answer.error_type.as_str(),
                answer.body["message"].as_str()
            ),
            (
                400,
                "BadRequestException",
                Some(format!("Invalid format {value} given").as_str())
            ),
            "case GetType format {value:?}: expected 400 BadRequestException naming the \
             value, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.body
        );
    }

    // Every value the set holds is accepted, so the refusal is about the value set and
    // not about the member being read at all.
    for format in OUTPUT_TYPES {
        let answer = schema_in(&router, &api.id, format).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "GraphQLSchemaException"),
            "case format {format:?}: expected the request to be read and answered from \
             the API's schema, got {} {:?} {:?}",
            answer.status,
            answer.error_type,
            answer.text()
        );
    }
}

#[tokio::test]
async fn an_api_nothing_can_name_has_no_introspection_schema() {
    let router = router();
    // An id longer than an AppSync API id names no API at all; one of the right length
    // that this account does not hold names a missing resource.
    let malformed = "a".repeat(state::API_ID_LEN + 1);
    let unknown = "abcdefghijklmnopqrstuvwxyz";
    let rows = [
        (
            "longer than an api id",
            malformed.as_str(),
            400_u16,
            "BadRequestException",
            state::invalid_api_id_message(&malformed),
        ),
        (
            "no api this account holds",
            unknown,
            404,
            "NotFoundException",
            state::API_NOT_FOUND_MESSAGE.to_string(),
        ),
    ];

    for (label, api_id, status, error_type, message) in rows {
        for format in OUTPUT_TYPES {
            let answer = schema_in(&router, api_id, format).await;
            assert_eq!(
                (answer.status, answer.error_type.as_str(), answer.message()),
                (status, error_type, message.clone()),
                "case {label:?} {format}: expected {status} {error_type} {message:?}, got {} {:?} {:?}",
                answer.status,
                answer.error_type,
                answer.text()
            );
        }
    }
}
