// SPDX-License-Identifier: CC0-1.0

//! The MediaLive SdiSource lifecycle, end to end over the restJson1 wire.
//!
//! Every test drives the service through the shared [`crate::wire`] harness, so
//! what is asserted is the emulator's wire behaviour, not an internal function's
//! return value.
//!
//! The suites are tables: the recorded case in the issue is ONE row of each.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, medialive, ACCOUNT};

/// The collection URI `CreateSdiSource` / `ListSdiSources` are bound to
/// (`smithy_query(query="operation", operation="CreateSdiSource")`:
/// `POST /prod/sdiSources`).
const SDI_SOURCES: &str = "/prod/sdiSources";

/// Every value `SdiSourceType` accepts, from
/// `smithy_query(query="shape", shape="SdiSourceType")`.
const TYPES: [&str; 2] = ["SINGLE", "QUAD"];

/// Every value `SdiSourceMode` accepts, from
/// `smithy_query(query="shape", shape="SdiSourceMode")`.
const MODES: [&str; 2] = ["QUADRANT", "INTERLEAVE"];

async fn create_source(router: &MultiServiceRouter, body: Value) -> (u16, Value) {
    let (status, response) = call(router, "POST", SDI_SOURCES, Some(body)).await;
    (status, response["sdiSource"].clone())
}

/// A created SdiSource, with the create itself asserted to have succeeded.
///
/// `201 Created` is the recorded status of a MediaLive create (see
/// `CreateSdiSource`'s `SUCCESS_STATUS`).
///
/// Shared with the node settings suite, whose SDI source mappings have to name
/// a source this account holds, so the two suites create one the same way.
pub(crate) async fn created(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let (status, source) = create_source(router, body).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateSdiSource: expected 201, got {status}: {source}"
    );
    source
}

