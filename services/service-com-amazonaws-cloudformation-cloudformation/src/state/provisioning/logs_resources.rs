// SPDX-License-Identifier: CC0-1.0

//! `AWS::Logs::LogGroup`, `AWS::Logs::LogStream`, `AWS::Logs::MetricFilter` and
//! `AWS::Logs::SubscriptionFilter`: the log group a stack declares, the stream inside it and
//! the metric and subscription filters on it, created in Amazon CloudWatch Logs itself.
//!
//! This is the per-type half of provisioning for four resource types, the half
//! [`super::super::provisioner`] holds for every other type this emulator creates; it lives
//! in its own module beside [`super::cognito_identity_pool`] for the reason that module
//! records — the catalogue file is far past the length a change to this repository may
//! touch — and is reached through the same seam ([`super::Owner`]).
//!
//! ## Why the resources are really created
//!
//! A log group is a handle every other API of its owning service takes: `PutMetricFilter`,
//! `PutRetentionPolicy`, `CreateLogStream`, `PutLogEvents`, `DescribeLogStreams` and
//! `TagResource` each address a group by the NAME CloudFormation decided for it, and each
//! answers `ResourceNotFoundException` for a name CloudWatch Logs holds nothing under. So a
//! stack that reported a log group created while nothing created it publishes a name that
//! resolves to nothing: the metric filter the next template declares on it cannot be put,
//! and the stream it declares inside it cannot be made. The same holds one level down — a
//! stream exists only inside the group that holds it, and a filter only on the group it
//! filters — which is why all four types are provisioned here rather than left
//! declared-only. A SUBSCRIPTION filter is the case where the difference is a delivery: the
//! filter is what forwards the group's matching events to its destination, so a stack that
//! reported one created while nothing put it leaves a group with no filter to reconcile
//! against and a destination nothing ever reaches.
//!
//! ## The properties
//!
//! `AWS::Logs::LogGroup`'s properties are three members of `CreateLogGroup` under the same
//! names up to case (`KmsKeyId`, `LogGroupClass`, `Tags`, beside the `LogGroupName` the
//! group is created under) plus three settings that owning service applies through a CALL OF
//! ITS OWN on the group that now exists, each of which is made here so the group ends up
//! configured as the declaration asks:
//!
//! * `RetentionInDays` is `PutRetentionPolicy` ("Sets the retention of the specified log
//!   group");
//! * `DataProtectionPolicy` is `PutDataProtectionPolicy`;
//! * `FieldIndexPolicies` is `PutIndexPolicy`, one call per policy the list carries.
//!
//! `AWS::Logs::LogStream`'s two properties are `CreateLogStream`'s own members, and
//! `AWS::Logs::MetricFilter`'s are `PutMetricFilter`'s — that one call being documented
//! "Creates or updates a metric filter", so it is this type's update as well as its create.
//! The one property spelled differently on the two sides is a transformation's `Dimensions`,
//! a `[{Key, Value}]` list in the template against the MAP the API's own
//! `MetricTransformation$dimensions` takes ([`metric_transformations`]).
//!
//! `AWS::Logs::SubscriptionFilter`'s properties are `PutSubscriptionFilter`'s own members
//! under the same names up to case — that call being "Creates or updates a subscription
//! filter", so it serves this type's create and its in-place update alike — and the filter
//! it configures is what forwards the group's matching events to the destination the
//! declaration names.
//!
//! A declared property none of those calls carries is REFUSED rather than dropped
//! ([`refuse_unapplied_properties`]): a resource reported created with less than its
//! declaration asked for is the shallow success these arms exist to avoid.
//!
//! ## A child moved to another group
//!
//! A stream, a metric filter and a subscription filter each exist INSIDE the log group their
//! `LogGroupName` names, and no call of that service moves one to another group: the
//! property is one of the type's "Update requires: Replacement" ones, so a declaration
//! naming a different group is applied by REPLACING the resource — the child is made in the
//! group now declared and the one it replaced is taken out of the group it was in
//! ([`replace_in_another_group`]) — rather than reported as a change nothing applied.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    applicable_tags, call_json_rpc, Created, Readiness, Refusal, Removal, AWS_JSON_1_1,
};
use crate::state::provisioner::tags_of;
use crate::state::StoredResource;

/// The resource types this module provisions.
pub(crate) const LOGS_LOG_GROUP: &str = "AWS::Logs::LogGroup";
pub(crate) const LOGS_LOG_STREAM: &str = "AWS::Logs::LogStream";
pub(crate) const LOGS_METRIC_FILTER: &str = "AWS::Logs::MetricFilter";
pub(crate) const LOGS_SUBSCRIPTION_FILTER: &str = "AWS::Logs::SubscriptionFilter";

/// The SigV4 signing name of the service that owns them, which is what routes the in-process
/// call to Amazon CloudWatch Logs's own handlers (that model's `sigv4Name`/`endpointPrefix`,
/// `smithy_query(query="service_info")`).
const LOGS_SIGNING_NAME: &str = "logs";

/// The service shape an awsJson `X-Amz-Target` names the operation under (that model's
/// service shape `Logs_20140328`). It speaks `awsJson1_1`.
const LOGS_TARGET_PREFIX: &str = "Logs_20140328";

