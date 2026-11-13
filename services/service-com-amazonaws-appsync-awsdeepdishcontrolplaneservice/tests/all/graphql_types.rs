// SPDX-License-Identifier: CC0-1.0

//! The GraphQL types of a GraphQL API, end to end over HTTP: `CreateType` and the
//! `GetType` that reads one back.
//!
//! The rules asserted here are stated over CLASSES of input rather than over the one
//! request that exposed the defect:
//!
//! 1. creating a type parses the SDL definition it carries and persists the ONE type
//!    that definition declares under its GraphQL API, named by the name the definition
//!    declares, described by the definition's own description, with its ARN named under
//!    the API's own path — and `GetType` answers the same type. Asserted over a table
//!    holding every kind of type definition GraphQL's SDL grammar declares;
//! 2. the `format` a call names chooses the REPRESENTATION the reported definition
//!    carries — the type's SDL, or its introspection JSON — so either representation of
//!    a stored type is readable whichever format created it; a READ answers the service's
//!    own rendering of the stored definition (the layout itself is pinned in
//!    `type_sdl_rendering.rs`), while the create answers the definition the request
//!    stated;
//! 3. that JSON is the type's own introspection document, stating what the definition
//!    states: each type reference by the name it names, or by the wrapper it writes
//!    around one, with every member a type has no value for left OUT of the document —
//!    and, for an interface, the object types the API holds that implement it when it is
//!    READ;
//! 4. a definition GraphQL cannot parse, or one that declares no type or several, is a
//!    400 `BadRequestException` and persists nothing;
//! 5. the declared name identifies the type within its API: a second create under the
//!    same name is refused and leaves the stored one untouched, while the same name
//!    under a different API is a different type;
//! 6. an `apiId` that cannot name an API is a 400 `BadRequestException`, one that names
//!    no API this account holds a 404 `NotFoundException`, and a type name the API does
//!    not hold a 404.
//!
//! Every assertion goes through the router the production emulator mounts, so these
//! exercise routing, restJson1 (de)serialization, the handlers and the shared engine
//! together.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

// The router, the region it signs for and the API-creating helper are the crate's
// shared test harness, reused rather than copied.
use super::graphql_api_lifecycle::{create_api as create_graphql_api, router, REGION};

/// The account id every emulated ARN carries.
const ACCOUNT: &str = "000000000000";

/// One answer from the emulator: the parts a client branches on.
#[derive(Debug, Clone)]
pub(crate) struct Answer {
    pub(crate) status: u16,
    /// The `x-amzn-errortype` header restJson1 resolves the error shape from.
    pub(crate) error_type: String,
    pub(crate) body: Value,
}

/// Send one request and return what the emulator answered.
pub(crate) async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://appsync.{REGION}.amazonaws.com{path}"))
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(payload)
        .expect("a well-formed request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer {
        status,
        error_type,
        body,
    }
}

/// Create one GraphQL API through the shared helper and return its `apiId`.
pub(crate) async fn create_api(router: &MultiServiceRouter, name: &str) -> String {
    let api = create_graphql_api(
        router,
        json!({
            "name": name,
            "authenticationType": "API_KEY",
            "queryDepthLimit": 0,
            "resolverCountLimit": 0,
            "xrayEnabled": false,
        }),
    )
    .await;
    api["apiId"].as_str().unwrap_or_default().to_string()
}

/// Create one type under `api_id` from `definition`, reported in `format`.
pub(crate) async fn create_type(
    router: &MultiServiceRouter,
    api_id: &str,
    definition: &str,
    format: &str,
) -> Answer {
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types"),
        Some(json!({"definition": definition, "format": format})),
    )
    .await
}

/// Read the type `type_name` of `api_id` back, in `format`.
pub(crate) async fn get_type(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    format: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types/{type_name}?format={format}"),
        None,
    )
    .await
}

/// Both `TypeDefinitionFormat` values. Source: `smithy_query` shape
/// `TypeDefinitionFormat` (`SDL`, `JSON`).
pub(crate) const FORMATS: &[&str] = &[state::SDL_FORMAT, state::JSON_FORMAT];

