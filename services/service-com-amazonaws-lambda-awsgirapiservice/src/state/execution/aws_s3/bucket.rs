// SPDX-License-Identifier: CC0-1.0

//! The BUCKET-WIDE half of the Amazon S3 seam a running handler calls S3 through: a
//! bucket's own configuration subresources, and the operations that act on the objects it
//! holds as a SET.
//!
//! A handler that administers storage rather than storing through it calls these: it reads
//! and replaces the notification container that decides where a bucket's object events are
//! published, reads and replaces the bucket's tag set, lists the versions a bucket holds and
//! removes a set of them in one call. Every one of them is bound exactly as the object calls
//! beside it are ([`super`]): the request is built, signed and parsed by the S3 client this
//! crate holds, over the in-process callback, so the answer is the emulated S3 service's
//! own — that operation's output members for a success, and S3's own code, message and
//! status for a refusal.
//!
//! The input of each is read out of the document the handler's client method was given, with
//! the nested containers these operations carry (`NotificationConfiguration`, `Tagging`,
//! `Delete`) walked member by member as the model lays them out. A member this binding does
//! not carry stops the call rather than travelling dropped, which is the rule the whole seam
//! holds to.

use ::serde_json::{Map, Value};

use ::aws_sdk_s3::types::{
    Delete, Event, EventBridgeConfiguration, FilterRule, FilterRuleName, LambdaFunctionConfiguration,
    NotificationConfiguration, NotificationConfigurationFilter, ObjectIdentifier, Owner,
    QueueConfiguration, S3KeyFilter, Tag, Tagging, TopicConfiguration,
};
use ::aws_sdk_s3::Client;

use super::super::aws_api::{Answer, Refusal};
use super::{insert_number, insert_text, insert_timestamp, refused, Input, Observed};

/// `GetBucketNotificationConfiguration`: the notification container the bucket carries.
///
/// A bucket with no notifications configured carries an EMPTY container
/// (`PutBucketNotificationConfiguration`: "by default, your bucket has no event
/// notifications configured. That is, the notification configuration will be an empty
/// NotificationConfiguration"), so a list the answer holds none of is left off the document
/// rather than reported as an empty one.
pub(super) async fn get_bucket_notification_configuration(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_notification_configuration()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_configurations(
        &mut document,
        "TopicConfigurations",
        read.topic_configurations()
            .iter()
            .map(|configuration| {
                configuration_document(
                    configuration.id(),
                    "TopicArn",
                    configuration.topic_arn(),
                    configuration.events(),
                    configuration.filter(),
                )
            })
            .collect(),
    );
    insert_configurations(
        &mut document,
        "QueueConfigurations",
        read.queue_configurations()
            .iter()
            .map(|configuration| {
                configuration_document(
                    configuration.id(),
                    "QueueArn",
                    configuration.queue_arn(),
                    configuration.events(),
                    configuration.filter(),
                )
            })
            .collect(),
    );
    insert_configurations(
        &mut document,
        "LambdaFunctionConfigurations",
        read.lambda_function_configurations()
            .iter()
            .map(|configuration| {
                configuration_document(
                    configuration.id(),
                    "LambdaFunctionArn",
                    configuration.lambda_function_arn(),
                    configuration.events(),
                    configuration.filter(),
                )
            })
            .collect(),
    );
    // The shape carries no members at all, so its PRESENCE is the whole value
    // (`NotificationConfiguration$EventBridgeConfiguration`: "enables delivery of events to
    // Amazon EventBridge").
    if read.event_bridge_configuration().is_some() {
        document.insert(
            "EventBridgeConfiguration".to_string(),
            Value::Object(Map::new()),
        );
    }
    observed.answer(document, Vec::new())
}

