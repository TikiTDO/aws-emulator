// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteAssetModelCompositeModel`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with, so every
//! assertion is about the wire behaviour a real client sees.
//!
//! The rules these suites pin:
//!
//! * deleting a composite model removes it — and every composite model nested
//!   inside it, at any depth — from its asset model's stored record, whatever
//!   its type, property count or position in the nesting, and leaves every
//!   other composite model of that model and of every other model exactly
//!   where it was;
//! * the delete is a write to the ASSET MODEL, so the model carries a new
//!   version and a new entity tag and owes the transition back to `ACTIVE`
//!   whose state the delete reports;
//! * the conditional headers are evaluated before anything is removed, so a
//!   failed precondition leaves the composite model readable;
//! * a composite model with a dependent — an asset created from its asset
//!   model, or a surviving formula expression that reads one of its properties
//!   — is the modelled refusal, and the same delete succeeds once the
//!   dependent is gone;
//! * an id, in either `CustomID` form, that resolves to no stored composite
//!   model of that asset model is the modelled 404.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, error_code, router};
use super::asset_model_composite_model::{
    call_with_headers, create_composite, describe_composite, model_etag, plain_model,
    PAST_THE_OWED_TRANSITION,
};

/// A well-formed `CustomID` no resource in these suites is stored under.
const STRANGER: &str = "99999999-8888-4777-8666-555555555555";

/// `DELETE /asset-models/{model_label}/composite-models/{composite_label}` with
/// `headers`, as `(status, parsed body)`.
async fn delete_composite(
    router: &MultiServiceRouter,
    model_label: &str,
    composite_label: &str,
    headers: &[(&str, &str)],
) -> (u16, Value) {
    let (status, body, _) = call_with_headers(
        router,
        "DELETE",
        &format!("/asset-models/{model_label}/composite-models/{composite_label}"),
        None,
        headers,
    )
    .await;
    (status, body)
}

/// One row of the removal table: a shape of composite models to build on a
/// fresh asset model, which of them the delete names, and which of them the
/// delete must take with it.
struct RemovalCase {
    /// What the row varies; every assertion carries it.
    case: &'static str,
    /// One `CreateAssetModelCompositeModel` body per composite model, in the
    /// order they are created. The `usize` indexes an EARLIER entry, for a
    /// composite model nested inside it.
    creates: Vec<(Value, Option<usize>)>,
    /// Which created composite model the delete names.
    delete: usize,
    /// The ones that must be gone afterwards — the deleted one included.
    gone: Vec<usize>,
}