/// One row of the round-trip table: a type definition, and what it declares.
#[derive(Debug, Clone)]
pub(crate) struct Row {
    /// The SDL the create carries.
    pub(crate) definition: &'static str,
    /// The name the definition declares, which the type is addressed by.
    pub(crate) name: &'static str,
    /// The introspection kind the definition's keyword declares.
    pub(crate) kind: &'static str,
    /// The definition's own description, which is the type's description.
    pub(crate) description: Option<&'static str>,
}

/// Every kind of type a GraphQL SDL definition can declare, one row each.
///
/// Source: GraphQL's SDL type definitions (object, interface, union, enum, input
/// object, scalar) and the matching `__TypeKind` values introspection reports them
/// under, as the `CreateType` definition member's own reference to the GraphQL SDL
/// documentation names them. The rows differ in kind, in whether the definition carries
/// a description, in whether its members carry arguments, defaults, list and non-null
/// types or a `@deprecated` directive — the shapes the rules generalise over.
///
/// The rows are ordered so that a type another row refers to is created before it,
/// which is what lets the JSON rendering resolve those references.
pub(crate) fn rows() -> Vec<Row> {
    vec![
        Row {
            definition: "\"\"\"Anything addressable by id.\"\"\"\n\
                         interface Node {\n  id: ID!\n}\n",
            name: "Node",
            kind: state::INTERFACE_KIND,
            description: Some("Anything addressable by id."),
        },
        Row {
            definition: "\"\"\"A blog post.\"\"\"\n\
                         type Post implements Node {\n\
                         \x20 id: ID!\n\
                         \x20 title: String!\n\
                         \x20 tags: [String]\n\
                         \x20 comments(limit: Int = 10, after: String): [Comment!]!\n\
                         \x20 legacyTitle: String @deprecated(reason: \"use title\")\n\
                         }\n",
            name: "Post",
            kind: state::OBJECT_KIND,
            description: Some("A blog post."),
        },
        Row {
            definition: "type Comment implements Node {\n  id: ID!\n  body: String\n}\n",
            name: "Comment",
            kind: state::OBJECT_KIND,
            description: None,
        },
        Row {
            definition: "\"\"\"What a post is created from.\"\"\"\n\
                         input PostInput {\n\
                         \x20 title: String!\n\
                         \x20 tags: [String!] = []\n\
                         }\n",
            name: "PostInput",
            kind: state::INPUT_OBJECT_KIND,
            description: Some("What a post is created from."),
        },
        Row {
            definition: "\"\"\"How posts are ordered.\"\"\"\n\
                         enum PostOrder {\n  NEWEST\n  OLDEST @deprecated\n}\n",
            name: "PostOrder",
            kind: state::ENUM_KIND,
            description: Some("How posts are ordered."),
        },
        Row {
            definition: "union SearchResult = Post | Comment\n",
            name: "SearchResult",
            kind: state::UNION_KIND,
            description: None,
        },
        Row {
            definition: "\"\"\"An amount of money.\"\"\"\nscalar Money\n",
            name: "Money",
            kind: state::SCALAR_KIND,
            description: Some("An amount of money."),
        },
    ]
}

/// The SDL a READ of a type defined by `definition` answers with: the service's own
/// rendering of the definition it parsed.
///
/// A read reports the stored type rather than the text a request supplied, so the
/// expectation here is derived from the definition the type was defined with rather than
/// taken from the request. The LAYOUT that rendering has is pinned byte for byte, over a
/// table of definitions, in `type_sdl_rendering.rs`; what these suites assert with it is
/// that a read reports the right DEFINITION.
pub(crate) fn rendered_sdl(definition: &str) -> String {
    state::parse_type_definition(definition)
        .unwrap_or_else(|_| panic!("the definition {definition:?} under test parses"))
        .render_sdl()
}

