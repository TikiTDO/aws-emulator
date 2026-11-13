// SPDX-License-Identifier: CC0-1.0

//! `UpdateType`, end to end over HTTP: what an update replaces, what it keeps, and which
//! updates it refuses.
//!
//! The rule asserted here is stated over CLASSES of input rather than over the one
//! request that exposed the defect: an update REPLACES the definition of the type the
//! request addresses by name under its API with the definition the request carries, keeps
//! everything that identifies that type (its name, its ARN, its place among the API's
//! types) and answers the stored type in the `TypeDefinitionFormat` the request names, so
//! every later reader answers the new definition; an update the service cannot carry out
//! changes nothing — a definition it cannot read as one type definition is a 400
//! `BadRequestException`, so is one that declares a type other than the addressed one, a
//! type name the API does not hold and an `apiId` this account does not hold are 404s, an
//! `apiId` that cannot name an API at all is a 400, and an update of a type system a
//! schema creation is still modifying is a 409.
//!
//! The rows span:
//!
//! 1. every kind of type a GraphQL SDL definition declares (object, interface, union,
//!    enum, input object, scalar), as both the definition replaced and the replacement,
//!    including a replacement that changes the kind the name is declared with;
//! 2. what about the definition changes: a field added, a field dropped, a field's type,
//!    an argument's default, an enum value and its `@deprecated` directive, a union's
//!    members, a description gained and a description lost;
//! 3. both `TypeDefinitionFormat` values, on the update's own answer and on every
//!    read-back, since the format chooses the representation a report carries and not
//!    what is stored;
//! 4. the forms of an update that carries out nothing: an unreadable definition (empty,
//!    truncated, not GraphQL at all, declaring two types, declaring something that is not
//!    a type), a definition declaring another type, a never-created name, a name a delete
//!    removed, a name another API holds, an `apiId` no API has, an `apiId` too long to be
//!    one, and an API whose schema creation is still owed;
//! 5. a request that carries NO definition, which states no new definition at all.
//!
//! The harness (`router`, `call`, `create_api`, `create_type`, `get_type`, the type
//! catalog `rows()` and the projection assertion) is the `graphql_types` suite's, reused
//! rather than copied.

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::graphql_api_lifecycle::router;
use super::graphql_types::{
    assert_projects, assert_states, call, create_api, create_type, get_type, rendered_sdl, rows,
    Answer, Row, FORMATS,
};
use super::schema_creation::await_creation;

/// The text a `BadRequestException` carries for a definition the service cannot read as
/// one type definition: a fixed sentence with no variable part. Pinned here rather than
/// read from the service, so a drift in the wording fails this suite.
const DEFINITION_NOT_READABLE: &str = "Unable to parse provided type definition.";

/// The text a `NotFoundException` carries for a type the addressed API does not hold.
const TYPE_NOT_FOUND: &str = "Type not found.";

/// The text a `BadRequestException` carries for a definition that declares a type other
/// than the one the request addresses: fixed text around the two type names the request
/// itself names. Spelled out here rather than read from the service, so a drift in the
/// wording fails this suite instead of agreeing with itself.
fn mismatched_name_text(addressed: &str, declared: &str) -> String {
    format!("Provided type definition must declare the type {addressed}, not {declared}.")
}

/// The text a `ConcurrentModificationException` carries, as the model documents the
/// shape.
const CONCURRENT_MODIFICATION: &str =
    "Another modification is in progress at this time and it must complete before you can \
     make your change.";

/// The non-terminal `SchemaStatus` a scheduled schema creation is in. Source:
/// `smithy_query` shape `SchemaStatus`.
const PROCESSING: &str = "PROCESSING";

/// Send one `UpdateType` for `type_name` under `api_id`.
///
/// `definition` is optional on the wire (`smithy_query` operation `UpdateType`:
/// `definition` is not required), so `None` sends a body that carries only the format.
async fn update_type(
    router: &MultiServiceRouter,
    api_id: &str,
    type_name: &str,
    definition: Option<&str>,
    format: &str,
) -> Answer {
    let mut body = json!({"format": format});
    if let Some(definition) = definition {
        body["definition"] = json!(definition);
    }
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/{type_name}"),
        Some(body),
    )
    .await
}

