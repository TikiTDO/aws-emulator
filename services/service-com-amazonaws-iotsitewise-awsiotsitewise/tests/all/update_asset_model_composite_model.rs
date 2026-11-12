// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateAssetModelCompositeModel`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * an update REWRITES the addressed composite model of the asset model's
//!   stored record from the definition the request carries — its name, its
//!   description, the external id it may be given once, and its property list in
//!   full — so every reader of that model, and of the assets created from it,
//!   reports the new definition; a property the request still names keeps the id
//!   it was read under and one it does not name is gone;
//! * the asset model is the versioned resource the write lands on, so it carries
//!   a new version and a new entity tag, reports `UPDATING` until the transition
//!   the clock owes falls due, and answers a read scoped to its ACTIVE version
//!   from the superseded definition until then;
//! * a component-model-based composite model has no properties of its own, so
//!   none can be stated for it and the reused model's are what it keeps
//!   reporting;
//! * a request outside a documented rule — an unknown id, a type or data-type
//!   change, a duplicate property, a count past the published quota, a failed
//!   precondition, an external id already taken or already set — is the modelled
//!   refusal, and leaves the stored definition exactly as it was.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::MAX_PROPERTIES_PER_COMPOSITE_MODEL;

use super::asset_lifecycle::{call, create_asset, error_code, is_modelled_id, property_id, router};
use super::asset_model_composite_model::{
    call_with_headers, create_composite, describe_composite, model_etag, plain_model,
    PAST_THE_OWED_TRANSITION,
};

/// `PUT /asset-models/{model}/composite-models/{composite}` with `body`.
async fn update_composite(
    router: &MultiServiceRouter,
    model_id: &str,
    composite_label: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/asset-models/{model_id}/composite-models/{composite_label}"),
        Some(body),
    )
    .await
}

/// The same, carrying the conditional request headers this operation binds.
async fn update_composite_with_headers(
    router: &MultiServiceRouter,
    model_id: &str,
    composite_label: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let (status, body, _) = call_with_headers(
        router,
        "PUT",
        &format!("/asset-models/{model_id}/composite-models/{composite_label}"),
        Some(body),
        headers,
    )
    .await;
    (status, body)
}

/// The definition every row updates: one attribute and one measurement property,
/// so a row can keep one, change one, drop one or add one beside them.
fn baseline_body(name: &str) -> Value {
    json!({
        "assetModelCompositeModelName": name,
        "assetModelCompositeModelType": "CUSTOM",
        "assetModelCompositeModelDescription": "the definition an update replaces",
        "assetModelCompositeModelProperties": [
            {
                "name": "Setpoint",
                "dataType": "DOUBLE",
                "unit": "Celsius",
                "type": {"attribute": {"defaultValue": "20"}},
            },
            {"name": "Cycles", "dataType": "INTEGER", "unit": "Count", "type": {"measurement": {}}},
        ],
    })
}

/// Create the baseline composite model on `model_id` and return
/// `(compositeModelId, setpointPropertyId, cyclesPropertyId)` as the read back
/// reports them.
async fn baseline_composite(
    router: &MultiServiceRouter,
    model_id: &str,
    name: &str,
) -> (String, String, String) {
    let (status, created) = create_composite(router, model_id, baseline_body(name)).await;
    assert_eq!(
        status, 202,
        "baseline {name}: create got {status} {created}"
    );
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("baseline {name}: no id in {created}"))
        .to_string();
    let (status, described) = describe_composite(router, model_id, &composite_id).await;
    assert_eq!(
        status, 200,
        "baseline {name}: describe got {status} {described}"
    );
    let properties = &described["assetModelCompositeModelProperties"];
    (
        composite_id,
        property_id(properties, "Setpoint"),
        property_id(properties, "Cycles"),
    )
}

/// Which baseline property an expected property must still BE: a property the
/// request still names keeps the stored id its asset-level data is recorded
/// against, and one the request describes for the first time gets a fresh one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kept {
    Setpoint,
    Cycles,
    Fresh,
}

/// One expected property of the read back.
struct ExpectProperty {
    name: &'static str,
    unit: &'static str,
    r#type: Value,
    kept: Kept,
}

/// The request body of one row, built from the baseline's `(setpointId,
/// cyclesId)` so a row can address a stored property by the id it was read under.
type BodyFor = Box<dyn Fn(&str, &str) -> Value>;

