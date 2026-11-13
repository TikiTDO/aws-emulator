// SPDX-License-Identifier: CC0-1.0

//! `DeleteType`, end to end over HTTP: what a delete removes, and which deletes it
//! refuses.
//!
//! The rule asserted here is stated over CLASSES of input rather than over the one
//! request that exposed the defect: a delete removes exactly the type the request
//! addresses by name from the API that holds it and answers an empty 200, so afterwards
//! every reader of that API answers without it (`GetType` a 404 `NotFoundException`,
//! in either `TypeDefinitionFormat`) while every other type — of that API and
//! of any other — is untouched and the freed name is creatable again; a request that
//! addresses no type removes nothing and is refused: a type name the API does not hold
//! and an `apiId` this account does not hold are 404s, an `apiId` that cannot name an API
//! at all is a 400, and a delete of a type system a schema creation is still modifying is
//! a 409.
//!
//! The rows span:
//!
//! 1. every kind of type a GraphQL SDL definition declares (object, interface, union,
//!    enum, input object, scalar), so the delete cannot be keyed to one kind of
//!    definition;
//! 2. the position of the deleted name among the types its API holds (first, middle,
//!    last), and the count of types the API holds, from several down to none;
//! 3. both `TypeDefinitionFormat` values on every read-back, since the format chooses the
//!    representation a report carries and not whether the type is there;
//! 4. the forms of a request that addresses nothing: a never-created name, a name an
//!    earlier delete already removed, a name another API holds, an `apiId` no API has, an
//!    `apiId` too long to be one, and an API whose schema creation is still owed;
//! 5. how the type got into the API: created one at a time, and installed by a whole
//!    schema document.
//!
//! The harness (`router`, `call`, `create_api`, `create_type`, `get_type`, the type
//! catalog `rows()` and the projection assertion) is the `graphql_types` suite's, reused
//! rather than copied.

use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::state;
use smithy_server_runtime::MultiServiceRouter;

use super::graphql_api_lifecycle::router;
use super::graphql_types::{
    assert_projects, assert_states, call, create_api, create_type, get_type, rows, Answer, Row,
    FORMATS,
};

/// How long a case that must observe the far side of a scheduled schema creation waits:
/// the service's own span plus slack. Only the TEST waits — the handler stores the due
/// instant.
const PAST_THE_CREATION: Duration = Duration::from_millis(250);

/// The text a `NotFoundException` carries for a type the addressed API does not hold: a
/// fixed sentence with no variable part. Pinned here rather than read from the service, so
/// a drift in the wording fails this suite.
const TYPE_NOT_FOUND: &str = "Type not found.";

/// The text a `ConcurrentModificationException` carries, as the model documents the
/// shape. Pinned here for the same reason.
const CONCURRENT_MODIFICATION: &str =
    "Another modification is in progress at this time and it must complete before you can \
     make your change.";

/// The non-terminal `SchemaStatus` a scheduled schema creation is in. Source:
/// `smithy_query` shape `SchemaStatus`.
const PROCESSING: &str = "PROCESSING";

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

/// Assert `answer` is the empty 200 the modelled output (no members) serializes to.
fn assert_empty_success(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected DeleteType 200, got {answer:?}"
    );
    assert!(
        answer
            .body
            .as_object()
            .is_none_or(serde_json::Map::is_empty),
        "case {case}: expected an empty body, got {answer:?}"
    );
}