/// `PutBucketNotificationConfiguration`: the whole container, replacing what the bucket
/// carried before.
pub(super) async fn put_bucket_notification_configuration(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "NotificationConfiguration",
        "SkipDestinationValidation",
        "ExpectedBucketOwner",
    ])?;
    let configuration = notification_configuration(
        &input.required_structure("NotificationConfiguration")?,
    )?;
    let sent = client
        .put_bucket_notification_configuration()
        .bucket(input.required_text("Bucket")?)
        .notification_configuration(configuration)
        .set_skip_destination_validation(input.boolean("SkipDestinationValidation")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        // The operation's output is `Unit`: the success IS the status, with no member to
        // report.
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `GetBucketTagging`: the bucket's own tag set.
pub(super) async fn get_bucket_tagging(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "ExpectedBucketOwner"])?;
    let sent = client
        .get_bucket_tagging()
        .bucket(input.required_text("Bucket")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let read = match sent {
        Ok(read) => read,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    // `TagSet` is a required output member, so it is reported whether or not it holds
    // anything (`GetBucketTaggingOutput$TagSet`).
    document.insert(
        "TagSet".to_string(),
        Value::Array(
            read.tag_set()
                .iter()
                .map(|tag| {
                    let mut entry = Map::new();
                    entry.insert("Key".to_string(), Value::String(tag.key().to_string()));
                    entry.insert("Value".to_string(), Value::String(tag.value().to_string()));
                    Value::Object(entry)
                })
                .collect(),
        ),
    );
    observed.answer(document, Vec::new())
}

/// `PutBucketTagging`: the whole tag set, replacing what the bucket carried before.
pub(super) async fn put_bucket_tagging(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&["Bucket", "Tagging", "ExpectedBucketOwner"])?;
    let tagging = input.required_structure("Tagging")?;
    tagging.only(&["TagSet"])?;
    let mut tags = Vec::new();
    // `TagSet` is required on the container (`smithy_query(query="shape",shape="Tagging")`),
    // so a container carrying none of it stops the call rather than replacing the bucket's
    // tag set with nothing.
    for tag in tagging.required_structures("TagSet")? {
        tag.only(&["Key", "Value"])?;
        tags.push(
            Tag::builder()
                .key(tag.required_text("Key")?)
                .value(tag.required_text("Value")?)
                .build()
                .map_err(not_made)?,
        );
    }
    let sent = client
        .put_bucket_tagging()
        .bucket(input.required_text("Bucket")?)
        .tagging(
            Tagging::builder()
                .set_tag_set(Some(tags))
                .build()
                .map_err(not_made)?,
        )
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    match sent {
        Ok(_) => observed.answer(Map::new(), Vec::new()),
        Err(error) => refused(&error, observed),
    }
}