/// One row of the refusal table: the delete to send, and the modelled refusal
/// it must answer with.
struct RefusalCase {
    /// What the row varies; every assertion carries it.
    case: &'static str,
    /// The `assetModelId` label the request puts in the path.
    model_label: String,
    /// The `assetModelCompositeModelId` label the request puts in the path.
    composite_label: String,
    /// The conditional request headers the row sends.
    headers: Vec<(&'static str, String)>,
    /// The HTTP status the refusal carries.
    status: u16,
    /// The wire error code the refusal carries.
    code: &'static str,
}

/// A `CUSTOM` composite model with no properties of its own.
fn plain_composite(name: &str) -> Value {
    json!({
        "assetModelCompositeModelName": name,
        "assetModelCompositeModelType": "CUSTOM",
    })
}

/// The removal table: one row per thing the rule generalises over — the
/// composite model's type (the documented catalog), how many properties it
/// carries, whether it has siblings, and where in a nesting chain it sits
/// (root, middle, leaf).
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "a lone CUSTOM composite model with one property",
            creates: vec![(
                json!({
                    "assetModelCompositeModelName": "Custom1",
                    "assetModelCompositeModelType": "CUSTOM",
                    "assetModelCompositeModelDescription": "a custom grouping",
                    "assetModelCompositeModelProperties": [
                        {"name": "Setpoint", "dataType": "DOUBLE", "unit": "Celsius",
                         "type": {"attribute": {"defaultValue": "20"}}},
                    ],
                }),
                None,
            )],
            delete: 0,
            gone: vec![0],
        },
        RemovalCase {
            case: "an AWS/ALARM composite model beside a sibling that stays",
            creates: vec![
                (
                    json!({
                        "assetModelCompositeModelName": "HighTemp",
                        "assetModelCompositeModelType": "AWS/ALARM",
                        "assetModelCompositeModelProperties": [
                            {"name": "State", "dataType": "STRUCT",
                             "dataTypeSpec": "AWS/ALARM_STATE", "type": {"measurement": {}}},
                        ],
                    }),
                    None,
                ),
                (plain_composite("Untouched"), None),
            ],
            delete: 0,
            gone: vec![0],
        },
        RemovalCase {
            case: "an AWS/L4E_ANOMALY composite model with three properties",
            creates: vec![(
                json!({
                    "assetModelCompositeModelName": "Anomaly",
                    "assetModelCompositeModelType": "AWS/L4E_ANOMALY",
                    "assetModelCompositeModelProperties": [
                        {"name": "P1", "dataType": "DOUBLE", "type": {"measurement": {}}},
                        {"name": "P2", "dataType": "INTEGER", "type": {"attribute": {}}},
                        {"name": "P3", "dataType": "STRING", "type": {"attribute": {}}},
                    ],
                }),
                None,
            )],
            delete: 0,
            gone: vec![0],
        },
        RemovalCase {
            case: "the root of a two-deep nesting, which takes its child",
            creates: vec![
                (plain_composite("Level0"), None),
                (plain_composite("Level1"), Some(0)),
            ],
            delete: 0,
            gone: vec![0, 1],
        },
        RemovalCase {
            case: "the middle of a three-deep chain, which takes the leaf below it",
            creates: vec![
                (plain_composite("Level0"), None),
                (plain_composite("Level1"), Some(0)),
                (plain_composite("Level2"), Some(1)),
            ],
            delete: 1,
            gone: vec![1, 2],
        },
        RemovalCase {
            case: "the leaf of a three-deep chain, which takes nothing with it",
            creates: vec![
                (plain_composite("Level0"), None),
                (plain_composite("Level1"), Some(0)),
                (plain_composite("Level2"), Some(1)),
            ],
            delete: 2,
            gone: vec![2],
        },
        RemovalCase {
            case: "a root with two children, which takes both",
            creates: vec![
                (plain_composite("Level0"), None),
                (plain_composite("ChildA"), Some(0)),
                (plain_composite("ChildB"), Some(0)),
                (plain_composite("Sibling"), None),
            ],
            delete: 0,
            gone: vec![0, 1, 2],
        },
    ]
}