/// Assert `reported` is the type `row` declares as a READ of it reports it, in `format`
/// under `api_id`.
pub(crate) fn assert_projects(
    row: &Row,
    api_id: &str,
    format: &str,
    reported: &Value,
    source: &str,
) {
    assert_members(row, api_id, format, reported, source);
    let definition = definition_text(row, format, reported, source);
    if format == state::JSON_FORMAT {
        assert_json_definition(row, &definition, format, source);
        return;
    }
    // The SDL a read answers with is the service's rendering of the stored definition.
    let expected = rendered_sdl(row.definition);
    assert_eq!(
        definition, expected,
        "case {row:?} ({source}, {format}): expected the rendered SDL {expected:?}, got \
         {definition:?}"
    );
}

/// Assert `reported` is the type `row` declares as the call that STATED its definition
/// reports it, in `format` under `api_id`.
pub(crate) fn assert_states(
    row: &Row,
    api_id: &str,
    format: &str,
    reported: &Value,
    source: &str,
) {
    assert_members(row, api_id, format, reported, source);
    let definition = definition_text(row, format, reported, source);
    if format == state::JSON_FORMAT {
        assert_json_definition(row, &definition, format, source);
        return;
    }
    // The SDL such a call answers with is the definition the request wrote.
    assert_eq!(
        definition, row.definition,
        "case {row:?} ({source}, {format}): expected the stated SDL {:?}, got {definition:?}",
        row.definition
    );
}

/// The definition `reported` carries.
fn definition_text(row: &Row, format: &str, reported: &Value, source: &str) -> String {
    reported["definition"]
        .as_str()
        .unwrap_or_else(|| panic!("case {row:?} ({source}, {format}): expected a definition"))
        .to_string()
}

/// Assert every member of `reported` other than the definition is the type `row`
/// declares: those are the same whichever call answered.
fn assert_members(row: &Row, api_id: &str, format: &str, reported: &Value, source: &str) {
    assert_eq!(
        reported["name"],
        json!(row.name),
        "case {row:?} ({source}, {format}): expected name {}, got {:?}",
        row.name,
        reported["name"]
    );
    let expected_arn = format!(
        "arn:aws:appsync:{REGION}:{ACCOUNT}:{}",
        state::type_resource(api_id, row.name)
    );
    assert_eq!(
        reported["arn"],
        json!(expected_arn),
        "case {row:?} ({source}, {format}): expected arn {expected_arn}, got {:?}",
        reported["arn"]
    );
    assert_eq!(
        reported["format"],
        json!(format),
        "case {row:?} ({source}): expected format {format}, got {:?}",
        reported["format"]
    );
    // The description is the definition's own description, and a definition that
    // carries none describes nothing.
    match row.description {
        Some(description) => assert_eq!(
            reported["description"],
            json!(description),
            "case {row:?} ({source}, {format}): expected description {description:?}, got {:?}",
            reported["description"]
        ),
        None => assert!(
            reported["description"].is_null(),
            "case {row:?} ({source}, {format}): expected no description, got {:?}",
            reported["description"]
        ),
    }
}

/// Assert `definition` is the introspection JSON of the type `row` declares.
fn assert_json_definition(row: &Row, definition: &str, format: &str, source: &str) {
    let parsed: Value = serde_json::from_str(definition).unwrap_or_else(|error| {
        panic!("case {row:?} ({source}, {format}): expected JSON, got {definition:?}: {error}")
    });
    assert_eq!(
        parsed["kind"],
        json!(row.kind),
        "case {row:?} ({source}): expected kind {}, got {:?}",
        row.kind,
        parsed["kind"]
    );
    assert_eq!(
        parsed["name"],
        json!(row.name),
        "case {row:?} ({source}): expected JSON name {}, got {:?}",
        row.name,
        parsed["name"]
    );
    // Introspection reports only the members the kind carries, and null for the rest.
    let carries_fields = row.kind == state::OBJECT_KIND || row.kind == state::INTERFACE_KIND;
    for (member, carried) in [
        ("fields", carries_fields),
        ("interfaces", carries_fields),
        ("inputFields", row.kind == state::INPUT_OBJECT_KIND),
        ("enumValues", row.kind == state::ENUM_KIND),
        (
            "possibleTypes",
            row.kind == state::UNION_KIND || row.kind == state::INTERFACE_KIND,
        ),
    ] {
        assert_eq!(
            parsed[member].is_array(),
            carried,
            "case {row:?} ({source}): expected {member} {}, got {:?}",
            if carried { "to be a list" } else { "to be null" },
            parsed[member]
        );
    }
}

