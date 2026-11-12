// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the asset-model COMPOSITE MODEL slice —
//! `CreateAssetModelCompositeModel` and the `DescribeAssetModelCompositeModel`
//! that reads it back — driven over HTTP through the same `MultiServiceRouter`
//! the emulator boots with, so every assertion is about the wire behaviour a
//! real client sees.
//!
//! The rules these suites pin:
//!
//! * a composite model created on an asset model is PERSISTED on that model's
//!   stored record and reads back, by its id and by its `externalId:` label,
//!   exactly as it was created — whatever its type, property count or nesting;
//! * a composite model's path is the chain of composite models from the
//!   outermost ancestor down to itself, and a composite model is summarised by
//!   the context it sits directly in;
//! * a component-model-based composite model carries no properties of its own:
//!   the `COMPONENT_MODEL` it names provides them;
//! * adding one changes the asset model, so the model carries a new version, a
//!   new entity tag and a transition back to `ACTIVE` that the clock owes;
//! * a request outside a documented rule — an unknown id, a duplicate id, a
//!   type outside the catalog, a failed precondition, a count past a bound — is
//!   the modelled refusal, never a fabricated success.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    COMPOSITE_MODEL_TYPES, MAX_COMPOSITE_MODELS_PER_ASSET_MODEL,
    MAX_PROPERTIES_PER_COMPOSITE_MODEL,
};

// The request harness this crate's suites share. Only the header-carrying
// variant below is local, since no other suite sends request headers.
use super::asset_lifecycle::{call, error_code, is_modelled_id, router};

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// Long enough for the transition the clock owes (`ACTIVATION_DELAY`) to fall
/// due, with room for a slow machine.
pub(crate) const PAST_THE_OWED_TRANSITION: Duration = Duration::from_millis(900);

/// Dispatch one request with `headers` and return `(status, parsed body, the
/// response's own headers)`.
///
/// The shared [`call`] sends no request headers and reports no response ones;
/// this operation's `If-Match` / `If-None-Match` / `Match-For-Version-Type`
/// inputs and `DescribeAssetModel`'s `ETag` output are header-bound, so this
/// suite needs both ends.
pub(crate) async fn call_with_headers(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (u16, Value, http::HeaderMap) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        // The SigV4 credential scope is what routes the request to this service.
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iotsitewise/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        );
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let request = request.body(payload).expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let response_headers = response.headers().clone();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
        })
    };
    (status, parsed, response_headers)
}

/// `true` when `tag` has the shape the modelled `ETag` type takes:
/// `^[\w-]{43}$`.
fn is_modelled_etag(tag: &str) -> bool {
    tag.len() == 43
        && tag
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Create an asset model from `body` — the shared `create_model` helper always
/// sends `assetModelProperties`, and these cases vary the whole request (an
/// `assetModelType` of `COMPONENT_MODEL`, no properties at all).
async fn create_model(router: &MultiServiceRouter, name: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(status, 202, "create model {name}: got {status} {created}");
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create model {name}: no assetModelId in {created}"))
        .to_string()
}

/// A plain `ASSET_MODEL` with no properties.
pub(crate) async fn plain_model(router: &MultiServiceRouter, name: &str) -> String {
    create_model(router, name, json!({ "assetModelName": name })).await
}

/// The asset model's current entity tag, as `DescribeAssetModel` reports it in
/// the `ETag` response header.
pub(crate) async fn model_etag(router: &MultiServiceRouter, model_id: &str) -> String {
    let (status, body, headers) = call_with_headers(
        router,
        "GET",
        &format!("/asset-models/{model_id}"),
        None,
        &[],
    )
    .await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {body}");
    let tag = headers
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_else(|| panic!("DescribeAssetModel reported no ETag header: {headers:?}"))
        .to_string();
    assert!(
        is_modelled_etag(&tag),
        "the ETag must take the modelled ^[\\w-]{{43}}$ form, got {tag:?}"
    );
    tag
}

/// `POST /asset-models/{model_id}/composite-models` with `body`.
pub(crate) async fn create_composite(
    router: &MultiServiceRouter,
    model_id: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/asset-models/{model_id}/composite-models"),
        Some(body),
    )
    .await
}

