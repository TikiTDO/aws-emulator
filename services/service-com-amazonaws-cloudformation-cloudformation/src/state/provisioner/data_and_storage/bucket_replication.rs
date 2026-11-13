// SPDX-License-Identifier: CC0-1.0

//! The `ReplicationConfiguration` an `AWS::S3::Bucket` declares, as the
//! `PutBucketReplication` request Amazon S3 takes.
//!
//! "ReplicationConfiguration — configuration for replicating objects in an S3 bucket" is a
//! property of that resource type, and no member of `CreateBucket`: the configuration is
//! installed on the created bucket by the ONE call Amazon S3 publishes for it
//! (`PutBucketReplication` "creates a replication configuration or replaces an existing
//! one"), which is what makes the bucket really replicate its writes — S3 assumes
//! "the Identity and Access Management (IAM) role that Amazon S3 assumes when replicating
//! objects" (`ReplicationConfiguration$Role`) and copies each covered version to the
//! destination each covering rule names. A declaration whose configuration never reaches
//! that call leaves a bucket that reports no replication configuration and replicates
//! nothing, however the template declared it.
//!
//! The template's spelling is the API's with two renames, which is the whole of what this
//! module reconciles: a rule's object scope spells the API's `Filter.Tag` as `TagFilter` and
//! its `Filter.And.Tags` as `TagFilters` (the spelling every id-keyed configuration list of
//! this resource type uses for the tag half of a scope, read by
//! [`super::declared_tag_filters`]). Every other member — `Role`, and a rule's `Id`,
//! `Priority`, `Prefix`, `Status`, `SourceSelectionCriteria`, `DeleteMarkerReplication` and
//! `Destination` — is spelled the same on both sides and travels as the template wrote it,
//! so Amazon S3's own deserializer and handler are what judge the values.
//!
//! A declaration missing a member the API REQUIRES is REFUSED rather than sent incomplete:
//! the stack must not complete with a replication the bucket never carried.

use super::*;

/// The property an `AWS::S3::Bucket` declares its replication under.
pub(super) const REPLICATION_CONFIGURATION: &str = "ReplicationConfiguration";

/// The whole `ReplicationConfiguration` a bucket's declaration states, as the
/// `PutBucketReplication` request Amazon S3 takes — `None` for a declaration that carries no
/// such property, which replicates nothing.
pub(super) fn declared_replication_configuration(
    resource: &StoredResource,
) -> Result<Option<::aws_sdk_s3::types::ReplicationConfiguration>, Refusal> {
    use ::aws_sdk_s3::types::ReplicationConfiguration;

    let Some(declared) = resource.properties.get(REPLICATION_CONFIGURATION) else {
        return Ok(None);
    };
    let role = declared_text(declared, "Role").ok_or_else(|| missing("Role", "the"))?;
    let listed = declared
        .get("Rules")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut rules = Vec::with_capacity(listed.len());
    for (index, rule) in listed.iter().enumerate() {
        // Counted from 1, the way the declaration orders the rules, because `Id` is
        // optional: a rule that states none is still identified in a refusal.
        rules.push(declared_rule(rule, index + 1)?);
    }
    Ok(Some(
        ReplicationConfiguration::builder()
            .role(role)
            .set_rules(Some(rules))
            .build()
            .map_err(|error| not_sendable("the configuration", &error))?,
    ))
}

/// One declared rule, as the API's own `ReplicationRule`.
//
// `ReplicationRule$Prefix` is deprecated in the API in favour of `Filter` — "This member has
// been deprecated. Please use Filter instead" — and the SDK marks its setter so. It is still
// what a rule written in the EARLIER schema states, and `PutBucketReplication` still accepts
// such a rule (a V1 rule names no `Filter`, `Priority` or `DeleteMarkerReplication`), so a
// declaration carrying it has to reach the call rather than be dropped on the way.
#[allow(deprecated)]
fn declared_rule(
    declared: &Value,
    position: usize,
) -> Result<::aws_sdk_s3::types::ReplicationRule, Refusal> {
    use ::aws_sdk_s3::types::{ReplicationRule, ReplicationRuleStatus};

    let named = format!("rule {position}");
    let status = declared_text(declared, "Status")
        .ok_or_else(|| missing("Status", &format!("{named} of the")))?;
    ReplicationRule::builder()
        .status(ReplicationRuleStatus::from(status.as_str()))
        .destination(declared_destination(declared, &named)?)
        .set_id(declared_text(declared, "Id"))
        .set_priority(whole_number(declared.get("Priority")))
        .set_prefix(declared_text(declared, "Prefix"))
        .set_filter(declared_filter(declared, &named)?)
        .set_source_selection_criteria(declared_source_selection(declared, &named)?)
        .set_delete_marker_replication(declared_delete_markers(declared))
        .build()
        .map_err(|error| not_sendable(&named, &error))
}

