// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateAssetProperty`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: `UpdateAssetProperty` OVERWRITES the named asset
//! property's alias, notification state and unit from the request — an omitted
//! `propertyAlias` removes the alias, an omitted `propertyNotificationState` sets
//! the state to `DISABLED`, an omitted `propertyUnit` falls back to the unit the
//! asset model's property definition carries — and the alias it writes is the one
//! an alias-addressed property lookup resolves through, for either `CustomID` form
//! of the asset and property ids.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness and the asset/model fixtures this crate's other suite already
// has: one router, one `call`, one `property_catalog`, so both suites talk to the
// service exactly the same way.
use super::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_catalog, property_id, router,
};
use super::support::now_seconds;

/// A `CustomID`-shaped id no create ever minted, for the refusal cases.
const ABSENT_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The MQTT topic IoT SiteWise reports for a property's value-update
/// notifications. Written out here as an INDEPENDENT oracle of the form the IoT
/// SiteWise User Guide documents:
/// `$aws/sitewise/asset-models/{assetModelId}/assets/{assetId}/properties/{propertyId}`.
///
/// `pub(crate)` so the sibling `DescribeAssetProperty` suite reads the SAME
/// oracle rather than growing a second copy of it.
pub(crate) fn expected_topic(model_id: &str, asset_id: &str, property_id: &str) -> String {
    format!("$aws/sitewise/asset-models/{model_id}/assets/{asset_id}/properties/{property_id}")
}

