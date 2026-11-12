// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAssetModelProperties`, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: `ListAssetModelProperties` enumerates the property
//! definitions the named asset model actually holds in the shared engine store —
//! its own (base) definitions under the default `BASE` filter, and under `ALL`
//! also every definition its composite models contribute — in one stable order,
//! cut into pages of at most `maxResults` (default 50) joined by a resumable
//! `nextToken`, and refuses a label or a token that names nothing stored.
//!
//! What a composite model CONTRIBUTES is its own definitions, or, for a
//! component-model-based composite model (one that carries none of its own), the
//! definitions of the component model it reuses — one set, so every view of that
//! composite model reports it: this listing, the describes of the model and of
//! the composite model, and the asset-level reads of an instance of it.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

// The wait every suite in this crate uses for an owed CREATING/UPDATING ->
// ACTIVE transition, reused rather than kept a second time here.
use crate::asset_lifecycle::create_asset;
use crate::asset_model_composite_model::{create_composite, describe_composite, PAST_THE_OWED_TRANSITION};

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// A router with just this service mounted on a fresh shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pin authentication off so the suite is independent of the process-wide switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return `(status, parsed body)`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service and
        // what the handlers read the ARN region from.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iotsitewise/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
        })
    };
    (status, parsed)
}

/// The wire error code of an error body (`__type`), or `""`.
fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// Hex-encode a resume key the way the handler's `nextToken` encodes it.
///
/// Hand-rolled on purpose, as an INDEPENDENT oracle: the handler encodes with the
/// `hex` crate, so re-using that crate here would let one encoding bug agree with
/// itself and pass.
fn hex_token(key: &str) -> String {
    key.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Fixed property ids, so the expected enumeration order and every page token are
/// known to the test without asking the emulator. Each is a lowercase hyphenated
/// UUID with RFC 4122's version-4 nibble, which is what the modelled `ID` pattern
/// admits.
const IDS: [&str; 7] = [
    "11111111-1111-4111-8111-111111111111",
    "22222222-2222-4222-8222-222222222222",
    "33333333-3333-4333-8333-333333333333",
    "44444444-4444-4444-8444-444444444444",
    "55555555-5555-4555-8555-555555555555",
    "66666666-6666-4666-8666-666666666666",
    "77777777-7777-4777-8777-777777777777",
];

/// The id of the composite model the `ALL` filter reaches into.
const COMPOSITE_ID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
/// The ids of the two properties that composite model defines.
const COMPOSITE_PROPERTY_IDS: [&str; 2] = [
    "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
    "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
];

/// The model's own property definitions: one row per `PropertyType` member a
/// definition may carry (attribute | measurement | transform | metric), spanning
/// the `PropertyDataType` values a model property can hold.
///
/// Source: `smithy_query(query="shape", shape="PropertyType,PropertyDataType")` —
/// `PropertyDataType` is STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO |
/// ANNOTATION | JSON, of which the last three are not model-property types, and
/// `STRUCT` is the one that also carries `dataTypeSpec`.
fn base_definitions() -> Vec<Value> {
    vec![
        json!({
            "id": IDS[0], "name": "Serial", "dataType": "STRING",
            "type": {"attribute": {"defaultValue": "unset"}},
        }),
        json!({
            "id": IDS[1], "name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius",
            "type": {"measurement": {}},
        }),
        json!({
            "id": IDS[2], "name": "Cycles", "dataType": "INTEGER", "unit": "Count",
            "type": {"measurement": {}},
        }),
        json!({
            "id": IDS[3], "name": "Running", "dataType": "BOOLEAN",
            "type": {"measurement": {}},
        }),
        json!({
            "id": IDS[4], "name": "AlarmState", "dataType": "STRUCT",
            "dataTypeSpec": "AWS/ALARM_STATE", "type": {"measurement": {}},
        }),
        json!({
            "id": IDS[5], "name": "Fahrenheit", "externalId": "fahrenheit_of_celsius",
            "dataType": "DOUBLE", "unit": "Fahrenheit",
            "type": {"transform": {
                "expression": "celsius * 9 / 5 + 32",
                "variables": [{"name": "celsius", "value": {"propertyId": IDS[1]}}],
            }},
        }),
        json!({
            "id": IDS[6], "name": "AverageTemperature", "dataType": "DOUBLE", "unit": "Celsius",
            "type": {"metric": {
                "expression": "avg(celsius)",
                "variables": [{"name": "celsius", "value": {"propertyId": IDS[1]}}],
                "window": {"tumbling": {"interval": "1h"}},
            }},
        }),
    ]
}

/// The property names of `base_definitions()`, in definition order.
const BASE_NAMES: [&str; 7] = [
    "Serial",
    "Temperature",
    "Cycles",
    "Running",
    "AlarmState",
    "Fahrenheit",
    "AverageTemperature",
];

/// The property names the composite model defines, in definition order.
const COMPOSITE_NAMES: [&str; 2] = ["State", "Threshold"];

/// Create the fixture asset model — the base definitions plus one composite model
/// of its own two properties — and return its id.
async fn create_fixture_model(router: &MultiServiceRouter, name: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelExternalId": "fixture_model",
            "assetModelProperties": base_definitions(),
            "assetModelCompositeModels": [{
                "id": COMPOSITE_ID,
                "name": "HighTemp",
                "type": "AWS/ALARM",
                "properties": [
                    {
                        "id": COMPOSITE_PROPERTY_IDS[0], "name": COMPOSITE_NAMES[0],
                        "dataType": "STRUCT", "dataTypeSpec": "AWS/ALARM_STATE",
                        "type": {"measurement": {}},
                    },
                    {
                        "id": COMPOSITE_PROPERTY_IDS[1], "name": COMPOSITE_NAMES[1],
                        "dataType": "DOUBLE", "unit": "Celsius",
                        "type": {"attribute": {"defaultValue": "80"}},
                    },
                ],
            }],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {body}");
    body["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {body}"))
        .to_string()
}