/// The `Destination` one rule replicates to, as Amazon S3 takes it.
///
/// `Destination` is REQUIRED on a rule and its `Bucket` — "the Amazon Resource Name (ARN) of
/// the bucket where you want Amazon S3 to store the results" — is REQUIRED on the
/// destination, so a rule declaring neither is refused rather than sent incomplete.
fn declared_destination(
    declared: &Value,
    named: &str,
) -> Result<::aws_sdk_s3::types::Destination, Refusal> {
    use ::aws_sdk_s3::types::Destination;

    let destination = declared
        .get("Destination")
        .ok_or_else(|| missing("Destination", &format!("{named} of the")))?;
    let bucket = declared_text(destination, "Bucket")
        .ok_or_else(|| missing("Destination.Bucket", &format!("{named} of the")))?;
    Destination::builder()
        .bucket(bucket)
        .set_account(declared_text(destination, "Account"))
        .set_storage_class(
            declared_text(destination, "StorageClass")
                .map(|class| ::aws_sdk_s3::types::StorageClass::from(class.as_str())),
        )
        .set_access_control_translation(declared_owner_translation(destination, named)?)
        .set_encryption_configuration(declared_encryption(destination))
        .set_metrics(declared_metrics(destination, named)?)
        .set_replication_time(declared_replication_time(destination, named)?)
        .build()
        .map_err(|error| not_sendable(&format!("the Destination of {named}"), &error))
}

/// `Destination.AccessControlTranslation` — "specifies the overrides to use for object
/// owners on replication" — whose `Owner` the API REQUIRES.
fn declared_owner_translation(
    destination: &Value,
    named: &str,
) -> Result<Option<::aws_sdk_s3::types::AccessControlTranslation>, Refusal> {
    use ::aws_sdk_s3::types::{AccessControlTranslation, OwnerOverride};

    let Some(declared) = destination.get("AccessControlTranslation") else {
        return Ok(None);
    };
    let owner = declared_text(declared, "Owner").ok_or_else(|| {
        missing(
            "Destination.AccessControlTranslation.Owner",
            &format!("{named} of the"),
        )
    })?;
    Ok(Some(
        AccessControlTranslation::builder()
            .owner(OwnerOverride::from(owner.as_str()))
            .build()
            .map_err(|error| {
                not_sendable(
                    &format!("the AccessControlTranslation of {named}"),
                    &error,
                )
            })?,
    ))
}

/// `Destination.EncryptionConfiguration` — the key replicas of a rule are encrypted with,
/// whose one member the API leaves optional.
fn declared_encryption(destination: &Value) -> Option<::aws_sdk_s3::types::EncryptionConfiguration> {
    use ::aws_sdk_s3::types::EncryptionConfiguration;

    let declared = destination.get("EncryptionConfiguration")?;
    Some(
        EncryptionConfiguration::builder()
            .set_replica_kms_key_id(declared_text(declared, "ReplicaKmsKeyID"))
            .build(),
    )
}