/// `ListObjectVersions`: one page of every version and delete marker the bucket holds.
pub(super) async fn list_object_versions(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Prefix",
        "Delimiter",
        "MaxKeys",
        "KeyMarker",
        "VersionIdMarker",
        "EncodingType",
        "ExpectedBucketOwner",
    ])?;
    let sent = client
        .list_object_versions()
        .bucket(input.required_text("Bucket")?)
        .set_prefix(input.text("Prefix")?)
        .set_delimiter(input.text("Delimiter")?)
        .set_max_keys(input.number("MaxKeys")?)
        .set_key_marker(input.text("KeyMarker")?)
        .set_version_id_marker(input.text("VersionIdMarker")?)
        .set_encoding_type(
            input
                .text("EncodingType")?
                .map(|value| ::aws_sdk_s3::types::EncodingType::from(value.as_str())),
        )
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let listed = match sent {
        Ok(listed) => listed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    insert_text(&mut document, "Name", listed.name());
    insert_text(&mut document, "Prefix", listed.prefix());
    insert_text(&mut document, "Delimiter", listed.delimiter());
    insert_text(&mut document, "KeyMarker", listed.key_marker());
    insert_text(&mut document, "VersionIdMarker", listed.version_id_marker());
    insert_text(&mut document, "NextKeyMarker", listed.next_key_marker());
    insert_text(
        &mut document,
        "NextVersionIdMarker",
        listed.next_version_id_marker(),
    );
    insert_text(
        &mut document,
        "EncodingType",
        listed
            .encoding_type()
            .map(::aws_sdk_s3::types::EncodingType::as_str),
    );
    insert_number(&mut document, "MaxKeys", listed.max_keys().map(i64::from));
    if let Some(truncated) = listed.is_truncated() {
        document.insert("IsTruncated".to_string(), Value::Bool(truncated));
    }
    let versions: Vec<Value> = listed
        .versions()
        .iter()
        .map(|version| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", version.key());
            insert_text(&mut entry, "VersionId", version.version_id());
            insert_text(&mut entry, "ETag", version.e_tag());
            insert_number(&mut entry, "Size", version.size());
            insert_timestamp(&mut entry, "LastModified", version.last_modified());
            insert_text(
                &mut entry,
                "StorageClass",
                version
                    .storage_class()
                    .map(::aws_sdk_s3::types::ObjectVersionStorageClass::as_str),
            );
            if let Some(latest) = version.is_latest() {
                entry.insert("IsLatest".to_string(), Value::Bool(latest));
            }
            insert_owner(&mut entry, version.owner());
            Value::Object(entry)
        })
        .collect();
    if !versions.is_empty() {
        document.insert("Versions".to_string(), Value::Array(versions));
    }
    let markers: Vec<Value> = listed
        .delete_markers()
        .iter()
        .map(|marker| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", marker.key());
            insert_text(&mut entry, "VersionId", marker.version_id());
            insert_timestamp(&mut entry, "LastModified", marker.last_modified());
            if let Some(latest) = marker.is_latest() {
                entry.insert("IsLatest".to_string(), Value::Bool(latest));
            }
            insert_owner(&mut entry, marker.owner());
            Value::Object(entry)
        })
        .collect();
    if !markers.is_empty() {
        document.insert("DeleteMarkers".to_string(), Value::Array(markers));
    }
    let prefixes: Vec<Value> = listed
        .common_prefixes()
        .iter()
        .map(|common| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Prefix", common.prefix());
            Value::Object(entry)
        })
        .collect();
    if !prefixes.is_empty() {
        document.insert("CommonPrefixes".to_string(), Value::Array(prefixes));
    }
    observed.answer(document, Vec::new())
}

/// `DeleteObjects`: the removal of a SET of objects, which answers what became of each one.
pub(super) async fn delete_objects(
    client: &Client,
    input: &Input,
    observed: &Observed,
) -> Result<Answer, Refusal> {
    input.only(&[
        "Bucket",
        "Delete",
        "MFA",
        "BypassGovernanceRetention",
        "ExpectedBucketOwner",
    ])?;
    let removals = input.required_structure("Delete")?;
    removals.only(&["Objects", "Quiet"])?;
    let mut objects = Vec::new();
    // `Objects` is required on the container (`smithy_query(query="shape",shape="Delete")`),
    // so a container carrying none of it stops the call rather than sending a removal that
    // names nothing.
    for object in removals.required_structures("Objects")? {
        object.only(&["Key", "VersionId", "ETag"])?;
        objects.push(
            ObjectIdentifier::builder()
                .key(object.required_text("Key")?)
                .set_version_id(object.text("VersionId")?)
                .set_e_tag(object.text("ETag")?)
                .build()
                .map_err(not_made)?,
        );
    }
    let sent = client
        .delete_objects()
        .bucket(input.required_text("Bucket")?)
        .delete(
            Delete::builder()
                .set_objects(Some(objects))
                .set_quiet(removals.boolean("Quiet")?)
                .build()
                .map_err(not_made)?,
        )
        .set_mfa(input.text("MFA")?)
        .set_bypass_governance_retention(input.boolean("BypassGovernanceRetention")?)
        .set_expected_bucket_owner(input.text("ExpectedBucketOwner")?)
        .send()
        .await;
    let removed = match sent {
        Ok(removed) => removed,
        Err(error) => return refused(&error, observed),
    };
    let mut document = Map::new();
    let deleted: Vec<Value> = removed
        .deleted()
        .iter()
        .map(|object| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", object.key());
            insert_text(&mut entry, "VersionId", object.version_id());
            insert_text(
                &mut entry,
                "DeleteMarkerVersionId",
                object.delete_marker_version_id(),
            );
            if let Some(marker) = object.delete_marker() {
                entry.insert("DeleteMarker".to_string(), Value::Bool(marker));
            }
            Value::Object(entry)
        })
        .collect();
    if !deleted.is_empty() {
        document.insert("Deleted".to_string(), Value::Array(deleted));
    }
    // What the service could NOT remove, per object: the code and sentence it reported for
    // that one key, which is the half of this answer a caller checks.
    let errors: Vec<Value> = removed
        .errors()
        .iter()
        .map(|failure| {
            let mut entry = Map::new();
            insert_text(&mut entry, "Key", failure.key());
            insert_text(&mut entry, "VersionId", failure.version_id());
            insert_text(&mut entry, "Code", failure.code());
            insert_text(&mut entry, "Message", failure.message());
            Value::Object(entry)
        })
        .collect();
    if !errors.is_empty() {
        document.insert("Errors".to_string(), Value::Array(errors));
    }
    observed.answer(document, Vec::new())
}