/// The `message` of an error body, or the empty string when there is none.
fn message(answer: &Answer) -> &str {
    answer
        .body
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Assert `api_id` holds no type called `type_name` any more: the read is a 404 carrying
/// the service's absent-type text, in either format.
async fn assert_gone(router: &MultiServiceRouter, api_id: &str, type_name: &str, case: &str) {
    for format in FORMATS {
        let read = get_type(router, api_id, type_name, format).await;
        assert_eq!(
            read.status, 404,
            "case {case} ({format}): expected GetType 404 after the delete, got {read:?}"
        );
        assert_eq!(
            read.error_type, "NotFoundException",
            "case {case} ({format}): expected NotFoundException after the delete, got {read:?}"
        );
        assert_eq!(
            message(&read),
            TYPE_NOT_FOUND,
            "case {case} ({format}): expected the absent-type refusal text, got {read:?}"
        );
    }
}

/// Assert every row in `survivors` still reads back as the type it declares, in both
/// formats: the delete removed one record, not a class.
async fn assert_survive(
    router: &MultiServiceRouter,
    api_id: &str,
    survivors: &[Row],
    case: &str,
) {
    for row in survivors {
        for format in FORMATS {
            let read = get_type(router, api_id, row.name, format).await;
            assert_eq!(
                read.status, 200,
                "case {case} ({format}): expected {:?} to survive, got {read:?}",
                row.name
            );
            assert_projects(row, api_id, format, &read.body["type"], case);
        }
    }
}

/// A delete removes exactly the named type, whatever kind of definition declared it,
/// whatever its position among the API's types and however many the API holds — down to
/// the last one.
#[tokio::test]
async fn a_delete_removes_exactly_the_named_type_of_every_kind() {
    let router = router();
    let api_id = create_api(&router, "type-delete-every-kind").await;

    for row in rows() {
        let created = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case create {:?}: expected 200, got {created:?}",
            row.name
        );
    }

    // The deletion order walks the first remaining type, the last, and then one from the
    // middle each time, so the removed name's position varies across the rows and the API
    // is driven from full down to empty.
    let mut remaining = rows();
    let mut step = 0usize;
    while !remaining.is_empty() {
        let position = match step % 3 {
            0 => 0,
            1 => remaining.len() - 1,
            _ => remaining.len() / 2,
        };
        let row = remaining.remove(position);
        let case = format!(
            "delete {:?} ({}) at position {position} of {}",
            row.name,
            row.kind,
            remaining.len() + 1
        );

        // The type is readable before the delete, so the 404 after it is the delete's own
        // effect.
        let before = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            before.status, 200,
            "case {case}: expected the type to exist before the delete, got {before:?}"
        );

        let deleted = delete_type(&router, &api_id, row.name).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &api_id, row.name, &case).await;
        assert_survive(&router, &api_id, &remaining, &case).await;

        step += 1;
    }
}

/// A deleted name is free again: the identical create is accepted after the delete and
/// reads back as the type it declares, so the delete reserved nothing.
#[tokio::test]
async fn a_deleted_name_can_be_created_again() {
    let router = router();
    let api_id = create_api(&router, "type-delete-recreate").await;

    for row in rows() {
        let case = format!("recreate {:?} ({}) after a delete", row.name, row.kind);
        let created = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the first create to be 200, got {created:?}"
        );

        let deleted = delete_type(&router, &api_id, row.name).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &api_id, row.name, &case).await;

        let recreated = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            recreated.status, 200,
            "case {case}: expected the freed name to be creatable again, got {recreated:?}"
        );
        assert_states(
            &row,
            &api_id,
            state::SDL_FORMAT,
            &recreated.body["type"],
            &case,
        );
        let read = get_type(&router, &api_id, row.name, state::SDL_FORMAT).await;
        assert_eq!(
            read.status, 200,
            "case {case}: expected the recreated type to read back, got {read:?}"
        );
    }
}

/// A request that addresses no type removes nothing and says which refusal it is: a name
/// nothing holds (including one an earlier delete removed) is a 404, an `apiId` this
/// account does not hold is a 404, and an `apiId` too long to be one is a 400.
#[tokio::test]
async fn a_delete_that_addresses_nothing_is_refused_and_removes_nothing() {
    let router = router();
    let api_id = create_api(&router, "type-delete-refusals").await;
    let kept = rows();
    for row in &kept {
        let created = create_type(&router, &api_id, row.definition, state::SDL_FORMAT).await;
        assert_eq!(
            created.status, 200,
            "case create {:?}: expected 200, got {created:?}",
            row.name
        );
    }

    // A name the API never held, and one an earlier delete already removed: neither
    // addresses a type, so both take the same refusal.
    let doomed = "DeletedTwice";
    let created = create_type(
        &router,
        &api_id,
        &format!("scalar {doomed}\n"),
        state::SDL_FORMAT,
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case create {doomed:?}: expected 200, got {created:?}"
    );
    let first = delete_type(&router, &api_id, doomed).await;
    assert_empty_success("the first delete of a name", &first);

    for (case, type_name) in [
        ("a never-created name", "NeverCreated"),
        ("a name an earlier delete removed", doomed),
    ] {
        let answer = delete_type(&router, &api_id, type_name).await;
        assert_eq!(
            answer.status, 404,
            "case delete {case}: expected 404, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, "NotFoundException",
            "case delete {case}: expected NotFoundException, got {answer:?}"
        );
        assert_eq!(
            message(&answer),
            TYPE_NOT_FOUND,
            "case delete {case}: expected the absent-type refusal text, got {answer:?}"
        );
    }

    // An `apiId` of the right shape that this account does not hold, and one longer than
    // an API id, which cannot name an API at all.
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
        let answer = delete_type(&router, &api, kept[0].name).await;
        assert_eq!(
            answer.status, expected_status,
            "case delete under {case}: expected {expected_status}, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, expected_error,
            "case delete under {case}: expected {expected_error}, got {answer:?}"
        );
    }

    // None of those refusals touched a type that does exist.
    assert_survive(&router, &api_id, &kept, "after the refusals").await;
}

