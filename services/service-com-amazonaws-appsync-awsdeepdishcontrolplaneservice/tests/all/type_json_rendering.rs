// SPDX-License-Identifier: CC0-1.0

//! The JSON a READ of a GraphQL type answers with, end to end over HTTP.
//!
//! The rule asserted here is stated over a CLASS of definitions rather than over the one
//! request that exposed the defect: a read asked for the `JSON` format reports
//! `Type$definition` as the type's own introspection document, written the way this
//! service writes one — compact, with its members in a fixed order, with every member the
//! type has no value for left OUT rather than written as null, and with each type
//! REFERENCE stating only what the reference itself says (the name it names, or the
//! wrapper's kind around the reference it wraps).
//!
//! The table spans the shapes the rule generalises over: every kind of type a GraphQL SDL
//! definition declares, bodies of none, one, two and three members, and definitions with
//! and without a description, arguments, default values, wrapped types and a
//! `@deprecated` directive. The document is asserted byte for byte, since the bytes are
//! what a client parses, diffs or hashes.
//!
//! Every assertion goes through the router the production emulator mounts.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;

// The router, the API-creating helper and the type calls are the crate's shared test
// harness, reused rather than copied.
use super::graphql_api_lifecycle::router;
use super::graphql_types::{call, create_api, create_type, get_type, Answer};

/// One row of the table: a definition as a request writes it, and the JSON document a
/// read of the type it declares answers with.
#[derive(Debug, Clone)]
struct Row {
    /// What the row covers, named in every assertion of it.
    label: &'static str,
    /// The name the definition declares, which the type is addressed by.
    name: &'static str,
    /// The definition the type is created from.
    stated: &'static str,
    /// The document a read of the stored type answers with.
    document: &'static str,
}

/// Every kind of type a GraphQL SDL definition declares, and the member shapes one can
/// carry.
///
/// Source: GraphQL's SDL type definitions and the `__TypeKind` values this service reports
/// them under (`state::OBJECT_KIND` and the rest), with the introspection members each
/// kind carries (`fields`, `inputFields`, `interfaces`, `enumValues`, `possibleTypes`) as
/// GraphQL's introspection system defines them.
fn rows() -> Vec<Row> {
    vec![
        Row {
            label: "an object type holding two fields",
            name: "Blog",
            stated: "type Blog { id: ID! title: String }",
            document: "{\"name\":\"Blog\",\"kind\":\"OBJECT\",\"fields\":[\
                       {\"name\":\"id\",\"type\":{\"kind\":\"NON_NULL\",\"ofType\":{\"name\":\"ID\"}},\
                       \"args\":[],\"directives\":[],\"isDeprecated\":false},\
                       {\"name\":\"title\",\"type\":{\"name\":\"String\"},\
                       \"args\":[],\"directives\":[],\"isDeprecated\":false}],\
                       \"interfaces\":[],\"directives\":[]}",
        },
        Row {
            label: "an object type holding one described field",
            name: "Gadget",
            stated: "type Gadget {\n  \"\"\"What it is called.\"\"\"\n  name: String\n}\n",
            document: "{\"name\":\"Gadget\",\"kind\":\"OBJECT\",\"fields\":[\
                       {\"name\":\"name\",\"description\":\"What it is called.\",\
                       \"type\":{\"name\":\"String\"},\
                       \"args\":[],\"directives\":[],\"isDeprecated\":false}],\
                       \"interfaces\":[],\"directives\":[]}",
        },
        Row {
            label: "a described object type implementing interfaces, with arguments, a \
                    default and a deprecation",
            name: "Post",
            stated: "\"\"\"A blog post.\"\"\"\n\
                     type Post implements Node & Timestamped {\n\
                     \x20 id: ID!\n\
                     \x20 comments(limit: Int = 10, after: String): [Comment!]!\n\
                     \x20 legacyTitle: String @deprecated(reason: \"use title\")\n\
                     }\n",
            document: "{\"name\":\"Post\",\"kind\":\"OBJECT\",\"description\":\"A blog post.\",\
                       \"fields\":[\
                       {\"name\":\"id\",\"type\":{\"kind\":\"NON_NULL\",\"ofType\":{\"name\":\"ID\"}},\
                       \"args\":[],\"directives\":[],\"isDeprecated\":false},\
                       {\"name\":\"comments\",\"type\":{\"kind\":\"NON_NULL\",\"ofType\":\
                       {\"kind\":\"LIST\",\"ofType\":{\"kind\":\"NON_NULL\",\
                       \"ofType\":{\"name\":\"Comment\"}}}},\
                       \"args\":[{\"name\":\"limit\",\"type\":{\"name\":\"Int\"},\
                       \"defaultValue\":\"10\"},\
                       {\"name\":\"after\",\"type\":{\"name\":\"String\"}}],\
                       \"directives\":[],\"isDeprecated\":false},\
                       {\"name\":\"legacyTitle\",\"type\":{\"name\":\"String\"},\
                       \"args\":[],\"directives\":[],\"isDeprecated\":true,\
                       \"deprecationReason\":\"use title\"}],\
                       \"interfaces\":[{\"name\":\"Node\"},{\"name\":\"Timestamped\"}],\
                       \"directives\":[]}",
        },
        Row {
            label: "an interface type nothing implements",
            name: "Node",
            stated: "interface Node {\n  id: ID!\n}\n",
            document: "{\"name\":\"Node\",\"kind\":\"INTERFACE\",\"fields\":[\
                       {\"name\":\"id\",\"type\":{\"kind\":\"NON_NULL\",\"ofType\":{\"name\":\"ID\"}},\
                       \"args\":[],\"directives\":[],\"isDeprecated\":false}],\
                       \"interfaces\":[],\"possibleTypes\":[],\"directives\":[]}",
        },
        Row {
            label: "an input object type with a default value",
            name: "PostInput",
            stated: "input PostInput { title: String! tags: [String!] = [] }",
            document: "{\"name\":\"PostInput\",\"kind\":\"INPUT_OBJECT\",\"inputFields\":[\
                       {\"name\":\"title\",\"type\":{\"kind\":\"NON_NULL\",\
                       \"ofType\":{\"name\":\"String\"}}},\
                       {\"name\":\"tags\",\"type\":{\"kind\":\"LIST\",\"ofType\":\
                       {\"kind\":\"NON_NULL\",\"ofType\":{\"name\":\"String\"}}},\
                       \"defaultValue\":\"[]\"}],\"directives\":[]}",
        },
        Row {
            label: "a described enum type with a deprecated value",
            name: "PostOrder",
            stated: "\"\"\"How posts are ordered.\"\"\"\n\
                     enum PostOrder {\n  NEWEST\n  OLDEST @deprecated\n}\n",
            document: "{\"name\":\"PostOrder\",\"kind\":\"ENUM\",\
                       \"description\":\"How posts are ordered.\",\"enumValues\":[\
                       {\"name\":\"NEWEST\",\"directives\":[],\"isDeprecated\":false},\
                       {\"name\":\"OLDEST\",\"directives\":[],\"isDeprecated\":true,\
                       \"deprecationReason\":\"No longer supported\"}],\"directives\":[]}",
        },
        Row {
            label: "a union type, whose members are what it can resolve to",
            name: "SearchResult",
            stated: "union SearchResult = Post | Blog\n",
            document: "{\"name\":\"SearchResult\",\"kind\":\"UNION\",\"possibleTypes\":[\
                       {\"name\":\"Post\"},{\"name\":\"Blog\"}],\"directives\":[]}",
        },
        Row {
            label: "a described scalar type, which carries no members at all",
            name: "Money",
            stated: "\"\"\"An amount of money.\"\"\"\nscalar Money\n",
            document: "{\"name\":\"Money\",\"kind\":\"SCALAR\",\
                       \"description\":\"An amount of money.\",\"directives\":[]}",
        },
    ]
}