#[tokio::test]
async fn deleting_a_composite_model_removes_it_and_everything_nested_in_it() {
    let router = router();

    for (at, case) in removal_cases().into_iter().enumerate() {
        let RemovalCase {
            case: name,
            creates,
            delete,
            gone,
        } = &case;
        // One model per row, under a name of its own: `assetModelName` is unique
        // per account and every row runs against the same router.
        let model_id = plain_model(&router, &format!("Boiler-{at}")).await;

        let mut ids: Vec<String> = Vec::new();
        for (at, (body, parent)) in creates.iter().enumerate() {
            let mut body = body.clone();
            if let Some(parent) = parent {
                body["parentAssetModelCompositeModelId"] = json!(ids[*parent]);
            }
            let (status, created) = create_composite(&router, &model_id, body).await;
            assert_eq!(
                status, 202,
                "case {name:?}: create {at} got {status} {created}"
            );
            ids.push(
                created["assetModelCompositeModelId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {name:?}: create {at} minted no id: {created}"))
                    .to_string(),
            );
        }

        // Past the transition the creates owe, so the state the delete reports
        // is the one a write to a SETTLED model reports.
        tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
        let tag_before = model_etag(&router, &model_id).await;
        let (status, model_before) =
            call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeAssetModel got {status} {model_before}"
        );

        let (status, deleted) = delete_composite(&router, &model_id, &ids[*delete], &[]).await;
        assert_eq!(status, 202, "case {name:?}: delete got {status} {deleted}");
        assert_eq!(
            deleted["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {name:?}: the delete must report the asset model it wrote to, got {deleted}"
        );
        assert_eq!(
            deleted["assetModelStatus"]["state"].as_str(),
            Some("UPDATING"),
            "case {name:?}: a settled model reports UPDATING while the write settles, got {deleted}"
        );

        for (at, id) in ids.iter().enumerate() {
            let (status, body) = describe_composite(&router, &model_id, id).await;
            if gone.contains(&at) {
                assert_eq!(
                    status, 404,
                    "case {name:?}: composite model {at} must be gone, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "ResourceNotFoundException",
                    "case {name:?}: composite model {at} is gone, got {body}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {name:?}: composite model {at} must survive, got {status} {body}"
                );
            }
        }

        // The asset model's own view agrees: it reports exactly the surviving
        // composite models it holds directly, in creation order — the built-in
        // ones under `assetModelCompositeModels`, the custom ones among the
        // summaries.
        let (status, model_after) =
            call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {name:?}: DescribeAssetModel got {status} {model_after}"
        );
        let surviving_top = |built_in: bool| -> Vec<&str> {
            creates
                .iter()
                .enumerate()
                .filter(|(at, (body, parent))| {
                    parent.is_none()
                        && !gone.contains(at)
                        && (body["assetModelCompositeModelType"] != json!("CUSTOM")) == built_in
                })
                .map(|(at, _)| ids[at].as_str())
                .collect()
        };
        for (member, built_in) in [
            ("assetModelCompositeModels", true),
            ("assetModelCompositeModelSummaries", false),
        ] {
            let expected_top = surviving_top(built_in);
            let reported: Vec<&str> = model_after[member]
                .as_array()
                .map(|list| list.iter().filter_map(|entry| entry["id"].as_str()).collect())
                .unwrap_or_default();
            assert_eq!(
                reported, expected_top,
                "case {name:?}: {member} must report the surviving top-level composite models \
                 {expected_top:?}, got {model_after}"
            );
        }

        // The model's definition changed, so both the version and the entity
        // tag a conditional write compares against move on.
        let tag_after = model_etag(&router, &model_id).await;
        assert_ne!(
            tag_before, tag_after,
            "case {name:?}: the delete changes the definition, so the entity tag must change"
        );
        assert_ne!(
            model_before["assetModelVersion"], model_after["assetModelVersion"],
            "case {name:?}: the delete must carry a new model version, got {model_after}"
        );
    }
}