/// One row of the conditional-header table: the case, the request headers, and
/// the status and error code the row expects.
type HeaderCase<'a> = (&'a str, Vec<(&'a str, &'a str)>, u16, &'a str);

/// One row of the update table: the request, and the definition the read back
/// must report afterwards.
struct UpdateCase {
    case: &'static str,
    /// Built from the baseline's `(setpointId, cyclesId)`, so a row can address
    /// a stored property by the id it was read under.
    body: BodyFor,
    name: &'static str,
    /// What `DescribeAssetModelCompositeModel` reports for the description, which
    /// the model declares required and so renders as the empty string when the
    /// update carried none.
    description: &'static str,
    external_id: Option<&'static str>,
    properties: Vec<ExpectProperty>,
}

#[tokio::test]
async fn an_updated_composite_model_reads_back_the_definition_the_request_carried() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;

    // One row per thing the rule generalises over: which members the request
    // carries, whether a stored property is kept, renamed, re-united, dropped or
    // joined by a new one, how many properties the new definition has, and
    // whether an external id is assigned.
    let cases: Vec<UpdateCase> = vec![
        UpdateCase {
            case: "the name alone, so every property is removed",
            body: Box::new(|_, _| json!({"assetModelCompositeModelName": "Renamed"})),
            name: "Renamed",
            description: "",
            external_id: None,
            properties: Vec::new(),
        },
        UpdateCase {
            case: "both properties kept by id, renamed, re-united and re-defaulted",
            body: Box::new(|setpoint, cycles| {
                json!({
                    "assetModelCompositeModelName": "Reshaped",
                    "assetModelCompositeModelDescription": "a replaced description",
                    "assetModelCompositeModelProperties": [
                        {
                            "id": setpoint,
                            "name": "TargetTemp",
                            "dataType": "DOUBLE",
                            "unit": "Kelvin",
                            "type": {"attribute": {"defaultValue": "293"}},
                        },
                        {
                            "id": cycles,
                            "name": "Runs",
                            "dataType": "INTEGER",
                            "unit": "Runs",
                            "type": {"measurement": {}},
                        },
                    ],
                })
            }),
            name: "Reshaped",
            description: "a replaced description",
            external_id: None,
            properties: vec![
                ExpectProperty {
                    name: "TargetTemp",
                    unit: "Kelvin",
                    r#type: json!({"attribute": {"defaultValue": "293"}}),
                    kept: Kept::Setpoint,
                },
                ExpectProperty {
                    name: "Runs",
                    unit: "Runs",
                    r#type: json!({"measurement": {}}),
                    kept: Kept::Cycles,
                },
            ],
        },
        UpdateCase {
            case: "one property kept, the other left out of the request",
            body: Box::new(|_, cycles| {
                json!({
                    "assetModelCompositeModelName": "Trimmed",
                    "assetModelCompositeModelProperties": [
                        {
                            "id": cycles,
                            "name": "Cycles",
                            "dataType": "INTEGER",
                            "unit": "Count",
                            "type": {"measurement": {}},
                        },
                    ],
                })
            }),
            name: "Trimmed",
            description: "",
            external_id: None,
            properties: vec![ExpectProperty {
                name: "Cycles",
                unit: "Count",
                r#type: json!({"measurement": {}}),
                kept: Kept::Cycles,
            }],
        },
        UpdateCase {
            case: "a property the request describes for the first time, beside a kept one",
            body: Box::new(|setpoint, _| {
                json!({
                    "assetModelCompositeModelName": "Grown",
                    "assetModelCompositeModelProperties": [
                        {
                            "id": setpoint,
                            "name": "Setpoint",
                            "dataType": "DOUBLE",
                            "unit": "Celsius",
                            "type": {"attribute": {"defaultValue": "20"}},
                        },
                        {
                            "name": "Pressure",
                            "dataType": "DOUBLE",
                            "unit": "Bar",
                            "type": {"measurement": {}},
                        },
                    ],
                })
            }),
            name: "Grown",
            description: "",
            external_id: None,
            properties: vec![
                ExpectProperty {
                    name: "Setpoint",
                    unit: "Celsius",
                    r#type: json!({"attribute": {"defaultValue": "20"}}),
                    kept: Kept::Setpoint,
                },
                ExpectProperty {
                    name: "Pressure",
                    unit: "Bar",
                    r#type: json!({"measurement": {}}),
                    kept: Kept::Fresh,
                },
            ],
        },
        UpdateCase {
            case: "an external id assigned to a composite model that had none",
            body: Box::new(|setpoint, _| {
                json!({
                    "assetModelCompositeModelName": "Labelled",
                    "assetModelCompositeModelExternalId": "boiler_custom_one",
                    "assetModelCompositeModelProperties": [
                        {
                            "id": setpoint,
                            "name": "Setpoint",
                            "dataType": "DOUBLE",
                            "unit": "Celsius",
                            "type": {"attribute": {"defaultValue": "20"}},
                        },
                    ],
                })
            }),
            name: "Labelled",
            description: "",
            external_id: Some("boiler_custom_one"),
            properties: vec![ExpectProperty {
                name: "Setpoint",
                unit: "Celsius",
                r#type: json!({"attribute": {"defaultValue": "20"}}),
                kept: Kept::Setpoint,
            }],
        },
        UpdateCase {
            case: "a STRUCT property, which carries its dataTypeSpec",
            body: Box::new(|_, _| {
                json!({
                    "assetModelCompositeModelName": "Structured",
                    "assetModelCompositeModelProperties": [
                        {
                            "name": "State",
                            "dataType": "STRUCT",
                            "dataTypeSpec": "AWS/ALARM_STATE",
                            "unit": "none",
                            "type": {"measurement": {}},
                        },
                    ],
                })
            }),
            name: "Structured",
            description: "",
            external_id: None,
            properties: vec![ExpectProperty {
                name: "State",
                unit: "none",
                r#type: json!({"measurement": {}}),
                kept: Kept::Fresh,
            }],
        },
    ];

    for (at, case) in cases.iter().enumerate() {
        let name = case.case;
        // A fresh composite model per row, so one row's rewrite is not the next
        // row's starting point.
        let (composite_id, setpoint_id, cycles_id) =
            baseline_composite(&router, &model_id, &format!("Base{at}")).await;
        let body = (case.body)(&setpoint_id, &cycles_id);

        let (status, updated) = update_composite(&router, &model_id, &composite_id, body).await;
        assert_eq!(
            status, 202,
            "case {name:?}: update got {status} {updated}"
        );
        assert_eq!(
            updated["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {name:?}: the update must report the asset model it wrote to, got {updated}"
        );
        // A top-level composite model's path is the asset model root followed by
        // the composite model itself.
        assert_eq!(
            updated["assetModelCompositeModelPath"]
                .as_array()
                .map(Vec::len),
            Some(2),
            "case {name:?}: expected the model root plus one segment, got {updated}"
        );
        assert_eq!(
            updated["assetModelCompositeModelPath"][0]["id"].as_str(),
            Some(model_id.as_str()),
            "case {name:?}: the path must start at the asset model itself, got {updated}"
        );
        assert_eq!(
            updated["assetModelCompositeModelPath"][1]["id"].as_str(),
            Some(composite_id.as_str()),
            "case {name:?}: the path must end at the composite model itself, got {updated}"
        );
        assert_eq!(
            updated["assetModelCompositeModelPath"][1]["name"].as_str(),
            Some(case.name),
            "case {name:?}: the path must carry the NEW name, got {updated}"
        );
        assert!(
            !updated["assetModelStatus"]["state"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {name:?}: the update must report the asset model's state, got {updated}"
        );

        // Read it back through the describe that reports one composite model.
        let (status, described) = describe_composite(&router, &model_id, &composite_id).await;
        assert_eq!(
            status, 200,
            "case {name:?}: describe got {status} {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelName"].as_str(),
            Some(case.name),
            "case {name:?}: the new name did not read back, got {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelDescription"].as_str(),
            Some(case.description),
            "case {name:?}: the new description did not read back, got {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelExternalId"].as_str(),
            case.external_id,
            "case {name:?}: the external id did not read back, got {described}"
        );
        // `assetModelCompositeModelType` has no member on this request, so the
        // kind of composite model this is cannot change.
        assert_eq!(
            described["assetModelCompositeModelType"].as_str(),
            Some("CUSTOM"),
            "case {name:?}: the type must be the one it was created with, got {described}"
        );

        let read = described["assetModelCompositeModelProperties"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            read.len(),
            case.properties.len(),
            "case {name:?}: expected {} properties, got {described}",
            case.properties.len()
        );
        for (expected, actual) in case.properties.iter().zip(read.iter()) {
            assert_eq!(
                actual["name"].as_str(),
                Some(expected.name),
                "case {name:?}: expected the property {:?}, got {actual}",
                expected.name
            );
            assert_eq!(
                actual["unit"].as_str(),
                Some(expected.unit),
                "case {name:?}: property {:?} expected unit {:?}, got {actual}",
                expected.name,
                expected.unit
            );
            assert_eq!(
                actual["type"], expected.r#type,
                "case {name:?}: property {:?} expected type {}, got {actual}",
                expected.name, expected.r#type
            );
            let read_id = actual["id"].as_str().unwrap_or_default();
            assert!(
                is_modelled_id(read_id),
                "case {name:?}: property {:?} must carry a modelled UUID id, got {actual}",
                expected.name
            );
            match expected.kept {
                Kept::Setpoint => assert_eq!(
                    read_id, setpoint_id,
                    "case {name:?}: property {:?} must keep the stored id it was read under, \
                     got {actual}",
                    expected.name
                ),
                Kept::Cycles => assert_eq!(
                    read_id, cycles_id,
                    "case {name:?}: property {:?} must keep the stored id it was read under, \
                     got {actual}",
                    expected.name
                ),
                Kept::Fresh => assert!(
                    read_id != setpoint_id && read_id != cycles_id,
                    "case {name:?}: property {:?} is new, so it must not take a stored \
                     property's id, got {actual}",
                    expected.name
                ),
            }
        }

        // An assigned external id is a second way to address the same composite
        // model, so the `externalId:` label resolves to it.
        if let Some(external) = case.external_id {
            let (status, by_label) =
                describe_composite(&router, &model_id, &format!("externalId:{external}")).await;
            assert_eq!(
                status, 200,
                "case {name:?}: describe by external id got {status} {by_label}"
            );
            assert_eq!(
                by_label["assetModelCompositeModelId"].as_str(),
                Some(composite_id.as_str()),
                "case {name:?}: the externalId: label must resolve to the updated composite \
                 model, got {by_label}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_moves_the_asset_models_version_and_the_active_version_follows() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (composite_id, setpoint_id, _) = baseline_composite(&router, &model_id, "Base").await;
    // Let the transition the create owed fall due, so the model is ACTIVE and has
    // an ACTIVE version for the update to supersede.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let before = model_etag(&router, &model_id).await;
    let (status, updated) = update_composite(
        &router,
        &model_id,
        &composite_id,
        json!({
            "assetModelCompositeModelName": "Reshaped",
            "assetModelCompositeModelProperties": [
                {
                    "id": &setpoint_id,
                    "name": "Setpoint",
                    "dataType": "DOUBLE",
                    "unit": "Celsius",
                    "type": {"attribute": {"defaultValue": "20"}},
                },
            ],
        }),
    )
    .await;
    assert_eq!(status, 202, "update got {status} {updated}");
    assert_eq!(
        updated["assetModelStatus"]["state"].as_str(),
        Some("UPDATING"),
        "the asset model owes a transition back to ACTIVE, got {updated}"
    );

    let after = model_etag(&router, &model_id).await;
    assert_ne!(
        before, after,
        "the asset model's definition changed, so its entity tag must move: {before} -> {after}"
    );

    // The LATEST version is the definition just written; the ACTIVE version is
    // the one it superseded, until the owed transition falls due.
    let (status, latest) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(status, 200, "describe LATEST got {status} {latest}");
    assert_eq!(
        latest["assetModelCompositeModelName"].as_str(),
        Some("Reshaped"),
        "the LATEST version must report the new definition, got {latest}"
    );
    let (status, active) = call(
        &router,
        "GET",
        &format!(
            "/asset-models/{model_id}/composite-models/{composite_id}?assetModelVersion=ACTIVE"
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe ACTIVE got {status} {active}");
    assert_eq!(
        active["assetModelCompositeModelName"].as_str(),
        Some("Base"),
        "the ACTIVE version must still report the superseded definition, got {active}"
    );

    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let (status, active) = call(
        &router,
        "GET",
        &format!(
            "/asset-models/{model_id}/composite-models/{composite_id}?assetModelVersion=ACTIVE"
        ),
        None,
    )
    .await;
    assert_eq!(status, 200, "describe ACTIVE after the wait got {status} {active}");
    assert_eq!(
        active["assetModelCompositeModelName"].as_str(),
        Some("Reshaped"),
        "once the owed transition falls due the new definition is the ACTIVE one, got {active}"
    );
}

#[tokio::test]
async fn an_update_outside_a_documented_rule_is_refused_and_changes_nothing() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (composite_id, setpoint_id, cycles_id) =
        baseline_composite(&router, &model_id, "Base").await;
    // A sibling composite model, so an external id can already be taken within
    // this asset model.
    let (status, sibling) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Sibling",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelExternalId": "taken_label",
        }),
    )
    .await;
    assert_eq!(status, 202, "create the sibling: got {status} {sibling}");

    // A property list one past the published 200-per-composite-model quota.
    let past_the_quota: Vec<Value> = (0..=MAX_PROPERTIES_PER_COMPOSITE_MODEL)
        .map(|at| {
            json!({
                "name": format!("P{at}"),
                "dataType": "DOUBLE",
                "type": {"measurement": {}},
            })
        })
        .collect();

    // An id of the modelled form that names no stored resource: the refusal is a
    // 404, not a validation error.
    let absent_id = "11111111-2222-4333-8444-555555555555";

    let cases: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "an asset model that does not exist",
            absent_id.to_string(),
            json!({"assetModelCompositeModelName": "Nope"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a composite model that does not exist",
            model_id.clone(),
            json!({"assetModelCompositeModelName": "Nope"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an external id label that names no composite model",
            model_id.clone(),
            json!({"assetModelCompositeModelName": "Nope"}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stored property changing its data type",
            model_id.clone(),
            json!({
                "assetModelCompositeModelName": "Base",
                "assetModelCompositeModelProperties": [
                    {
                        "id": &setpoint_id,
                        "name": "Setpoint",
                        "dataType": "STRING",
                        "type": {"attribute": {"defaultValue": "20"}},
                    },
                ],
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a stored property changing its property type",
            model_id.clone(),
            json!({
                "assetModelCompositeModelName": "Base",
                "assetModelCompositeModelProperties": [
                    {
                        "id": &setpoint_id,
                        "name": "Setpoint",
                        "dataType": "DOUBLE",
                        "type": {"measurement": {}},
                    },
                ],
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "one stored property named twice",
            model_id.clone(),
            json!({
                "assetModelCompositeModelName": "Base",
                "assetModelCompositeModelProperties": [
                    {
                        "id": &cycles_id,
                        "name": "Cycles",
                        "dataType": "INTEGER",
                        "type": {"measurement": {}},
                    },
                    {
                        "id": &cycles_id,
                        "name": "CyclesAgain",
                        "dataType": "INTEGER",
                        "type": {"measurement": {}},
                    },
                ],
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a property specifying two property types",
            model_id.clone(),
            json!({
                "assetModelCompositeModelName": "Base",
                "assetModelCompositeModelProperties": [
                    {
                        "name": "Both",
                        "dataType": "DOUBLE",
                        "type": {"measurement": {}, "attribute": {"defaultValue": "1"}},
                    },
                ],
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "a STRUCT property without its dataTypeSpec",
            model_id.clone(),
            json!({
                "assetModelCompositeModelName": "Base",
                "assetModelCompositeModelProperties": [
                    {"name": "State", "dataType": "STRUCT", "type": {"measurement": {}}},
                ],
            }),
            400,
            "InvalidRequestException",
        ),
        (
            "one property past the published per-composite-model quota",
            model_id.clone(),
            json!({
                "assetModelCompositeModelName": "Base",
                "assetModelCompositeModelProperties": past_the_quota,
            }),
            410,
            "LimitExceededException",
        ),
        (
            "an external id another composite model of this model already uses",
            model_id.clone(),
            json!({
                "assetModelCompositeModelName": "Base",
                "assetModelCompositeModelExternalId": "taken_label",
            }),
            409,
            "ResourceAlreadyExistsException",
        ),
    ];

    for (case, target_model, body, expected_status, expected_code) in cases {
        // The two rows about an absent composite model are the only ones that
        // address anything but the stored composite model.
        let composite_label = match case {
            "a composite model that does not exist" => absent_id.to_string(),
            "an external id label that names no composite model" => {
                "externalId:no_such_label".to_string()
            }
            _ => composite_id.clone(),
        };
        let (status, refused) =
            update_composite(&router, &target_model, &composite_label, body).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            expected_code,
            "case {case:?}: expected {expected_code}, got {refused}"
        );
        assert!(
            !refused["message"]
                .as_str()
                .unwrap_or_default()
                .is_empty(),
            "case {case:?}: a refusal states why, got {refused}"
        );

        // A refused update changed nothing: the stored definition is still the
        // baseline, properties and all.
        let (status, described) = describe_composite(&router, &model_id, &composite_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe after the refusal got {status} {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelName"].as_str(),
            Some("Base"),
            "case {case:?}: a refused update must leave the definition alone, got {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelProperties"]
                .as_array()
                .map(Vec::len),
            Some(2),
            "case {case:?}: a refused update must leave the properties alone, got {described}"
        );
        assert_eq!(
            described["assetModelCompositeModelExternalId"].as_str(),
            None,
            "case {case:?}: a refused update must assign no external id, got {described}"
        );
    }
}

#[tokio::test]
async fn an_external_id_cannot_be_replaced_but_can_be_restated() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Labelled",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelExternalId": "first_label",
        }),
    )
    .await;
    assert_eq!(status, 202, "create got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .expect("assetModelCompositeModelId")
        .to_string();

    // Restating the id it already carries asks for the state it is already in.
    let (status, restated) = update_composite(
        &router,
        &model_id,
        &composite_id,
        json!({
            "assetModelCompositeModelName": "Still labelled",
            "assetModelCompositeModelExternalId": "first_label",
        }),
    )
    .await;
    assert_eq!(status, 202, "restating the same external id got {status} {restated}");

    // A different one cannot replace it.
    let (status, refused) = update_composite(
        &router,
        &model_id,
        &composite_id,
        json!({
            "assetModelCompositeModelName": "Relabelled",
            "assetModelCompositeModelExternalId": "second_label",
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "replacing an external id that is already set got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "expected InvalidRequestException, got {refused}"
    );

    let (status, described) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(status, 200, "describe got {status} {described}");
    assert_eq!(
        described["assetModelCompositeModelExternalId"].as_str(),
        Some("first_label"),
        "the external id the composite model was created with must survive, got {described}"
    );
    assert_eq!(
        described["assetModelCompositeModelName"].as_str(),
        Some("Still labelled"),
        "the served update's name must be the stored one, got {described}"
    );
}

#[tokio::test]
async fn the_conditional_headers_decide_whether_the_update_lands() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (composite_id, _, _) = baseline_composite(&router, &model_id, "Base").await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let tag = model_etag(&router, &model_id).await;
    // An ETag of the modelled `^[\w-]{43}$` form that is not the model's.
    let other_tag = "A".repeat(43);

    let cases: Vec<HeaderCase<'_>> = vec![
        (
            "If-Match naming the current latest version's tag",
            vec![("if-match", tag.as_str())],
            202,
            "",
        ),
        (
            "If-Match naming another tag",
            vec![("if-match", other_tag.as_str())],
            412,
            "PreconditionFailedException",
        ),
        (
            "If-None-Match * while a latest version exists",
            vec![("if-none-match", "*")],
            412,
            "PreconditionFailedException",
        ),
        (
            "If-None-Match * against the ACTIVE version, which exists too",
            vec![
                ("if-none-match", "*"),
                ("match-for-version-type", "ACTIVE"),
            ],
            412,
            "PreconditionFailedException",
        ),
    ];

    for (case, headers, expected_status, expected_code) in cases {
        let (status, body) = update_composite_with_headers(
            &router,
            &model_id,
            &composite_id,
            json!({"assetModelCompositeModelName": "Conditioned"}),
            &headers,
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if !expected_code.is_empty() {
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code}, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_retried_update_replays_and_a_token_spent_elsewhere_conflicts() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (composite_id, _, _) = baseline_composite(&router, &model_id, "Base").await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;

    let token = "c0ffee00-1111-2222-3333-444444444444-token";
    let request = json!({
        "assetModelCompositeModelName": "Tokened",
        "clientToken": token,
    });

    let (status, first) =
        update_composite(&router, &model_id, &composite_id, request.clone()).await;
    assert_eq!(status, 202, "the first call got {status} {first}");
    let after_first = model_etag(&router, &model_id).await;

    let (status, retry) = update_composite(&router, &model_id, &composite_id, request).await;
    assert_eq!(status, 202, "the retry got {status} {retry}");
    assert_eq!(
        retry["assetModelCompositeModelPath"], first["assetModelCompositeModelPath"],
        "a retry reports the same composite model path, got {retry}"
    );
    assert_eq!(
        model_etag(&router, &model_id).await,
        after_first,
        "a retry writes nothing, so the asset model's entity tag must not move"
    );

    // The same token on a DIFFERENT request belongs to another call.
    let (status, conflict) = update_composite(
        &router,
        &model_id,
        &composite_id,
        json!({
            "assetModelCompositeModelName": "SomethingElse",
            "clientToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 409,
        "a token spent on another request got {status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "ConflictingOperationException",
        "expected ConflictingOperationException, got {conflict}"
    );
    assert_eq!(
        conflict["resourceId"].as_str(),
        Some(model_id.as_str()),
        "the conflict names the asset model the call wrote to, got {conflict}"
    );
}

#[tokio::test]
async fn the_assets_created_from_the_model_report_the_updated_composite_model() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (composite_id, setpoint_id, _) = baseline_composite(&router, &model_id, "Base").await;
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;

    let (status, updated) = update_composite(
        &router,
        &model_id,
        &composite_id,
        json!({
            "assetModelCompositeModelName": "Inherited",
            "assetModelCompositeModelProperties": [
                {
                    "id": &setpoint_id,
                    "name": "TargetTemp",
                    "dataType": "DOUBLE",
                    "unit": "Kelvin",
                    "type": {"attribute": {"defaultValue": "293"}},
                },
            ],
        }),
    )
    .await;
    assert_eq!(status, 202, "update got {status} {updated}");

    let (status, described) = call(
        &router,
        "GET",
        &format!("/assets/{asset_id}/composite-models/{composite_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAssetCompositeModel got {status} {described}"
    );
    assert_eq!(
        described["assetCompositeModelName"].as_str(),
        Some("Inherited"),
        "the asset inherits the updated composite model's name, got {described}"
    );
    let names: Vec<String> = described["assetCompositeModelProperties"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|property| property["name"].as_str())
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        names,
        vec!["TargetTemp".to_string()],
        "the asset inherits the updated property definitions, got {described}"
    );
    assert_eq!(
        described["assetCompositeModelProperties"][0]["id"].as_str(),
        Some(setpoint_id.as_str()),
        "a kept property keeps the id the asset's data is recorded against, got {described}"
    );
}

#[tokio::test]
async fn a_component_model_based_composite_model_keeps_the_reused_properties() {
    let router = router();
    let (status, component) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Pump",
            "assetModelType": "COMPONENT_MODEL",
            "assetModelProperties": [
                {"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "create the component model: got {status} {component}");
    let component_id = component["assetModelId"]
        .as_str()
        .expect("assetModelId")
        .to_string();

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
    assert_eq!(status, 202, "create the composite model: got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .expect("assetModelCompositeModelId")
        .to_string();

    // Properties cannot be stated for one: the model it is composed of provides
    // them.
    let (status, refused) = update_composite(
        &router,
        &model_id,
        &composite_id,
        json!({
            "assetModelCompositeModelName": "ThePump",
            "assetModelCompositeModelProperties": [
                {"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "stating properties for a component-model-based composite model got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "expected InvalidRequestException, got {refused}"
    );

    // A rename is served, and the reused properties are still the ones reported.
    let (status, updated) = update_composite(
        &router,
        &model_id,
        &composite_id,
        json!({"assetModelCompositeModelName": "RenamedPump"}),
    )
    .await;
    assert_eq!(status, 202, "renaming it got {status} {updated}");

    let (status, described) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(status, 200, "describe got {status} {described}");
    assert_eq!(
        described["assetModelCompositeModelName"].as_str(),
        Some("RenamedPump"),
        "the new name did not read back, got {described}"
    );
    assert_eq!(
        described["assetModelCompositeModelProperties"][0]["name"].as_str(),
        Some("Flow"),
        "the composed component model's properties must still be reported, got {described}"
    );
    assert_eq!(
        described["compositionDetails"]["compositionRelationship"][0]["id"].as_str(),
        Some(component_id.as_str()),
        "the composition relationship must survive the update, got {described}"
    );
}