/// Send one `DeleteType` for `type_name` under `api_id`.
async fn delete_type(router: &MultiServiceRouter, api_id: &str, type_name: &str) -> Answer {
    call(
        router,
        "DELETE",
        &format!("/v1/apis/{api_id}/types/{type_name}"),
        None,
    )
    .await
}

/// Start the creation of the schema `definition` on `api_id`.
///
/// The definition is a blob, which restJson1 carries base64-encoded in the JSON body.
async fn start_schema_creation(
    router: &MultiServiceRouter,
    api_id: &str,
    definition: &str,
) -> Answer {
    let encoded = base64::engine::general_purpose::STANDARD.encode(definition.as_bytes());
    call(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": encoded})),
    )
    .await
}

/// The `message` of an error body, or the empty string when there is none.
fn message(answer: &Answer) -> &str {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// One row of the replacement table: the definition a type is created with and the
/// definition an update replaces it by, both declaring the SAME name.
#[derive(Debug)]
struct Change {
    label: &'static str,
    created: Row,
    updated: Row,
}

/// The replacement table: every kind of type GraphQL's SDL grammar declares, replaced by
/// a definition that differs from it in the shapes the rule generalises over — a field
/// added or dropped, a field's type, an argument's default, an enum value and its
/// `@deprecated` directive, a union's members, a description gained or lost, and the kind
/// the name is declared with.
///
/// Source for the kinds and the `__TypeKind` values they report under: GraphQL's SDL type
/// definitions, as `UpdateTypeRequest$definition` ("The new definition") and
/// `CreateTypeRequest$definition` (which cites the GraphQL SDL documentation) name them.
fn changes() -> Vec<Change> {
    vec![
        Change {
            label: "an object type gains fields and a description",
            created: Row {
                definition: "type Widget {\n  id: ID!\n}\n",
                name: "Widget",
                kind: state::OBJECT_KIND,
                description: None,
            },
            updated: Row {
                definition: "\"\"\"A widget.\"\"\"\n\
                             type Widget {\n\
                             \x20 id: ID!\n\
                             \x20 price: Int\n\
                             \x20 tags: [String!]!\n\
                             }\n",
                name: "Widget",
                kind: state::OBJECT_KIND,
                description: Some("A widget."),
            },
        },
        Change {
            label: "an object type drops a field, its arguments and its description",
            created: Row {
                definition: "\"\"\"A catalogue.\"\"\"\n\
                             type Catalogue {\n\
                             \x20 id: ID!\n\
                             \x20 widgets(limit: Int = 10, after: String): [Widget!]!\n\
                             \x20 legacy: String @deprecated(reason: \"use widgets\")\n\
                             }\n",
                name: "Catalogue",
                kind: state::OBJECT_KIND,
                description: Some("A catalogue."),
            },
            updated: Row {
                definition: "type Catalogue {\n  id: ID!\n}\n",
                name: "Catalogue",
                kind: state::OBJECT_KIND,
                description: None,
            },
        },
        Change {
            label: "an interface type's field becomes a non-null list",
            created: Row {
                definition: "interface Named {\n  name: String\n}\n",
                name: "Named",
                kind: state::INTERFACE_KIND,
                description: None,
            },
            updated: Row {
                definition: "\"\"\"Anything with names.\"\"\"\n\
                             interface Named {\n  name: [String!]!\n}\n",
                name: "Named",
                kind: state::INTERFACE_KIND,
                description: Some("Anything with names."),
            },
        },
        Change {
            label: "an enum type gains a deprecated value",
            created: Row {
                definition: "enum Order {\n  NEWEST\n}\n",
                name: "Order",
                kind: state::ENUM_KIND,
                description: None,
            },
            updated: Row {
                definition: "\"\"\"How things are ordered.\"\"\"\n\
                             enum Order {\n  NEWEST\n  OLDEST @deprecated\n}\n",
                name: "Order",
                kind: state::ENUM_KIND,
                description: Some("How things are ordered."),
            },
        },
        Change {
            label: "an input object type's field default changes",
            created: Row {
                definition: "input WidgetInput {\n  tags: [String!] = []\n}\n",
                name: "WidgetInput",
                kind: state::INPUT_OBJECT_KIND,
                description: None,
            },
            updated: Row {
                definition: "input WidgetInput {\n\
                             \x20 tags: [String!] = [\"new\"]\n\
                             \x20 price: Int = 0\n\
                             }\n",
                name: "WidgetInput",
                kind: state::INPUT_OBJECT_KIND,
                description: None,
            },
        },
        Change {
            label: "a union type's members change",
            created: Row {
                definition: "union Found = Widget | Catalogue\n",
                name: "Found",
                kind: state::UNION_KIND,
                description: None,
            },
            updated: Row {
                definition: "union Found = Widget\n",
                name: "Found",
                kind: state::UNION_KIND,
                description: None,
            },
        },
        Change {
            label: "a scalar type gains a description",
            created: Row {
                definition: "scalar Money\n",
                name: "Money",
                kind: state::SCALAR_KIND,
                description: None,
            },
            updated: Row {
                definition: "\"\"\"An amount of money.\"\"\"\nscalar Money\n",
                name: "Money",
                kind: state::SCALAR_KIND,
                description: Some("An amount of money."),
            },
        },
        Change {
            label: "the kind the name is declared with changes",
            created: Row {
                definition: "scalar Weight\n",
                name: "Weight",
                kind: state::SCALAR_KIND,
                description: None,
            },
            updated: Row {
                definition: "\"\"\"How heavy a widget is.\"\"\"\n\
                             type Weight {\n  grams: Int!\n}\n",
                name: "Weight",
                kind: state::OBJECT_KIND,
                description: Some("How heavy a widget is."),
            },
        },
    ]
}

/// Read `type_name` back in both formats and assert it projects `row`.
async fn assert_reads_back(
    router: &MultiServiceRouter,
    api_id: &str,
    row: &Row,
    case: &str,
) {
    for format in FORMATS {
        let read = get_type(router, api_id, row.name, format).await;
        assert_eq!(
            read.status, 200,
            "case {case} ({format}): expected GetType 200, got {read:?}"
        );
        assert_projects(row, api_id, format, &read.body["type"], case);
    }
}

/// An update replaces the addressed type's definition, whatever kind of definition
/// declared it and whatever the replacement declares, and the answer and every later read
/// report the NEW definition in the format they are asked for.
#[tokio::test]
async fn an_update_replaces_the_definition_of_the_addressed_type() {
    let router = router();

    for (index, change) in changes().into_iter().enumerate() {
        // One API per row, and the update answered in BOTH formats: the format chooses
        // the representation the report carries, not what the update stored.
        for format in FORMATS {
            let case = format!("{} ({format})", change.label);
            let api_id = create_api(&router, &format!("type-update-{index}-{format}")).await;

            let created =
                create_type(&router, &api_id, change.created.definition, state::SDL_FORMAT).await;
            assert_eq!(
                created.status, 200,
                "case {case}: expected CreateType 200, got {created:?}"
            );
            let created_arn = created.body["type"]["arn"].clone();
            assert_reads_back(&router, &api_id, &change.created, &format!("{case}, before")).await;

            let updated = update_type(
                &router,
                &api_id,
                change.created.name,
                Some(change.updated.definition),
                format,
            )
            .await;
            assert_eq!(
                updated.status, 200,
                "case {case}: expected UpdateType 200, got {updated:?}"
            );
            assert_states(
                &change.updated,
                &api_id,
                format,
                &updated.body["type"],
                &case,
            );

            // The type keeps its identity: the update is not a delete followed by a
            // create, so the ARN the create named it under is the ARN it still carries.
            assert_eq!(
                updated.body["type"]["arn"], created_arn,
                "case {case}: expected the stored ARN {created_arn:?} to be kept, got {:?}",
                updated.body["type"]["arn"]
            );

            // Every later reader answers the new definition, in either representation.
            assert_reads_back(&router, &api_id, &change.updated, &format!("{case}, after")).await;
        }
    }
}

/// `definition` is an optional member, so an update that carries none states no new
/// definition: the stored type is answered unchanged and stays readable as it was.
#[tokio::test]
async fn an_update_without_a_definition_leaves_the_type_as_it_is() {
    let router = router();
    let api_id = create_api(&router, "type-update-no-definition").await;

    for row in rows() {
        let created = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case create {:?}: expected 200, got {created:?}",
            row.name
        );
    }

    for row in rows() {
        for format in FORMATS {
            let case = format!("{:?} ({}) updated with no definition, {format}", row.name, row.kind);
            let answer = update_type(&router, &api_id, row.name, None, format).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected UpdateType 200, got {answer:?}"
            );
            assert_states(&row, &api_id, format, &answer.body["type"], &case);
            assert_reads_back(&router, &api_id, &row, &case).await;
        }
    }
}