#[tokio::test]
async fn a_delete_outside_a_documented_rule_is_the_modelled_refusal_and_removes_nothing() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let other_model_id = plain_model(&router, "Pump").await;

    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelExternalId": "custom_one",
            "assetModelCompositeModelProperties": [
                {"name": "Setpoint", "dataType": "DOUBLE",
                 "type": {"attribute": {"defaultValue": "20"}}},
            ],
        }),
    )
    .await;
    assert_eq!(status, 202, "create composite: got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create composite minted no id: {created}"))
        .to_string();

    // Past the owed transition, so the model HAS an active version and the
    // `ACTIVE` target of the conditional headers is not vacuous.
    tokio::time::sleep(PAST_THE_OWED_TRANSITION).await;
    let tag = model_etag(&router, &model_id).await;
    // A tag of the modelled `^[\w-]{43}$` form that is not this model's, so the
    // refusal is the precondition failing rather than input validation.
    let stale_tag = "a".repeat(43);
    assert_ne!(
        stale_tag, tag,
        "the stale tag must differ from the model's current one"
    );

    // One row per way of being outside a documented rule: an id that names
    // nothing, an id that names a composite model of another model, and each
    // conditional header evaluating to FALSE against either version target.
    let cases = vec![
        RefusalCase {
            case: "an asset model that is not stored",
            model_label: STRANGER.to_string(),
            composite_label: composite_id.clone(),
            headers: vec![],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a composite model that is not stored",
            model_label: model_id.clone(),
            composite_label: STRANGER.to_string(),
            headers: vec![],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "an externalId label naming no composite model",
            model_label: model_id.clone(),
            composite_label: "externalId:no_such_composite".to_string(),
            headers: vec![],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "a composite model of another asset model",
            model_label: other_model_id.clone(),
            composite_label: composite_id.clone(),
            headers: vec![],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RefusalCase {
            case: "an If-Match that is not the model's current tag",
            model_label: model_id.clone(),
            composite_label: composite_id.clone(),
            headers: vec![("if-match", stale_tag.clone())],
            status: 412,
            code: "PreconditionFailedException",
        },
        RefusalCase {
            case: "an If-Match on the ACTIVE version that is not its tag",
            model_label: model_id.clone(),
            composite_label: composite_id.clone(),
            headers: vec![
                ("if-match", stale_tag.clone()),
                ("match-for-version-type", "ACTIVE".to_string()),
            ],
            status: 412,
            code: "PreconditionFailedException",
        },
        RefusalCase {
            case: "If-None-Match * while a LATEST version exists",
            model_label: model_id.clone(),
            composite_label: composite_id.clone(),
            headers: vec![("if-none-match", "*".to_string())],
            status: 412,
            code: "PreconditionFailedException",
        },
        RefusalCase {
            case: "If-None-Match * while an ACTIVE version exists",
            model_label: model_id.clone(),
            composite_label: composite_id.clone(),
            headers: vec![
                ("if-none-match", "*".to_string()),
                ("match-for-version-type", "ACTIVE".to_string()),
            ],
            status: 412,
            code: "PreconditionFailedException",
        },
    ];

    for row in cases {
        let RefusalCase {
            case,
            model_label,
            composite_label,
            headers,
            status: expected_status,
            code: expected_code,
        } = &row;
        let sent: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let (status, body) = delete_composite(&router, model_label, composite_label, &sent).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // A refused delete removes NOTHING: the composite model still reads
        // back, and the model's entity tag is the one it had.
        let (status, still_there) = describe_composite(&router, &model_id, &composite_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must leave the composite model readable, got \
             {status} {still_there}"
        );
        let unchanged = model_etag(&router, &model_id).await;
        assert_eq!(
            unchanged, tag,
            "case {case:?}: a refused delete must not change the model's entity tag"
        );
    }

    // The precondition a caller CAN satisfy: the tag it last read.
    let (status, deleted) = delete_composite(
        &router,
        &model_id,
        &composite_id,
        &[("if-match", tag.as_str()), ("match-for-version-type", "LATEST")],
    )
    .await;
    assert_eq!(
        status, 202,
        "If-Match with the model's current tag must be accepted, got {status} {deleted}"
    );
    let (status, body) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(
        status, 404,
        "the composite model must be gone after the accepted delete, got {status} {body}"
    );
}

#[tokio::test]
async fn an_asset_of_the_model_refuses_the_delete_until_the_asset_is_gone() {
    let router = router();
    let model_id = plain_model(&router, "Boiler").await;
    let (status, created) = create_composite(
        &router,
        &model_id,
        json!({
            "assetModelCompositeModelName": "Custom1",
            "assetModelCompositeModelType": "CUSTOM",
            "assetModelCompositeModelProperties": [
                {"name": "Setpoint", "dataType": "DOUBLE", "type": {"measurement": {}}},
            ],
        }),
    )
    .await;
    assert_eq!(status, 202, "create composite: got {status} {created}");
    let composite_id = created["assetModelCompositeModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create composite minted no id: {created}"))
        .to_string();

    // "You must delete all assets created from a composite model before you can
    // delete the model" — the asset instantiates the model this composite model
    // is part of.
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;
    let (status, refused) = delete_composite(&router, &model_id, &composite_id, &[]).await;
    assert_eq!(
        status, 400,
        "an asset of the model must refuse the delete, got {status} {refused}"
    );
    assert_eq!(
        error_code(&refused),
        "InvalidRequestException",
        "expected InvalidRequestException, got {refused}"
    );
    let (status, still_there) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(
        status, 200,
        "the refused delete must leave the composite model readable, got {status} {still_there}"
    );

    // With the asset gone the very same delete goes through.
    let (status, body) = call(&router, "DELETE", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 202, "DeleteAsset got {status} {body}");
    let (status, deleted) = delete_composite(&router, &model_id, &composite_id, &[]).await;
    assert_eq!(
        status, 202,
        "with no asset left the delete must be accepted, got {status} {deleted}"
    );
    let (status, body) = describe_composite(&router, &model_id, &composite_id).await;
    assert_eq!(
        status, 404,
        "the composite model must be gone after the accepted delete, got {status} {body}"
    );
}