/// `GET /asset-models/{model_id}/composite-models/{composite_label}`.
pub(crate) async fn describe_composite(
    router: &MultiServiceRouter,
    model_id: &str,
    composite_label: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/asset-models/{model_id}/composite-models/{composite_label}"),
        None,
    )
    .await
}

/// One row of the create/read-back table: what is asked for, and what the read
/// back must report.
struct CompositeCase {
    /// What the row varies: the case name every assertion carries.
    case: &'static str,
    /// The `CreateAssetModelCompositeModel` request body.
    body: Value,
    /// How many property definitions the read back must report.
    properties: usize,
}

#[tokio::test]
async fn a_created_composite_model_reads_back_whatever_its_type_or_shape() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;

    // One row per thing the rule generalises over: the composite model type (the
    // whole documented catalog, including the AWS/L4E_ANOMALY the issue never
    // cited), the number of property definitions, whether a description, an
    // external id or a caller-chosen id is supplied.
    let cases = vec![
        CompositeCase {
            case: "CUSTOM, one property, described, external id",
            body: json!({
                "assetModelCompositeModelName": "Custom1",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelDescription": "a custom grouping",
                "assetModelCompositeModelExternalId": "custom_one",
                "assetModelCompositeModelProperties": [{
                    "name": "Setpoint",
                    "dataType": "DOUBLE",
                    "unit": "Celsius",
                    "type": {"attribute": {"defaultValue": "20"}},
                }],
            }),
            properties: 1,
        },
        CompositeCase {
            case: "AWS/ALARM, struct property, caller-chosen id",
            body: json!({
                "assetModelCompositeModelName": "HighTemp",
                "assetModelCompositeModelType": "AWS/ALARM",
                "assetModelCompositeModelId": "11111111-2222-4333-8444-555555555555",
                "assetModelCompositeModelProperties": [{
                    "name": "State",
                    "dataType": "STRUCT",
                    "dataTypeSpec": "AWS/ALARM_STATE",
                    "type": {"measurement": {}},
                }],
            }),
            properties: 1,
        },
        CompositeCase {
            case: "AWS/L4E_ANOMALY, three properties",
            body: json!({
                "assetModelCompositeModelName": "Anomaly",
                "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
                "assetModelCompositeModelProperties": [
                    {"name": "P1", "dataType": "DOUBLE", "type": {"measurement": {}}},
                    {"name": "P2", "dataType": "INTEGER", "type": {"attribute": {}}},
                    {"name": "P3", "dataType": "STRING", "type": {"attribute": {}}},
                ],
            }),
            properties: 3,
        },
        CompositeCase {
            case: "CUSTOM, no properties at all",
            body: json!({
                "assetModelCompositeModelName": "Empty",
                "assetModelCompositeModelType": "CUSTOM",
            }),
            properties: 0,
        },
    ];

    for case in &cases {
        let CompositeCase {
            case: name,
            body,
            properties,
        } = case;
        let (status, created) = create_composite(&router, &model_id, body.clone()).await;
        assert_eq!(status, 202, "case {name:?}: create got {status} {created}");

        let composite_id = created["assetModelCompositeModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {name:?}: no id in {created}"))
            .to_string();
        assert!(
            is_modelled_id(&composite_id),
            "case {name:?}: expected a modelled UUID id, got {composite_id:?}"
        );
        if let Some(requested) = body["assetModelCompositeModelId"].as_str() {
            assert_eq!(
                composite_id, requested,
                "case {name:?}: a caller-supplied id must be the one stored, got {created}"
            );
        }
        assert_eq!(
            created["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {name:?}: the create must report the asset model it wrote to, got {created}"
        );
        // A top-level composite model's path is the asset model root followed by
        // the composite model itself.
        assert_eq!(
            created["assetModelCompositeModelPath"]
                .as_array()
                .map(Vec::len),
            Some(2),
            "case {name:?}: expected the model root plus one segment, got {created}"
        );
        assert_eq!(
            created["assetModelCompositeModelPath"][0]["id"].as_str(),
            Some(model_id.as_str()),
            "case {name:?}: the path must start at the asset model itself, got {created}"
        );
        assert_eq!(
            created["assetModelCompositeModelPath"][1]["id"].as_str(),
            Some(composite_id.as_str()),
            "case {name:?}: the path must end at the composite model itself, got {created}"
        );

        // Read it back through the describe that reports one composite model.
        let (status, described) = describe_composite(&router, &model_id, &composite_id).await;
        assert_eq!(status, 200, "case {name:?}: describe got {status} {described}");
        for (member, expected) in [
            ("assetModelCompositeModelName", &body["assetModelCompositeModelName"]),
            ("assetModelCompositeModelType", &body["assetModelCompositeModelType"]),
            (
                "assetModelCompositeModelExternalId",
                &body["assetModelCompositeModelExternalId"],
            ),
        ] {
            if expected.is_null() {
                continue;
            }
            assert_eq!(
                &described[member], expected,
                "case {name:?}: {member} did not read back, got {described}"
            );
        }
        assert_eq!(
            described["assetModelCompositeModelDescription"],
            body["assetModelCompositeModelDescription"]
                .as_str()
                .map_or(Value::String(String::new()), |text| Value::String(
                    text.to_string()
                )),
            "case {name:?}: the description did not read back, got {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelProperties"]
                .as_array()
                .map(Vec::len)
                .unwrap_or(0),
            *properties,
            "case {name:?}: expected {properties} properties, got {described}"
        );
        for property in described["assetModelCompositeModelProperties"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            let id = property["id"].as_str().unwrap_or("");
            assert!(
                is_modelled_id(id),
                "case {name:?}: every property carries a minted id, got {property}"
            );
        }
        assert_eq!(
            described["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {name:?}: describe must report its asset model, got {described}"
        );
        // A composite model with no composite models nested in it summarises none.
        assert_eq!(
            described["assetModelCompositeModelSummaries"]
                .as_array()
                .map(Vec::len),
            Some(0),
            "case {name:?}: a leaf composite model has no nested summaries, got {described}"
        );

        // The `externalId:` label resolves to the same composite model as the id.
        if let Some(external) = body["assetModelCompositeModelExternalId"].as_str() {
            let (status, by_label) =
                describe_composite(&router, &model_id, &format!("externalId:{external}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: describe by externalId got {status} {by_label}"
            );
            assert_eq!(
                by_label["assetModelCompositeModelId"].as_str(),
                Some(composite_id.as_str()),
                "case {name:?}: the externalId label must resolve to the same composite model, \
                 got {by_label}"
            );
        }
    }

    // Every one of them is on the asset model, reported ONCE: a built-in
    // composite model in `assetModelCompositeModels`, a custom one among the
    // summaries.
    let (status, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    let built_in = cases
        .iter()
        .filter(|case| case.body["assetModelCompositeModelType"] != json!("CUSTOM"))
        .count();
    for (member, expected) in [
        ("assetModelCompositeModels", built_in),
        (
            "assetModelCompositeModelSummaries",
            cases.len() - built_in,
        ),
    ] {
        assert_eq!(
            model[member].as_array().map(Vec::len),
            Some(expected),
            "case {member:?}: every created composite model must be reported once, got {model}"
        );
    }
}

#[tokio::test]
async fn a_composite_model_type_outside_the_documented_catalog_is_refused() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;

    // The catalog itself is the source: `assetModelCompositeModelType` is a free
    // `Name` string whose documentation states "Valid values are AWS/ALARM,
    // CUSTOM, or AWS/L4E_ANOMALY".
    for accepted in COMPOSITE_MODEL_TYPES {
        let (status, body) = create_composite(
            &router,
            &model_id,
            json!({
                "assetModelCompositeModelName": format!("Of{accepted}"),
                "assetModelCompositeModelType": accepted,
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {accepted:?}: a documented type must be accepted, got {status} {body}"
        );
    }

    // One row per way of being outside it: unknown, wrong case, near-miss of a
    // documented value, a namespace that does not exist.
    for refused in ["GROUPING", "custom", "AWS/ALARMS", "AWS/L4E-ANOMALY"] {
        let (status, body) = create_composite(
            &router,
            &model_id,
            json!({
                "assetModelCompositeModelName": "Rejected",
                "assetModelCompositeModelType": refused,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {refused:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {refused:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_nested_composite_model_reports_the_path_from_the_outermost_ancestor() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;

    // A chain three deep, so the path is tested at every length: 1, 2 and 3.
    let mut chain: Vec<(String, String)> = Vec::new();
    for depth in 0..3u32 {
        let name = format!("Level{depth}");
        let mut body = json!({
            "assetModelCompositeModelName": name,
            "assetModelCompositeModelType": "CUSTOM",
        });
        if let Some((parent_id, _)) = chain.last() {
            body["parentAssetModelCompositeModelId"] = json!(parent_id);
        }
        let (status, created) = create_composite(&router, &model_id, body).await;
        assert_eq!(
            status, 202,
            "case depth {depth}: create got {status} {created}"
        );
        let id = created["assetModelCompositeModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case depth {depth}: no id in {created}"))
            .to_string();

        let path = created["assetModelCompositeModelPath"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        chain.push((id.clone(), name.clone()));
        // The asset model root, then one segment per composite model from the
        // outermost ancestor down to this one.
        assert_eq!(
            path.len(),
            chain.len() + 1,
            "case depth {depth}: the path must carry the model root plus one segment per \
             ancestor, got {created}"
        );
        assert_eq!(
            path[0]["id"].as_str(),
            Some(model_id.as_str()),
            "case depth {depth}: the path must start at the asset model itself in {created}"
        );
        for (at, (expected_id, expected_name)) in chain.iter().enumerate() {
            assert_eq!(
                path[at + 1]["id"].as_str(),
                Some(expected_id.as_str()),
                "case depth {depth}: segment {at} is the wrong composite model in {created}"
            );
            assert_eq!(
                path[at + 1]["name"].as_str(),
                Some(expected_name.as_str()),
                "case depth {depth}: segment {at} carries the wrong name in {created}"
            );
        }

        // The describe reports the same path for the same composite model.
        let (status, described) = describe_composite(&router, &model_id, &id).await;
        assert_eq!(
            status, 200,
            "case depth {depth}: describe got {status} {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelPath"], created["assetModelCompositeModelPath"],
            "case depth {depth}: the create and the describe report different paths: {described}"
        );
    }

    // A composite model summarises the composite models nested DIRECTLY in it.
    for (at, (id, _)) in chain.iter().enumerate() {
        let (status, described) = describe_composite(&router, &model_id, id).await;
        assert_eq!(status, 200, "case {at}: describe got {status} {described}");
        let summaries = described["assetModelCompositeModelSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let expected_child = chain.get(at + 1).map(|(child, _)| child.as_str());
        assert_eq!(
            summaries.len(),
            usize::from(expected_child.is_some()),
            "case {at}: expected {} nested summary, got {described}",
            usize::from(expected_child.is_some())
        );
        if let Some(child) = expected_child {
            assert_eq!(
                summaries[0]["id"].as_str(),
                Some(child),
                "case {at}: the wrong composite model is summarised in {described}"
            );
            assert_eq!(
                summaries[0]["path"].as_array().map(Vec::len),
                Some(at + 3),
                "case {at}: a nested summary carries its full path from the model root, got \
                 {described}"
            );
        }
    }

    // And the asset model itself reports only what it holds directly: the chain's
    // outermost composite model, which is CUSTOM and so is summarised.
    let (status, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    for (member, expected) in [
        ("assetModelCompositeModels", 0),
        ("assetModelCompositeModelSummaries", 1),
    ] {
        assert_eq!(
            model[member].as_array().map(Vec::len),
            Some(expected),
            "case {member:?}: only the top-level composite model is the model's own, got {model}"
        );
        if expected > 0 {
            assert_eq!(
                model[member][0]["id"].as_str(),
                Some(chain[0].0.as_str()),
                "case {member:?}: the wrong composite model is reported in {model}"
            );
        }
    }
}

#[tokio::test]
async fn a_component_model_based_composite_model_reuses_the_component_models_properties() {
    let router = router();
    let component_id = create_model(
        &router,
        "Pump",
        json!({
            "assetModelName": "Pump",
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}},
                {"name": "Serial", "dataType": "STRING", "type": {"attribute": {}}},
            ],
        }),
    )
    .await;
    let model_id = plain_model(&router, "Boiler").await;

    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "ThePump",
            "assetModelCompositeModelType": "CUSTOM",
            "composedAssetModelId": &component_id,
        }),
    )
    .await;
    assert_eq!(status, 202, "create got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .expect("an id")
        .to_string();

    let (status, described) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(status, 200, "describe got {status} {described}");
    // The component model provides the properties; they are read from it rather
    // than copied, so they are the ones it holds now.
    let names: Vec<&str> = described["assetModelCompositeModelProperties"]
        .as_array()
        .map(|properties| {
            properties
                .iter()
                .map(|property| property["name"].as_str().unwrap_or(""))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec!["Flow", "Serial"],
        "the composed component model's properties must be reported: {described}"
    );
    assert_eq!(
        described["compositionDetails"]["compositionRelationship"][0]["id"].as_str(),
        Some(component_id.as_str()),
        "the composition relationship must name the composed component model: {described}"
    );

    // One row per way of naming a composed model the rule refuses.
    let asset_model_id = plain_model(&router, "NotAComponent").await;
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an asset model that is not a COMPONENT_MODEL",
            json!({
                "assetModelCompositeModelName": "Bad1",
                "assetModelCompositeModelType": "CUSTOM",
                "composedAssetModelId": &asset_model_id,
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "both a composed model and properties of its own",
            json!({
                "assetModelCompositeModelName": "Bad2",
                "assetModelCompositeModelType": "CUSTOM",
                "composedAssetModelId": &component_id,
                "assetModelCompositeModelProperties": [
                    {"name": "P", "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a composed model that is not stored",
            json!({
                "assetModelCompositeModelName": "Bad3",
                "assetModelCompositeModelType": "CUSTOM",
                "composedAssetModelId": "99999999-8888-4777-8666-555555555555",
            }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a composed model named by an externalId label that is not stored",
            json!({
                "assetModelCompositeModelName": "Bad4",
                "assetModelCompositeModelType": "CUSTOM",
                "composedAssetModelId": "externalId:no_such_component",
            }),
            404,
            "ResourceNotFoundException",
        ),
    ];
    for (case, body, expected_status, expected_code) in cases {
        let (status, body) = create_composite(&router, &model_id, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn an_id_that_resolves_to_nothing_is_a_modelled_refusal() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (_, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Known",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .expect("an id")
        .to_string();
    // A well-formed id that nothing was ever stored under.
    let stranger = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";

    // One row per id member that has to resolve, on both operations.
    let cases: Vec<(&str, String, String, Option<Value>)> = vec![
        (
            "create on an asset model that is not stored",
            format!("/asset-models/{stranger}/composite-models"),
            "POST".to_string(),
            Some(json!({
                "assetModelCompositeModelName": "Orphan",
                "assetModelCompositeModelType": "CUSTOM",
            })),
        ),
        (
            "create under a parent composite model that is not stored",
            format!("/asset-models/{model_id}/composite-models"),
            "POST".to_string(),
            Some(json!({
                "assetModelCompositeModelName": "Orphan",
                "assetModelCompositeModelType": "CUSTOM",
                "parentAssetModelCompositeModelId": stranger,
            })),
        ),
        (
            "describe on an asset model that is not stored",
            format!("/asset-models/{stranger}/composite-models/{composite_id}"),
            "GET".to_string(),
            None,
        ),
        (
            "describe a composite model that is not stored",
            format!("/asset-models/{model_id}/composite-models/{stranger}"),
            "GET".to_string(),
            None,
        ),
        (
            "describe a composite model by an externalId label that is not stored",
            format!("/asset-models/{model_id}/composite-models/externalId:no_such_composite"),
            "GET".to_string(),
            None,
        ),
    ];
    for (case, uri, method, body) in cases {
        let (status, answered) = call(&router, &method, &uri, body).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {answered}");
        assert_eq!(
            error_code(&answered),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {answered}"
        );
    }
}

#[tokio::test]
async fn an_id_or_external_id_that_is_already_taken_is_refused() {
    let router = router();
    let first_model = plain_model(&router, "First").await;
    let second_model = plain_model(&router, "Second").await;

    let taken_id = "12345678-1234-4234-8234-123456789abc";
    let (status, created) = create_composite(
        &router,
        &first_model,
        json!({
            "assetModelCompositeModelName": "Taken",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelId": taken_id,
            "assetModelCompositeModelExternalId": "taken_external",
        }),
    )
    .await;
    assert_eq!(status, 202, "create got {status} {created}");

    // One row per scope the collision happens in: the same asset model, and a
    // different one — a composite model id is globally unique, an external id
    // names a composite model within its asset model.
    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "the same id on the same asset model",
            &first_model,
            json!({
                "assetModelCompositeModelName": "Again",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelId": taken_id,
            }),
        ),
        (
            "the same id on another asset model",
            &second_model,
            json!({
                "assetModelCompositeModelName": "Again",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelId": taken_id,
            }),
        ),
        (
            "the same external id on the same asset model",
            &first_model,
            json!({
                "assetModelCompositeModelName": "Again",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelExternalId": "taken_external",
            }),
        ),
    ];
    for (case, model_id, body) in cases {
        let (status, answered) = create_composite(&router, model_id, body).await;
        assert_eq!(status, 409, "case {case:?}: expected 409, got {status} {answered}");
        assert_eq!(
            error_code(&answered),
            "ResourceAlreadyExistsException",
            "case {case:?}: unexpected error code in {answered}"
        );
    }

    // The same EXTERNAL id on a different asset model is free, since it names a
    // composite model within its own model.
    let (status, created) = create_composite(
        &router,
        &second_model,
        json!({
            "assetModelCompositeModelName": "Elsewhere",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelExternalId": "taken_external",
        }),
    )
    .await;
    assert_eq!(
        status, 202,
        "an external id is free on another asset model: got {status} {created}"
    );
}

#[tokio::test]
async fn adding_a_composite_model_updates_the_model_until_the_owed_transition_falls_due() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let (status, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    assert_eq!(
        model["assetModelStatus"]["state"], "ACTIVE",
        "the model must have settled before the write: {model}"
    );
    let version_before = model["assetModelVersion"].as_str().unwrap_or("").to_string();

    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    assert_eq!(status, 202, "create got {status} {created}");
    // The write changed the model's definition, so it owes a transition back to
    // ACTIVE and reports UPDATING until that falls due.
    assert_eq!(
        created["assetModelStatus"]["state"], "UPDATING",
        "the create must report the model as UPDATING: {created}"
    );
    let (_, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        model["assetModelStatus"]["state"], "UPDATING",
        "DescribeAssetModel straight after the write: got {model}"
    );
    assert_ne!(
        model["assetModelVersion"].as_str().unwrap_or(""),
        version_before,
        "the model must carry a new version after the write: {model}"
    );

    // Nobody has to call anything for the owed transition to happen.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let (_, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        model["assetModelStatus"]["state"], "ACTIVE",
        "DescribeAssetModel after the owed transition: got {model}"
    );
}

#[tokio::test]
async fn the_preconditions_are_evaluated_against_the_models_entity_tag() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let body = |name: &str| {
        json!({
            "assetModelCompositeModelName": name,
            "assetModelCompositeModelType": "CUSTOM",
        })
    };
    let uri = format!("/asset-models/{model_id}/composite-models");
    let settled_tag = model_etag(&router, &model_id).await;
    // A well-formed tag of the modelled form that is not the model's.
    let stranger_tag = "0123456789abcdefghijklmnopqrstuvwxyz-_ABCDE";
    assert!(is_modelled_etag(stranger_tag), "the fixture tag is well-formed");

    // The tag the model reports is the one If-Match accepts.
    let (status, answered, _) = call_with_headers(
        &router,
        "POST",
        &uri,
        Some(body("Matched")),
        &[("If-Match", &settled_tag)],
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"the model's own tag\": expected 202, got {status} {answered}"
    );

    // That write changed the definition, so the old tag is stale.
    let updated_tag = model_etag(&router, &model_id).await;
    assert_ne!(
        updated_tag, settled_tag,
        "the entity tag must change when the definition does"
    );

    // One row per precondition that must fail, and why.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        (
            "If-Match with the tag of the previous definition",
            vec![("If-Match", settled_tag.as_str())],
        ),
        (
            "If-Match with a tag the service never issued",
            vec![("If-Match", stranger_tag)],
        ),
        (
            "If-None-Match * against the latest version, which exists",
            vec![("If-None-Match", "*"), ("Match-For-Version-Type", "LATEST")],
        ),
        (
            "If-Match with the latest version's tag against the ACTIVE version",
            vec![
                ("If-Match", updated_tag.as_str()),
                ("Match-For-Version-Type", "ACTIVE"),
            ],
        ),
    ];
    for (case, headers) in cases {
        let (status, answered, _) =
            call_with_headers(&router, "POST", &uri, Some(body("Refused")), &headers).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected 412, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "PreconditionFailedException",
            "case {case:?}: unexpected error code in {answered}"
        );
    }

    // While the model is UPDATING, the version the write superseded is still its
    // ACTIVE one: `If-None-Match *` against that version is refused, because the
    // version it asks not to exist does exist, and `If-Match` carrying the tag
    // THAT version reported is accepted.
    let (status, answered, _) = call_with_headers(
        &router,
        "POST",
        &uri,
        Some(body("AgainstTheActiveVersion")),
        &[("If-None-Match", "*"), ("Match-For-Version-Type", "ACTIVE")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case \"If-None-Match * against the ACTIVE version, which exists\": expected 412, got \
         {status} {answered}"
    );
    let (status, answered, _) = call_with_headers(
        &router,
        "POST",
        &uri,
        Some(body("AgainstTheActiveVersion")),
        &[
            ("If-Match", settled_tag.as_str()),
            ("Match-For-Version-Type", "ACTIVE"),
        ],
    )
    .await;
    assert_eq!(
        status, 202,
        "case \"If-Match with the ACTIVE version's own tag\": expected 202, got {status} {answered}"
    );

    // Once it settles the latest version IS the active one, and the same
    // `If-None-Match *` is still refused: that version exists either way.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let (status, answered, _) = call_with_headers(
        &router,
        "POST",
        &uri,
        Some(body("Settled")),
        &[("If-None-Match", "*"), ("Match-For-Version-Type", "ACTIVE")],
    )
    .await;
    assert_eq!(
        status, 412,
        "case \"If-None-Match * with an ACTIVE version\": expected 412, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "PreconditionFailedException",
        "case \"If-None-Match * with an ACTIVE version\": unexpected error code in {answered}"
    );

    // And the tag of an unchanged model is the same on every read.
    let read_again = model_etag(&router, &model_id).await;
    let and_again = model_etag(&router, &model_id).await;
    assert_eq!(
        read_again, and_again,
        "two reads of an unchanged model must report the same entity tag"
    );
}

#[tokio::test]
async fn the_active_version_alias_answers_only_once_the_model_has_an_active_version() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    assert_eq!(status, 202, "create got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .expect("an id")
        .to_string();
    let uri = |version: &str| {
        format!("/asset-models/{model_id}/composite-models/{composite_id}{version}")
    };

    // One row per form of the `assetModelVersion` filter, while the model has not
    // settled yet: LATEST (explicitly and by default) is the stored version;
    // ACTIVE names a version the model does not have yet.
    for (case, version, expected) in [
        ("the default, which is LATEST", "", 200),
        ("LATEST", "?assetModelVersion=LATEST", 200),
        ("ACTIVE while the model has not settled", "?assetModelVersion=ACTIVE", 404),
    ] {
        let (status, answered) = call(&router, "GET", &uri(version), None).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );
        if expected == 404 {
            assert_eq!(
                error_code(&answered),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code in {answered}"
            );
        }
    }

    // Once the transition the clock owes falls due, the model HAS an active
    // version and the same request is answered.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let (status, answered) = call(&router, "GET", &uri("?assetModelVersion=ACTIVE"), None).await;
    assert_eq!(
        status, 200,
        "case \"ACTIVE once the model has settled\": expected 200, got {status} {answered}"
    );
    assert_eq!(
        answered["assetModelCompositeModelId"].as_str(),
        Some(composite_id.as_str()),
        "case \"ACTIVE once the model has settled\": the wrong composite model in {answered}"
    );
}

#[tokio::test]
async fn the_property_and_composite_model_counts_are_bounded() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;

    // The published quota: "You can specify up to 200 properties per composite
    // model". Tested at the bound and one past it.
    let definitions = |count: usize| -> Value {
        Value::Array(
            (0..count)
                .map(|at| {
                    json!({
                        "name": format!("P{at}"),
                        "dataType": "DOUBLE",
                        "type": {"measurement": {}},
                    })
                })
                .collect(),
        )
    };
    for (case, count, expected) in [
        ("at the bound", MAX_PROPERTIES_PER_COMPOSITE_MODEL, 202),
        ("one past the bound", MAX_PROPERTIES_PER_COMPOSITE_MODEL + 1, 410),
    ] {
        let (status, answered) = create_composite(
            &router,
            &model_id,
            json!({
                "assetModelCompositeModelName": format!("Properties{count}"),
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelProperties": definitions(count),
            }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected {expected}, got {status} {answered}"
        );
        if expected == 410 {
            assert_eq!(
                error_code(&answered),
                "LimitExceededException",
                "case {case:?}: unexpected error code in {answered}"
            );
        }
    }

    // The target asset model is resolved before the request's own shape is
    // judged: a request naming no stored asset model is a 404 even when it is
    // also past the property bound.
    let (status, answered) = create_composite(
        &router,
        "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
        json!({
            "assetModelCompositeModelName": "Orphan",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties":
                definitions(MAX_PROPERTIES_PER_COMPOSITE_MODEL + 1),
        }),
    )
    .await;
    assert_eq!(
        status, 404,
        "case \"past the bound on an asset model that is not stored\": expected 404, got {status} \
         {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "ResourceNotFoundException",
        "case \"past the bound on an asset model that is not stored\": unexpected error code in \
         {answered}"
    );

    // The store's own bound on how many composite models one asset model holds:
    // one is already stored, so fill it to the cap and then ask for one more.
    let bounded_model = plain_model(&router, "Bounded").await;
    for at in 0..MAX_COMPOSITE_MODELS_PER_ASSET_MODEL {
        let (status, answered) = create_composite(
            &router,
            &bounded_model,
            json!({
                "assetModelCompositeModelName": format!("Custom{at}"),
                "assetModelCompositeModelType": "CUSTOM",
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {at}: a create below the cap must be accepted, got {status} {answered}"
        );
    }
    let (status, answered) = create_composite(
        &router,
        &bounded_model,
        json!({
            "assetModelCompositeModelName": "OneTooMany",
            "assetModelCompositeModelType": "CUSTOM",
        }),
    )
    .await;
    assert_eq!(
        status, 410,
        "case \"one past the store's cap\": expected 410, got {status} {answered}"
    );
    assert_eq!(
        error_code(&answered),
        "LimitExceededException",
        "case \"one past the store's cap\": unexpected error code in {answered}"
    );
}

#[tokio::test]
async fn a_property_definition_is_refused_by_the_same_rule_as_on_the_asset_model() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;

    // The rule the shapes document, enforced wherever property definitions
    // arrive: exactly one property type, and `dataTypeSpec` on a STRUCT.
    let cases: Vec<(&str, Value)> = vec![
        (
            "no property type at all",
            json!({"name": "P", "dataType": "DOUBLE", "type": {}}),
        ),
        (
            "two property types",
            json!({
                "name": "P",
                "dataType": "DOUBLE",
                "type": {"measurement": {}, "attribute": {}},
            }),
        ),
        (
            "a STRUCT with no dataTypeSpec",
            json!({"name": "P", "dataType": "STRUCT", "type": {"measurement": {}}}),
        ),
    ];
    for (case, definition) in cases {
        let (status, answered) = create_composite(
            &router,
            &model_id,
            json!({
                "assetModelCompositeModelName": "Bad",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelProperties": [definition],
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {answered}"
        );
    }
}