/// A definition the service cannot read as ONE type definition is refused, and the stored
/// type is exactly where it was: the refusal replaced nothing.
#[tokio::test]
async fn a_definition_that_cannot_be_read_is_refused_and_replaces_nothing() {
    let router = router();
    let api_id = create_api(&router, "type-update-unreadable").await;

    let stored = Row {
        definition: "\"\"\"A widget.\"\"\"\ntype Widget {\n  id: ID!\n  price: Int\n}\n",
        name: "Widget",
        kind: state::OBJECT_KIND,
        description: Some("A widget."),
    };
    let created = create_type(&router, &api_id, stored.definition, state::SDL_FORMAT).await;
    assert_eq!(
        created.status, 200,
        "case create the stored type: expected 200, got {created:?}"
    );

    // Each row is a definition that is not one type definition: unreadable GraphQL, or
    // readable GraphQL that declares no type or more than one.
    for (label, definition) in [
        ("an empty definition", ""),
        ("only whitespace", "   \n\t\n"),
        ("a truncated object type", "type Widget {\n  id: ID!\n"),
        ("not GraphQL at all", "this is not a type definition"),
        ("a stray brace", "}\n"),
        ("an operation rather than a type", "query Widgets {\n  widget\n}\n"),
        (
            "a directive definition rather than a type",
            "directive @widget on FIELD_DEFINITION\n",
        ),
        (
            "an extension rather than a type",
            "extend type Widget {\n  price: Int\n}\n",
        ),
        (
            "two types",
            "type Widget {\n  id: ID!\n}\ntype Gadget {\n  id: ID!\n}\n",
        ),
    ] {
        for format in FORMATS {
            let case = format!("{label} ({definition:?}, {format})");
            let refused =
                update_type(&router, &api_id, stored.name, Some(definition), format).await;
            assert_eq!(
                (refused.status, refused.error_type.as_str()),
                (400, "BadRequestException"),
                "case {case}: expected a 400 BadRequestException, got {refused:?}"
            );
            assert_eq!(
                message(&refused),
                DEFINITION_NOT_READABLE,
                "case {case}: expected the unreadable-definition text, got {refused:?}"
            );
        }

        // The stored type is untouched by the refusal, in either representation.
        assert_reads_back(&router, &api_id, &stored, label).await;
    }
}