#[tokio::test]
async fn a_surviving_formula_that_reads_a_doomed_property_refuses_the_delete() {
    /// How the referrer's formula variable names what it reads
    /// (`smithy_query(query="shape", shape="VariableValue")`: `propertyId`, or
    /// a `propertyPath` of segments), across the two property types that carry
    /// a formula at all (`transform`, `metric`).
    #[derive(Debug, Clone, Copy)]
    enum Reference {
        /// A `metric` variable naming the property by `propertyId`.
        MetricPropertyId,
        /// A `transform` variable naming it through a `propertyPath`.
        TransformPropertyPath,
        /// A `transform` variable whose `propertyPath` names a composite model
        /// that defines no properties of its own.
        TransformPathToPropertylessComposite,
    }

    for (case, reference) in [
        (
            "a metric variable naming the property by id",
            Reference::MetricPropertyId,
        ),
        (
            "a transform variable naming it through a property path",
            Reference::TransformPropertyPath,
        ),
        (
            "a property path naming a composite model that has no properties",
            Reference::TransformPathToPropertylessComposite,
        ),
    ] {
        let router = router();
        let model_id = plain_model(&router, "Boiler").await;

        let source_body = match reference {
            Reference::TransformPathToPropertylessComposite => json!({
                "assetModelCompositeModelName": "Source",
                "assetModelCompositeModelType": "CUSTOM",
            }),
            _ => json!({
                "assetModelCompositeModelName": "Source",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelProperties": [
                    {"name": "Setpoint", "dataType": "DOUBLE", "type": {"measurement": {}}},
                ],
            }),
        };
        let (status, created) = create_composite(&router, &model_id, source_body).await;
        assert_eq!(status, 202, "case {case:?}: create source got {status} {created}");
        let source_id = created["assetModelCompositeModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: create source minted no id: {created}"))
            .to_string();

        let (status, described) = describe_composite(&router, &model_id, &source_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe source got {status} {described}"
        );
        let source_property_id = described["assetModelCompositeModelProperties"][0]["id"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            source_property_id.is_empty(),
            matches!(reference, Reference::TransformPathToPropertylessComposite),
            "case {case:?}: only the property-less row has no source property, got {described}"
        );

        let referrer_property = match reference {
            Reference::MetricPropertyId => json!({
                "name": "Average",
                "dataType": "DOUBLE",
                "type": {"metric": {
                    "expression": "avg(setpoint)",
                    "variables": [{"name": "setpoint",
                                   "value": {"propertyId": source_property_id}}],
                    "window": {"tumbling": {"interval": "1h"}},
                }},
            }),
            Reference::TransformPropertyPath => json!({
                "name": "Doubled",
                "dataType": "DOUBLE",
                "type": {"transform": {
                    "expression": "setpoint * 2",
                    "variables": [{"name": "setpoint", "value": {"propertyPath": [
                        {"id": source_id, "name": "Source"},
                        {"id": source_property_id, "name": "Setpoint"},
                    ]}}],
                }},
            }),
            Reference::TransformPathToPropertylessComposite => json!({
                "name": "Doubled",
                "dataType": "DOUBLE",
                "type": {"transform": {
                    "expression": "setpoint * 2",
                    "variables": [{"name": "setpoint", "value": {"propertyPath": [
                        {"id": source_id, "name": "Source"},
                    ]}}],
                }},
            }),
        };
        let (status, created) = create_composite(
            &router,
            &model_id,
            json!({
                "assetModelCompositeModelName": "Derived",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelProperties": [referrer_property],
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: create referrer got {status} {created}"
        );
        let referrer_id = created["assetModelCompositeModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: create referrer minted no id: {created}"))
            .to_string();

        // "You can't delete a composite model if a parent asset model exists
        // that contains a property formula expression that depends on" it.
        let (status, refused) = delete_composite(&router, &model_id, &source_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a surviving formula that reads it must refuse the delete, got \
             {status} {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "InvalidRequestException",
            "case {case:?}: expected InvalidRequestException, got {refused}"
        );
        let (status, still_there) = describe_composite(&router, &model_id, &source_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the refused delete must leave it readable, got {status} {still_there}"
        );

        // The referrer itself has no dependent, and once it is gone so is the
        // dependency that refused the first delete.
        let (status, deleted) = delete_composite(&router, &model_id, &referrer_id, &[]).await;
        assert_eq!(
            status, 202,
            "case {case:?}: the referrer has no dependent, got {status} {deleted}"
        );
        let (status, deleted) = delete_composite(&router, &model_id, &source_id, &[]).await;
        assert_eq!(
            status, 202,
            "case {case:?}: with the formula gone the delete must be accepted, got \
             {status} {deleted}"
        );
        let (status, body) = describe_composite(&router, &model_id, &source_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: it must be gone after the accepted delete, got {status} {body}"
        );
    }
}