/// `DescribeSdiSource` on one source, answering its status and the source body
/// (or the refusal). Shared with the suite that pins `DeleteSdiSource`'s
/// mapping precondition, which reads the source back to show a refused delete
/// wrote nothing.
pub(crate) async fn describe_source(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    let (status, response) = call(router, "GET", &format!("{SDI_SOURCES}/{id}"), None).await;
    if status == 200 {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

async fn update_source(
    router: &MultiServiceRouter,
    id: &str,
    body: Value,
) -> (u16, Value) {
    let (status, response) =
        call(router, "PUT", &format!("{SDI_SOURCES}/{id}"), Some(body)).await;
    if status == 200 {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

/// `DeleteSdiSource` on one source, which puts it into the terminal `DELETED`
/// state. Shared with the node settings suite, whose rule is that a deleted
/// source is nothing a new SDI source mapping can connect a port to.
pub(crate) async fn delete_source(router: &MultiServiceRouter, id: &str) -> (u16, Value) {
    let (status, response) = call(router, "DELETE", &format!("{SDI_SOURCES}/{id}"), None).await;
    if status == 200 {
        (status, response["sdiSource"].clone())
    } else {
        (status, response)
    }
}

/// The sources `ListSdiSources` reports, walking every page it hands out.
async fn listed_sources(router: &MultiServiceRouter, max_results: Option<i64>) -> Vec<Value> {
    let mut sources = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..32 {
        let uri = match (max_results, token.as_deref()) {
            (Some(max), Some(next)) => {
                format!("{SDI_SOURCES}?maxResults={max}&nextToken={next}")
            }
            (Some(max), None) => format!("{SDI_SOURCES}?maxResults={max}"),
            (None, Some(next)) => format!("{SDI_SOURCES}?nextToken={next}"),
            (None, None) => SDI_SOURCES.to_string(),
        };
        let (status, body) = call(router, "GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "ListSdiSources({uri}): expected 200, got {status}: {body}"
        );
        let page = body["sdiSources"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("ListSdiSources({uri}): expected a list, got {body}"));
        if let Some(max) = max_results {
            assert!(
                page.len() as i64 <= max,
                "maxResults {max}: expected at most {max} source(s) in a page, got {}",
                page.len()
            );
        }
        sources.extend(page);
        match body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return sources,
        }
    }
    panic!(
        "ListSdiSources handed out more than 32 pages for {} source(s)",
        sources.len()
    );
}

/// The ids `ListSdiSources` reports, in the order it reports them.
async fn listed_ids(router: &MultiServiceRouter, max_results: Option<i64>) -> Vec<String> {
    listed_sources(router, max_results)
        .await
        .iter()
        .map(|source| source["id"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Rule: `CreateSdiSource` mints a numeric `Id`, builds the `Arn` from the
/// request's region and that id, stores the `Name`, `Type` and `Mode` the request
/// supplied, and reports the source as `IDLE` with no attached `Inputs` — and
/// `DescribeSdiSource` reads that same stored source back.
///
/// The rows span the `SdiSourceType` and `SdiSourceMode` enums (`TYPES` /
/// `MODES`) and the rows that omit each member, because that is what the rule
/// generalises over.
#[tokio::test]
async fn a_created_sdi_source_is_read_back_by_describe() {
    // (case, request, expected type, expected mode)
    let mut rows: Vec<(String, Value, Option<&str>, Option<&str>)> = Vec::new();
    for r#type in TYPES {
        rows.push((
            format!("{type} with no mode"),
            json!({ "name": format!("camera-{type}"), "type": r#type }),
            Some(r#type),
            None,
        ));
        for mode in MODES {
            rows.push((
                format!("{type} in {mode}"),
                json!({ "name": format!("camera-{type}-{mode}"), "type": r#type, "mode": mode }),
                Some(r#type),
                Some(mode),
            ));
        }
    }
    rows.push((
        "name only".to_string(),
        json!({ "name": "curling-cameraA" }),
        None,
        None,
    ));
    rows.push(("nothing at all".to_string(), json!({}), None, None));

    for (case, request, expected_type, expected_mode) in rows {
        let router = medialive();
        let source = created(&router, &case, request.clone()).await;

        let id = source["id"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a numeric SdiSource id, got {:?}",
            source["id"]
        );
        assert_eq!(
            source["arn"],
            json!(format!("arn:aws:medialive:us-west-2:{ACCOUNT}:sdiSource:{id}")),
            "case {case:?}: expected the ARN built from the signed region and the minted id, got {:?}",
            source["arn"]
        );
        assert_eq!(
            source["name"],
            request.get("name").cloned().unwrap_or(Value::Null),
            "case {case:?}: expected the requested name, got {:?}",
            source["name"]
        );
        assert_eq!(
            source["type"],
            expected_type.map_or(Value::Null, |value| json!(value)),
            "case {case:?}: expected type {expected_type:?}, got {:?}",
            source["type"]
        );
        assert_eq!(
            source["mode"],
            expected_mode.map_or(Value::Null, |value| json!(value)),
            "case {case:?}: expected mode {expected_mode:?}, got {:?}",
            source["mode"]
        );
        assert_eq!(
            source["state"],
            json!("IDLE"),
            "case {case:?}: expected a source no input is attached to to be IDLE, got {:?}",
            source["state"]
        );
        assert_eq!(
            source["inputs"],
            json!([]),
            "case {case:?}: expected no inputs to be using a new source, got {:?}",
            source["inputs"]
        );

        // The round trip: the reader answers with the stored source, member for
        // member, not with anything derived from its own request.
        let (status, described) = describe_source(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSdiSource: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described, source,
            "case {case:?}: expected DescribeSdiSource to report the created source, got {described}"
        );

        // And the collection reports exactly that one source.
        assert_eq!(
            listed_ids(&router, None).await,
            vec![id.clone()],
            "case {case:?}: expected ListSdiSources to carry only the created source"
        );
    }
}

/// Rule: `UpdateSdiSource` changes the settings the request names and leaves
/// every setting it omits as it was, keeping the source's identity (`Id`, `Arn`).
///
/// The rows differ along which members the update carries, including none at
/// all, because that is what the rule generalises over.
#[tokio::test]
async fn an_update_changes_only_the_settings_it_names() {
    // (case, update body, expected name, expected type, expected mode)
    let rows: [(&str, Value, &str, &str, &str); 5] = [
        ("name only", json!({ "name": "renamed" }), "renamed", "QUAD", "QUADRANT"),
        ("mode only", json!({ "mode": "INTERLEAVE" }), "base", "QUAD", "INTERLEAVE"),
        ("type only", json!({ "type": "SINGLE" }), "base", "SINGLE", "QUADRANT"),
        (
            "every member",
            json!({ "name": "all-three", "type": "SINGLE", "mode": "INTERLEAVE" }),
            "all-three",
            "SINGLE",
            "INTERLEAVE",
        ),
        ("no member", json!({}), "base", "QUAD", "QUADRANT"),
    ];

    for (case, body, name, r#type, mode) in rows {
        let router = medialive();
        let source = created(
            &router,
            case,
            json!({ "name": "base", "type": "QUAD", "mode": "QUADRANT" }),
        )
        .await;
        let id = source["id"].as_str().unwrap_or_default().to_string();

        let (status, updated) = update_source(&router, &id, body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateSdiSource: expected 200, got {status}: {updated}"
        );
        assert_eq!(
            updated["name"],
            json!(name),
            "case {case:?}: expected name {name}, got {:?}",
            updated["name"]
        );
        assert_eq!(
            updated["type"],
            json!(r#type),
            "case {case:?}: expected type {type}, got {:?}",
            updated["type"]
        );
        assert_eq!(
            updated["mode"],
            json!(mode),
            "case {case:?}: expected mode {mode}, got {:?}",
            updated["mode"]
        );
        assert_eq!(
            updated["id"], source["id"],
            "case {case:?}: expected the update to keep the id, got {:?}",
            updated["id"]
        );
        assert_eq!(
            updated["arn"], source["arn"],
            "case {case:?}: expected the update to keep the ARN, got {:?}",
            updated["arn"]
        );

        // The write landed in the store, not just in the response.
        let (status, described) = describe_source(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSdiSource: expected 200, got {status}: {described}"
        );
        assert_eq!(
            described, updated,
            "case {case:?}: expected DescribeSdiSource to report the updated source, got {described}"
        );
    }
}

/// Rule: a create retried with the `RequestId` (`@idempotencyToken`) of an
/// earlier one answers with the source that one created; two creates that do not
/// share a token create two sources.
#[tokio::test]
async fn a_retried_create_returns_the_source_the_first_one_created() {
    // (case, first RequestId, second RequestId, how many sources exist after)
    let rows: [(&str, Option<&str>, Option<&str>, usize); 4] = [
        ("the same token", Some("token-a"), Some("token-a"), 1),
        ("two tokens", Some("token-a"), Some("token-b"), 2),
        ("no token at all", None, None, 2),
        ("only the second names one", None, Some("token-b"), 2),
    ];

    for (case, first_token, second_token, expected) in rows {
        let router = medialive();
        let mut first = json!({ "name": "first", "type": "SINGLE" });
        if let Some(token) = first_token {
            first["requestId"] = json!(token);
        }
        let mut second = json!({ "name": "second", "type": "QUAD" });
        if let Some(token) = second_token {
            second["requestId"] = json!(token);
        }

        let one = created(&router, case, first).await;
        let two = created(&router, case, second).await;
        let ids = listed_ids(&router, None).await;
        assert_eq!(
            ids.len(),
            expected,
            "case {case:?}: expected {expected} source(s) to exist, got {ids:?}"
        );

        if expected == 1 {
            assert_eq!(
                two, one,
                "case {case:?}: expected the retry to answer with the first source, got {two}"
            );
        } else {
            assert_ne!(
                two["id"], one["id"],
                "case {case:?}: expected two distinct sources, got {:?} twice",
                one["id"]
            );
            // The second create's own values are what the second source carries.
            assert_eq!(
                two["name"],
                json!("second"),
                "case {case:?}: expected the second source to carry its own name, got {:?}",
                two["name"]
            );
        }
    }
}

/// Rule: `ListSdiSources` walks every stored source in id order, in pages of at
/// most `MaxResults`, handing out a `NextToken` only while sources are left; a
/// non-positive `MaxResults` is refused.
#[tokio::test]
async fn list_pages_through_every_source_in_id_order() {
    let router = medialive();
    let mut minted = Vec::new();
    for index in 0..5 {
        let source = created(
            &router,
            "paging",
            json!({ "name": format!("camera-{index}"), "type": "SINGLE" }),
        )
        .await;
        minted.push(source["id"].as_str().unwrap_or_default().to_string());
    }
    let mut expected = minted.clone();
    expected.sort();

    // Page sizes below, at and above the number of stored sources, plus the
    // request that names none.
    for max_results in [Some(1), Some(2), Some(5), Some(7), None] {
        let ids = listed_ids(&router, max_results).await;
        assert_eq!(
            ids, expected,
            "maxResults {max_results:?}: expected every source once, in id order, got {ids:?}"
        );
    }

    // A page size that asks for nothing is a bad request, not an empty page.
    for max_results in [0, -1] {
        let (status, body) =
            call(&router, "GET", &format!("{SDI_SOURCES}?maxResults={max_results}"), None).await;
        assert_eq!(
            status, 400,
            "maxResults {max_results}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "BadRequestException",
            "maxResults {max_results}: expected BadRequestException, got {body}"
        );
    }
}

/// Rule: an SdiSource's `Inputs` is every input whose `SdiSources` names it and
/// its `State` is `IN_USE` exactly while that list is non-empty, so a source an
/// input is using cannot be deleted ("The SdiSource must not be part of any
/// SdiSourceMapping and must not be attached to any input").
///
/// The rows differ along how many inputs use the source and along which of a
/// multi-source input's entries it is, because that is what the rule generalises
/// over.
#[tokio::test]
async fn a_source_an_input_uses_is_in_use_by_that_input() {
    // (case, how many inputs name the source, how many other sources each names)
    let rows: [(&str, usize, usize); 3] = [
        ("one input, sole source", 1, 0),
        ("one input, one other source too", 1, 1),
        ("two inputs", 2, 0),
    ];

    for (case, users, others) in rows {
        let router = medialive();
        let source = created(&router, case, json!({ "name": "wired", "type": "SINGLE" })).await;
        let id = source["id"].as_str().unwrap_or_default().to_string();

        // The other sources exist so the input's list is not just this one id.
        let mut sdi_sources = vec![json!(id.clone())];
        for other in 0..others {
            let extra = created(
                &router,
                case,
                json!({ "name": format!("other-{other}"), "type": "SINGLE" }),
            )
            .await;
            sdi_sources.push(extra["id"].clone());
        }

        let mut input_ids = Vec::new();
        for user in 0..users {
            let (status, response) = call(
                &router,
                "POST",
                "/prod/inputs",
                Some(json!({
                    "name": format!("sdi-input-{user}"),
                    "type": "SDI",
                    "sdiSources": sdi_sources.clone(),
                })),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: CreateInput: expected 201, got {status}: {response}"
            );
            input_ids.push(
                response["input"]["id"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        input_ids.sort();

        let (status, described) = describe_source(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSdiSource: expected 200, got {status}: {described}"
        );
        let reported: Vec<String> = described["inputs"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|input| input.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            reported, input_ids,
            "case {case:?}: expected the inputs using the source, got {:?}",
            described["inputs"]
        );
        assert_eq!(
            described["state"],
            json!("IN_USE"),
            "case {case:?}: expected a source an input uses to be IN_USE, got {:?}",
            described["state"]
        );

        // A source in use cannot be deleted.
        let (status, body) = delete_source(&router, &id).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected deleting a source in use to conflict, got {status}: {body}"
        );
        assert_eq!(
            error_code(status, &body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );

        // And it is still there, still IN_USE.
        let (status, after) = describe_source(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSdiSource: expected 200, got {status}: {after}"
        );
        assert_eq!(
            after, described,
            "case {case:?}: expected the refused delete to change nothing, got {after}"
        );
    }
}

/// Rule: `DeleteSdiSource` answers 200 with the source it deleted, which settles
/// into the terminal `DELETED` state with no attached inputs and stays in the
/// account under that state; an `SdiSourceId` that names no live source is
/// refused with `NotFoundException`.
#[tokio::test]
async fn a_deleted_source_settles_into_the_terminal_deleted_state() {
    let router = medialive();
    let first = created(&router, "delete", json!({ "name": "keep", "type": "SINGLE" })).await;
    let second = created(
        &router,
        "delete",
        json!({ "name": "remove", "type": "QUAD", "mode": "QUADRANT" }),
    )
    .await;
    let kept = first["id"].as_str().unwrap_or_default().to_string();
    let removed = second["id"].as_str().unwrap_or_default().to_string();

    let (status, deleted) = delete_source(&router, &removed).await;
    assert_eq!(
        status, 200,
        "DeleteSdiSource: expected 200, got {status}: {deleted}"
    );
    assert_eq!(
        deleted["state"],
        json!("DELETED"),
        "expected DeleteSdiSource to answer DELETED, got {:?}",
        deleted["state"]
    );
    assert_eq!(
        deleted["inputs"],
        json!([]),
        "expected no inputs on a just-deleted source, got {:?}",
        deleted["inputs"]
    );
    for member in ["id", "arn", "name", "type", "mode"] {
        assert_eq!(
            deleted[member], second[member],
            "case {member:?}: expected the delete to report the source it deleted, got {:?}",
            deleted[member]
        );
    }

    // `DELETED` is terminal and the source keeps it: an independent read answers
    // with the same source in that state rather than losing it.
    let (status, described) = describe_source(&router, &removed).await;
    assert_eq!(
        status, 200,
        "DescribeSdiSource after a delete: expected 200, got {status}: {described}"
    );
    assert_eq!(
        described, deleted,
        "expected DescribeSdiSource to report the deleted source, got {described}"
    );

    // Deleting it again changes nothing — it is already DELETED.
    let (status, again) = delete_source(&router, &removed).await;
    assert_eq!(
        status, 200,
        "DeleteSdiSource on a DELETED source: expected 200, got {status}: {again}"
    );
    assert_eq!(
        again, deleted,
        "expected a repeated delete to answer with the same DELETED source, got {again}"
    );

    // A terminal source cannot be changed: there is no live source with that id.
    let (status, body) = update_source(&router, &removed, json!({ "name": "ghost" })).await;
    assert_eq!(
        status, 404,
        "UpdateSdiSource on a DELETED source: expected 404, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "NotFoundException",
        "expected NotFoundException, got {body}"
    );
    // The refusal names the id the request asked for and says it is already
    // deleted, which is what tells it apart from an id no source ever had.
    assert_eq!(
        body["message"],
        json!(format!("SdiSource {removed} already deleted")),
        "expected the refusal to name the deleted id, got {body}"
    );

    // Both sources are still in the account, each reporting its own state.
    let listed = listed_sources(&router, None).await;
    let states: Vec<(String, String)> = listed
        .iter()
        .map(|source| {
            (
                source["id"].as_str().unwrap_or_default().to_string(),
                source["state"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let mut expected = vec![
        (kept.clone(), "IDLE".to_string()),
        (removed.clone(), "DELETED".to_string()),
    ];
    expected.sort();
    assert_eq!(
        states, expected,
        "expected ListSdiSources to report both sources with their own states, got {states:?}"
    );

    // An id no source has is a different answer from a deleted one.
    let unknown = "9999999";
    let (status, body) = describe_source(&router, unknown).await;
    assert_eq!(
        status, 404,
        "DescribeSdiSource({unknown}): expected 404, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "NotFoundException",
        "DescribeSdiSource({unknown}): expected NotFoundException, got {body}"
    );

    let (status, body) = delete_source(&router, unknown).await;
    assert_eq!(
        status, 404,
        "DeleteSdiSource({unknown}): expected 404, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "NotFoundException",
        "DeleteSdiSource({unknown}): expected NotFoundException, got {body}"
    );

    // `UpdateSdiSource`'s own error list omits `NotFoundException`
    // (`smithy_query(query="operation", operation="UpdateSdiSource")`), but the
    // real service answers an id that names no live source with it, so it is
    // widened in: an unknown id gets the same refusal a deleted one does.
    let (status, body) = update_source(&router, unknown, json!({ "name": "ghost" })).await;
    assert_eq!(
        status, 404,
        "UpdateSdiSource({unknown}): expected 404, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, &body),
        "NotFoundException",
        "UpdateSdiSource({unknown}): expected NotFoundException, got {body}"
    );
    assert!(
        body["message"].as_str().unwrap_or_default().contains(unknown),
        "UpdateSdiSource({unknown}): expected the refusal to name the id, got {body}"
    );

    // And none of those refusals touched the source that was left alone.
    let (status, survivor) = describe_source(&router, &kept).await;
    assert_eq!(
        status, 200,
        "expected the other source to survive, got {status}: {survivor}"
    );
    assert_eq!(
        survivor, first,
        "expected the surviving source to be unchanged, got {survivor}"
    );
}