/// The error code CloudWatch Logs answers for a group, stream or filter it does not hold
/// (`ResourceNotFoundException`, the modelled refusal of its reads, writes and deletes),
/// which is what says a delete has nothing left to remove and a read has nothing to report.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The code a refusal of this module's own carries for a declaration it cannot act on: the
/// same one the catalogue's arms use for a property this emulator does not apply.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The code a refusal carries for a declaration that states none of a required property.
const MISSING_PROPERTY: &str = "InvalidRequest";

/// Which of the three types one resource is.
///
/// Visible to the whole of this service's state module, because it travels with
/// [`super::Owner`]: the algorithm resolves a type's owner once and passes it back here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::state) enum Kind {
    /// `AWS::Logs::LogGroup`.
    LogGroup,
    /// `AWS::Logs::LogStream`.
    LogStream,
    /// `AWS::Logs::MetricFilter`.
    MetricFilter,
    /// `AWS::Logs::SubscriptionFilter`.
    SubscriptionFilter,
}

/// The kind `resource_type` names, or `None` for a type this module does not own.
pub(super) fn kind_of(resource_type: &str) -> Option<Kind> {
    match resource_type {
        LOGS_LOG_GROUP => Some(Kind::LogGroup),
        LOGS_LOG_STREAM => Some(Kind::LogStream),
        LOGS_METRIC_FILTER => Some(Kind::MetricFilter),
        LOGS_SUBSCRIPTION_FILTER => Some(Kind::SubscriptionFilter),
        _ => None,
    }
}

/// The property naming the group a stream belongs to and a filter is put on, which is also
/// the group's own name property.
const LOG_GROUP_NAME: &str = "LogGroupName";

/// The properties `AWS::Logs::LogGroup` publishes. Every one is applied: the three
/// `CreateLogGroup` carries, and the three the owning service applies with a call of its own
/// ([`group_settings`]).
const LOG_GROUP_PROPERTIES: &[&str] = &[
    "DataProtectionPolicy",
    "FieldIndexPolicies",
    "KmsKeyId",
    "LogGroupClass",
    "LogGroupName",
    "RetentionInDays",
    "Tags",
];

/// Every property `AWS::Logs::LogStream` publishes: the group that holds the stream, and the
/// stream's own name.
const LOG_STREAM_PROPERTIES: &[&str] = &["LogGroupName", "LogStreamName"];

/// Every property `AWS::Logs::MetricFilter` publishes, each a member of `PutMetricFilter`
/// under the same name up to case.
const METRIC_FILTER_PROPERTIES: &[&str] = &[
    "ApplyOnTransformedLogs",
    "FilterName",
    "FilterPattern",
    "LogGroupName",
    "MetricTransformations",
];

/// Every property `AWS::Logs::SubscriptionFilter` publishes, each a member of
/// `PutSubscriptionFilter` under the same name up to case
/// (`smithy_query(query="operation",operation="PutSubscriptionFilter")`).
const SUBSCRIPTION_FILTER_PROPERTIES: &[&str] = &[
    "ApplyOnTransformedLogs",
    "DestinationArn",
    "Distribution",
    "FilterName",
    "FilterPattern",
    "LogGroupName",
    "RoleArn",
];

/// Create the resource one of these types declares, in Amazon CloudWatch Logs itself — so a
/// client that deployed the stack can pass `!Ref <group>` straight to `PutMetricFilter` or
/// `PutLogEvents` and read the stream and the filter back with that service's own reads.
pub(super) async fn create(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    match kind {
        Kind::LogGroup => create_log_group(ctx, region, resource, propagated).await,
        // Neither a stream nor a metric filter is taggable: CloudWatch Logs tags a log
        // group (and a destination), so there is no call that could carry a tag to either,
        // and CloudFormation propagates none to a resource type that is not taggable.
        Kind::LogStream => create_log_stream(ctx, region, resource, &resource.properties).await,
        Kind::MetricFilter => put_metric_filter(ctx, region, resource, &resource.properties).await,
        // Nor is a subscription filter, for the same reason.
        Kind::SubscriptionFilter => {
            put_subscription_filter(ctx, region, resource, &resource.properties).await
        }
    }
}

/// Create the group an `AWS::Logs::LogGroup` declares, then put the settings its owning
/// service applies with a call of its own on the group that now exists.
///
/// A setting that service REFUSES fails the resource, and the group created on the way there
/// is deleted again: a log group reported created without the retention, the data-protection
/// policy or the field indexes its declaration asked for is configured differently from the
/// template, which is exactly what this arm exists to avoid.
async fn create_log_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        &resource.properties,
        &resource.resource_type,
        LOG_GROUP_PROPERTIES,
    )?;
    let name = resource.physical_resource_id.clone();
    let mut request = Map::new();
    request.insert("logGroupName".to_string(), Value::String(name.clone()));
    for (property, member) in [("KmsKeyId", "kmsKeyId"), ("LogGroupClass", "logGroupClass")] {
        if let Some(value) = declared_text(&resource.properties, property) {
            request.insert(member.to_string(), Value::String(value));
        }
    }
    // The group carries the tags its declaration asks for plus the ones CloudFormation gives
    // the resource, as `CreateLogGroup`'s own `tags` map — less the keys in AWS's reserved
    // namespace, which this service documents as its own ("Do not use the `aws:` prefix in
    // your tag names or values because it is reserved for Amazon Web Services use") and
    // refuses on every tagging call a caller makes.
    let tags = applicable_tags(&tags_of(&resource.properties, propagated));
    if !tags.is_empty() {
        request.insert(
            "tags".to_string(),
            Value::Object(
                tags.into_iter()
                    .map(|(key, value)| (key, Value::String(value)))
                    .collect(),
            ),
        );
    }
    call_logs(ctx, region, "CreateLogGroup", &Value::Object(request)).await?;
    if let Err(refusal) = group_settings(ctx, region, &name, &resource.properties).await {
        // Nothing half-configured is left behind for the rollback to find: the group this
        // call created is taken away again before the resource is reported failed.
        let _ = delete_log_group(ctx, region, &name).await;
        return Err(refusal);
    }
    Ok(Created::attributes(BTreeMap::new()))
}