/// The `assetModelCompositeModelProperties`/`assetModelProperties` definitions
/// honour a caller-supplied `id`, so the rows below can name the property the
/// component model lends before anything mints an id for it.
const LENT_PROPERTY_ID: &str = "11111111-2222-4333-8444-555555555555";

/// A `metric` property definition averaging whatever `variable` reads, under
/// `name` — the smallest formula-carrying property the dependency rule looks at
/// (`smithy_query(query="shape", shape="Metric")`: an expression, its variables
/// and a window).
fn metric_property(name: &str, variable: Value) -> Value {
    json!({
        "name": name,
        "dataType": "DOUBLE",
        "type": {"metric": {
            "expression": "avg(lent)",
            "variables": [{"name": "lent", "value": variable}],
            "window": {"tumbling": {"interval": "1h"}},
        }},
    })
}

/// `POST /asset-models` with the whole request body, for the rows that vary the
/// model's type and its own properties.
async fn create_model_from(router: &MultiServiceRouter, case: &str, body: Value) -> String {
    let (status, created) = call(router, "POST", "/asset-models", Some(body)).await;
    assert_eq!(
        status, 202,
        "case {case:?}: CreateAssetModel got {status} {created}"
    );
    created["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: CreateAssetModel minted no id: {created}"))
        .to_string()
}