/// The container a `PutBucketNotificationConfiguration` carries, read out of the document
/// the handler passed.
///
/// Every destination family the container holds is read the same way — an optional `Id`, the
/// ARN of the destination, the events it is published for, and an optional key-name filter —
/// because that is how the model shapes each of them (`TopicConfiguration`,
/// `QueueConfiguration`, `LambdaFunctionConfiguration`).
fn notification_configuration(document: &Input) -> Result<NotificationConfiguration, Refusal> {
    document.only(&[
        "TopicConfigurations",
        "QueueConfigurations",
        "LambdaFunctionConfigurations",
        "EventBridgeConfiguration",
    ])?;
    let mut topics = Vec::new();
    for entry in document.structures("TopicConfigurations")? {
        entry.only(&["Id", "TopicArn", "Events", "Filter"])?;
        topics.push(
            TopicConfiguration::builder()
                .set_id(entry.text("Id")?)
                .topic_arn(entry.required_text("TopicArn")?)
                .set_events(Some(events(&entry)?))
                .set_filter(key_filter(&entry)?)
                .build()
                .map_err(not_made)?,
        );
    }
    let mut queues = Vec::new();
    for entry in document.structures("QueueConfigurations")? {
        entry.only(&["Id", "QueueArn", "Events", "Filter"])?;
        queues.push(
            QueueConfiguration::builder()
                .set_id(entry.text("Id")?)
                .queue_arn(entry.required_text("QueueArn")?)
                .set_events(Some(events(&entry)?))
                .set_filter(key_filter(&entry)?)
                .build()
                .map_err(not_made)?,
        );
    }
    let mut functions = Vec::new();
    for entry in document.structures("LambdaFunctionConfigurations")? {
        entry.only(&["Id", "LambdaFunctionArn", "Events", "Filter"])?;
        functions.push(
            LambdaFunctionConfiguration::builder()
                .set_id(entry.text("Id")?)
                .lambda_function_arn(entry.required_text("LambdaFunctionArn")?)
                .set_events(Some(events(&entry)?))
                .set_filter(key_filter(&entry)?)
                .build()
                .map_err(not_made)?,
        );
    }
    // The EventBridge container carries no members, so what the request says is whether it
    // is there at all.
    let event_bridge = match document.structure("EventBridgeConfiguration")? {
        Some(carried) => {
            carried.only(&[])?;
            Some(EventBridgeConfiguration::builder().build())
        }
        None => None,
    };
    Ok(NotificationConfiguration::builder()
        .set_topic_configurations(Some(topics))
        .set_queue_configurations(Some(queues))
        .set_lambda_function_configurations(Some(functions))
        .set_event_bridge_configuration(event_bridge)
        .build())
}

/// The events one destination is published for, as the model's own `Event` values.
///
/// `Events` is a REQUIRED member of each destination family
/// (`smithy_query(query="shape",shape="QueueConfiguration")`), so a destination carrying none
/// of it stops the call rather than travelling with that member emptied; which values S3
/// publishes for is S3's own to judge.
fn events(entry: &Input) -> Result<Vec<Event>, Refusal> {
    Ok(entry
        .required_text_list("Events")?
        .into_iter()
        .map(|value| Event::from(value.as_str()))
        .collect())
}