/// Put the declared settings CloudWatch Logs applies through a call of its own on the group
/// `name`: its retention, its data-protection policy and its field-index policies.
///
/// A declaration that states none of them calls nothing. Each call addresses the group by the
/// name it was created under, so what is configured is the group this deployment made.
async fn group_settings(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    properties: &Value,
) -> Result<(), Refusal> {
    if let Some(days) = properties.get("RetentionInDays") {
        let days = retention_days(days).ok_or_else(|| Refusal {
            code: MISSING_PROPERTY.to_string(),
            message: format!(
                "the RetentionInDays property of {LOGS_LOG_GROUP} is a number of days, and \
                 {days} is not one"
            ),
        })?;
        let request = serde_json::json!({
            "logGroupName": name,
            "retentionInDays": days,
        });
        call_logs(ctx, region, "PutRetentionPolicy", &request).await?;
    }
    if let Some(policy) = properties.get("DataProtectionPolicy") {
        let request = serde_json::json!({
            "logGroupIdentifier": name,
            "policyDocument": policy_document(policy),
        });
        call_logs(ctx, region, "PutDataProtectionPolicy", &request).await?;
    }
    for policy in field_index_policies(properties) {
        let request = serde_json::json!({
            "logGroupIdentifier": name,
            "policyDocument": policy_document(&policy),
        });
        call_logs(ctx, region, "PutIndexPolicy", &request).await?;
    }
    Ok(())
}

/// The `FieldIndexPolicies` a declaration states, as the documents `PutIndexPolicy` is called
/// with one by one. The property is a LIST of policy documents, so a declaration that states
/// a single document rather than a list is read as the one policy it is.
fn field_index_policies(properties: &Value) -> Vec<Value> {
    match properties.get("FieldIndexPolicies") {
        Some(Value::Array(policies)) => policies.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(policy) => vec![policy.clone()],
    }
}