#[tokio::test]
async fn a_formula_reading_a_property_a_component_model_lends_refuses_the_delete() {
    /// Where the surviving formula sits, and how it names the property the
    /// deleted composite model contributed.
    #[derive(Debug, Clone, Copy)]
    enum Row {
        /// A `metric` on a SIBLING composite model naming the property by
        /// `propertyId`.
        SiblingMetricById,
        /// A `transform` on a sibling whose `propertyPath` names the property
        /// itself, rather than the composite model that contributes it.
        SiblingTransformByPath,
        /// A `metric` the asset model carries DIRECTLY, outside any composite
        /// model.
        ModelMetricById,
        /// The formula is the COMPONENT model's own, over its own property: the
        /// component model goes on exposing it, so nothing dangles.
        ComponentModelOwnMetric,
        /// A second composite model of the same asset model reuses the same
        /// component model, so the asset model still exposes the property.
        SecondCompositeLendsItToo,
    }

    // Every row: the surviving formula, the status the delete must answer, and
    // — for a refusal — the name of the property it must name.
    for (case, row, expected_status, referrer) in [
        (
            "a sibling composite model's metric naming the lent property by id",
            Row::SiblingMetricById,
            400,
            "Average",
        ),
        (
            "a sibling composite model's transform naming it through a property path",
            Row::SiblingTransformByPath,
            400,
            "Doubled",
        ),
        (
            "the asset model's own metric naming it by id",
            Row::ModelMetricById,
            400,
            "ModelAverage",
        ),
        (
            "the component model's own metric over its own property",
            Row::ComponentModelOwnMetric,
            202,
            "",
        ),
        (
            "a second composite model that lends the same property",
            Row::SecondCompositeLendsItToo,
            202,
            "",
        ),
    ] {
        let router = router();

        // The COMPONENT_MODEL whose properties a component-model-based
        // composite model reuses. One row gives it a formula of its own.
        let mut component_properties = vec![json!({
            "id": LENT_PROPERTY_ID,
            "name": "Setpoint",
            "dataType": "DOUBLE",
            "type": {"measurement": {}},
        })];
        if matches!(row, Row::ComponentModelOwnMetric) {
            component_properties.push(metric_property(
                "PumpAverage",
                json!({"propertyId": LENT_PROPERTY_ID}),
            ));
        }
        let component_id = create_model_from(
            &router,
            case,
            json!({
                "assetModelName": "Pump",
                "assetModelType": "COMPONENT_MODEL",
                "assetModelProperties": component_properties,
            }),
        )
        .await;

        // The asset model starts out with no formula of its own: a variable
        // resolves only inside the model it is written in, and until the
        // component-model-based composite model below lends it, the property the
        // row's formula reads belongs to another model. The row that puts the
        // formula ON THE MODEL adds it once the composite model lends it.
        let model_id = create_model_from(
            &router,
            case,
            json!({
                "assetModelName": "Boiler",
                "assetModelProperties": [],
            }),
        )
        .await;

        // The composite model under test: component-model-based, so it stores
        // no properties of its own and contributes the component model's.
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
        assert_eq!(
            status, 202,
            "case {case:?}: create the component-model-based composite got {status} {created}"
        );
        let source_id = created["assetModelCompositeModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: create minted no id: {created}"))
            .to_string();

        // The premise every row rests on: the composite model REPORTS the
        // property it contributes, under the component model's property id.
        let (status, described) = describe_composite(&router, &model_id, &source_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: describe the source got {status} {described}"
        );
        let contributed: Vec<&str> = described["assetModelCompositeModelProperties"]
            .as_array()
            .map(|list| list.iter().filter_map(|entry| entry["id"].as_str()).collect())
            .unwrap_or_default();
        assert!(
            contributed.contains(&LENT_PROPERTY_ID),
            "case {case:?}: the composite model must contribute {LENT_PROPERTY_ID}, got {described}"
        );

        // The formula the row puts on the asset model ITSELF, added now that the
        // composite model lends the property it reads. The composite model is
        // resubmitted by its id and with no properties of its own, which is what
        // keeps the component model it reuses lending them.
        if matches!(row, Row::ModelMetricById) {
            let (status, updated) = call(
                &router,
                "PUT",
                &format!("/asset-models/{model_id}"),
                Some(json!({
                    "assetModelName": "Boiler",
                    "assetModelProperties": [metric_property(
                        "ModelAverage",
                        json!({"propertyId": LENT_PROPERTY_ID}),
                    )],
                    "assetModelCompositeModels": [{
                        "id": &source_id,
                        "name": "ThePump",
                        "type": "CUSTOM",
                    }],
                })),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: adding the model's own metric got {status} {updated}"
            );
        }

        // The surviving formula, where the row puts it.
        let sibling = match row {
            Row::SiblingMetricById | Row::SecondCompositeLendsItToo => Some(metric_property(
                "Average",
                json!({"propertyId": LENT_PROPERTY_ID}),
            )),
            Row::SiblingTransformByPath => Some(json!({
                "name": "Doubled",
                "dataType": "DOUBLE",
                "type": {"transform": {
                    "expression": "lent * 2",
                    "variables": [{"name": "lent", "value": {"propertyPath": [
                        {"id": LENT_PROPERTY_ID, "name": "Setpoint"},
                    ]}}],
                }},
            })),
            Row::ModelMetricById | Row::ComponentModelOwnMetric => None,
        };
        if let Some(property) = sibling {
            let (status, created) = create_composite(
                &router,
                &model_id,
                json!({
                    "assetModelCompositeModelName": "Derived",
                    "assetModelCompositeModelType": "CUSTOM",
                    "assetModelCompositeModelProperties": [property],
                }),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: create the referrer got {status} {created}"
            );
        }
        // The row that keeps the property exposed by another route.
        let mut second_source = None;
        if matches!(row, Row::SecondCompositeLendsItToo) {
            let (status, created) = create_composite(
                &router,
                &model_id,
                json!({
                    "assetModelCompositeModelName": "TheOtherPump",
                    "assetModelCompositeModelType": "CUSTOM",
                    "composedAssetModelId": &component_id,
                }),
            )
            .await;
            assert_eq!(
                status, 202,
                "case {case:?}: create the second reuse got {status} {created}"
            );
            second_source = created["assetModelCompositeModelId"]
                .as_str()
                .map(str::to_string);
        }

        let (status, answered) = delete_composite(&router, &model_id, &source_id, &[]).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {answered}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&answered),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got {answered}"
            );
            assert_eq!(
                answered["message"].as_str(),
                Some(
                    format!(
                        "Composite model {source_id} is referenced by the formula expression of \
                         property {referrer} of asset model {model_id} when delete was called"
                    )
                    .as_str()
                ),
                "case {case:?}: the refusal must name the referring property, got {answered}"
            );
            let (status, still_there) = describe_composite(&router, &model_id, &source_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the refused delete must leave it readable, got \
                 {status} {still_there}"
            );
        } else {
            let (status, gone) = describe_composite(&router, &model_id, &source_id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: it must be gone after the accepted delete, got {status} {gone}"
            );
            // Whatever lent the property still lends it.
            if let Some(second) = second_source.as_deref() {
                let (status, other) = describe_composite(&router, &model_id, second).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: the second reuse must survive, got {status} {other}"
                );
                let still: Vec<&str> = other["assetModelCompositeModelProperties"]
                    .as_array()
                    .map(|list| list.iter().filter_map(|entry| entry["id"].as_str()).collect())
                    .unwrap_or_default();
                assert!(
                    still.contains(&LENT_PROPERTY_ID),
                    "case {case:?}: the surviving reuse must still contribute \
                     {LENT_PROPERTY_ID}, got {other}"
                );
            }
        }
    }
}