#[tokio::test]
async fn create_persists_every_kind_of_type_definition_and_get_reads_it_back() {
    let router = router();

    // One API per creating format: the format a create names chooses how that ONE
    // answer reports the definition, so the same definitions are created under both.
    for format in FORMATS {
        let api_id = create_api(&router, &format!("type-round-trip-{format}")).await;

        for row in rows() {
            let created = create_type(&router, &api_id, row.definition, format).await;
            assert_eq!(
                created.status, 200,
                "case {row:?} ({format}): expected CreateType 200, got {} {:?}",
                created.status, created.body
            );
            assert_states(&row, &api_id, format, &created.body["type"], "create");

            // ... and the stored type is readable in EITHER representation, whichever
            // format created it.
            for read_format in FORMATS {
                let read = get_type(&router, &api_id, row.name, read_format).await;
                assert_eq!(
                    read.status, 200,
                    "case {row:?} (get {read_format}): expected GetType 200, got {} {:?}",
                    read.status, read.body
                );
                assert_projects(&row, &api_id, read_format, &read.body["type"], "get");
            }

            // The read in the creating format answers the same type the create did,
            // member for member — the definition apart, which a read RENDERS and the
            // create answered as the request stated it.
            let read = get_type(&router, &api_id, row.name, format).await;
            for member in ["name", "arn", "description", "format"] {
                assert_eq!(
                    read.body["type"][member], created.body["type"][member],
                    "case {row:?} ({format}): expected GetType to answer the created {member} \
                     {:?}, got {:?}",
                    created.body["type"][member], read.body["type"][member]
                );
            }
        }
    }
}

/// The introspection JSON the stored type `type_name` of `api_id` reports.
async fn rendered_json(router: &MultiServiceRouter, api_id: &str, type_name: &str) -> Value {
    let read = get_type(router, api_id, type_name, state::JSON_FORMAT).await;
    assert_eq!(
        read.status, 200,
        "case {type_name}: expected GetType 200, got {} {:?}",
        read.status, read.body
    );
    let definition = read.body["type"]["definition"]
        .as_str()
        .unwrap_or_else(|| panic!("case {type_name}: expected a definition, got {:?}", read.body))
        .to_string();
    serde_json::from_str(&definition).unwrap_or_else(|error| {
        panic!("case {type_name}: expected JSON, got {definition:?}: {error}")
    })
}