/// A type is addressed by the name its definition declares, so a definition that declares
/// another type defines nothing the request can update: it is refused, the addressed type
/// keeps its definition and the name the definition declared is not created.
#[tokio::test]
async fn a_definition_that_declares_another_type_is_refused() {
    let router = router();
    let api_id = create_api(&router, "type-update-name-mismatch").await;

    let stored = Row {
        definition: "type Widget {\n  id: ID!\n}\n",
        name: "Widget",
        kind: state::OBJECT_KIND,
        description: None,
    };
    let created = create_type(&router, &api_id, stored.definition, state::SDL_FORMAT).await;
    assert_eq!(
        created.status, 200,
        "case create the stored type: expected 200, got {created:?}"
    );

    // The declaring definition is well formed and declares ONE type, of several kinds —
    // what is wrong is only which type it declares.
    for (declared, definition) in [
        ("Gadget", "type Gadget {\n  id: ID!\n}\n"),
        ("Money", "scalar Money\n"),
        ("Named", "interface Named {\n  name: String\n}\n"),
        ("widget", "type widget {\n  id: ID!\n}\n"),
    ] {
        let case = format!("a definition declaring {declared:?}");
        let refused = update_type(
            &router,
            &api_id,
            stored.name,
            Some(definition),
            state::SDL_FORMAT,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (400, "BadRequestException"),
            "case {case}: expected a 400 BadRequestException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            mismatched_name_text(stored.name, declared),
            "case {case}: expected the mismatched-name text, got {refused:?}"
        );

        // Neither the addressed type nor the declared name moved.
        assert_reads_back(&router, &api_id, &stored, &case).await;
        let read = get_type(&router, &api_id, declared, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 404,
            "case {case}: expected the declared name to hold no type, got {read:?}"
        );
    }
}