/// `Destination.Metrics` — "a container specifying replication metrics-related settings" —
/// whose `Status` the API REQUIRES, beside the optional `EventThreshold`.
fn declared_metrics(
    destination: &Value,
    named: &str,
) -> Result<Option<::aws_sdk_s3::types::Metrics>, Refusal> {
    use ::aws_sdk_s3::types::{Metrics, MetricsStatus};

    let Some(declared) = destination.get("Metrics") else {
        return Ok(None);
    };
    let status = declared_text(declared, "Status")
        .ok_or_else(|| missing("Destination.Metrics.Status", &format!("{named} of the")))?;
    Ok(Some(
        Metrics::builder()
            .status(MetricsStatus::from(status.as_str()))
            .set_event_threshold(declared_minutes(declared.get("EventThreshold")))
            .build()
            .map_err(|error| not_sendable(&format!("the Metrics of {named}"), &error))?,
    ))
}

/// `Destination.ReplicationTime` — "a container specifying S3 Replication Time Control" —
/// whose `Status` and `Time` the API both REQUIRE.
fn declared_replication_time(
    destination: &Value,
    named: &str,
) -> Result<Option<::aws_sdk_s3::types::ReplicationTime>, Refusal> {
    use ::aws_sdk_s3::types::{ReplicationTime, ReplicationTimeStatus};

    let Some(declared) = destination.get("ReplicationTime") else {
        return Ok(None);
    };
    let owner = format!("{named} of the");
    let status = declared_text(declared, "Status")
        .ok_or_else(|| missing("Destination.ReplicationTime.Status", &owner))?;
    let time = declared_minutes(declared.get("Time"))
        .ok_or_else(|| missing("Destination.ReplicationTime.Time", &owner))?;
    Ok(Some(
        ReplicationTime::builder()
            .status(ReplicationTimeStatus::from(status.as_str()))
            .time(time)
            .build()
            .map_err(|error| not_sendable(&format!("the ReplicationTime of {named}"), &error))?,
    ))
}

/// A `ReplicationTimeValue` — the `{ Minutes }` container both `Metrics.EventThreshold` and
/// `ReplicationTime.Time` are, whose one member the API leaves optional.
fn declared_minutes(declared: Option<&Value>) -> Option<::aws_sdk_s3::types::ReplicationTimeValue> {
    use ::aws_sdk_s3::types::ReplicationTimeValue;

    let declared = declared?;
    Some(
        ReplicationTimeValue::builder()
            .set_minutes(whole_number(declared.get("Minutes")))
            .build(),
    )
}

/// The `ReplicationRuleFilter` one declared rule's object scope amounts to, or `None` for a
/// rule that declares no `Filter` at all — the older spelling, where the rule's own `Prefix`
/// is the whole scope.
///
/// The template's container carries the same three alternatives the API's does, with the tag
/// half spelled `TagFilter` where the API spells it `Tag` and `TagFilters` where the API
/// spells it `Tags`.
fn declared_filter(
    declared: &Value,
    named: &str,
) -> Result<Option<::aws_sdk_s3::types::ReplicationRuleFilter>, Refusal> {
    use ::aws_sdk_s3::types::{ReplicationRuleAndOperator, ReplicationRuleFilter};

    let Some(filter) = declared.get("Filter") else {
        return Ok(None);
    };
    let conjunction = match filter.get("And") {
        None => None,
        Some(and) => {
            let tags = declared_tag_filters(and, REPLICATION_CONFIGURATION, named)?;
            Some(
                ReplicationRuleAndOperator::builder()
                    .set_prefix(declared_text(and, "Prefix"))
                    .set_tags((!tags.is_empty()).then_some(tags))
                    .build(),
            )
        }
    };
    Ok(Some(
        ReplicationRuleFilter::builder()
            .set_prefix(declared_text(filter, "Prefix"))
            .set_tag(declared_tag(filter, named)?)
            .set_and(conjunction)
            .build(),
    ))
}

/// The single `Filter.TagFilter` one rule narrows by, as the API's own `Tag`.
///
/// `Tag` REQUIRES both `Key` and `Value`, so a filter stating one half of a tag is refused
/// rather than sent as a predicate the service cannot evaluate.
fn declared_tag(filter: &Value, named: &str) -> Result<Option<::aws_sdk_s3::types::Tag>, Refusal> {
    use ::aws_sdk_s3::types::Tag;

    let Some(declared) = filter.get("TagFilter") else {
        return Ok(None);
    };
    let owner = format!("{named} of the");
    let key = declared_text(declared, "Key").ok_or_else(|| missing("Filter.TagFilter.Key", &owner))?;
    let value = declared
        .get("Value")
        .and_then(scalar_text)
        .ok_or_else(|| missing("Filter.TagFilter.Value", &owner))?;
    Ok(Some(
        Tag::builder()
            .key(key)
            .value(value)
            .build()
            .map_err(|error| not_sendable(&format!("the Filter of {named}"), &error))?,
    ))
}