/// The `name` member of every summary in a `ListAssetModelProperties` body.
fn listed_names(body: &Value) -> Vec<String> {
    body["assetModelPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelPropertySummaries in {body}"))
        .iter()
        .map(|summary| {
            summary["name"]
                .as_str()
                .unwrap_or_else(|| panic!("a summary without a name in {body}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn the_filter_selects_the_stored_definitions_the_listing_reports() {
    let router = router();
    let model_id = create_fixture_model(&router, "Boiler").await;

    let base: Vec<String> = BASE_NAMES.iter().map(|name| name.to_string()).collect();
    let mut all = base.clone();
    all.extend(COMPOSITE_NAMES.iter().map(|name| name.to_string()));

    // One row per way a caller can ask for the filter: absent (the documented BASE
    // default), BASE, and ALL.
    let cases: [(&str, &Vec<String>); 3] = [
        ("", &base),
        ("&filter=BASE", &base),
        ("&filter=ALL", &all),
    ];
    for (query, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/asset-models/{model_id}/properties?maxResults=250{query}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {query:?}: ListAssetModelProperties got {status} {body}"
        );
        assert_eq!(
            &listed_names(&body),
            expected,
            "case {query:?}: the listing must report exactly the stored definitions, got {body}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {query:?}: a page that holds everything must carry no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn every_summary_member_comes_from_the_stored_definition() {
    let router = router();
    let model_id = create_fixture_model(&router, "Boiler").await;

    // The round trip: the implemented reader for the same stored record.
    let (status, described) =
        call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");

    let (status, listed) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}/properties?maxResults=250&filter=ALL"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssetModelProperties: got {status} {listed}");
    let summaries = listed["assetModelPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelPropertySummaries in {listed}"))
        .clone();

    // Each row is one stored definition and where it is defined: `None` for a base
    // property, the composite model's id for one it defines.
    let described_base = described["assetModelProperties"].clone();
    let described_composite = described["assetModelCompositeModels"][0]["properties"].clone();
    let rows: Vec<(&str, Option<&str>, &Value)> = BASE_NAMES
        .iter()
        .map(|name| (*name, None, &described_base))
        .chain(
            COMPOSITE_NAMES
                .iter()
                .map(|name| (*name, Some(COMPOSITE_ID), &described_composite)),
        )
        .collect();

    for (name, composite_id, defined_in) in rows {
        let summary = summaries
            .iter()
            .find(|summary| summary["name"] == json!(name))
            .unwrap_or_else(|| panic!("case {name:?}: not listed in {listed}"));
        let stored = defined_in
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: no stored definitions in {described}"))
            .iter()
            .find(|property| property["name"] == json!(name))
            .unwrap_or_else(|| panic!("case {name:?}: not stored in {described}"));

        // Every member the summary shares with the stored definition must be the
        // stored value, not a fresh one.
        for member in ["id", "externalId", "name", "dataType", "dataTypeSpec", "unit", "type"] {
            assert_eq!(
                summary.get(member).unwrap_or(&Value::Null),
                stored.get(member).unwrap_or(&Value::Null),
                "case {name:?}/{member:?}: the summary disagrees with the stored definition \
                 ({summary} vs {stored})"
            );
        }
        assert_eq!(
            summary.get("assetModelCompositeModelId").and_then(Value::as_str),
            composite_id,
            "case {name:?}: assetModelCompositeModelId must name the composite model that \
             defines the property, got {summary}"
        );

        // `path` is the structured path from the root of the asset model: the
        // model itself, the containing composite model when there is one, then the
        // property.
        let path = summary["path"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: no path in {summary}"));
        let expected_path: Vec<Value> = std::iter::once(json!({"id": &model_id, "name": "Boiler"}))
            .chain(
                composite_id
                    .map(|id| json!({"id": id, "name": "HighTemp"}))
                    .into_iter(),
            )
            .chain(std::iter::once(json!({"id": stored["id"], "name": name})))
            .collect();
        assert_eq!(
            path, &expected_path,
            "case {name:?}: path must run from the model root to the property, got {summary}"
        );
    }
}

#[tokio::test]
async fn paging_visits_every_property_exactly_once_whatever_the_page_size() {
    let router = router();
    let model_id = create_fixture_model(&router, "Boiler").await;

    let mut expected: Vec<String> = BASE_NAMES.iter().map(|name| name.to_string()).collect();
    expected.extend(COMPOSITE_NAMES.iter().map(|name| name.to_string()));

    // Rows: page sizes from one-per-page to larger than the enumeration, including
    // each published bound of `maxResults` (range 1..250, per
    // `smithy_query(query="operation", operation="ListAssetModelProperties")`).
    for max_results in [1_usize, 2, 3, 8, 9, 10, 250] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let resume = token
                .as_deref()
                .map(|token| format!("&nextToken={token}"))
                .unwrap_or_default();
            let (status, body) = call(
                &router,
                "GET",
                &format!(
                    "/asset-models/{model_id}/properties?filter=ALL&maxResults={max_results}{resume}"
                ),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: page {pages} got {status} {body}"
            );
            let page = listed_names(&body);
            assert!(
                page.len() <= max_results,
                "case maxResults={max_results}: page {pages} holds {} summaries, more than asked: {body}",
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
async fn max_results_is_refused_outside_its_published_range() {
    let router = router();
    let model_id = create_fixture_model(&router, "Boiler").await;

    // `maxResults` carries the modelled range 1..250; each bound is accepted and the
    // value just past each bound is refused.
    for (max_results, accepted) in [(0_i64, false), (1, true), (250, true), (251, false)] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/asset-models/{model_id}/properties?maxResults={max_results}"),
            None,
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: an in-range page size must be served, got {status} {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case maxResults={max_results}: an out-of-range page size must be refused, got {status} {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_label_or_token_that_names_nothing_stored_is_a_modelled_refusal() {
    let router = router();
    let model_id = create_fixture_model(&router, "Boiler").await;

    // Rows: the ways a request can name something this store does not hold.
    let unknown_model = "99999999-9999-4999-8999-999999999999";
    let stale_token = hex_token("|99999999-9999-4999-8999-999999999999");
    let cases: [(&str, String, u16, &str); 4] = [
        (
            "unknown id",
            format!("/asset-models/{unknown_model}/properties"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown externalId label",
            "/asset-models/externalId:no_such_model/properties".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "token for a property not in this enumeration",
            format!("/asset-models/{model_id}/properties?nextToken={stale_token}"),
            400,
            "InvalidRequestException",
        ),
        (
            "token that is not a token this operation issued",
            format!("/asset-models/{model_id}/properties?nextToken=zzz"),
            400,
            "InvalidRequestException",
        ),
    ];
    for (case, uri, want_status, want_code) in cases {
        let (status, body) = call(&router, "GET", &uri, None).await;
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
async fn an_external_id_label_and_either_version_alias_list_the_same_properties() {
    let router = router();
    let model_id = create_fixture_model(&router, "Boiler").await;
    // Waited out, so the model's single stored version is both its LATEST and its
    // ACTIVE one: the two aliases name one definition only once the transition the
    // create owes has fallen due.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let (status, by_id) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}/properties?filter=ALL&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(status, 200, "by raw id: got {status} {by_id}");

    // Rows: the other ways to name the SAME stored version of the same model. The
    // model has had no write since it settled, so LATEST and ACTIVE select it too.
    let cases: [(&str, String); 3] = [
        (
            "externalId label",
            "/asset-models/externalId:fixture_model/properties?filter=ALL&maxResults=250"
                .to_string(),
        ),
        (
            "assetModelVersion=LATEST",
            format!(
                "/asset-models/{model_id}/properties?filter=ALL&maxResults=250&assetModelVersion=LATEST"
            ),
        ),
        (
            "assetModelVersion=ACTIVE",
            format!(
                "/asset-models/{model_id}/properties?filter=ALL&maxResults=250&assetModelVersion=ACTIVE"
            ),
        ),
    ];
    for (case, uri) in cases {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["assetModelPropertySummaries"], by_id["assetModelPropertySummaries"],
            "case {case:?}: must list the same properties as the raw id, got {body}"
        );
    }
}

#[tokio::test]
async fn a_model_with_no_definitions_lists_an_empty_page() {
    let router = router();
    let (status, created) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({"assetModelName": "Bare", "assetModelProperties": []})),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel(Bare): got {status} {created}");
    let model_id = created["assetModelId"].as_str().expect("an id").to_string();

    for filter in ["BASE", "ALL"] {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/asset-models/{model_id}/properties?filter={filter}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {filter:?}: got {status} {body}");
        assert_eq!(
            body["assetModelPropertySummaries"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {filter:?}: a model with no definitions must list nothing, got {body}"
        );
        assert!(
            body.get("nextToken").is_none_or(Value::is_null),
            "case {filter:?}: an empty listing must carry no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn the_next_token_names_the_first_property_of_the_next_page() {
    let router = router();
    let model_id = create_fixture_model(&router, "Boiler").await;

    // The token the handler issues after the first page of one is the hex of this
    // enumeration's resume key for the SECOND base property — `<composite id>|<property
    // id>`, with the composite id empty for a base property. Computed here by hand so
    // the assertion is about the token's content, not just that paging round-trips.
    let (status, first) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}/properties?maxResults=1"),
        None,
    )
    .await;
    assert_eq!(status, 200, "first page: got {status} {first}");
    assert_eq!(
        first["nextToken"].as_str().unwrap_or(""),
        hex_token(&format!("|{}", IDS[1])),
        "the nextToken must name the first property of the next page, got {first}"
    );

    // And the composite-model rows of an ALL listing carry the composite model's id
    // in their resume key, so resuming from one works.
    let resume = hex_token(&format!("{COMPOSITE_ID}|{}", COMPOSITE_PROPERTY_IDS[1]));
    let (status, tail) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}/properties?filter=ALL&nextToken={resume}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "resumed page: got {status} {tail}");
    assert_eq!(
        listed_names(&tail),
        vec![COMPOSITE_NAMES[1].to_string()],
        "resuming from the last property must answer exactly it, got {tail}"
    );
}

/// The one base property of the contributing fixture's asset model.
const BASE_PROPERTY: &str = "Uptime";

/// One composite model of the contributing fixture and what it CONTRIBUTES.
///
/// The rows span the ways a composite model can come by its property
/// definitions: inline definitions of its own, the definitions of the component
/// model it reuses, nesting inside another composite model, a reused component
/// model that holds no definitions, and no definitions at all.
struct Contributor {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The composite model's id.
    id: String,
    /// The chain of composite models from the root of the asset model down to
    /// this one, as `(id, name)`: one segment for a top-level composite model,
    /// one per ancestor plus itself for a nested one.
    path: Vec<(String, &'static str)>,
    /// The property names, in definition order, this composite model contributes.
    properties: Vec<&'static str>,
    /// Whether this composite model is one the asset model holds DIRECTLY, and so
    /// one `DescribeAssetModel` reports.
    top_level: bool,
    /// Whether its type is a BUILT-IN one, in which case `DescribeAssetModel`
    /// reports it in `assetModelCompositeModels` rather than among the custom
    /// summaries.
    built_in: bool,
}

/// Create a `COMPONENT_MODEL` carrying `properties` and return its id — the model
/// a component-model-based composite model reuses the definitions of.
async fn component_model(router: &MultiServiceRouter, name: &str, properties: Value) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": name,
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": properties,
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {created}"))
        .to_string()
}

/// Create one composite model of the fixture and return its id.
async fn composite_id(
    router: &MultiServiceRouter,
    case: &str,
    model_id: &str,
    body: Value,
) -> String {
    let (status, created) = create_composite(router, model_id, body).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModelCompositeModel got {status} {created}"
    );
    created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: no composite model id in {created}"))
        .to_string()
}

/// An asset model with one base property and one composite model per way of
/// coming by property definitions, with the names each contributes.
async fn contributing_fixture(router: &MultiServiceRouter) -> (String, Vec<Contributor>) {
    // Source for the `type` / `dataType` rows: `smithy_query(query="shape",
    // shape="PropertyType,PropertyDataType")`.
    let reusable = component_model(
        router,
        "Pump",
        json!([
            {"name": "Flow", "dataType": "DOUBLE", "unit": "m3/s", "type": {"measurement": {}}},
            {
                "name": "Serial", "dataType": "STRING", "externalId": "pump_serial",
                "type": {"attribute": {"defaultValue": "unset"}},
            },
        ]),
    )
    .await;
    let without_properties = component_model(router, "BareComponent", json!([])).await;

    let (status, created) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Plant",
            "assetModelProperties": [
                {"name": BASE_PROPERTY, "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel(Plant): got {status} {created}");
    let model_id = created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel(Plant): no assetModelId in {created}"))
        .to_string();

    let inline = composite_id(
        router,
        "inline definitions of its own",
        &model_id,
        json!({
            "assetModelCompositeModelName": "Inline",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Setpoint", "dataType": "DOUBLE", "type": {"attribute": {"defaultValue": "20"}}},
            ],
        }),
    )
    .await;
    let reused = composite_id(
        router,
        "a reused component model's definitions",
        &model_id,
        json!({
            "assetModelCompositeModelName": "Reused",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &reusable,
        }),
    )
    .await;
    let nested = composite_id(
        router,
        "a reused component model, nested inside another composite model",
        &model_id,
        json!({
            "assetModelCompositeModelName": "NestedReused",
            "assetModelCompositeModelType": "CUSTOM",
            "parentAssetModelCompositeModelId": &inline,
            "composedAssetModelId": &reusable,
        }),
    )
    .await;
    let reused_bare = composite_id(
        router,
        "a reused component model that holds no definitions",
        &model_id,
        json!({
            "assetModelCompositeModelName": "ReusedBare",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &without_properties,
        }),
    )
    .await;
    let bare = composite_id(
        router,
        "no definitions at all",
        &model_id,
        json!({
            "assetModelCompositeModelName": "Bare",
            "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
        }),
    )
    .await;

    let contributors = vec![
        Contributor {
            case: "inline definitions of its own",
            id: inline.clone(),
            path: vec![(inline.clone(), "Inline")],
            properties: vec!["Setpoint"],
            top_level: true,
            built_in: false,
        },
        Contributor {
            case: "a reused component model's definitions",
            id: reused.clone(),
            path: vec![(reused, "Reused")],
            properties: vec!["Flow", "Serial"],
            top_level: true,
            built_in: false,
        },
        Contributor {
            case: "a reused component model, nested inside another composite model",
            id: nested.clone(),
            path: vec![(inline, "Inline"), (nested, "NestedReused")],
            properties: vec!["Flow", "Serial"],
            top_level: false,
            built_in: false,
        },
        Contributor {
            case: "a reused component model that holds no definitions",
            id: reused_bare.clone(),
            path: vec![(reused_bare, "ReusedBare")],
            properties: Vec::new(),
            top_level: true,
            built_in: false,
        },
        Contributor {
            case: "no definitions at all",
            id: bare.clone(),
            path: vec![(bare, "Bare")],
            properties: Vec::new(),
            top_level: true,
            built_in: true,
        },
    ];
    (model_id, contributors)
}

/// The `name` of every entry of a property list in a body, in stored order.
fn names_of(properties: &Value) -> Vec<&str> {
    properties
        .as_array()
        .map(|properties| {
            properties
                .iter()
                .map(|property| property["name"].as_str().unwrap_or(""))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn the_all_filter_reports_what_every_composite_model_contributes() {
    let router = router();
    let (model_id, contributors) = contributing_fixture(&router).await;

    let (status, listed) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}/properties?filter=ALL&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssetModelProperties: got {status} {listed}");
    let summaries = listed["assetModelPropertySummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelPropertySummaries in {listed}"))
        .clone();

    // BASE is the model's own definitions alone, whatever its composite models
    // contribute.
    let (status, base) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}/properties?filter=BASE&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(status, 200, "BASE listing: got {status} {base}");
    assert_eq!(
        listed_names(&base),
        vec![BASE_PROPERTY.to_string()],
        "case \"BASE\": only the model's own definitions may be listed, got {base}"
    );

    // The model's own property, plus what each composite model contributes.
    let mut expected_total = 1;
    for Contributor {
        case,
        id,
        path,
        properties,
        ..
    } in &contributors
    {
        expected_total += properties.len();
        let contributed: Vec<&Value> = summaries
            .iter()
            .filter(|summary| summary["assetModelCompositeModelId"] == json!(id))
            .collect();
        let listed_from_composite: Vec<&str> = contributed
            .iter()
            .map(|summary| summary["name"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            &listed_from_composite, properties,
            "case {case:?}: the ALL listing must report exactly what the composite model \
             contributes, got {listed}"
        );

        // The other view of the SAME composite model: the two must agree member for
        // member, since one set of definitions is what either reports.
        let (status, described) = describe_composite(&router, &model_id, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModelCompositeModel got {status} {described}"
        );
        let stored = described["assetModelCompositeModelProperties"].clone();
        assert_eq!(
            &names_of(&stored), properties,
            "case {case:?}: the describe must report the same definitions, got {described}"
        );
        for (summary, stored) in contributed
            .iter()
            .zip(stored.as_array().cloned().unwrap_or_default().iter())
        {
            for member in [
                "id",
                "externalId",
                "name",
                "dataType",
                "dataTypeSpec",
                "unit",
                "type",
            ] {
                assert_eq!(
                    summary.get(member).unwrap_or(&Value::Null),
                    stored.get(member).unwrap_or(&Value::Null),
                    "case {case:?}/{member:?}: the summary disagrees with the stored \
                     definition ({summary} vs {stored})"
                );
            }
            // `path` is "the structured path to the property from the root of the
            // asset model": the model itself, the composite models the property
            // sits inside, outermost first, then the property.
            let expected_path: Vec<Value> =
                std::iter::once(json!({"id": &model_id, "name": "Plant"}))
                    .chain(
                        path.iter()
                            .map(|(id, name)| json!({"id": id, "name": name})),
                    )
                    .chain(std::iter::once(
                        json!({"id": stored["id"], "name": stored["name"]}),
                    ))
                    .collect();
            assert_eq!(
                summary["path"].as_array(),
                Some(&expected_path),
                "case {case:?}: path must run from the model root to the property, got {summary}"
            );
        }
    }
    assert_eq!(
        summaries.len(),
        expected_total,
        "the ALL listing must hold the model's own definitions and every contributed one \
         exactly once, got {listed}"
    );
}

#[tokio::test]
async fn paging_an_all_listing_visits_every_contributed_property_once() {
    let router = router();
    let (model_id, contributors) = contributing_fixture(&router).await;

    let mut expected: Vec<String> = vec![BASE_PROPERTY.to_string()];
    for contributor in &contributors {
        expected.extend(contributor.properties.iter().map(|name| name.to_string()));
    }

    // Rows: page sizes from one-per-page to larger than the enumeration, so a
    // contributed property is the first, a middle and the last row of a page.
    for max_results in [1_usize, 2, 3, 250] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let resume = token
                .as_deref()
                .map(|token| format!("&nextToken={token}"))
                .unwrap_or_default();
            let (status, body) = call(
                &router,
                "GET",
                &format!(
                    "/asset-models/{model_id}/properties?filter=ALL&maxResults={max_results}{resume}"
                ),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case maxResults={max_results}: page {pages} got {status} {body}"
            );
            seen.extend(listed_names(&body));
            pages += 1;
            assert!(
                pages <= expected.len() + 1,
                "case maxResults={max_results}: paging did not terminate after {pages} pages"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={max_results}: the pages joined must be the whole enumeration once"
        );
    }
}

#[tokio::test]
async fn describe_asset_model_reports_what_each_composite_model_contributes() {
    let router = router();
    let (model_id, contributors) = contributing_fixture(&router).await;

    let (status, described) =
        call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");
    let composites = described["assetModelCompositeModels"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let summarised: Vec<&str> = described["assetModelCompositeModelSummaries"]
        .as_array()
        .map(|list| list.iter().filter_map(|entry| entry["id"].as_str()).collect())
        .unwrap_or_default();
    for contributor in contributors.iter().filter(|row| row.top_level) {
        let case = contributor.case;
        if contributor.built_in {
            // A built-in composite model is the one `assetModelCompositeModels`
            // reports, with what it contributes.
            let entry = composites
                .iter()
                .find(|composite| composite["id"] == json!(contributor.id))
                .unwrap_or_else(|| panic!("case {case:?}: not reported in {described}"));
            assert_eq!(
                names_of(&entry["properties"]),
                contributor.properties,
                "case {case:?}: the model description must report what the composite model \
                 contributes, got {entry}"
            );
            continue;
        }
        // A custom one is reported ONCE, as a summary; what it contributes is the
        // composite-model read's own answer.
        assert!(
            summarised.contains(&contributor.id.as_str()),
            "case {case:?}: a custom immediate child must be summarised, got {described}"
        );
        let (status, composite_read) = call(
            &router,
            "GET",
            &format!(
                "/asset-models/{model_id}/composite-models/{}",
                contributor.id
            ),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModelCompositeModel got {status} {composite_read}"
        );
        assert_eq!(
            names_of(&composite_read["assetModelCompositeModelProperties"]),
            contributor.properties,
            "case {case:?}: the composite-model read must report what it contributes, got \
             {composite_read}"
        );
    }

    // `excludeProperties` asks for the model without its property definitions, so
    // it blanks a composite model's contributed ones too.
    let (status, lean) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}?excludeProperties=true"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeAssetModel(lean): got {status} {lean}");
    for entry in lean["assetModelCompositeModels"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        assert!(
            entry.get("properties").is_none_or(Value::is_null),
            "case \"excludeProperties\": no definitions may be reported, got {entry}"
        );
    }
}

#[tokio::test]
async fn an_asset_reads_the_properties_its_models_composite_models_contribute() {
    let router = router();
    let (model_id, contributors) = contributing_fixture(&router).await;
    // The model has to have settled before an asset can instantiate it.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let asset_id = create_asset(&router, "Plant1", &model_id).await;

    let (status, listed) = call(
        &router,
        "GET",
        &format!("/assets/{asset_id}/properties?filter=ALL&maxResults=250"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssetProperties: got {status} {listed}");
    let summaries = listed["assetPropertySummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    for contributor in &contributors {
        let case = contributor.case;
        // The asset-level listing of the instance of this composite model reports
        // the same set the model-level one does.
        let instance: Vec<&str> = summaries
            .iter()
            .filter(|summary| summary["assetCompositeModelId"] == json!(contributor.id))
            .map(|summary| summary["id"].as_str().unwrap_or(""))
            .collect();
        let (status, described) = describe_composite(&router, &model_id, &contributor.id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeAssetModelCompositeModel got {status} {described}"
        );
        let definitions = described["assetModelCompositeModelProperties"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let defined: Vec<&str> = definitions
            .iter()
            .map(|property| property["id"].as_str().unwrap_or(""))
            .collect();
        assert_eq!(
            instance, defined,
            "case {case:?}: the asset's instance must carry every contributed property, \
             got {listed}"
        );

        // And each of them is describable by id through a composite model that
        // contributes it. Two composite models may reuse the SAME component
        // model, so one definition can be contributed by more than one of them:
        // the describe reports the first that holds it, which is one of them.
        for property in &definitions {
            let property_id = property["id"].as_str().unwrap_or_default();
            let contributing: Vec<&str> = summaries
                .iter()
                .filter(|summary| summary["id"] == json!(property_id))
                .filter_map(|summary| summary["assetCompositeModelId"].as_str())
                .collect();
            let (status, body) = call(
                &router,
                "GET",
                &format!("/assets/{asset_id}/properties/{property_id}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}/{property_id}: DescribeAssetProperty got {status} {body}"
            );
            let reported = body["compositeModel"]["id"].as_str().unwrap_or_default();
            assert!(
                contributing.contains(&reported),
                "case {case:?}/{property_id}: the property must be reported under a composite \
                 model that contributes it (one of {contributing:?}), got {body}"
            );
            assert_eq!(
                body["compositeModel"]["assetProperty"]["name"],
                property["name"],
                "case {case:?}/{property_id}: the described property must be the contributed \
                 definition, got {body}"
            );
        }
    }
}
