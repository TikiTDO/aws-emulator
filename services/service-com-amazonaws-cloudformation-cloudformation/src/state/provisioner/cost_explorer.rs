// SPDX-License-Identifier: CC0-1.0

//! The per-type provisioning arms of AWS Cost Explorer: `AWS::CE::AnomalyMonitor` and
//! `AWS::CE::AnomalySubscription`. Dispatched to by `Provisioner`.

use super::*;

pub(super) const COST_EXPLORER_SIGNING_NAME: &str = "ce";
pub(super) const COST_EXPLORER_TARGET_PREFIX: &str = "AWSInsightsIndexService";

const CE_UNKNOWN_MONITOR: &str = "UnknownMonitorException";
const CE_UNKNOWN_SUBSCRIPTION: &str = "UnknownSubscriptionException";
const CE_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

// ---------------------------------------------------------------------------
// AWS::CE::AnomalyMonitor
// ---------------------------------------------------------------------------

/// Create the cost anomaly monitor an `AWS::CE::AnomalyMonitor` declares.
pub(super) async fn create_anomaly_monitor(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut monitor = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    if !monitor.contains_key("MonitorName") {
        monitor.insert(
            "MonitorName".to_string(),
            Value::String(resource.physical_resource_id.clone()),
        );
    }
    let tags = tags_of(&resource.properties, propagated);
    let mut request = Map::new();
    request.insert("AnomalyMonitor".to_string(), Value::Object(monitor));
    if !tags.is_empty() {
        request.insert("ResourceTags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }

    let answer = call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.CreateAnomalyMonitor"),
        &Value::Object(request),
    )
    .await?;

    let physical_id = answer
        .get("MonitorArn")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(Created {
        physical_resource_id: physical_id,
        attributes: BTreeMap::new(),
    })
}

/// Update a cost anomaly monitor (`UpdateAnomalyMonitor`).
pub(super) async fn update_anomaly_monitor(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut request = Map::new();
    request.insert(
        "MonitorArn".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    if let Some(name) = declared.get("MonitorName").and_then(Value::as_str) {
        request.insert("MonitorName".to_string(), Value::String(name.to_string()));
    }
    call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.UpdateAnomalyMonitor"),
        &Value::Object(request),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(resource.physical_resource_id.clone()),
        attributes: BTreeMap::new(),
    })
}

/// Delete a cost anomaly monitor (`DeleteAnomalyMonitor`).
pub(super) async fn delete_anomaly_monitor(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "MonitorArn": physical_resource_id,
    });
    match call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.DeleteAnomalyMonitor"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal)
            if refusal.code == CE_UNKNOWN_MONITOR || refusal.code == CE_RESOURCE_NOT_FOUND =>
        {
            Ok(())
        }
        Err(refusal) => Err(refusal),
    }
}

/// Read back a cost anomaly monitor configuration for drift detection.
pub(super) async fn describe_anomaly_monitor(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "MonitorArnList": [physical_resource_id],
    });
    let answer = call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.GetAnomalyMonitors"),
        &request,
    )
    .await?;

    Ok(answer
        .get("AnomalyMonitors")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter().find(|m| {
                m.get("MonitorArn").and_then(Value::as_str) == Some(physical_resource_id)
            })
        })
        .cloned())
}

// ---------------------------------------------------------------------------
// AWS::CE::AnomalySubscription
// ---------------------------------------------------------------------------

/// Create the cost anomaly subscription an `AWS::CE::AnomalySubscription` declares.
pub(super) async fn create_anomaly_subscription(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let mut subscription = match &resource.properties {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    if !subscription.contains_key("SubscriptionName") {
        subscription.insert(
            "SubscriptionName".to_string(),
            Value::String(resource.physical_resource_id.clone()),
        );
    }
    let tags = tags_of(&resource.properties, propagated);
    let mut request = Map::new();
    request.insert("AnomalySubscription".to_string(), Value::Object(subscription));
    if !tags.is_empty() {
        request.insert("ResourceTags".to_string(), tags_as_list(&tags, UPPER_CASED_TAG));
    }

    let answer = call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.CreateAnomalySubscription"),
        &Value::Object(request),
    )
    .await?;

    let physical_id = answer
        .get("SubscriptionArn")
        .and_then(Value::as_str)
        .map(ToString::to_string);

    Ok(Created {
        physical_resource_id: physical_id,
        attributes: BTreeMap::new(),
    })
}

/// Update a cost anomaly subscription (`UpdateAnomalySubscription`).
pub(super) async fn update_anomaly_subscription(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let mut request = match declared {
        Value::Object(object) => object.clone(),
        _ => Map::new(),
    };
    request.insert(
        "SubscriptionArn".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.UpdateAnomalySubscription"),
        &Value::Object(request),
    )
    .await?;
    Ok(Created {
        physical_resource_id: Some(resource.physical_resource_id.clone()),
        attributes: BTreeMap::new(),
    })
}

/// Delete a cost anomaly subscription (`DeleteAnomalySubscription`).
pub(super) async fn delete_anomaly_subscription(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({
        "SubscriptionArn": physical_resource_id,
    });
    match call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.DeleteAnomalySubscription"),
        &request,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(refusal)
            if refusal.code == CE_UNKNOWN_SUBSCRIPTION || refusal.code == CE_RESOURCE_NOT_FOUND =>
        {
            Ok(())
        }
        Err(refusal) => Err(refusal),
    }
}

/// Read back a cost anomaly subscription configuration for drift detection.
pub(super) async fn describe_anomaly_subscription(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    physical_resource_id: &str,
) -> Result<Option<Value>, Refusal> {
    let request = serde_json::json!({
        "SubscriptionArnList": [physical_resource_id],
    });
    let answer = call_json_rpc(
        ctx,
        COST_EXPLORER_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{COST_EXPLORER_TARGET_PREFIX}.GetAnomalySubscriptions"),
        &request,
    )
    .await?;

    Ok(answer
        .get("AnomalySubscriptions")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter().find(|s| {
                s.get("SubscriptionArn").and_then(Value::as_str) == Some(physical_resource_id)
            })
        })
        .cloned())
}
