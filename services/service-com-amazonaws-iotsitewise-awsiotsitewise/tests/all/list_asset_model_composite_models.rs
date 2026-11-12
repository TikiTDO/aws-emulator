// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAssetModelCompositeModels`, driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with, so every assertion is
//! about the wire behaviour a real client sees.
//!
//! The rule these suites pin: the listing enumerates the composite models the
//! named asset model actually holds in the shared engine store — every one of
//! them, at any nesting depth — in one stable order, cut into pages of at most
//! `maxResults` (default 50) joined by a resumable `nextToken`, reporting each
//! summary from the stored composite model the describes read back, and refusing
//! a label, a version alias or a token that names nothing stored.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, hex_token, router};
use super::asset_model_composite_model::PAST_THE_OWED_TRANSITION;

/// The ids the fixture's composite models carry, so the expected enumeration
/// order and every page token are known to the test without asking the emulator.
/// Each is a lowercase hyphenated UUID with RFC 4122's version-4 nibble, which is
/// what the modelled `ID` pattern admits.
const IDS: [&str; 5] = [
    "11111111-1111-4111-8111-111111111111",
    "22222222-2222-4222-8222-222222222222",
    "33333333-3333-4333-8333-333333333333",
    "44444444-4444-4444-8444-444444444444",
    "55555555-5555-4555-8555-555555555555",
];

/// The external id the fixture asset model carries, so the `externalId:` label
/// form of `assetModelId` is exercised.
const MODEL_EXTERNAL_ID: &str = "fixture_asset_model";

/// A `CustomID`-shaped id no create ever minted, for the refusal rows.
const ABSENT_ID: &str = "99999999-9999-4999-8999-999999999999";

/// One row of the fixture: a composite model the model ends up holding, and what
/// the request that created it asked for.
struct Row {
    /// The composite model's id, fixed by the request that created it.
    id: &'static str,
    /// Its name.
    name: &'static str,
    /// The expected `path` BELOW the asset model root, as the ids from the
    /// outermost ancestor down to it.
    path: Vec<&'static str>,
}

/// The fixture's composite models, in the order the creates append them — which
/// is the order the listing enumerates.
///
/// One row per thing the rule generalises over: the composite model type (the
/// whole documented catalog — `AWS/ALARM`, `CUSTOM`, `AWS/L4E_ANOMALY`, per the
/// `AssetModelCompositeModelSummary$type` documentation "Valid values are
/// AWS/ALARM, CUSTOM, or AWS/L4E_ANOMALY"), how the composite model came to be
/// (inline in `CreateAssetModel`, or its own `CreateAssetModelCompositeModel`
/// call), whether it carries an `externalId` or a `description`, and its nesting
/// depth (1, 2 and 3).
fn rows() -> Vec<Row> {
    vec![
        Row {
            id: IDS[0],
            name: "Alarm",
            path: vec![IDS[0]],
        },
        Row {
            id: IDS[1],
            name: "Plain",
            path: vec![IDS[1]],
        },
        Row {
            id: IDS[2],
            name: "Anomaly",
            path: vec![IDS[2]],
        },
        Row {
            id: IDS[3],
            name: "Nested",
            path: vec![IDS[0], IDS[3]],
        },
        Row {
            id: IDS[4],
            name: "Deeper",
            path: vec![IDS[0], IDS[3], IDS[4]],
        },
    ]
}

/// The ids the listing must enumerate, in order.
fn expected_ids() -> Vec<String> {
    rows().into_iter().map(|row| row.id.to_string()).collect()
}

/// Build the fixture asset model and return its id: two composite models declared
/// inline by `CreateAssetModel`, then three added by
/// `CreateAssetModelCompositeModel` — one more top-level, one nested inside the
/// first and one nested inside that.
async fn fixture_model(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelExternalId": MODEL_EXTERNAL_ID,
            "assetModelCompositeModels": [
                {
                    "id": IDS[0],
                    "name": "Alarm",
                    "type": "AWS/ALARM",
                    "externalId": "alarm_composite",
                    "description": "The alarm this model raises",
                    "properties": [{
                        "name": "State", "dataType": "STRUCT",
                        "dataTypeSpec": "AWS/ALARM_STATE", "type": {"measurement": {}},
                    }],
                },
                {"id": IDS[1], "name": "Plain", "type": "CUSTOM"},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {created}"))
        .to_string();

    // The three `CreateAssetModelCompositeModel` rows, in the order they append.
    let creates = [
        json!({
            "assetModelCompositeModelId": IDS[2],
            "assetModelCompositeModelName": "Anomaly",
            "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
        }),
        json!({
            "assetModelCompositeModelId": IDS[3],
            "assetModelCompositeModelName": "Nested",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelExternalId": "nested_composite",
            "parentAssetModelCompositeModelId": IDS[0],
        }),
        json!({
            "assetModelCompositeModelId": IDS[4],
            "assetModelCompositeModelName": "Deeper",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelDescription": "Three deep",
            "parentAssetModelCompositeModelId": IDS[3],
        }),
    ];
    for body in creates {
        let (status, answered) = call(
            router,
            "POST",
            &format!("/asset-models/{model_id}/composite-models"),
            Some(body.clone()),
        )
        .await;
        assert_eq!(
            status, 202,
            "CreateAssetModelCompositeModel({body}): got {status} {answered}"
        );
    }
    model_id
}

/// `GET /asset-models/{label}/composite-models{query}`.
async fn list(router: &MultiServiceRouter, label: &str, query: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/asset-models/{label}/composite-models{query}"),
        None,
    )
    .await
}