/// Send one `ListTypes` for `api_id`, in `format`.
async fn list_types(router: &MultiServiceRouter, api_id: &str, format: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/apis/{api_id}/types?format={format}"),
        None,
    )
    .await
}

/// The definition a reported `Type` carries.
fn definition_of(reported: &Value) -> &str {
    reported["definition"].as_str().unwrap_or_default()
}

#[tokio::test]
async fn a_json_read_answers_the_types_own_introspection_document() {
    let router = router();

    for row in rows() {
        // One API per row, so what an interface can resolve to is the row's own type
        // system and not the rows created before it.
        let api_id = create_api(&router, &format!("type-json-{}", row.name)).await;
        let created = create_type(&router, &api_id, row.stated, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateType 200, got {created:?}",
            row.label
        );

        let read = get_type(&router, &api_id, row.name, state::JSON_FORMAT).await;
        assert_eq!(
            read.status, 200,
            "case {}: expected GetType 200, got {read:?}",
            row.label
        );
        assert_eq!(
            definition_of(&read.body["type"]),
            row.document,
            "case {}: expected GetType to answer {:?}, got {:?}",
            row.label,
            row.document,
            definition_of(&read.body["type"])
        );

        // The listing answers the same document for the same type.
        let listing = list_types(&router, &api_id, state::JSON_FORMAT).await;
        let listed = listing.body["types"]
            .as_array()
            .and_then(|types| types.first())
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {}: expected the type listed, got {listing:?}", row.label)
            });
        assert_eq!(
            definition_of(&listed),
            row.document,
            "case {}: expected ListTypes to answer {:?}, got {:?}",
            row.label,
            row.document,
            definition_of(&listed)
        );
    }
}

#[tokio::test]
async fn what_an_interface_can_resolve_to_is_what_the_api_holds_when_it_is_read() {
    let router = router();
    let api_id = create_api(&router, "type-json-implementors").await;

    // The interface first, then the object types: one implements it, one does not, so
    // what the read reports can only come from the type system the API holds.
    for (definition, expected) in [
        ("interface Node {\n  id: ID!\n}\n", serde_json::json!([])),
        (
            "type Post implements Node {\n  id: ID!\n}\n",
            serde_json::json!([{"name": "Post"}]),
        ),
        (
            "type Money {\n  amount: Int\n}\n",
            serde_json::json!([{"name": "Post"}]),
        ),
        (
            "type Comment implements Node {\n  id: ID!\n}\n",
            serde_json::json!([{"name": "Comment"}, {"name": "Post"}]),
        ),
    ] {
        let created = create_type(&router, &api_id, definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case create {definition:?}: expected 200, got {created:?}"
        );
        let read = get_type(&router, &api_id, "Node", state::JSON_FORMAT).await;
        let document: Value = serde_json::from_str(definition_of(&read.body["type"]))
            .unwrap_or_else(|error| panic!("case after {definition:?}: expected JSON: {error}"));
        // Only the types that implement the interface are what it can resolve to.
        assert_eq!(
            document["possibleTypes"], expected,
            "case after {definition:?}: expected {expected:?}, got {:?}",
            document["possibleTypes"]
        );
    }
}