/// An update that addresses no type creates nothing and says which refusal it is: a name
/// nothing holds (never created, deleted, or held by another API) is a 404, an `apiId`
/// this account does not hold is a 404, and an `apiId` too long to be one is a 400.
#[tokio::test]
async fn an_update_that_addresses_no_type_is_refused_and_creates_nothing() {
    let router = router();
    let api_id = create_api(&router, "type-update-refusals").await;
    let other_api_id = create_api(&router, "type-update-refusals-other").await;

    let definition = "type Widget {\n  id: ID!\n}\n";
    // A name only the OTHER API holds, and a name this API held until a delete removed it.
    let elsewhere = create_type(&router, &other_api_id, definition, state::SDL_FORMAT).await;
    assert_eq!(
        elsewhere.status, 200,
        "case create under the other API: expected 200, got {elsewhere:?}"
    );
    let doomed = "scalar Removed\n";
    let created = create_type(&router, &api_id, doomed, state::SDL_FORMAT).await;
    assert_eq!(
        created.status, 200,
        "case create the doomed type: expected 200, got {created:?}"
    );
    let deleted = delete_type(&router, &api_id, "Removed").await;
    assert_eq!(
        deleted.status, 200,
        "case delete the doomed type: expected 200, got {deleted:?}"
    );

    for (case, type_name, carried) in [
        ("a never-created name", "NeverCreated", definition),
        ("a name a delete removed", "Removed", doomed),
        ("a name only another API holds", "Widget", definition),
    ] {
        let refused = update_type(
            &router,
            &api_id,
            type_name,
            Some(carried),
            state::SDL_FORMAT,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (404, "NotFoundException"),
            "case update {case}: expected a 404 NotFoundException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            TYPE_NOT_FOUND,
            "case update {case}: expected the absent-type text, got {refused:?}"
        );

        // An update never CREATES the type it could not find.
        let read = get_type(&router, &api_id, type_name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 404,
            "case update {case}: expected the name to hold no type afterwards, got {read:?}"
        );
    }

    // An `apiId` of the right shape this account does not hold, and one longer than an
    // API id, which cannot name an API at all.
    for (case, api, expected_status, expected_error) in [
        (
            "an apiId no API has",
            "z".repeat(state::API_ID_LEN),
            404,
            "NotFoundException",
        ),
        (
            "an apiId too long to be one",
            "z".repeat(state::API_ID_LEN + 1),
            400,
            "BadRequestException",
        ),
    ] {
        let refused =
            update_type(&router, &api, "Widget", Some(definition), state::SDL_FORMAT).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (expected_status, expected_error),
            "case update under {case}: expected {expected_status} {expected_error}, got {refused:?}"
        );
    }

    // The other API's type is exactly where it was: none of those refusals touched it.
    let read = get_type(&router, &other_api_id, "Widget", state::SDL_FORMAT).await;
    assert_eq!(
        read.status, 200,
        "case after the refusals: expected the other API's type to survive, got {read:?}"
    );
    assert_eq!(
        read.body["type"]["definition"].as_str(),
        Some(rendered_sdl(definition).as_str()),
        "case after the refusals: expected the other API's definition to be unchanged, got {read:?}"
    );
}