#[tokio::test]
async fn the_json_rendering_states_what_the_definition_says() {
    let router = router();
    let api_id = create_api(&router, "type-json-rendering").await;

    let node = "interface Node {\n  id: ID!\n}\n";
    let post = "type Post implements Node {\n  id: ID!\n  author: Author\n  tags: [String!]\n}\n";
    let author = "type Author {\n  name: String\n}\n";
    let comment = "type Comment implements Node {\n  id: ID!\n}\n";

    for definition in [node, post] {
        let created = create_type(&router, &api_id, definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {definition:?}: expected CreateType 200, got {} {:?}",
            created.status, created.body
        );
    }

    let rendered = rendered_json(&router, &api_id, "Post").await;
    let fields = rendered["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case Post: expected fields, got {rendered:?}"))
        .clone();
    let field = |name: &str| -> Value {
        fields
            .iter()
            .find(|field| field["name"] == json!(name))
            .cloned()
            .unwrap_or_else(|| panic!("case Post.{name}: expected the field, got {fields:?}"))
    };

    // A reference carries only what the reference itself states: the wrapper's kind
    // around what it wraps, and nothing of the type it names beyond that name.
    assert_eq!(
        field("id")["type"],
        json!({"kind": state::NON_NULL_KIND, "ofType": {"name": "ID"}}),
        "case Post.id: expected a non-null reference, got {:?}",
        field("id")["type"]
    );
    // A list of non-null references keeps both wrappers, outermost first.
    assert_eq!(
        field("tags")["type"],
        json!({
            "kind": state::LIST_KIND,
            "ofType": {"kind": state::NON_NULL_KIND, "ofType": {"name": "String"}},
        }),
        "case Post.tags: expected a list of non-null references, got {:?}",
        field("tags")["type"]
    );
    // A reference to a type the API holds states the same thing: its name.
    assert_eq!(
        field("author")["type"],
        json!({"name": "Author"}),
        "case Post.author: expected the referenced name, got {:?}",
        field("author")["type"]
    );
    // The interfaces the definition implements are named the same way.
    assert_eq!(
        rendered["interfaces"],
        json!([{"name": "Node"}]),
        "case Post: expected the implemented interface, got {:?}",
        rendered["interfaces"]
    );
    // An object type has no description, no input fields, no enum values and no possible
    // types, and the document leaves every member a type has no value for OUT.
    for member in ["description", "inputFields", "enumValues", "possibleTypes"] {
        assert!(
            rendered.get(member).is_none(),
            "case Post: expected no {member} member at all, got {:?}",
            rendered.get(member)
        );
    }

    // The interface reports the object types that implement it — the state at the time
    // of the READ, so what implements it is never frozen at create time.
    assert_eq!(
        rendered_json(&router, &api_id, "Node").await["possibleTypes"],
        json!([{"name": "Post"}]),
        "case Node with one implementor: expected Post among its possible types"
    );

    for definition in [author, comment] {
        let created = create_type(&router, &api_id, definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {definition:?}: expected CreateType 200, got {} {:?}",
            created.status, created.body
        );
    }
    // The type that implements the interface joins its possible types; the one that
    // implements nothing does not.
    assert_eq!(
        rendered_json(&router, &api_id, "Node").await["possibleTypes"],
        json!([{"name": "Comment"}, {"name": "Post"}]),
        "case Node after another implementor: expected both implementors"
    );
}

#[tokio::test]
async fn a_definition_that_declares_no_single_type_is_refused_and_persists_nothing() {
    let router = router();
    let api_id = create_api(&router, "type-definition-refusals").await;

    // Each row is a definition that declares no ONE type: unreadable GraphQL, a
    // document that declares nothing at all, one that declares something a type cannot
    // be (an operation, a fragment, a directive, a schema), one that only EXTENDS a
    // type, and one that declares two.
    for (label, definition, type_name) in [
        ("unreadable", "type Post {", "Post"),
        ("not GraphQL at all", "{\"name\": \"Post\"}", "Post"),
        ("empty", "", "Post"),
        ("blank", "   \n\t", "Post"),
        ("a comment only", "# just a comment\n", "Post"),
        ("an operation", "query Posts {\n  posts {\n    id\n  }\n}\n", "Posts"),
        (
            "a fragment",
            "fragment PostFields on Post {\n  id\n}\n",
            "PostFields",
        ),
        (
            "a directive",
            "directive @auth on FIELD_DEFINITION\n",
            "auth",
        ),
        ("a schema", "schema {\n  query: Query\n}\n", "Query"),
        ("an extension", "extend type Post {\n  body: String\n}\n", "Post"),
        (
            "two types",
            "type Post {\n  id: ID!\n}\ntype Comment {\n  id: ID!\n}\n",
            "Post",
        ),
    ] {
        let answer = create_type(&router, &api_id, definition, state::SDL_FORMAT).await;
        assert_eq!(
            answer.status, 400,
            "case {label} ({definition:?}): expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {label} ({definition:?}): expected BadRequestException, got {:?} {:?}",
            answer.error_type, answer.body
        );
        // A refused create persists nothing, so the name it would have taken is still
        // free of a type.
        let read = get_type(&router, &api_id, type_name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 404,
            "case {label}: a refused create persists nothing, got {} {:?}",
            read.status, read.body
        );
    }
}