/// A type belongs to the API it was created under, so deleting one by name under another
/// API removes nothing: the name is addressed within its own API.
#[tokio::test]
async fn a_delete_under_another_api_removes_nothing() {
    let router = router();
    let first = create_api(&router, "type-delete-scope-first").await;
    let second = create_api(&router, "type-delete-scope-second").await;

    for row in rows() {
        let case = format!("delete {:?} ({}) under the other API", row.name, row.kind);
        // The SAME definition under both APIs: two different types.
        for api_id in [&first, &second] {
            let created = create_type(&router, api_id, row.definition, state::SDL_FORMAT).await;
            assert_eq!(
                created.status, 200,
                "case {case}: expected the create under {api_id} to be 200, got {created:?}"
            );
        }

        let deleted = delete_type(&router, &second, row.name).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &second, row.name, &case).await;

        // The first API's type is exactly where it was.
        assert_survive(
            &router,
            &first,
            std::slice::from_ref(&row),
            &format!("{case}, the other API's type"),
        )
        .await;
    }
}

/// While a schema creation of the API is still owed, that API's type system is already
/// being modified, so a delete of one of its types is refused — and once the creation has
/// been performed, the types its document declares are deletable like any other.
#[tokio::test]
async fn a_delete_during_a_schema_creation_is_refused_then_allowed() {
    let router = router();
    let api_id = create_api(&router, "type-delete-schema-creation").await;

    // The document declares one type per kind the catalog spans, so what the creation
    // installs — and what the deletes afterwards address — is not one kind of definition.
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
    // the modification in progress is what the delete is refused for, whichever name it
    // addresses.
    for type_name in declared
        .iter()
        .map(|row| row.name)
        .chain(std::iter::once("NeverDeclared"))
    {
        let refused = delete_type(&router, &api_id, type_name).await;
        assert_eq!(
            (refused.status, refused.error_type.as_str()),
            (409, "ConcurrentModificationException"),
            "case delete {type_name:?} while the creation is owed: expected a 409 \
             ConcurrentModificationException, got {refused:?}"
        );
        assert_eq!(
            message(&refused),
            CONCURRENT_MODIFICATION,
            "case delete {type_name:?} while the creation is owed: expected the \
             concurrent-modification text, got {refused:?}"
        );
    }

    // The refusals left the creation where it was, so the clock still performs it.
    tokio::time::sleep(state::SCHEMA_CREATION_SPAN + PAST_THE_CREATION).await;

    let mut remaining = declared.clone();
    while !remaining.is_empty() {
        let row = remaining.remove(0);
        let case = format!("delete the installed {:?} ({})", row.name, row.kind);
        let deleted = delete_type(&router, &api_id, row.name).await;
        assert_empty_success(&case, &deleted);
        assert_gone(&router, &api_id, row.name, &case).await;
    }

    // A name the document never declared is still a not-found, not a 409: nothing is in
    // progress any more.
    let absent = delete_type(&router, &api_id, "NeverDeclared").await;
    assert_eq!(
        absent.status, 404,
        "case delete a never-declared name after the creation: expected 404, got {absent:?}"
    );
    assert_eq!(
        message(&absent),
        TYPE_NOT_FOUND,
        "case delete a never-declared name after the creation: expected the absent-type \
         refusal text, got {absent:?}"
    );
}