/// The key-name filter one destination carries, or `None` where it filters on nothing.
///
/// `NotificationConfigurationFilter` carries exactly one member, `Key`, whose own `S3Key`
/// holds the rules (`smithy_query(query="shape",shape="S3KeyFilter")`), and each rule is a
/// name out of `FilterRuleName` (`prefix`, `suffix`) with the value it matches on.
fn key_filter(entry: &Input) -> Result<Option<NotificationConfigurationFilter>, Refusal> {
    let Some(filter) = entry.structure("Filter")? else {
        return Ok(None);
    };
    filter.only(&["Key"])?;
    let Some(key) = filter.structure("Key")? else {
        return Ok(Some(NotificationConfigurationFilter::builder().build()));
    };
    key.only(&["FilterRules"])?;
    let mut rules = Vec::new();
    for rule in key.structures("FilterRules")? {
        rule.only(&["Name", "Value"])?;
        rules.push(
            FilterRule::builder()
                .set_name(
                    rule.text("Name")?
                        .map(|name| FilterRuleName::from(name.as_str())),
                )
                .set_value(rule.text("Value")?)
                .build(),
        );
    }
    Ok(Some(
        NotificationConfigurationFilter::builder()
            .key(S3KeyFilter::builder().set_filter_rules(Some(rules)).build())
            .build(),
    ))
}

/// One destination of a notification container, as the answer reports it.
fn configuration_document(
    id: Option<&str>,
    arn_member: &str,
    arn: &str,
    events: &[Event],
    filter: Option<&NotificationConfigurationFilter>,
) -> Value {
    let mut entry = Map::new();
    insert_text(&mut entry, "Id", id);
    entry.insert(arn_member.to_string(), Value::String(arn.to_string()));
    entry.insert(
        "Events".to_string(),
        Value::Array(
            events
                .iter()
                .map(|event| Value::String(event.as_str().to_string()))
                .collect(),
        ),
    );
    if let Some(filter) = filter {
        entry.insert("Filter".to_string(), filter_document(filter));
    }
    Value::Object(entry)
}

/// The key-name filter one reported destination carries, as the answer reports it.
fn filter_document(filter: &NotificationConfigurationFilter) -> Value {
    let mut document = Map::new();
    if let Some(key) = filter.key() {
        let rules: Vec<Value> = key
            .filter_rules()
            .iter()
            .map(|rule| {
                let mut entry = Map::new();
                insert_text(&mut entry, "Name", rule.name().map(FilterRuleName::as_str));
                insert_text(&mut entry, "Value", rule.value());
                Value::Object(entry)
            })
            .collect();
        let mut carried = Map::new();
        carried.insert("FilterRules".to_string(), Value::Array(rules));
        document.insert("Key".to_string(), Value::Object(carried));
    }
    Value::Object(document)
}

/// Put a destination list on the answer's document, where the answer carried any.
fn insert_configurations(document: &mut Map<String, Value>, name: &str, entries: Vec<Value>) {
    if !entries.is_empty() {
        document.insert(name.to_string(), Value::Array(entries));
    }
}

/// Put the owner of a listed version on its entry, where the answer carried one.
fn insert_owner(entry: &mut Map<String, Value>, owner: Option<&Owner>) {
    let Some(owner) = owner else {
        return;
    };
    let mut carried = Map::new();
    insert_text(&mut carried, "ID", owner.id());
    insert_text(&mut carried, "DisplayName", owner.display_name());
    entry.insert("Owner".to_string(), Value::Object(carried));
}

/// A container the handler's document could not be read into is a call that was NOT MADE,
/// carrying what the client said was missing.
fn not_made<E: ::std::fmt::Display>(error: E) -> Refusal {
    Refusal::NotMade(format!(
        "this Amazon S3 call could not be built from the parameters it was given: {error}"
    ))
}
