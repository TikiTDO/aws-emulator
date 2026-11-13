// SPDX-License-Identifier: CC0-1.0

//! A CloudFormation trail is an actual CloudTrail resource. Every effect/read uses
//! the signed in-process owner API; no CloudTrail registry or destination is edited here.
//! Multi-call changes keep an owner-read preimage for compensation, never a claim
//! that an unsuccessful compensating call rolled anything back.

use super::{
    AWS_JSON_1_1, Created, Readiness, Refusal, Removal, applicable_tags, call_json_rpc,
    key_value_tag_list,
};
use crate::state::{StoredResource, provisioner::tags_of};
use serde_json::{Map, Value, json};
use smithy_server_runtime::OperationContext;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const TRAIL: &str = "AWS::CloudTrail::Trail";
const MISSING: &str = "TrailNotFoundException";
const STRINGS: &[(&str, &str)] = &[
    ("S3BucketName", "S3BucketName"),
    ("S3KeyPrefix", "S3KeyPrefix"),
    ("SnsTopicName", "SnsTopicName"),
    ("CloudWatchLogsLogGroupArn", "CloudWatchLogsLogGroupArn"),
    ("CloudWatchLogsRoleArn", "CloudWatchLogsRoleArn"),
    ("KMSKeyId", "KmsKeyId"),
];
const FLAGS: &[(&str, bool)] = &[
    ("EnableLogFileValidation", false),
    ("IncludeGlobalServiceEvents", true),
    ("IsMultiRegionTrail", false),
    ("IsOrganizationTrail", false),
    ("RecursiveLogging", true),
];
const PROPERTIES: &[&str] = &[
    "TrailName",
    "IsLogging",
    "S3BucketName",
    "S3KeyPrefix",
    "SnsTopicName",
    "CloudWatchLogsLogGroupArn",
    "CloudWatchLogsRoleArn",
    "KMSKeyId",
    "EnableLogFileValidation",
    "IncludeGlobalServiceEvents",
    "IsMultiRegionTrail",
    "IsOrganizationTrail",
    "RecursiveLogging",
    "EventSelectors",
    "AdvancedEventSelectors",
    "InsightSelectors",
    "AggregationConfigurations",
    "Tags",
];
type Tags = BTreeMap<String, String>;

#[derive(Clone, Debug, PartialEq)]
struct Configuration {
    base: Map<String, Value>,
    selectors: Map<String, Value>,
    insights: Value,
    aggregation: Value,
    logging: bool,
}
struct Snapshot {
    name: String,
    arn: String,
    config: Configuration,
    tags: Tags,
}