/// The property named `name` in a `DescribeAsset` body.
fn property_by_name(properties: &Value, name: &str) -> Value {
    properties
        .as_array()
        .unwrap_or_else(|| panic!("expected a property list, got {properties}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("no property named {name} in {properties}"))
        .clone()
}

/// `DescribeAsset`'s view of `asset_id`.
async fn describe_asset(router: &MultiServiceRouter, asset_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {asset_id}: got {status} {body}");
    body
}

/// `UpdateAssetProperty` on `(asset_label, property_label)` with `body`.
async fn update_property(
    router: &MultiServiceRouter,
    asset_label: &str,
    property_label: &str,
    body: Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/assets/{asset_label}/properties/{property_label}"),
        Some(body),
    )
    .await
}

/// One row of the overwrite table: which property to address, the request to
/// make, and what that property must report once it has been made.
struct OverwriteCase {
    case: &'static str,
    property: &'static str,
    request: Value,
    alias: Option<&'static str>,
    notification_state: &'static str,
    unit: Option<&'static str>,
}

/// A model from the shared property catalog plus an asset of it, as
/// `(model_id, asset_id)`.
async fn model_and_asset(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let model_id = create_model(router, name, property_catalog()).await;
    let asset_id = create_asset(router, &format!("{name}-1"), &model_id).await;
    (model_id, asset_id)
}

#[tokio::test]
async fn update_asset_property_overwrites_alias_notification_state_and_unit() {
    let router = router();
    let (model_id, asset_id) = model_and_asset(&router, "OverwriteBoiler").await;
    let asset = describe_asset(&router, &asset_id).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let status_property = property_id(&asset["assetProperties"], "Status");

    // The rows differ along every member the rule generalises over: the alias
    // (set, kept, rebound, omitted), the notification state (ENABLED, DISABLED,
    // omitted) and the unit (set, omitted), over a property whose model
    // definition carries a unit ("Temperature" -> Celsius) and one whose does not
    // ("Status"). Applied in order against the same asset, so each row also
    // asserts the OVERWRITE: what the previous row wrote is gone unless this one
    // says otherwise.
    let rows = [
        OverwriteCase {
            case: "alias + ENABLED + unit",
            property: "Temperature",
            request: json!({
                "propertyAlias": "/plant/1/boiler/temperature",
                "propertyNotificationState": "ENABLED",
                "propertyUnit": "Kelvin",
            }),
            alias: Some("/plant/1/boiler/temperature"),
            notification_state: "ENABLED",
            unit: Some("Kelvin"),
        },
        OverwriteCase {
            case: "same alias restated, DISABLED, unit omitted",
            property: "Temperature",
            request: json!({
                "propertyAlias": "/plant/1/boiler/temperature",
                "propertyNotificationState": "DISABLED",
            }),
            alias: Some("/plant/1/boiler/temperature"),
            notification_state: "DISABLED",
            // Omitted -> the asset model property's unit, not the Kelvin above.
            unit: Some("Celsius"),
        },
        OverwriteCase {
            case: "alias rebound to another stream",
            property: "Temperature",
            request: json!({
                "propertyAlias": "/plant/1/boiler/temperature-2",
                "propertyNotificationState": "ENABLED",
            }),
            alias: Some("/plant/1/boiler/temperature-2"),
            notification_state: "ENABLED",
            unit: Some("Celsius"),
        },
        OverwriteCase {
            case: "every optional member omitted",
            property: "Temperature",
            request: json!({}),
            alias: None,
            notification_state: "DISABLED",
            unit: Some("Celsius"),
        },
        OverwriteCase {
            case: "unit only",
            property: "Temperature",
            request: json!({"propertyUnit": "Fahrenheit"}),
            alias: None,
            notification_state: "DISABLED",
            unit: Some("Fahrenheit"),
        },
        OverwriteCase {
            case: "model property without a unit, nothing set",
            property: "Status",
            request: json!({}),
            alias: None,
            notification_state: "DISABLED",
            unit: None,
        },
        OverwriteCase {
            case: "model property without a unit, alias and unit set",
            property: "Status",
            request: json!({
                "propertyAlias": "/plant/1/boiler/status",
                "propertyUnit": "Code",
            }),
            alias: Some("/plant/1/boiler/status"),
            notification_state: "DISABLED",
            unit: Some("Code"),
        },
    ];

    for row in rows {
        let OverwriteCase {
            case,
            property: name,
            request,
            alias,
            notification_state,
            unit,
        } = row;
        let id = if name == "Temperature" {
            temperature.clone()
        } else {
            status_property.clone()
        };
        let (status, answer) = update_property(&router, &asset_id, &id, request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAssetProperty got {status} {answer}"
        );

        let asset = describe_asset(&router, &asset_id).await;
        let property = property_by_name(&asset["assetProperties"], name);
        assert_eq!(
            property.get("alias").and_then(Value::as_str),
            alias,
            "case {case:?}: expected alias {alias:?}, got {property}"
        );
        assert_eq!(
            property["notification"]["state"].as_str(),
            Some(notification_state),
            "case {case:?}: expected notification state {notification_state:?}, got {property}"
        );
        assert_eq!(
            property["notification"]["topic"].as_str(),
            Some(expected_topic(&model_id, &asset_id, &id).as_str()),
            "case {case:?}: unexpected notification topic in {property}"
        );
        assert_eq!(
            property.get("unit").and_then(Value::as_str),
            unit,
            "case {case:?}: expected unit {unit:?}, got {property}"
        );

        // An alias the row wrote addresses that property; one it removed resolves
        // to nothing. `GetAssetPropertyValue` is the alias-addressed reader.
        if let Some(alias) = alias {
            let (status, body) = call(
                &router,
                "GET",
                &format!("/properties/latest?propertyAlias={alias}"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the alias {alias} must address the property, got {status} {body}"
            );
        }
    }

    // A property no row named kept exactly what `CreateAsset` materialised from
    // the model: this operation writes only the property it is addressed to.
    let asset = describe_asset(&router, &asset_id).await;
    let untouched = property_by_name(&asset["assetProperties"], "Cycles");
    assert_eq!(
        untouched.get("unit").and_then(Value::as_str),
        Some("Count"),
        "an unaddressed property must keep its model unit, got {untouched}"
    );
    assert!(
        untouched.get("alias").is_none_or(Value::is_null),
        "an unaddressed property must carry no alias, got {untouched}"
    );
    // The notification block is reported for EVERY asset property, so an
    // unaddressed one carries the derived topic in the default DISABLED state
    // rather than nothing.
    assert_eq!(
        untouched["notification"]["state"].as_str(),
        Some("DISABLED"),
        "an unaddressed property's notification state is the documented default, got \
         {untouched}"
    );
    assert_eq!(
        untouched["notification"]["topic"].as_str(),
        Some(
            expected_topic(
                &model_id,
                &asset_id,
                untouched["id"].as_str().unwrap_or_default()
            )
            .as_str()
        ),
        "an unaddressed property must still report the topic its updates publish to, got \
         {untouched}"
    );

    // The alias each earlier row replaced stopped addressing anything.
    let dropped = [
        "/plant/1/boiler/temperature",
        "/plant/1/boiler/temperature-2",
    ];
    for alias in dropped {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/properties/latest?propertyAlias={alias}"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {alias:?}: a replaced alias must address nothing, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {alias:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn an_alias_bound_by_update_asset_property_carries_ingest_and_retrieval() {
    let router = router();
    let (_, asset_id) = model_and_asset(&router, "AliasBoiler").await;
    let asset = describe_asset(&router, &asset_id).await;
    let temperature = property_id(&asset["assetProperties"], "Temperature");

    let alias = "/company/windfarm/3/turbine/7/temperature";
    let (status, answer) = update_property(
        &router,
        &asset_id,
        &temperature,
        json!({"propertyAlias": alias, "propertyNotificationState": "ENABLED"}),
    )
    .await;
    assert_eq!(status, 200, "UpdateAssetProperty: got {status} {answer}");

    // Ingest addressed by the alias alone lands on the property the alias was
    // bound to, so the ids-addressed reader reports it.
    let (status, put) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": "alias-write",
            "propertyAlias": alias,
            "propertyValues": [{
                "value": {"doubleValue": 42.5},
                // Anchored a minute in the past: BatchPutAssetPropertyValue ingests
                // only TQVs inside `[now - 7 days, now + 10 minutes]`.
                "timestamp": {"timeInSeconds": now_seconds() - 60},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "BatchPutAssetPropertyValue: got {status} {put}");
    assert_eq!(
        put["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "BatchPutAssetPropertyValue by alias must accept the entry, got {put}"
    );

    for (case, uri) in [
        (
            "by ids",
            format!("/properties/latest?assetId={asset_id}&propertyId={temperature}"),
        ),
        ("by alias", format!("/properties/latest?propertyAlias={alias}")),
    ] {
        let (status, latest) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {latest}");
        assert_eq!(
            latest["propertyValue"]["value"]["doubleValue"].as_f64(),
            Some(42.5),
            "case {case:?}: expected the ingested value, got {latest}"
        );
    }

    // Omitting `propertyAlias` removes the binding: the alias addresses nothing,
    // while the samples written through it stay on the property.
    let (status, answer) = update_property(&router, &asset_id, &temperature, json!({})).await;
    assert_eq!(status, 200, "UpdateAssetProperty: got {status} {answer}");
    let (status, body) = call(
        &router,
        "GET",
        &format!("/properties/latest?propertyAlias={alias}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "a removed alias must address nothing, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "unexpected error code in {body}"
    );
    let (status, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={temperature}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "read back by ids: got {status} {latest}");
    assert_eq!(
        latest["propertyValue"]["value"]["doubleValue"].as_f64(),
        Some(42.5),
        "removing an alias must not drop the property's values, got {latest}"
    );
}

#[tokio::test]
async fn an_alias_another_property_already_carries_is_refused() {
    let router = router();
    let (_, asset_id) = model_and_asset(&router, "ConflictBoiler").await;
    let second_asset = {
        let asset = describe_asset(&router, &asset_id).await;
        let model_id = asset["assetModelId"].as_str().expect("assetModelId");
        create_asset(&router, "ConflictBoiler-2", model_id).await
    };
    let first = describe_asset(&router, &asset_id).await;
    let temperature = property_id(&first["assetProperties"], "Temperature");
    let cycles = property_id(&first["assetProperties"], "Cycles");

    let alias = "/plant/2/shared/stream";
    let (status, answer) = update_property(
        &router,
        &asset_id,
        &temperature,
        json!({"propertyAlias": alias}),
    )
    .await;
    assert_eq!(status, 200, "first binding: got {status} {answer}");

    // One alias names one data stream, so a second property cannot take it —
    // neither another property of the same asset nor the same property of
    // another asset.
    let contenders = [
        ("another property of the same asset", asset_id.as_str(), cycles.as_str()),
        ("the same property of another asset", second_asset.as_str(), temperature.as_str()),
    ];
    for (case, contender_asset, contender_property) in contenders {
        let (status, body) = update_property(
            &router,
            contender_asset,
            contender_property,
            json!({"propertyAlias": alias}),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictingOperationException",
            "case {case:?}: unexpected error code in {body}"
        );

        // The refusal wrote nothing: the contender has no alias and the first
        // binding still stands.
        let contender = property_by_name(
            &describe_asset(&router, contender_asset).await["assetProperties"],
            if contender_property == cycles { "Cycles" } else { "Temperature" },
        );
        if contender_asset != asset_id || contender_property != temperature {
            assert!(
                contender.get("alias").is_none_or(Value::is_null),
                "case {case:?}: a refused update must leave the property unaliased, got {contender}"
            );
        }
    }
    let holder = property_by_name(
        &describe_asset(&router, &asset_id).await["assetProperties"],
        "Temperature",
    );
    assert_eq!(
        holder.get("alias").and_then(Value::as_str),
        Some(alias),
        "the first binding must survive the refusals, got {holder}"
    );

    // Restating the alias on the property that already carries it is the
    // documented way to keep it, so it is NOT a conflict.
    let (status, answer) = update_property(
        &router,
        &asset_id,
        &temperature,
        json!({"propertyAlias": alias, "propertyNotificationState": "ENABLED"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "restating a property's own alias: got {status} {answer}"
    );
}

#[tokio::test]
async fn update_asset_property_accepts_both_custom_id_forms() {
    let router = router();
    // A model and an asset that carry external ids, plus a property definition
    // with one, so each `CustomID` label form has something to resolve to.
    let model_id = create_model(
        &router,
        "ExternalIdBoiler",
        json!([
            {"name": "Flow", "dataType": "DOUBLE", "unit": "m3/s",
             "externalId": "flow-definition", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let (status, created) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "ExternalIdBoiler-1",
            "assetModelId": model_id,
            "assetExternalId": "boiler-external-1",
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created}");
    let asset_id = created["assetId"].as_str().expect("assetId").to_string();
    let flow = property_id(
        &describe_asset(&router, &asset_id).await["assetProperties"],
        "Flow",
    );

    // Each row names the same property through a different pair of label forms.
    let rows = [
        (
            "raw asset id, raw property id",
            asset_id.clone(),
            flow.clone(),
            "/plant/3/flow/raw",
        ),
        (
            "external asset id, raw property id",
            "externalId:boiler-external-1".to_string(),
            flow.clone(),
            "/plant/3/flow/external-asset",
        ),
        (
            "raw asset id, external property id",
            asset_id.clone(),
            "externalId:flow-definition".to_string(),
            "/plant/3/flow/external-property",
        ),
        (
            "both external",
            "externalId:boiler-external-1".to_string(),
            "externalId:flow-definition".to_string(),
            "/plant/3/flow/both-external",
        ),
    ];
    for (case, asset_label, property_label, alias) in rows {
        let (status, answer) = update_property(
            &router,
            &asset_label,
            &property_label,
            json!({"propertyAlias": alias, "propertyUnit": "l/s"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateAssetProperty got {status} {answer}"
        );

        // Read back through the raw ids, so the label form only decided WHICH
        // property was written, never where.
        let property = property_by_name(
            &describe_asset(&router, &asset_id).await["assetProperties"],
            "Flow",
        );
        assert_eq!(
            property.get("alias").and_then(Value::as_str),
            Some(alias),
            "case {case:?}: expected alias {alias:?}, got {property}"
        );
        assert_eq!(
            property.get("unit").and_then(Value::as_str),
            Some("l/s"),
            "case {case:?}: expected unit l/s, got {property}"
        );
        assert_eq!(
            property["id"].as_str(),
            Some(flow.as_str()),
            "case {case:?}: a label form must not change the property id, got {property}"
        );
    }
}

#[tokio::test]
async fn a_label_that_names_no_stored_property_is_a_modelled_refusal() {
    let router = router();
    let (_, asset_id) = model_and_asset(&router, "RefusalBoiler").await;
    let temperature = property_id(
        &describe_asset(&router, &asset_id).await["assetProperties"],
        "Temperature",
    );

    let rows = [
        (
            "unknown asset id",
            format!("/assets/{ABSENT_ID}/properties/{temperature}"),
        ),
        (
            "unknown property id",
            format!("/assets/{asset_id}/properties/{ABSENT_ID}"),
        ),
        (
            "unknown asset external id",
            format!("/assets/externalId:no-such-asset/properties/{temperature}"),
        ),
        (
            "unknown property external id",
            format!("/assets/{asset_id}/properties/externalId:no-such-property"),
        ),
    ];
    for (case, uri) in rows {
        let (status, body) = call(&router, "PUT", &uri, Some(json!({}))).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A notification state outside the modelled enum is refused before any write,
    // and the property stays as it was.
    let (status, body) = update_property(
        &router,
        &asset_id,
        &temperature,
        json!({"propertyNotificationState": "SOMETIMES"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "an unmodelled notification state: expected 400, got {status} {body}"
    );
    let property = property_by_name(
        &describe_asset(&router, &asset_id).await["assetProperties"],
        "Temperature",
    );
    assert_eq!(
        property["notification"]["state"].as_str(),
        Some("DISABLED"),
        "a refused update must write nothing, so the state stays the default, got {property}"
    );
}