#[tokio::test]
async fn a_name_the_api_already_holds_is_refused_and_the_stored_one_is_untouched() {
    let router = router();
    let api_id = create_api(&router, "type-name-taken").await;

    for row in rows() {
        let created = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {row:?}: expected CreateType 200, got {} {:?}",
            created.status, created.body
        );

        // The same name declared by a DIFFERENT definition: the second create must not
        // replace a type the caller did not name for replacement.
        let clash = format!("scalar {}\n", row.name);
        let answer = create_type(&router, &api_id, &clash, state::SDL_FORMAT).await;
        assert_eq!(
            answer.status, 400,
            "case {row:?}: expected 400 for a name the API holds, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type, "BadRequestException",
            "case {row:?}: expected BadRequestException, got {:?} {:?}",
            answer.error_type, answer.body
        );

        let read = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 200,
            "case {row:?}: expected GetType 200, got {} {:?}",
            read.status, read.body
        );
        assert_projects(
            &row,
            &api_id,
            state::SDL_FORMAT,
            &read.body["type"],
            "after the clash",
        );
    }
}

#[tokio::test]
async fn a_type_belongs_to_the_api_it_was_created_under() {
    let router = router();
    let first = create_api(&router, "type-scope-first").await;
    let second = create_api(&router, "type-scope-second").await;

    for row in rows() {
        let created = create_type(&router, &first, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {row:?}: expected CreateType 200, got {} {:?}",
            created.status, created.body
        );

        // The other API does not hold it, however many types it holds of its own.
        let read = get_type(&router, &second, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 404,
            "case {row:?}: another API does not hold it, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type, "NotFoundException",
            "case {row:?}: expected NotFoundException, got {:?} {:?}",
            read.error_type, read.body
        );

        // The same definition under the other API is a DIFFERENT type, named under
        // that API's own path.
        let twin = create_type(&router, &second, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            twin.status, 200,
            "case {row:?}: expected the same name under another API to be created, got {} {:?}",
            twin.status, twin.body
        );
        assert_states(
            &row,
            &second,
            state::SDL_FORMAT,
            &twin.body["type"],
            "the other API",
        );
        assert_ne!(
            twin.body["type"]["arn"], created.body["type"]["arn"],
            "case {row:?}: two APIs' types share an ARN: {:?}",
            twin.body["type"]["arn"]
        );
    }
}