/// `SourceSelectionCriteria` — "a container that describes additional filters for
/// identifying the source objects that you want to replicate" — whose two members are each
/// a `{ Status }` container the API REQUIRES the status of.
fn declared_source_selection(
    declared: &Value,
    named: &str,
) -> Result<Option<::aws_sdk_s3::types::SourceSelectionCriteria>, Refusal> {
    use ::aws_sdk_s3::types::{
        ReplicaModifications, ReplicaModificationsStatus, SourceSelectionCriteria,
        SseKmsEncryptedObjects, SseKmsEncryptedObjectsStatus,
    };

    let Some(criteria) = declared.get("SourceSelectionCriteria") else {
        return Ok(None);
    };
    let owner = format!("{named} of the");
    let modifications = match criteria.get("ReplicaModifications") {
        None => None,
        Some(declared) => {
            let status = declared_text(declared, "Status").ok_or_else(|| {
                missing("SourceSelectionCriteria.ReplicaModifications.Status", &owner)
            })?;
            Some(
                ReplicaModifications::builder()
                    .status(ReplicaModificationsStatus::from(status.as_str()))
                    .build()
                    .map_err(|error| {
                        not_sendable(&format!("the ReplicaModifications of {named}"), &error)
                    })?,
            )
        }
    };
    let encrypted = match criteria.get("SseKmsEncryptedObjects") {
        None => None,
        Some(declared) => {
            let status = declared_text(declared, "Status").ok_or_else(|| {
                missing("SourceSelectionCriteria.SseKmsEncryptedObjects.Status", &owner)
            })?;
            Some(
                SseKmsEncryptedObjects::builder()
                    .status(SseKmsEncryptedObjectsStatus::from(status.as_str()))
                    .build()
                    .map_err(|error| {
                        not_sendable(&format!("the SseKmsEncryptedObjects of {named}"), &error)
                    })?,
            )
        }
    };
    Ok(Some(
        SourceSelectionCriteria::builder()
            .set_replica_modifications(modifications)
            .set_sse_kms_encrypted_objects(encrypted)
            .build(),
    ))
}

/// `DeleteMarkerReplication` — "specifies whether Amazon S3 replicates delete markers" —
/// whose one member the API leaves optional, so a rule declaring the empty container sends
/// the empty container.
fn declared_delete_markers(declared: &Value) -> Option<::aws_sdk_s3::types::DeleteMarkerReplication> {
    use ::aws_sdk_s3::types::{DeleteMarkerReplication, DeleteMarkerReplicationStatus};

    let container = declared.get("DeleteMarkerReplication")?;
    Some(
        DeleteMarkerReplication::builder()
            .set_status(
                declared_text(container, "Status")
                    .map(|status| DeleteMarkerReplicationStatus::from(status.as_str())),
            )
            .build(),
    )
}

/// The refusal for a declaration missing a member Amazon S3 REQUIRES: `owner` names which
/// part of the property the member belongs to, so the failing declaration can be found.
fn missing(member: &str, owner: &str) -> Refusal {
    Refusal {
        code: MISSING_PROPERTY_CODE.to_string(),
        message: format!(
            "{owner} {REPLICATION_CONFIGURATION} of {S3_BUCKET} declares no {member}"
        ),
    }
}

/// The refusal for a declared replication the SDK will not build into the request shape.
fn not_sendable(named: &str, error: &dyn ::std::fmt::Display) -> Refusal {
    Refusal {
        code: UNIDENTIFIED_FAILURE.to_string(),
        message: format!(
            "{named} of the {REPLICATION_CONFIGURATION} of {S3_BUCKET} cannot be sent to \
             Amazon S3: {error}"
        ),
    }
}