fn invalid(message: impl Into<String>) -> Refusal {
    Refusal {
        code: "InvalidRequest".into(),
        message: message.into(),
    }
}
fn malformed(operation: &str) -> Refusal {
    Refusal {
        code: "InternalFailure".into(),
        message: format!("CloudTrail {operation} returned an incomplete owner response"),
    }
}
fn boolean(value: &Value) -> Option<bool> {
    value.as_bool().or_else(|| {
        value.as_str().and_then(|s| {
            if s.eq_ignore_ascii_case("true") {
                Some(true)
            } else if s.eq_ignore_ascii_case("false") {
                Some(false)
            } else {
                None
            }
        })
    })
}
fn object<'a>(value: &'a Value, allowed: &[&str]) -> Result<&'a Map<String, Value>, Refusal> {
    let fields = value
        .as_object()
        .ok_or_else(|| invalid("CloudTrail properties must be objects"))?;
    if let Some(key) = fields.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(Refusal {
            code: "NotImplemented".into(),
            message: format!("this emulator does not apply CloudTrail property {key}"),
        });
    }
    Ok(fields)
}
fn string_array(value: &Value) -> bool {
    value
        .as_array()
        .is_some_and(|items| items.iter().all(Value::is_string))
}
// Check nested shapes before any effect. Semantic limits/enums and referenced
// resources remain the owner's job; unknown nested fields must not disappear.
fn structures(value: &Value, kind: &str) -> Result<Value, Refusal> {
    let items = value
        .as_array()
        .ok_or_else(|| invalid(format!("{kind} must be an array")))?;
    let mut normalized = Vec::new();
    let mut tag_keys = BTreeSet::new();
    for item in items {
        let allowed: &[&str] = match kind {
            "EventSelectors" => &[
                "ReadWriteType",
                "IncludeManagementEvents",
                "DataResources",
                "ExcludeManagementEventSources",
            ],
            "DataResources" => &["Type", "Values"],
            "AdvancedEventSelectors" => &["Name", "FieldSelectors"],
            "FieldSelectors" => &[
                "Field",
                "Equals",
                "StartsWith",
                "EndsWith",
                "NotEquals",
                "NotStartsWith",
                "NotEndsWith",
            ],
            "InsightSelectors" => &["InsightType", "EventCategories"],
            "AggregationConfigurations" => &["EventCategory", "Templates"],
            "Tags" => &["Key", "Value"],
            _ => return Err(invalid("unknown CloudTrail structure")),
        };
        let mut fields = object(item, allowed)?.clone();
        for (key, field) in fields.iter_mut() {
            if key == "IncludeManagementEvents" {
                *field = Value::Bool(
                    boolean(field)
                        .ok_or_else(|| invalid("IncludeManagementEvents must be boolean"))?,
                );
            } else if matches!(key.as_str(), "DataResources" | "FieldSelectors") {
                *field = structures(field, key)?;
            } else if matches!(
                key.as_str(),
                "Values"
                    | "ExcludeManagementEventSources"
                    | "EventCategories"
                    | "Templates"
                    | "Equals"
                    | "StartsWith"
                    | "EndsWith"
                    | "NotEquals"
                    | "NotStartsWith"
                    | "NotEndsWith"
            ) {
                if !string_array(field) {
                    return Err(invalid(format!("{key} must be an array of strings")));
                }
            } else if !field.is_string() {
                return Err(invalid(format!("{key} must be a string")));
            }
        }
        let required: &[&str] = match kind {
            "DataResources" => &["Type", "Values"],
            "AdvancedEventSelectors" => &["FieldSelectors"],
            "FieldSelectors" => &["Field"],
            "InsightSelectors" => &["InsightType"],
            "AggregationConfigurations" => &["EventCategory", "Templates"],
            "Tags" => &["Key", "Value"],
            _ => &[],
        };
        if required.iter().any(|key| !fields.contains_key(*key)) {
            return Err(invalid(format!("{kind} is missing a required member")));
        }
        if kind == "Tags" {
            let key = fields["Key"].as_str().expect("string checked above");
            if key.is_empty() || !tag_keys.insert(key.to_string()) {
                return Err(invalid("Tags must have nonempty unique keys"));
            }
        }
        if kind == "EventSelectors" {
            for (key, default) in [
                ("ReadWriteType", json!("All")),
                ("IncludeManagementEvents", json!(true)),
                ("DataResources", json!([])),
                ("ExcludeManagementEventSources", json!([])),
            ] {
                fields.entry(key.to_string()).or_insert(default);
            }
        }
        if kind == "InsightSelectors"
            && fields
                .get("EventCategories")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
        {
            fields.insert("EventCategories".into(), json!(["Management"]));
        }
        normalized.push(Value::Object(fields));
    }
    Ok(Value::Array(normalized))
}
fn declaration(properties: &Value) -> Result<Configuration, Refusal> {
    let fields = object(properties, PROPERTIES)?;
    if let Some(name) = fields.get("TrailName") {
        if name.as_str().is_none_or(str::is_empty) {
            return Err(invalid("TrailName must be a nonempty string"));
        }
    }
    let logging = fields
        .get("IsLogging")
        .and_then(boolean)
        .ok_or_else(|| invalid("IsLogging is required and must be boolean"))?;
    let mut base = Map::new();
    for (property, member) in STRINGS {
        let value = match fields.get(*property) {
            Some(value) => value
                .as_str()
                .ok_or_else(|| invalid(format!("{property} must be a string")))?,
            None => "",
        };
        if *property == "S3BucketName" && value.is_empty() {
            return Err(invalid("S3BucketName is required"));
        }
        base.insert((*member).into(), json!(value));
    }
    for (property, default) in FLAGS {
        let value = match fields.get(*property) {
            Some(value) => {
                boolean(value).ok_or_else(|| invalid(format!("{property} must be boolean")))?
            }
            None => *default,
        };
        base.insert((*property).into(), json!(value));
    }
    if fields.contains_key("EventSelectors") && fields.contains_key("AdvancedEventSelectors") {
        return Err(invalid(
            "EventSelectors and AdvancedEventSelectors are mutually exclusive",
        ));
    }
    let mut selectors = Map::new();
    if let Some(value) = fields.get("AdvancedEventSelectors") {
        selectors.insert(
            "AdvancedEventSelectors".into(),
            structures(value, "AdvancedEventSelectors")?,
        );
    } else {
        selectors.insert(
            "EventSelectors".into(),
            structures(
                fields.get("EventSelectors").unwrap_or(&json!([{}])),
                "EventSelectors",
            )?,
        );
    }
    if let Some(tags) = fields.get("Tags") {
        structures(tags, "Tags")?;
    }
    Ok(Configuration {
        base,
        selectors,
        logging,
        insights: structures(
            fields.get("InsightSelectors").unwrap_or(&json!([])),
            "InsightSelectors",
        )?,
        aggregation: structures(
            fields
                .get("AggregationConfigurations")
                .unwrap_or(&json!([])),
            "AggregationConfigurations",
        )?,
    })
}
async fn call(
    ctx: &OperationContext<'_>,
    region: &str,
    operation: &str,
    request: &Value,
) -> Result<Value, Refusal> {
    call_json_rpc(
        ctx,
        "cloudtrail",
        region,
        AWS_JSON_1_1,
        &format!("CloudTrail_20131101.{operation}"),
        request,
    )
    .await
}
fn required_string<'a>(value: &'a Value, key: &str, operation: &str) -> Result<&'a str, Refusal> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| malformed(operation))
}
async fn trail(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Value>, Refusal> {
    match call(ctx, region, "GetTrail", &json!({"Name":name})).await {
        Err(error) if error.code == MISSING => Ok(None),
        Err(error) => Err(error),
        Ok(answer) => {
            let value = answer
                .get("Trail")
                .filter(|value| value.is_object())
                .ok_or_else(|| malformed("GetTrail"))?;
            required_string(value, "Name", "GetTrail")?;
            required_string(value, "TrailARN", "GetTrail")?;
            let identity = if name.starts_with("arn:") {
                "TrailARN"
            } else {
                "Name"
            };
            if value.get(identity).and_then(Value::as_str) != Some(name) {
                return Err(malformed("GetTrail"));
            }
            Ok(Some(value.clone()))
        }
    }
}
async fn read_tags(ctx: &OperationContext<'_>, region: &str, arn: &str) -> Result<Tags, Refusal> {
    let answer = call(ctx, region, "ListTags", &json!({"ResourceIdList":[arn]})).await?;
    let entries = answer
        .get("ResourceTagList")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("ListTags"))?;
    let tags = entries
        .iter()
        .find(|entry| entry.get("ResourceId").and_then(Value::as_str) == Some(arn))
        .and_then(|entry| entry.get("TagsList"))
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("ListTags"))?;
    let mut result = Tags::new();
    for tag in tags {
        let key = required_string(tag, "Key", "ListTags")?;
        let value = tag
            .get("Value")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("ListTags"))?;
        if result.insert(key.into(), value.into()).is_some() {
            return Err(malformed("ListTags"));
        }
    }
    Ok(result)
}
async fn snapshot(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Option<Snapshot>, Refusal> {
    let Some(trail) = trail(ctx, region, name).await? else {
        return Ok(None);
    };
    let name = required_string(&trail, "Name", "GetTrail")?.to_string();
    let arn = required_string(&trail, "TrailARN", "GetTrail")?.to_string();
    let status = call(ctx, region, "GetTrailStatus", &json!({"Name":name})).await?;
    let logging = status
        .get("IsLogging")
        .and_then(Value::as_bool)
        .ok_or_else(|| malformed("GetTrailStatus"))?;
    let selection = call(ctx, region, "GetEventSelectors", &json!({"TrailName":name})).await?;
    let mut selectors = Map::new();
    for member in ["EventSelectors", "AdvancedEventSelectors"] {
        if let Some(value) = selection.get(member) {
            selectors.insert(member.into(), structures(value, member)?);
        }
    }
    if selectors.len() != 1 {
        return Err(malformed("GetEventSelectors"));
    }
    let insights = match call(
        ctx,
        region,
        "GetInsightSelectors",
        &json!({"TrailName":name}),
    )
    .await
    {
        Err(error) if error.code == "InsightNotEnabledException" => json!([]),
        Err(error) => return Err(error),
        Ok(answer) => structures(
            answer
                .get("InsightSelectors")
                .ok_or_else(|| malformed("GetInsightSelectors"))?,
            "InsightSelectors",
        )?,
    };
    let aggregation = call(
        ctx,
        region,
        "GetEventConfiguration",
        &json!({"TrailName":name}),
    )
    .await?;
    let aggregation = structures(
        aggregation
            .get("AggregationConfigurations")
            .unwrap_or(&json!([])),
        "AggregationConfigurations",
    )?;
    let mut base = Map::new();
    for (_, member) in STRINGS {
        let wire = if *member == "SnsTopicName" {
            "SnsTopicARN"
        } else {
            member
        };
        let value = match trail.get(wire) {
            Some(value) => value.as_str().ok_or_else(|| malformed("GetTrail"))?,
            None => "",
        };
        if *member == "S3BucketName" && value.is_empty() {
            return Err(malformed("GetTrail"));
        }
        base.insert((*member).into(), json!(value));
    }
    for (member, _) in FLAGS {
        let wire = if *member == "EnableLogFileValidation" {
            "LogFileValidationEnabled"
        } else {
            member
        };
        let value = trail
            .get(wire)
            .and_then(Value::as_bool)
            .ok_or_else(|| malformed("GetTrail"))?;
        base.insert((*member).into(), json!(value));
    }
    let tags = read_tags(ctx, region, &arn).await?;
    Ok(Some(Snapshot {
        name,
        arn,
        tags,
        config: Configuration {
            base,
            selectors,
            insights,
            aggregation,
            logging,
        },
    }))
}
fn attributes(trail: &Value) -> Result<BTreeMap<String, String>, Refusal> {
    let mut result = BTreeMap::new();
    result.insert(
        "Arn".into(),
        required_string(trail, "TrailARN", "GetTrail")?.into(),
    );
    // Empty means no notification topic, and overwrites a stale previous attribute on update.
    result.insert(
        "SnsTopicArn".into(),
        trail
            .get("SnsTopicARN")
            .and_then(Value::as_str)
            .unwrap_or("")
            .into(),
    );
    Ok(result)
}
async fn apply(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
    config: &Configuration,
) -> Result<(), Refusal> {
    let mut base = config.base.clone();
    base.insert("Name".into(), json!(name));
    call(ctx, region, "UpdateTrail", &Value::Object(base)).await?;
    settings(ctx, region, name, config).await
}
async fn settings(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
    config: &Configuration,
) -> Result<(), Refusal> {
    let mut selectors = config.selectors.clone();
    selectors.insert("TrailName".into(), json!(name));
    call(ctx, region, "PutEventSelectors", &Value::Object(selectors)).await?;
    call(
        ctx,
        region,
        "PutInsightSelectors",
        &json!({"TrailName":name,"InsightSelectors":config.insights}),
    )
    .await?;
    call(
        ctx,
        region,
        "PutEventConfiguration",
        &json!({"TrailName":name,"AggregationConfigurations":config.aggregation}),
    )
    .await?;
    call(
        ctx,
        region,
        if config.logging {
            "StartLogging"
        } else {
            "StopLogging"
        },
        &json!({"Name":name}),
    )
    .await?;
    Ok(())
}
fn equivalent(wanted: &Configuration, actual: &Configuration) -> bool {
    let mut expected = wanted.clone();
    // GetTrail reports only SnsTopicARN; a bare desired name is equivalent to that ARN's name.
    if let (Some(Value::String(name)), Some(Value::String(arn))) = (
        expected.base.get("SnsTopicName"),
        actual.base.get("SnsTopicName"),
    ) {
        if !name.is_empty()
            && !name.starts_with("arn:")
            && arn.rsplit(':').next() == Some(name.as_str())
        {
            expected.base.insert("SnsTopicName".into(), json!(arn));
        }
    }
    expected == *actual
}
async fn verify(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
    expected: &Configuration,
) -> Result<Snapshot, Refusal> {
    let actual = snapshot(ctx, region, name)
        .await?
        .ok_or_else(|| malformed("GetTrail"))?;
    if !equivalent(expected, &actual.config) {
        return Err(invalid(
            "CloudTrail readback differs from the requested effective configuration",
        ));
    }
    Ok(actual)
}
fn with_compensation(mut original: Refusal, result: Result<(), Refusal>) -> Refusal {
    if let Err(error) = result {
        original.message.push_str(&format!(
            "; CloudTrail compensation failed ({}): {}",
            error.code, error.message
        ));
    }
    original
}
async fn delete_name(ctx: &OperationContext<'_>, region: &str, name: &str) -> Result<(), Refusal> {
    match call(ctx, region, "DeleteTrail", &json!({"Name":name})).await {
        Ok(_) => Ok(()),
        Err(error) if error.code == MISSING => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) async fn create(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    propagated: &[(String, String)],
) -> Result<Created, Refusal> {
    let config = declaration(&resource.properties)?;
    let name = &resource.physical_resource_id;
    if resource
        .properties
        .get("TrailName")
        .and_then(Value::as_str)
        .is_some_and(|declared| declared != name)
    {
        return Err(invalid(
            "CloudFormation physical name differs from declared TrailName",
        ));
    }
    let tags = applicable_tags(&tags_of(&resource.properties, propagated));
    let mut body = config.base.clone();
    // Absent optional strings are not passed on Create; Update uses empty strings to clear.
    body.retain(|_, value| value.as_str() != Some(""));
    body.insert("Name".into(), json!(name));
    body.insert("TagsList".into(), key_value_tag_list(&tags));
    call(ctx, region, "CreateTrail", &Value::Object(body)).await?;
    // Only after this call succeeded do we own anything to clean up. A collision never deletes.
    let configured = async {
        settings(ctx, region, name, &config).await?;
        let actual = verify(ctx, region, name, &config).await?;
        if tags
            .iter()
            .any(|(key, value)| actual.tags.get(key) != Some(value))
        {
            return Err(malformed("ListTags"));
        }
        Ok(Created {
            physical_resource_id: Some(actual.name),
            attributes: ready_attributes(ctx, region, name).await?,
        })
    }
    .await;
    match configured {
        Ok(created) => Ok(created),
        Err(error) => Err(with_compensation(
            error,
            delete_name(ctx, region, name).await,
        )),
    }
}
pub(super) async fn update(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
) -> Result<Option<Created>, Refusal> {
    let desired = declaration(declared)?;
    // A drift rollback can replay an explicit actual name for a declaration that
    // originally omitted TrailName. Compare with owner identity, not template shape.
    if declared
        .get("TrailName")
        .and_then(Value::as_str)
        .is_some_and(|name| name != resource.physical_resource_id)
    {
        return Ok(None);
    }
    let name = &resource.physical_resource_id;
    let before = snapshot(ctx, region, name).await?.ok_or_else(|| Refusal {
        code: MISSING.into(),
        message: "the trail to update does not exist".into(),
    })?;
    let changed = async {
        apply(ctx, region, name, &desired).await?;
        verify(ctx, region, name, &desired).await?;
        Ok(Some(Created::attributes(
            ready_attributes(ctx, region, name).await?,
        )))
    }
    .await;
    match changed {
        Ok(created) => Ok(created),
        Err(error) => {
            let restored = async {
                apply(ctx, region, name, &before.config).await?;
                verify(ctx, region, name, &before.config).await?;
                Ok(())
            }
            .await;
            Err(with_compensation(error, restored))
        }
    }
}
// The caller must supply prior CFN-managed tags, from prior declaration AND stack
// provenance. Current properties alone cannot recover keys removed by this update.
pub(super) async fn retag(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
    declared: &Value,
    propagated: &[(String, String)],
    previous_managed: &[(String, String)],
) -> Result<(), Refusal> {
    declaration(declared)?;
    let actual = snapshot(ctx, region, &resource.physical_resource_id)
        .await?
        .ok_or_else(|| malformed("GetTrail"))?;
    let wanted: Tags = applicable_tags(&tags_of(declared, propagated))
        .into_iter()
        .collect();
    let previous: Tags = applicable_tags(previous_managed).into_iter().collect();
    let managed: BTreeSet<String> = previous.keys().chain(wanted.keys()).cloned().collect();
    match write_tags(ctx, region, &actual.arn, &wanted, &managed).await {
        Ok(()) => Ok(()),
        Err(error) => Err(with_compensation(
            error,
            write_tags(ctx, region, &actual.arn, &actual.tags, &managed).await,
        )),
    }
}
async fn write_tags(
    ctx: &OperationContext<'_>,
    region: &str,
    arn: &str,
    desired: &Tags,
    managed: &BTreeSet<String>,
) -> Result<(), Refusal> {
    let held = read_tags(ctx, region, arn).await?;
    let (removed, added) = tag_delta(&held, desired, managed);
    // Remove before adding so a rename at the tag-count limit remains legal.
    if !removed.is_empty() {
        call(
            ctx,
            region,
            "RemoveTags",
            &json!({"ResourceId":arn,"TagsList":removed}),
        )
        .await?;
    }
    if !added.is_empty() {
        call(
            ctx,
            region,
            "AddTags",
            &json!({"ResourceId":arn,"TagsList":added}),
        )
        .await?;
    }
    let actual = read_tags(ctx, region, arn).await?;
    if managed
        .iter()
        .any(|key| actual.get(key) != desired.get(key))
    {
        return Err(malformed("ListTags"));
    }
    Ok(())
}
fn tag_delta(held: &Tags, desired: &Tags, managed: &BTreeSet<String>) -> (Vec<Value>, Vec<Value>) {
    let removed = held
        .keys()
        .filter(|key| managed.contains(*key) && !desired.contains_key(*key))
        .map(|key| json!({"Key":key}))
        .collect();
    let added = desired
        .iter()
        .filter(|(key, value)| managed.contains(*key) && held.get(*key) != Some(*value))
        .map(|(key, value)| json!({"Key":key,"Value":value}))
        .collect();
    (removed, added)
}
pub(super) async fn readiness(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Readiness, Refusal> {
    Ok(if trail(ctx, region, name).await?.is_some() {
        Readiness::Ready
    } else {
        Readiness::Failed("CloudTrail trail is absent".into())
    })
}
pub(super) async fn ready_attributes(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<BTreeMap<String, String>, Refusal> {
    attributes(
        &trail(ctx, region, name)
            .await?
            .ok_or_else(|| malformed("GetTrail"))?,
    )
}
pub(super) async fn delete(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<(), Refusal> {
    delete_name(ctx, region, &resource.physical_resource_id).await
}
pub(super) async fn removal(
    ctx: &OperationContext<'_>,
    region: &str,
    name: &str,
) -> Result<Removal, Refusal> {
    Ok(if trail(ctx, region, name).await?.is_none() {
        Removal::Gone
    } else {
        Removal::Pending("CloudTrail trail still exists".into())
    })
}
pub(super) async fn actual_configuration(
    ctx: &OperationContext<'_>,
    region: &str,
    resource: &StoredResource,
) -> Result<Option<Value>, Refusal> {
    let Some(actual) = snapshot(ctx, region, &resource.physical_resource_id).await? else {
        return Ok(None);
    };
    let mut result = Map::new();
    result.insert("TrailName".into(), json!(actual.name));
    result.insert("IsLogging".into(), json!(actual.config.logging));
    for (property, member) in STRINGS {
        result.insert((*property).into(), actual.config.base[*member].clone());
    }
    // The declaration may use an SNS name rather than ARN; compare in that vocabulary.
    if resource
        .properties
        .get("SnsTopicName")
        .and_then(Value::as_str)
        .is_some_and(|name| !name.starts_with("arn:"))
    {
        if let Some(topic) = result
            .get("SnsTopicName")
            .and_then(Value::as_str)
            .map(|arn| arn.rsplit(':').next().unwrap_or(arn).to_string())
        {
            result.insert("SnsTopicName".into(), json!(topic));
        }
    }
    for (property, _) in FLAGS {
        result.insert((*property).into(), actual.config.base[*property].clone());
    }
    result.extend(actual.config.selectors);
    result.insert("InsightSelectors".into(), actual.config.insights);
    result.insert(
        "AggregationConfigurations".into(),
        actual.config.aggregation,
    );
    result.insert(
        "Tags".into(),
        key_value_tag_list(&actual.tags.into_iter().collect::<Vec<_>>()),
    );
    Ok(Some(Value::Object(result)))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn minimal() -> Value {
        json!({"S3BucketName":"generic-trail-bucket","IsLogging":true})
    }
    #[test]
    fn omitted_settings_restore_owner_defaults_not_stale_values() {
        let config = declaration(&minimal()).unwrap();
        assert_eq!(config.base["S3KeyPrefix"], "");
        assert_eq!(config.base["KmsKeyId"], "");
        assert_eq!(config.base["IncludeGlobalServiceEvents"], true);
        assert_eq!(config.base["RecursiveLogging"], true);
        assert_eq!(
            config.selectors["EventSelectors"][0]["ReadWriteType"],
            "All"
        );
        assert_eq!(config.insights, json!([]));
        assert_eq!(config.aggregation, json!([]));
    }
    #[test]
    fn strict_admission_does_not_drop_unknown_or_malformed_properties() {
        for (key, value) in [
            ("Unknown", json!(true)),
            ("IsLogging", json!("maybe")),
            ("Tags", json!([{"Key":"missing-value"}])),
            (
                "AdvancedEventSelectors",
                json!([{"FieldSelectors":[{"Field":"eventCategory","Bogus":["x"]}]}]),
            ),
        ] {
            let mut props = minimal();
            props[key] = value;
            assert!(declaration(&props).is_err());
        }
        let mut props = minimal();
        props["IsLogging"] = json!("FALSE");
        assert!(!declaration(&props).unwrap().logging);
    }
    #[test]
    fn selectors_have_exactly_one_family_and_nested_defaults() {
        let mut props = minimal();
        props["EventSelectors"] = json!([{"IncludeManagementEvents":"false","DataResources":[{"Type":"AWS::S3::Object","Values":["arn:aws:s3:::fixture/"]}]}]);
        let config = declaration(&props).unwrap();
        assert_eq!(
            config.selectors["EventSelectors"][0]["IncludeManagementEvents"],
            false
        );
        props["AdvancedEventSelectors"] = json!([]);
        assert!(declaration(&props).is_err());
    }
    #[test]
    fn incomplete_owner_success_cannot_publish_fabricated_attributes() {
        assert!(attributes(&json!({"Name":"trail"})).is_err());
        assert_eq!(
            attributes(
                &json!({"TrailARN":"arn:aws:cloudtrail:us-east-1:000000000000:trail/example"})
            )
            .unwrap()["SnsTopicArn"],
            ""
        );
    }
    #[test]
    fn failed_compensation_preserves_original_owner_refusal() {
        let original = Refusal {
            code: "Original".into(),
            message: "original failure".into(),
        };
        let error = with_compensation(original, Err(invalid("restore refused")));
        assert_eq!(error.code, "Original");
        assert!(error.message.contains("original failure"));
        assert!(error.message.contains("compensation failed"));
    }
    #[test]
    fn tag_diff_does_not_remove_or_overwrite_unrelated_live_keys() {
        let held: Tags = [
            ("old".into(), "old".into()),
            ("outside".into(), "new-live-value".into()),
        ]
        .into();
        let desired: Tags = [
            ("new".into(), "wanted".into()),
            ("outside".into(), "stale-snapshot".into()),
        ]
        .into();
        let managed: BTreeSet<String> = ["old".into(), "new".into()].into();
        let (removed, added) = tag_delta(&held, &desired, &managed);
        assert_eq!(removed, vec![json!({"Key":"old"})]);
        assert_eq!(added, vec![json!({"Key":"new","Value":"wanted"})]);
    }
    #[test]
    fn readback_accepts_owner_topic_arn_but_not_changed_logging() {
        let mut wanted = declaration(&minimal()).unwrap();
        wanted.base.insert("SnsTopicName".into(), json!("fixture"));
        let mut actual = wanted.clone();
        actual.base.insert(
            "SnsTopicName".into(),
            json!("arn:aws:sns:us-east-1:000000000000:fixture"),
        );
        assert!(equivalent(&wanted, &actual));
        actual.logging = false;
        assert!(!equivalent(&wanted, &actual));
    }
}