#[tokio::test]
async fn the_json_rendering_reports_arguments_defaults_and_deprecations() {
    let router = router();
    let api_id = create_api(&router, "type-json-members").await;

    // One object type carrying every member shape a field definition can hold — an
    // argument list with and without a default, a list of non-null types, and a
    // `@deprecated` directive with and without a reason — and one enum whose values
    // carry the same directive.
    let post = "type Post {\n\
                \x20 comments(limit: Int = 10, after: String): [Comment!]!\n\
                \x20 legacyTitle: String @deprecated(reason: \"use title\")\n\
                \x20 body: String @deprecated\n\
                }\n";
    let input = "input PostInput {\n  title: String!\n  tags: [String!] = []\n}\n";
    let order = "enum PostOrder {\n  NEWEST\n  OLDEST @deprecated(reason: \"unordered\")\n}\n";

    for definition in [post, input, order] {
        let created = create_type(&router, &api_id, definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {definition:?}: expected CreateType 200, got {} {:?}",
            created.status, created.body
        );
    }

    let rendered = rendered_json(&router, &api_id, "Post").await;
    let fields = rendered["fields"]
        .as_array()
        .unwrap_or_else(|| panic!("case Post: expected fields, got {rendered:?}"))
        .clone();
    let field = |name: &str| -> Value {
        fields
            .iter()
            .find(|field| field["name"] == json!(name))
            .cloned()
            .unwrap_or_else(|| panic!("case Post.{name}: expected the field, got {fields:?}"))
    };

    // An argument reports its type and the default the definition wrote, as GraphQL
    // writes it; an argument with no default reports none at all.
    assert_eq!(
        field("comments")["args"],
        json!([
            {"name": "limit", "type": {"name": "Int"}, "defaultValue": "10"},
            {"name": "after", "type": {"name": "String"}},
        ]),
        "case Post.comments: expected its two arguments, got {:?}",
        field("comments")["args"]
    );
    // A `@deprecated` field is reported deprecated, with the reason the directive
    // states — and with the reason the directive defaults to when it states none.
    for (name, reason) in [("legacyTitle", "use title"), ("body", "No longer supported")] {
        assert_eq!(
            field(name)["isDeprecated"],
            json!(true),
            "case Post.{name}: expected a deprecated field, got {:?}",
            field(name)
        );
        assert_eq!(
            field(name)["deprecationReason"],
            json!(reason),
            "case Post.{name}: expected the reason {reason:?}, got {:?}",
            field(name)["deprecationReason"]
        );
    }
    // A field with no directive is not deprecated.
    assert_eq!(
        field("comments")["isDeprecated"],
        json!(false),
        "case Post.comments: expected an undeprecated field, got {:?}",
        field("comments")
    );

    // An input object reports its fields as input fields, with their defaults.
    assert_eq!(
        rendered_json(&router, &api_id, "PostInput").await["inputFields"],
        json!([
            {
                "name": "title",
                "type": {"kind": state::NON_NULL_KIND, "ofType": {"name": "String"}},
            },
            {
                "name": "tags",
                "type": {
                    "kind": state::LIST_KIND,
                    "ofType": {"kind": state::NON_NULL_KIND, "ofType": {"name": "String"}},
                },
                "defaultValue": "[]",
            },
        ]),
        "case PostInput: expected its two input fields"
    );

    // An enum reports its values in the order the definition writes them, each with
    // its own deprecation.
    assert_eq!(
        rendered_json(&router, &api_id, "PostOrder").await["enumValues"],
        json!([
            {"name": "NEWEST", "directives": [], "isDeprecated": false},
            {
                "name": "OLDEST",
                "directives": [],
                "isDeprecated": true,
                "deprecationReason": "unordered",
            },
        ]),
        "case PostOrder: expected its two values"
    );
}

#[tokio::test]
async fn an_unnameable_api_is_a_bad_request_and_an_absent_one_a_not_found() {
    let router = router();
    let api_id = create_api(&router, "type-api-refusals").await;
    let row = &rows()[0];

    // An `apiId` longer than an AppSync API id holds more characters than one has, so
    // it names no API at all.
    let unnameable = "z".repeat(state::API_ID_LEN + 1);
    // An id of the right shape this account simply does not hold.
    let absent = "z".repeat(state::API_ID_LEN);

    for (api, expected_status, expected_error) in [
        (unnameable.as_str(), 400, "BadRequestException"),
        (absent.as_str(), 404, "NotFoundException"),
    ] {
        let created = create_type(&router, api, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, expected_status,
            "case create under {api:?}: expected {expected_status}, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.error_type, expected_error,
            "case create under {api:?}: expected {expected_error}, got {:?} {:?}",
            created.error_type, created.body
        );

        let read = get_type(&router, api, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, expected_status,
            "case get under {api:?}: expected {expected_status}, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.error_type, expected_error,
            "case get under {api:?}: expected {expected_error}, got {:?} {:?}",
            read.error_type, read.body
        );
    }

    // A name the API does not hold is a not-found, not an empty answer.
    let read = get_type(&router, &api_id, "NeverCreated", state::SDL_FORMAT).await;
    assert_eq!(
        read.status, 404,
        "case get an uncreated name: expected 404, got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.error_type, "NotFoundException",
        "case get an uncreated name: expected NotFoundException, got {:?} {:?}",
        read.error_type, read.body
    );
}