/// One policy property as the TEXT its owning call takes: both `PutDataProtectionPolicy` and
/// `PutIndexPolicy` take their `policyDocument` as a string, while a template may write the
/// policy as the JSON object it is or as the text of one.
fn policy_document(policy: &Value) -> String {
    match policy {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// The `RetentionInDays` a declaration states, as the number `PutRetentionPolicy` takes: a
/// template may write it as a JSON number or as the text a parameter reference resolves to.
fn retention_days(value: &Value) -> Option<i64> {
    match value {
        Value::Number(number) => number.as_i64(),
        Value::String(text) => text.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// Create the stream an `AWS::Logs::LogStream` declares, inside the group `declared` names.
///
/// `declared` is the document in force, which on the create of a REPLACEMENT is the new
/// declaration rather than the one the record carries.
async fn create_log_stream(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(declared, &resource.resource_type, LOG_STREAM_PROPERTIES)?;
    let request = serde_json::json!({
        "logGroupName": declared_log_group(declared).ok_or_else(|| missing(resource, LOG_GROUP_NAME))?,
        "logStreamName": resource.physical_resource_id.clone(),
    });
    call_logs(ctx, region, "CreateLogStream", &request).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Put the filter an `AWS::Logs::SubscriptionFilter` declares on the group its declaration
/// names, so the group's matching events are delivered to the destination it names.
///
/// `PutSubscriptionFilter` "creates or updates a subscription filter", so this one call
/// serves the type's create and its in-place update alike — `declared` is the document in
/// force, which on an update is the new declaration rather than the one the record carries.
/// Every member the call takes is a property of the type under the same name up to case, so
/// each declared one travels and the owning service's own validation judges the filter it
/// would judge for a client's own call.
async fn put_subscription_filter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(
        declared,
        &resource.resource_type,
        SUBSCRIPTION_FILTER_PROPERTIES,
    )?;
    let mut request = Map::new();
    request.insert(
        "logGroupName".to_string(),
        Value::String(declared_log_group(declared).ok_or_else(|| missing(resource, LOG_GROUP_NAME))?),
    );
    request.insert(
        "filterName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    // `filterPattern` is required by the call and by the type, and "" is a pattern — the one
    // that matches every log event — so a declaration that states none is sent as it.
    request.insert(
        "filterPattern".to_string(),
        Value::String(declared_text(declared, "FilterPattern").unwrap_or_default()),
    );
    request.insert(
        "destinationArn".to_string(),
        Value::String(
            declared_text(declared, "DestinationArn")
                .ok_or_else(|| missing(resource, "DestinationArn"))?,
        ),
    );
    for (property, member) in [("RoleArn", "roleArn"), ("Distribution", "distribution")] {
        if let Some(value) = declared_text(declared, property) {
            request.insert(member.to_string(), Value::String(value));
        }
    }
    if let Some(flag) = declared.get("ApplyOnTransformedLogs").and_then(declared_flag) {
        request.insert("applyOnTransformedLogs".to_string(), Value::Bool(flag));
    }
    call_logs(ctx, region, "PutSubscriptionFilter", &Value::Object(request)).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// Put the filter an `AWS::Logs::MetricFilter` declares on the group its declaration names.
///
/// `PutMetricFilter` "creates or updates a metric filter", so this one call serves the type's
/// create and its in-place update alike — `declared` is the document in force, which on an
/// update is the new declaration rather than the one the record carries.
async fn put_metric_filter(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    refuse_unapplied_properties(declared, &resource.resource_type, METRIC_FILTER_PROPERTIES)?;
    let mut request = Map::new();
    request.insert(
        "logGroupName".to_string(),
        Value::String(declared_log_group(declared).ok_or_else(|| missing(resource, LOG_GROUP_NAME))?),
    );
    request.insert(
        "filterName".to_string(),
        Value::String(resource.physical_resource_id.clone()),
    );
    // `filterPattern` is required by the call and by the type, and "" is a pattern — the one
    // that matches every log event — so a declaration that states none is sent as it.
    request.insert(
        "filterPattern".to_string(),
        Value::String(declared_text(declared, "FilterPattern").unwrap_or_default()),
    );
    request.insert(
        "metricTransformations".to_string(),
        metric_transformations(declared),
    );
    if let Some(flag) = declared.get("ApplyOnTransformedLogs").and_then(declared_flag) {
        request.insert("applyOnTransformedLogs".to_string(), Value::Bool(flag));
    }
    call_logs(ctx, region, "PutMetricFilter", &Value::Object(request)).await?;
    Ok(Created::attributes(BTreeMap::new()))
}

/// The `metricTransformations` member of the request, built from the declaration's own list.
///
/// A transformation's members are `MetricTransformation`'s own under the same names up to
/// case, except `Dimensions`: the template writes the `[{Key, Value}]` list every taggable
/// type writes its tags with, while the API's own member is a MAP from dimension name to the
/// field the value is taken from. A member the API does not carry is left out rather than
/// sent, so that service's own validation judges the transformation it would judge for a
/// client's own call.
fn metric_transformations(properties: &Value) -> Value {
    let declared = match properties.get("MetricTransformations") {
        Some(Value::Array(transformations)) => transformations.clone(),
        Some(Value::Null) | None => Vec::new(),
        Some(one) => vec![one.clone()],
    };
    Value::Array(
        declared
            .iter()
            .map(|transformation| {
                let mut carried = Map::new();
                for (property, member) in [
                    ("MetricName", "metricName"),
                    ("MetricNamespace", "metricNamespace"),
                    ("MetricValue", "metricValue"),
                    ("Unit", "unit"),
                ] {
                    if let Some(value) = declared_text(transformation, property) {
                        carried.insert(member.to_string(), Value::String(value));
                    }
                }
                if let Some(default) = transformation
                    .get("DefaultValue")
                    .and_then(declared_number)
                {
                    carried.insert("defaultValue".to_string(), default);
                }
                if let Some(dimensions) = declared_dimensions(transformation) {
                    carried.insert("dimensions".to_string(), dimensions);
                }
                Value::Object(carried)
            })
            .collect(),
    )
}

/// The `dimensions` map one declared transformation states, read from the `[{Key, Value}]`
/// list the template writes it as — or from a map, which is what the API itself takes, so a
/// declaration written that way travels as it is.
fn declared_dimensions(transformation: &Value) -> Option<Value> {
    match transformation.get("Dimensions")? {
        Value::Array(dimensions) => {
            let mut carried = Map::new();
            for dimension in dimensions {
                let key = dimension.get("Key").and_then(Value::as_str)?;
                let value = dimension.get("Value").and_then(Value::as_str).unwrap_or("");
                carried.insert(key.to_string(), Value::String(value.to_string()));
            }
            Some(Value::Object(carried))
        }
        Value::Object(dimensions) => Some(Value::Object(dimensions.clone())),
        _ => None,
    }
}

/// Whether the owning service reports the resource ready.
///
/// Nothing is asked, and nothing waits: a group EXISTS from the moment `CreateLogGroup`
/// answers — a stream can be made in it and events put to it — a stream from the moment
/// `CreateLogStream` answers, and a filter is in force the moment `PutMetricFilter` or
/// `PutSubscriptionFilter` answers. None of those calls reports a non-terminal state for the
/// resource to settle out of.
pub(super) fn ready() -> Readiness {
    Readiness::Ready
}

/// Whether the owning service has finished removing a deleted resource.
///
/// Nothing is asked: each of `DeleteLogGroup`, `DeleteLogStream`, `DeleteMetricFilter` and
/// `DeleteSubscriptionFilter` takes the resource away as it answers, and CloudWatch Logs
/// reports no state a removed one is still being deleted in.
pub(super) fn removal() -> Removal {
    Removal::Gone
}

/// Apply a CHANGED declaration to the resource the stack already has, through the owning
/// service's own calls.
///
/// A metric filter is `PutMetricFilter` again, the one call documented to create OR update
/// one, so the filter keeps its identity and the whole declaration is applied.
///
/// A log group's changeable configuration is the three settings CloudWatch Logs publishes a
/// call for ([`group_settings`]): a declaration that adds or re-values one applies it, and
/// one that no longer states it is applied with that service's own call for having none
/// (`DeleteRetentionPolicy`, `DeleteDataProtectionPolicy`, `DeleteIndexPolicy`). Its
/// `KmsKeyId` is the key the group's data is encrypted with, which that service changes with
/// `AssociateKmsKey`/`DisassociateKmsKey`.
///
/// `Ok(None)` is a change no call can make: a log group's `LogGroupClass` is fixed when the
/// group is created, and a stream's own NAME is its identity, so such a deployment reports
/// the change as not applied rather than as a success that dropped the caller's change.
pub(super) async fn update(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    match kind {
        Kind::LogGroup => {
            refuse_unapplied_properties(declared, &resource.resource_type, LOG_GROUP_PROPERTIES)?;
            // The class a group was created under cannot be changed on the group that
            // exists: `CreateLogGroup` is the one call that states it.
            if declared_text(declared, "LogGroupClass")
                != declared_text(&resource.properties, "LogGroupClass")
            {
                return Ok(None);
            }
            let name = resource.physical_resource_id.clone();
            update_log_group(ctx, region, &name, &resource.properties, declared).await?;
            Ok(Some(Created::attributes(BTreeMap::new())))
        }
        // A stream's own NAME is its identity, so a declaration that states another one
        // describes a different stream and the deployment replaces the resource itself. The
        // group that HOLDS the stream is the other half of its identity: a declaration
        // naming another group is applied by making the stream there and taking the old one
        // out of the group it was in.
        Kind::LogStream => {
            if moved_to_another_group(resource, declared) {
                return replace_in_another_group(ctx, region, kind, resource, declared)
                    .await
                    .map(Some);
            }
            Ok(None)
        }
        Kind::MetricFilter => {
            if moved_to_another_group(resource, declared) {
                return replace_in_another_group(ctx, region, kind, resource, declared)
                    .await
                    .map(Some);
            }
            put_metric_filter(ctx, region, resource, declared)
                .await
                .map(Some)
        }
        Kind::SubscriptionFilter => {
            if moved_to_another_group(resource, declared) {
                return replace_in_another_group(ctx, region, kind, resource, declared)
                    .await
                    .map(Some);
            }
            put_subscription_filter(ctx, region, resource, declared)
                .await
                .map(Some)
        }
    }
}

/// Whether a declaration states a DIFFERENT log group from the one the record's resource
/// lives in, for one of the three types whose resource exists inside a group.
///
/// A declaration that states no group at all moves nothing: the create's own refusal
/// ([`missing`]) is what judges it.
fn moved_to_another_group(resource: &StoredResource, declared: &Value) -> bool {
    declared_log_group(declared)
        .is_some_and(|group| Some(group) != declared_log_group(&resource.properties))
}

/// Apply a declaration that moves a stream or a filter to ANOTHER log group, by REPLACING
/// the resource: the child is made in the group now declared, and the one it replaced is
/// taken out of the group it was in.
///
/// That order is CloudFormation's own for a replacement — the new resource is created and
/// the old one cleaned up afterwards — so a failure to make the new child leaves the old one
/// standing. What the removal answers changes nothing about the deployment, exactly as it
/// changes nothing for the cleanup `Delete` the algorithm itself owes a replaced resource
/// ([`super::reconcile`]): the replacement is already in force, and a child the owning
/// service could not take out of the group it was in is not a child this resource still
/// publishes. A child whose GROUP has already gone is in that position too, which is why the
/// removal is the same call [`delete`] makes and is judged the same way.
async fn replace_in_another_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Created, Refusal> {
    let created = match kind {
        Kind::LogStream => create_log_stream(ctx, region, resource, declared).await?,
        Kind::MetricFilter => put_metric_filter(ctx, region, resource, declared).await?,
        Kind::SubscriptionFilter => put_subscription_filter(ctx, region, resource, declared).await?,
        // A log group is not inside a group: it is the parent, and a renamed group is a
        // different resource the deployment replaces for itself.
        Kind::LogGroup => return Ok(Created::attributes(BTreeMap::new())),
    };
    let _ = delete(ctx, region, kind, resource).await;
    Ok(created)
}

/// Apply the three settings of a log group that a deployment states differently, and the KMS
/// key it names differently, to the group `name`.
async fn update_log_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    before: &Value,
    after: &Value,
) -> Result<(), Refusal> {
    // Retention: put where the new declaration states one, deleted where it no longer does
    // ("Deletes the specified retention policy", which is what takes a group back to
    // retaining its events for ever).
    if after.get("RetentionInDays") != before.get("RetentionInDays") {
        match after.get("RetentionInDays") {
            Some(days) if !days.is_null() => {
                let days = retention_days(days).ok_or_else(|| Refusal {
                    code: MISSING_PROPERTY.to_string(),
                    message: format!(
                        "the RetentionInDays property of {LOGS_LOG_GROUP} is a number of \
                         days, and {days} is not one"
                    ),
                })?;
                let request = serde_json::json!({
                    "logGroupName": name,
                    "retentionInDays": days,
                });
                call_logs(ctx, region, "PutRetentionPolicy", &request).await?;
            }
            _ => {
                let request = serde_json::json!({"logGroupName": name});
                call_logs(ctx, region, "DeleteRetentionPolicy", &request).await?;
            }
        }
    }
    if after.get("DataProtectionPolicy") != before.get("DataProtectionPolicy") {
        match after.get("DataProtectionPolicy") {
            Some(policy) if !policy.is_null() => {
                let request = serde_json::json!({
                    "logGroupIdentifier": name,
                    "policyDocument": policy_document(policy),
                });
                call_logs(ctx, region, "PutDataProtectionPolicy", &request).await?;
            }
            _ => {
                let request = serde_json::json!({"logGroupIdentifier": name});
                call_logs(ctx, region, "DeleteDataProtectionPolicy", &request).await?;
            }
        }
    }
    if after.get("FieldIndexPolicies") != before.get("FieldIndexPolicies") {
        let policies = field_index_policies(after);
        if policies.is_empty() {
            let request = serde_json::json!({"logGroupIdentifier": name});
            call_logs(ctx, region, "DeleteIndexPolicy", &request).await?;
        }
        for policy in policies {
            let request = serde_json::json!({
                "logGroupIdentifier": name,
                "policyDocument": policy_document(&policy),
            });
            call_logs(ctx, region, "PutIndexPolicy", &request).await?;
        }
    }
    let key_before = declared_text(before, "KmsKeyId");
    let key_after = declared_text(after, "KmsKeyId");
    if key_after != key_before {
        match key_after {
            Some(key) => {
                let request = serde_json::json!({
                    "logGroupName": name,
                    "kmsKeyId": key,
                });
                call_logs(ctx, region, "AssociateKmsKey", &request).await?;
            }
            None => {
                let request = serde_json::json!({"logGroupName": name});
                call_logs(ctx, region, "DisassociateKmsKey", &request).await?;
            }
        }
    }
    Ok(())
}

/// Delete the resource a stack created.
///
/// A group is `DeleteLogGroup` ("Deletes the specified log group and permanently deletes all
/// the archived log events associated with the log group"), a stream `DeleteLogStream`, a
/// metric filter `DeleteMetricFilter` and a subscription filter `DeleteSubscriptionFilter`,
/// each addressed by the name the create used. A resource the
/// owning service no longer holds is already in the state the delete asks for, judged on the
/// CODE and never on a message — which is also what makes deleting a stream or a filter
/// whose GROUP has already gone a completed deletion rather than a failure.
pub(super) async fn delete(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    let (operation, request) = match kind {
        Kind::LogGroup => (
            "DeleteLogGroup",
            serde_json::json!({"logGroupName": resource.physical_resource_id.clone()}),
        ),
        Kind::LogStream => (
            "DeleteLogStream",
            serde_json::json!({
                "logGroupName": required_log_group(resource)?,
                "logStreamName": resource.physical_resource_id.clone(),
            }),
        ),
        Kind::MetricFilter => (
            "DeleteMetricFilter",
            serde_json::json!({
                "logGroupName": required_log_group(resource)?,
                "filterName": resource.physical_resource_id.clone(),
            }),
        ),
        Kind::SubscriptionFilter => (
            "DeleteSubscriptionFilter",
            serde_json::json!({
                "logGroupName": required_log_group(resource)?,
                "filterName": resource.physical_resource_id.clone(),
            }),
        ),
    };
    match call_logs(ctx, region, operation, &request).await {
        Ok(_) => Ok(()),
        Err(refusal) if refusal.code == NOT_FOUND => Ok(()),
        Err(refusal) => Err(refusal),
    }
}

/// Delete one log group, for the create that has to take back the group it made.
async fn delete_log_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<(), Refusal> {
    let request = serde_json::json!({"logGroupName": name});
    call_logs(ctx, region, "DeleteLogGroup", &request)
        .await
        .map(|_| ())
}

/// The resource's configuration as the owning service reports it, for drift detection.
///
/// A group is read with `DescribeLogGroups` filtered to its own name, a metric filter with
/// `DescribeMetricFilters` and a subscription filter with `DescribeSubscriptionFilters`, each
/// filtered to its own name and each answer mapped back onto the
/// property vocabulary the type declares. A STREAM reports nothing to compare: both of its
/// properties are its identity, so there is no configuration of it that could drift.
/// `None` is a resource the owning service no longer holds, which is a deleted one.
pub(super) async fn actual_configuration(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    kind: Kind,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let name = resource.physical_resource_id.clone();
    match kind {
        Kind::LogGroup => {
            let request = serde_json::json!({"logGroupNamePrefix": name.clone()});
            let answer = match call_logs(ctx, region, "DescribeLogGroups", &request).await {
                Ok(answer) => answer,
                Err(refusal) if refusal.code == NOT_FOUND => return Ok(None),
                Err(refusal) => return Err(refusal),
            };
            let Some(group) = answer
                .get("logGroups")
                .and_then(Value::as_array)
                .and_then(|groups| {
                    groups.iter().find(|group| {
                        group.get("logGroupName").and_then(Value::as_str) == Some(name.as_str())
                    })
                })
            else {
                return Ok(None);
            };
            let mut configuration = Map::new();
            for (property, member) in [
                ("LogGroupName", "logGroupName"),
                ("KmsKeyId", "kmsKeyId"),
                ("LogGroupClass", "logGroupClass"),
                ("RetentionInDays", "retentionInDays"),
            ] {
                if let Some(value) = group.get(member) {
                    configuration.insert(property.to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(configuration)))
        }
        Kind::LogStream => Ok(None),
        Kind::MetricFilter => {
            let request = serde_json::json!({
                "logGroupName": required_log_group(resource)?,
                "filterNamePrefix": name.clone(),
            });
            let answer = match call_logs(ctx, region, "DescribeMetricFilters", &request).await {
                Ok(answer) => answer,
                Err(refusal) if refusal.code == NOT_FOUND => return Ok(None),
                Err(refusal) => return Err(refusal),
            };
            let Some(filter) = answer
                .get("metricFilters")
                .and_then(Value::as_array)
                .and_then(|filters| {
                    filters.iter().find(|filter| {
                        filter.get("filterName").and_then(Value::as_str) == Some(name.as_str())
                    })
                })
            else {
                return Ok(None);
            };
            let mut configuration = Map::new();
            for (property, member) in [
                ("FilterName", "filterName"),
                ("FilterPattern", "filterPattern"),
                ("LogGroupName", "logGroupName"),
            ] {
                if let Some(value) = filter.get(member) {
                    configuration.insert(property.to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(configuration)))
        }
        Kind::SubscriptionFilter => {
            let request = serde_json::json!({
                "logGroupName": required_log_group(resource)?,
                "filterNamePrefix": name.clone(),
            });
            let answer =
                match call_logs(ctx, region, "DescribeSubscriptionFilters", &request).await {
                    Ok(answer) => answer,
                    Err(refusal) if refusal.code == NOT_FOUND => return Ok(None),
                    Err(refusal) => return Err(refusal),
                };
            let Some(filter) = answer
                .get("subscriptionFilters")
                .and_then(Value::as_array)
                .and_then(|filters| {
                    filters.iter().find(|filter| {
                        filter.get("filterName").and_then(Value::as_str) == Some(name.as_str())
                    })
                })
            else {
                return Ok(None);
            };
            let mut configuration = Map::new();
            for (property, member) in [
                ("ApplyOnTransformedLogs", "applyOnTransformedLogs"),
                ("DestinationArn", "destinationArn"),
                ("Distribution", "distribution"),
                ("FilterName", "filterName"),
                ("FilterPattern", "filterPattern"),
                ("LogGroupName", "logGroupName"),
                ("RoleArn", "roleArn"),
            ] {
                if let Some(value) = filter.get(member) {
                    configuration.insert(property.to_string(), value.clone());
                }
            }
            Ok(Some(Value::Object(configuration)))
        }
    }
}

/// The group a stream or a filter belongs to, read off the record.
///
/// `LogGroupName` is a required property of both types, so a declaration carrying none
/// describes no resource that could be created: it is refused here, naming that property,
/// rather than sent as a request the owning service would reject.
fn required_log_group(resource: &StoredResource) -> Result<String, Refusal> {
    declared_log_group(&resource.properties).ok_or_else(|| missing(resource, LOG_GROUP_NAME))
}

/// The `LogGroupName` one declaration states, if it states one at all.
fn declared_log_group(properties: &Value) -> Option<String> {
    declared_text(properties, LOG_GROUP_NAME)
}

/// The refusal for a declaration that states none of a property its type requires.
fn missing(resource: &StoredResource, property: &str) -> Refusal {
    Refusal {
        code: MISSING_PROPERTY.to_string(),
        message: format!("{} declares no {property}", resource.resource_type),
    }
}

/// One declared property as the text the request carries, or `None` where the declaration
/// states no value for it. A value that is not a string is carried as the text it is, since
/// each of these members is a string on the wire.
fn declared_text(properties: &Value, property: &str) -> Option<String> {
    match properties.get(property)? {
        Value::Null => None,
        Value::String(text) if text.is_empty() => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A declared boolean in either spelling a template gives one: JSON's boolean, or the text a
/// template spells one as, read case-insensitively. `None` is a value that is no boolean at
/// all, which the owning service's own validation is what judges.
fn declared_flag(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::String(text) if text.eq_ignore_ascii_case("true") => Some(true),
        Value::String(text) if text.eq_ignore_ascii_case("false") => Some(false),
        _ => None,
    }
}

/// A declared number in either spelling a template gives one, for the one transformation
/// member the API models as a double.
fn declared_number(value: &Value) -> Option<Value> {
    match value {
        Value::Number(_) => Some(value.clone()),
        Value::String(text) => text
            .trim()
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        _ => None,
    }
}

/// The refusal for a declared property no call this module makes carries, naming which
/// property it was: refusing is what keeps a resource from being reported as created — or as
/// updated — with less than its declaration asked for. `properties` is the document the
/// deployment DECLARES, which on an update is not the one the record carries.
fn refuse_unapplied_properties(
    properties: &Value,
    resource_type: &str,
    published: &[&str],
) -> Result<(), Refusal> {
    let Value::Object(properties) = properties else {
        return Ok(());
    };
    for property in properties.keys() {
        if published.contains(&property.as_str()) {
            continue;
        }
        return Err(Refusal {
            code: NOT_IMPLEMENTED.to_string(),
            message: format!(
                "this emulator does not apply the {property} property of {resource_type}"
            ),
        });
    }
    Ok(())
}

/// Call one Amazon CloudWatch Logs operation on the awsJson1_1 wire its model declares.
async fn call_logs(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        LOGS_SIGNING_NAME,
        region,
        AWS_JSON_1_1,
        &format!("{LOGS_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four type names this module owns are exactly the ones it claims, and nothing
    /// else: a near miss of one is another type's, which this module must not answer for.
    #[test]
    fn the_owned_types_are_the_four_logs_types() {
        let cases: &[(&str, Option<Kind>)] = &[
            (LOGS_LOG_GROUP, Some(Kind::LogGroup)),
            (LOGS_LOG_STREAM, Some(Kind::LogStream)),
            (LOGS_METRIC_FILTER, Some(Kind::MetricFilter)),
            (LOGS_SUBSCRIPTION_FILTER, Some(Kind::SubscriptionFilter)),
            ("AWS::Logs::LogGroupAlias", None),
            ("AWS::SQS::Queue", None),
            ("", None),
        ];
        for (resource_type, expected) in cases {
            let actual = kind_of(resource_type);
            assert_eq!(
                actual, *expected,
                "case {resource_type:?}: expected kind_of {expected:?}, got {actual:?}"
            );
        }
    }

    /// A transformation's members reach `PutMetricFilter` under that call's own names, with
    /// the template's `[{Key, Value}]` dimensions as the MAP the API takes — and a member the
    /// declaration leaves out is left out of the request rather than sent empty.
    #[test]
    fn a_transformation_is_carried_under_the_api_member_names() {
        let declared = serde_json::json!({
            "MetricTransformations": [{
                "MetricName": "Errors",
                "MetricNamespace": "Service/Logs",
                "MetricValue": "1",
                "DefaultValue": 0,
                "Unit": "Count",
                "Dimensions": [{"Key": "Stage", "Value": "$.stage"}]
            }]
        });
        let carried = metric_transformations(&declared);
        let expected = serde_json::json!([{
            "metricName": "Errors",
            "metricNamespace": "Service/Logs",
            "metricValue": "1",
            "defaultValue": 0,
            "unit": "Count",
            "dimensions": {"Stage": "$.stage"}
        }]);
        assert_eq!(
            carried, expected,
            "case \"a full transformation\": expected {expected}, got {carried}"
        );

        let sparse = serde_json::json!({
            "MetricTransformations": [{
                "MetricName": "Hits",
                "MetricNamespace": "Service/Logs",
                "MetricValue": "1"
            }]
        });
        let carried = metric_transformations(&sparse);
        let expected = serde_json::json!([{
            "metricName": "Hits",
            "metricNamespace": "Service/Logs",
            "metricValue": "1"
        }]);
        assert_eq!(
            carried, expected,
            "case \"only the required members\": expected {expected}, got {carried}"
        );
    }

    /// A retention a template writes as text is the same number of days as one written as a
    /// JSON number, and a value that is no number of days at all is none.
    #[test]
    fn a_retention_is_read_in_either_spelling() {
        let cases: &[(&str, Value, Option<i64>)] = &[
            ("a JSON number", serde_json::json!(14), Some(14)),
            ("the text of one", serde_json::json!("14"), Some(14)),
            ("text with spaces", serde_json::json!(" 30 "), Some(30)),
            ("a word", serde_json::json!("forever"), None),
            ("a boolean", serde_json::json!(true), None),
        ];
        for (case, value, expected) in cases {
            let actual = retention_days(value);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected retention_days {expected:?}, got {actual:?}"
            );
        }
    }

    /// A policy property reaches its call as TEXT whichever way the template wrote it: the
    /// document as an object, or the text of one.
    #[test]
    fn a_policy_reaches_its_call_as_text() {
        let object = serde_json::json!({"Name": "a-policy", "Version": "2021-06-01"});
        let as_text = policy_document(&object);
        assert_eq!(
            as_text, r#"{"Name":"a-policy","Version":"2021-06-01"}"#,
            "case \"an object\": expected the document as text, got {as_text:?}"
        );
        let written = Value::String("{\"Name\":\"a-policy\"}".to_string());
        let as_text = policy_document(&written);
        assert_eq!(
            as_text, "{\"Name\":\"a-policy\"}",
            "case \"text\": expected the text as written, got {as_text:?}"
        );
    }

    /// A property no call this module makes carries is refused, naming it, so nothing is
    /// reported created with less than the declaration asked for — and every property each
    /// type publishes is accepted.
    #[test]
    fn an_unapplied_property_is_refused() {
        let declared = serde_json::json!({
            "LogGroupName": "a-group",
            "RetentionInDays": 7,
            "KmsKeyId": "a-key",
            "LogGroupClass": "STANDARD",
            "DataProtectionPolicy": {},
            "FieldIndexPolicies": [],
            "Tags": []
        });
        assert!(
            refuse_unapplied_properties(&declared, LOGS_LOG_GROUP, LOG_GROUP_PROPERTIES).is_ok(),
            "case \"every published property\": expected it to be applied, got a refusal"
        );
        let unknown = serde_json::json!({"LogGroupName": "a-group", "Transformer": {}});
        let refusal = refuse_unapplied_properties(&unknown, LOGS_LOG_GROUP, LOG_GROUP_PROPERTIES)
            .expect_err("case \"a property no call carries\": expected a refusal");
        assert!(
            refusal.message.contains("Transformer"),
            "case \"a property no call carries\": expected the refusal to name it, got \
             {:?}",
            refusal.message
        );
    }
}