#[tokio::test]
async fn externalid_labels_delete_the_same_composite_model_as_the_raw_ids() {
    let router = router();

    // Both id members are `CustomID`, so each takes either form the pattern
    // admits: the raw UUID, or `externalId:<externalId>`. One row per
    // combination of the two forms across the two members.
    for (case, model_by_label, composite_by_label) in [
        ("both ids raw", false, false),
        ("the asset model by externalId label", true, false),
        ("the composite model by externalId label", false, true),
        ("both by externalId label", true, true),
    ] {
        let model_external = format!("boiler_{}", case.replace(' ', "_"));
        let (status, created) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({
                // A name of its own per row: `assetModelName` is unique per
                // account, and every row of this table runs against one router.
                "assetModelName": format!("Boiler {case}"),
                "assetModelExternalId": model_external,
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: CreateAssetModel got {status} {created}"
        );
        let model_id = created["assetModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {created}"))
            .to_string();

        let composite_external = "custom_one";
        let (status, created) = create_composite(
            &router,
            &model_id,
            json!({
                "assetModelCompositeModelName": "Custom1",
                "assetModelCompositeModelType": "CUSTOM",
                "assetModelCompositeModelExternalId": composite_external,
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: create composite got {status} {created}"
        );
        let composite_id = created["assetModelCompositeModelId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: create minted no id: {created}"))
            .to_string();

        let model_label = if model_by_label {
            format!("externalId:{model_external}")
        } else {
            model_id.clone()
        };
        let composite_label = if composite_by_label {
            format!("externalId:{composite_external}")
        } else {
            composite_id.clone()
        };
        let (status, deleted) = delete_composite(&router, &model_label, &composite_label, &[]).await;
        assert_eq!(
            status, 202,
            "case {case:?}: delete got {status} {deleted}"
        );
        assert_eq!(
            deleted["assetModelId"].as_str(),
            Some(model_id.as_str()),
            "case {case:?}: the delete must report the RESOLVED asset model id, got {deleted}"
        );
        let (status, body) = describe_composite(&router, &model_id, &composite_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the raw id must resolve to nothing afterwards, got {status} {body}"
        );
    }
}