/// The summaries a `ListAssetModelCompositeModels` body reports.
fn summaries(body: &Value) -> Vec<Value> {
    body["assetModelCompositeModelSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelSummaries in {body}"))
        .clone()
}

/// The id of each summary, in the order the page reports them.
fn listed_ids(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["id"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary without an id in {summary}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_listing_reports_every_stored_composite_model_whatever_its_shape() {
    let router = router();
    let model_id = fixture_model(&router, "Boiler").await;

    let (status, body) = list(&router, &model_id, "?maxResults=250").await;
    assert_eq!(status, 200, "ListAssetModelCompositeModels: got {status} {body}");
    assert_eq!(
        listed_ids(&body),
        expected_ids(),
        "the listing must report every stored composite model, nested ones included, got {body}"
    );
    assert!(
        body.get("nextToken").is_none_or(Value::is_null),
        "a page that holds everything must carry no nextToken, got {body}"
    );

    // Every summary member must come from the SAME stored composite model the
    // implemented reader reports — the round trip through
    // `DescribeAssetModelCompositeModel`.
    let listed = summaries(&body);
    for row in rows() {
        let case = row.name;
        let summary = listed
            .iter()
            .find(|summary| summary["id"] == json!(row.id))
            .unwrap_or_else(|| panic!("case {case:?}: not listed in {body}"));
        let (status, described) = call(
            &router,
            "GET",
            &format!("/asset-models/{model_id}/composite-models/{}", row.id),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModelCompositeModel got {status} {described}"
        );

        for (member, describes_as) in [
            ("id", "assetModelCompositeModelId"),
            ("externalId", "assetModelCompositeModelExternalId"),
            ("name", "assetModelCompositeModelName"),
            ("type", "assetModelCompositeModelType"),
            ("path", "assetModelCompositeModelPath"),
        ] {
            assert_eq!(
                summary.get(member).unwrap_or(&Value::Null),
                described.get(describes_as).unwrap_or(&Value::Null),
                "case {case:?}/{member:?}: the summary disagrees with the stored composite \
                 model ({summary} vs {described})"
            );
        }
        // The describe states an absent description as the empty string; the
        // summary leaves it absent. Either way it is the stored one.
        assert_eq!(
            summary.get("description").and_then(Value::as_str).unwrap_or(""),
            described["assetModelCompositeModelDescription"]
                .as_str()
                .unwrap_or(""),
            "case {case:?}: the description must be the stored one ({summary} vs {described})"
        );

        // `path` is "the path that includes all the pieces that make up the
        // composite model": the asset model root, then the chain from the
        // outermost ancestor down to it.
        let path: Vec<Value> = summary["path"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no path in {summary}"))
            .iter()
            .map(|segment| segment["id"].clone())
            .collect();
        let expected: Vec<Value> = std::iter::once(json!(&model_id))
            .chain(row.path.iter().map(|id| json!(id)))
            .collect();
        assert_eq!(
            path, expected,
            "case {case:?}: the path must run from the asset model root down, got {summary}"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_composite_model_exactly_once_whatever_the_page_size() {
    let router = router();
    let model_id = fixture_model(&router, "Boiler").await;
    let expected = expected_ids();

    // Rows: page sizes from one-per-page to larger than the enumeration, including
    // the exact length, one either side of it, and the published upper bound of
    // `maxResults` (range 1..250, per `smithy_query(query="operation",
    // operation="ListAssetModelCompositeModels")`).
    for max_results in [1_usize, 2, 4, 5, 6, 250] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let resume = token
                .as_deref()
                .map(|token| format!("&nextToken={token}"))
                .unwrap_or_default();
            let (status, body) = list(
                &router,
                &model_id,
                &format!("?maxResults={max_results}{resume}"),
            )
            .await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: page {pages} got {status} {body}"
            );
            let page = listed_ids(&body);
            assert!(
                page.len() <= max_results,
                "case maxResults={max_results}: page {pages} holds {} summaries, more than \
                 asked: {body}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={max_results}: paging did not terminate after {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => {
                    assert!(
                        seen.len().is_multiple_of(max_results),
                        "case maxResults={max_results}: a page followed by a nextToken must be \
                         full, saw {} summaries: {body}",
                        seen.len()
                    );
                    token = Some(next.to_string());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={max_results}: the pages joined must be the whole enumeration once"
        );
        let want_pages = expected.len().div_ceil(max_results);
        assert_eq!(
            pages, want_pages,
            "case maxResults={max_results}: expected {want_pages} pages, walked {pages}"
        );
    }
}

#[tokio::test]
async fn the_next_token_names_the_first_composite_model_of_the_next_page() {
    let router = router();
    let model_id = fixture_model(&router, "Boiler").await;

    // The token the handler issues after a page is the hex of the id the next page
    // resumes from, computed here by an independent oracle — so the assertion is
    // about the token's content and not just that paging round-trips.
    for (page_size, resumes_at) in [(1_usize, IDS[1]), (2, IDS[2]), (4, IDS[4])] {
        let (status, body) = list(&router, &model_id, &format!("?maxResults={page_size}")).await;
        assert_eq!(status, 200, "case maxResults={page_size}: got {status} {body}");
        assert_eq!(
            body["nextToken"].as_str().unwrap_or(""),
            hex_token(resumes_at),
            "case maxResults={page_size}: the nextToken must name the first composite model of \
             the next page, got {body}"
        );
    }

    // And resuming from the last composite model answers exactly it.
    let (status, tail) = list(
        &router,
        &model_id,
        &format!("?nextToken={}", hex_token(IDS[4])),
    )
    .await;
    assert_eq!(status, 200, "resumed page: got {status} {tail}");
    assert_eq!(
        listed_ids(&tail),
        vec![IDS[4].to_string()],
        "resuming from the last composite model must answer exactly it, got {tail}"
    );
}

#[tokio::test]
async fn max_results_is_refused_outside_its_published_range() {
    let router = router();
    let model_id = fixture_model(&router, "Boiler").await;

    // `maxResults` carries the modelled range 1..250: each published bound is
    // accepted and the value just past each bound is refused.
    for (max_results, accepted) in [(0_i64, false), (1, true), (250, true), (251, false)] {
        let (status, body) = list(&router, &model_id, &format!("?maxResults={max_results}")).await;
        if accepted {
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: an in-range page size must be served, \
                 got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults={max_results}: an out-of-range page size must be refused, \
                 got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_label_or_token_that_names_nothing_stored_is_a_modelled_refusal() {
    let router = router();
    let model_id = fixture_model(&router, "Boiler").await;

    // Rows: the ways a request can name something this store does not hold.
    let cases: [(&str, String, String, u16, &str); 5] = [
        (
            "unknown asset model id",
            ABSENT_ID.to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown externalId label",
            "externalId:no_such_model".to_string(),
            String::new(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "token for a composite model this model does not hold",
            model_id.clone(),
            format!("?nextToken={}", hex_token(ABSENT_ID)),
            400,
            "InvalidRequestException",
        ),
        (
            "token that is not a token this operation issued",
            model_id.clone(),
            "?nextToken=zzz".to_string(),
            400,
            "InvalidRequestException",
        ),
        (
            "token that decodes to text naming no composite model",
            model_id.clone(),
            format!("?nextToken={}", hex_token("not-an-id")),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, label, query, want_status, want_code) in cases {
        let (status, body) = list(&router, &label, &query).await;
        assert_eq!(
            status, want_status,
            "case {case:?}: expected HTTP {want_status}, got {status} {body}"
        );
        assert!(
            error_code(&body).ends_with(want_code),
            "case {case:?}: expected {want_code}, got {:?} in {body}",
            error_code(&body)
        );
    }
}

#[tokio::test]
async fn the_active_version_alias_answers_only_once_the_model_has_an_active_version() {
    let router = router();
    let model_id = fixture_model(&router, "Boiler").await;

    // One row per form of the `assetModelVersion` filter while the model has not
    // settled yet: LATEST (explicitly and by default) is the stored version; ACTIVE
    // names a version the model does not have yet. The modelled pattern
    // `^(LATEST|ACTIVE)$` admits nothing else, so a third value is refused before
    // the handler runs.
    for (case, query, expected) in [
        ("default", "?maxResults=250", 200),
        ("LATEST", "?maxResults=250&assetModelVersion=LATEST", 200),
        ("ACTIVE", "?maxResults=250&assetModelVersion=ACTIVE", 404),
    ] {
        let (status, body) = list(&router, &model_id, query).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body}"
        );
        if expected == 200 {
            assert_eq!(
                listed_ids(&body),
                expected_ids(),
                "case {case:?}: must list every stored composite model, got {body}"
            );
        } else {
            assert!(
                error_code(&body).ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?} in {body}",
                error_code(&body)
            );
        }
    }

    // A value outside the modelled pattern is refused.
    let (status, body) = list(&router, &model_id, "?assetModelVersion=PREVIOUS").await;
    assert_eq!(
        status, 400,
        "case \"a version alias outside the modelled pattern\": got {status} {body}"
    );

    // Once the transition the clock owes has fallen due, the model HAS an ACTIVE
    // version and the same request is served from it.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let (status, body) = list(
        &router,
        &model_id,
        "?maxResults=250&assetModelVersion=ACTIVE",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ACTIVE once the model has settled\": got {status} {body}"
    );
    assert_eq!(
        listed_ids(&body),
        expected_ids(),
        "case \"ACTIVE once the model has settled\": must list every stored composite model, \
         got {body}"
    );

    // And once the model HAS an active version, a write that is still settling
    // makes the two aliases name two different definitions: LATEST is the
    // definition the write produced, ACTIVE the one it superseded — so the
    // composite model it added is in the first listing and not in the second.
    let (status, created) = call(
        &router,
        "POST",
        &format!("/asset-models/{model_id}/composite-models"),
        Some(json!({
            "assetModelCompositeModelName": "WhileUpdating",
            "assetModelCompositeModelType": "CUSTOM",
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "CreateAssetModelCompositeModel while settled: got {status} {created}"
    );
    let added = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelId in {created}"))
        .to_string();

    let mut latest = expected_ids();
    latest.push(added);
    let (status, latest_body) = list(&router, &model_id, "?maxResults=250").await;
    assert_eq!(
        status, 200,
        "case \"LATEST while the write settles\": got {status} {latest_body}"
    );
    let (status, active_body) = list(
        &router,
        &model_id,
        "?maxResults=250&assetModelVersion=ACTIVE",
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"ACTIVE while the write settles\": got {status} {active_body}"
    );
    assert_eq!(
        listed_ids(&latest_body), latest,
        "case \"LATEST while the write settles\": the listing must report the composite models \
         of the definition the write produced, got {latest_body}"
    );
    // Whether the write was still settling is read from the model itself AFTER
    // both listings rather than assumed from the wall clock: the clock only moves
    // forward, so a model that still reports a settling state here was settling
    // when both calls ran, and the ACTIVE row is asserted only then.
    let (status, described) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");
    if described["assetModelStatus"]["state"] != json!("ACTIVE") {
        assert_eq!(
            listed_ids(&active_body),
            expected_ids(),
            "case \"ACTIVE while the write settles\": the listing must report the composite \
             models of the definition the write superseded, got {active_body}"
        );
    }
}

#[tokio::test]
async fn an_external_id_label_lists_the_same_composite_models_and_the_read_changes_nothing() {
    let router = router();
    let model_id = fixture_model(&router, "Boiler").await;

    let (status, by_id) = list(&router, &model_id, "?maxResults=250").await;
    assert_eq!(status, 200, "by raw id: got {status} {by_id}");

    // Rows: the other way to name the SAME stored asset model, and the same request
    // repeated — a reporting operation must answer identically both times.
    let external_label = format!("externalId:{MODEL_EXTERNAL_ID}");
    for (case, label) in [
        ("externalId label", external_label.as_str()),
        ("the raw id again", model_id.as_str()),
    ] {
        let (status, body) = list(&router, label, "?maxResults=250").await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["assetModelCompositeModelSummaries"], by_id["assetModelCompositeModelSummaries"],
            "case {case:?}: must list the same composite models as the first read, got {body}"
        );
    }
}

#[tokio::test]
async fn a_model_with_no_composite_models_lists_an_empty_page() {
    let router = router();

    // Rows: the two ways a model ends up holding none — no `assetModelCompositeModels`
    // member at all, and an empty one.
    for (case, body) in [
        ("no composite models member", json!({"assetModelName": "Bare"})),
        (
            "an empty composite models list",
            json!({"assetModelName": "AlsoBare", "assetModelCompositeModels": []}),
        ),
    ] {
        let (status, created) = call(&router, "POST", "/asset-models", Some(body)).await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateAssetModel got {status} {created}"
        );
        let model_id = created["assetModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {created}"))
            .to_string();

        let (status, listed) = list(&router, &model_id, "").await;
        assert_eq!(status, 200, "case {case:?}: got {status} {listed}");
        assert_eq!(
            summaries(&listed).len(),
            0,
            "case {case:?}: a model with no composite models must list nothing, got {listed}"
        );
        assert!(
            listed.get("nextToken").is_none_or(Value::is_null),
            "case {case:?}: an empty listing must carry no nextToken, got {listed}"
        );
    }
}