/// While a schema creation of the API is still owed, that API's type system is already
/// being modified, so an update of one of its types is refused — and once the creation has
/// been performed, the types its document declares are updatable like any other.
#[tokio::test]
async fn an_update_during_a_schema_creation_is_refused_then_allowed() {
    let router = router();
    let api_id = create_api(&router, "type-update-schema-creation").await;

    // The document declares one type per kind the catalog spans, so what the creation
    // installs — and what the updates afterwards address — is not one kind of definition.
    let declared = rows();
    let document: String = declared
        .iter()
        .map(|row| row.definition.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let started = start_schema_creation(&router, &api_id, &document).await;
    assert_eq!(
        (started.status, started.body["status"].as_str()),
        (200, Some(PROCESSING)),
        "case start the creation: expected 200 {PROCESSING:?}, got {started:?}"
    );

    // Every name the document declares, and one it does not: while the creation is owed,
    // the modification in progress is what the update is refused for, whichever name it
    // addresses and whatever it carries.
    for type_name in declared
        .iter()
        .map(|row| row.name)
        .chain(std::iter::once("NeverDeclared"))
    {
        let refused = update_type(
            &router,
            &api_id,
            type_name,
            Some(&format!("scalar {type_name}\n")),
            state::SDL_FORMAT,
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (409, "ConcurrentModificationException"),
            "case update {type_name:?} while the creation is owed: expected a 409 \
             ConcurrentModificationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            CONCURRENT_MODIFICATION,
            "case update {type_name:?} while the creation is owed: expected the \
             concurrent-modification text, got {refused:?}"
        );
    }

    // The refusals left the creation where it was, so the clock still performs it: the
    // wait looks again for that rather than for a fixed time.
    await_creation(&router, &api_id, "the creation the refusals left alone").await;

    for row in &declared {
        let case = format!("update the installed {:?} ({})", row.name, row.kind);
        // The replacement is a DIFFERENT definition of the same name — a described scalar
        // — so what the read answers afterwards can only come from this update.
        let replacement = format!("\"\"\"Updated after the creation.\"\"\"\nscalar {}\n", row.name);
        let updated = update_type(
            &router,
            &api_id,
            row.name,
            Some(&replacement),
            state::SDL_FORMAT,
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case}: expected UpdateType 200 once the creation is performed, got {updated:?}"
        );
        for (member, expected) in [
            ("name", json!(row.name)),
            ("definition", json!(replacement)),
            ("description", json!("Updated after the creation.")),
            ("format", json!(state::SDL_FORMAT)),
        ] {
            assert_eq!(
                updated.body["type"][member], expected,
                "case {case}: expected {member} {expected:?}, got {:?}",
                updated.body["type"][member]
            );
        }

        let read = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected GetType 200 after the update, got {read:?}"
        );
        assert_eq!(
            read.body["type"]["definition"], json!(rendered_sdl(&replacement)),
            "case {case}: expected the read to answer the new definition, got {read:?}"
        );
    }

    // A name the document never declared is still a not-found, not a 409: nothing is in
    // progress any more.
    let absent = update_type(
        &router,
        &api_id,
        "NeverDeclared",
        Some("scalar NeverDeclared\n"),
        state::SDL_FORMAT,
    )
    .await;
    assert_eq!(
        (absent.status, absent.error_type.as_str()),
        (404, "NotFoundException"),
        "case update a never-declared name after the creation: expected a 404 \
         NotFoundException, got {absent:?}"
    );
}
