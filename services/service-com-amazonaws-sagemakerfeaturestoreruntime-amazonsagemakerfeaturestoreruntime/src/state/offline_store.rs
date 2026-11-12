// SPDX-License-Identifier: CC0-1.0

//! The OFFLINE STORE side of an ingestion: the Amazon S3 object one record lands as.
//!
//! "If a new record's EventTime is greater, the new record is written to both the
//! OnlineStore and OfflineStore. Otherwise, the record is a historic record and it is
//! written only to the OfflineStore" (`PutRecord`): so every whole-record write of this data
//! plane into a feature group the control plane gave an `OfflineStoreConfig` replicates that
//! record into the group's offline location, whether or not the online store took it. A
//! DELETION appends to the same location rather than ingesting — "In both cases, Feature
//! Store appends the deleted record marker to the OfflineStore" (`DeleteRecord`) — so the row
//! one write lands as is either an ingested record or a delete marker
//! ([`OfflineRecord::delete_marker`]), and the store is append-only either way.
//!
//! WHERE it goes, UNDER WHICH role and WHEN that destination is blocked are the facts the
//! two planes of the Feature Store share, so they are read from
//! [`::service_common::feature_store_offline_store`] — the control plane resolves the output
//! path when it creates the group and reports the status of the same location. What is left
//! here is this plane's own half: the BYTES of the object, and the write itself.
//!
//! The object is an Apache Parquet file, which is the format the offline store holds its
//! data in (the Glue table format a feature group gets by default is Parquet), written by
//! the Apache implementation of the format through its row-group writer rather than
//! assembled here — so what lands in the bucket is a file any Parquet reader reads. Its
//! columns are the record's own features plus the three Feature Store adds to every offline
//! record: `write_time`, `api_invocation_time` and `is_deleted` — the three names a feature
//! group may therefore not declare ("FeatureNames cannot be any of the following:
//! is_deleted, write_time, api_invocation_time", `FeatureDefinitions`).
//!
//! Nothing here holds this service's store guard: the permission question and the write are
//! both awaited.

use std::time::SystemTime;

use ::parquet::basic::{Compression, ConvertedType, Repetition, Type as PhysicalType};
use ::parquet::data_type::{BoolType, ByteArray, ByteArrayType, Int64Type};
use ::parquet::file::properties::WriterProperties;
use ::parquet::file::writer::SerializedFileWriter;
use ::parquet::schema::types::Type as SchemaType;
use ::service_common::feature_store_offline_store::{OfflineStoreDelivery, OfflineStoreLocation};

/// The extension one offline-store object's name ends with: the file IS a Parquet file.
const OBJECT_SUFFIX: &str = ".parquet";

/// The column carrying "the time the record was written into the offline store", one of the
/// three Feature Store adds to every offline record.
const WRITE_TIME_COLUMN: &str = "write_time";

/// The column carrying the time the ingestion call itself was made; see
/// [`WRITE_TIME_COLUMN`].
const API_INVOCATION_TIME_COLUMN: &str = "api_invocation_time";

/// The column carrying whether the row is a delete marker; see [`WRITE_TIME_COLUMN`].
const IS_DELETED_COLUMN: &str = "is_deleted";

/// One row the offline store holds: the feature values the write carried, as text, the
/// event time they belong to, and whether the row is a DELETE MARKER.
#[derive(Debug, Clone)]
pub struct OfflineRecord {
    /// The record's feature values in the order the request wrote them, each as the text
    /// the offline column holds.
    features: Vec<(String, String)>,
    /// The instant the record's event-time feature named, and `None` when the record
    /// carries no value for it — the one case the partition has no event time to take.
    event_time: Option<SystemTime>,
    /// `is_deleted`: whether this row marks the record as DELETED rather than ingested —
    /// "The deleted record marker is a record with the same RecordIdentifer as the original,
    /// but with is_deleted value set to True" (`DeleteRecord`).
    is_deleted: bool,
}

impl OfflineRecord {
    /// The record `record` posts to the feature group `definition` describes, read the way
    /// the online ingestion reads it: the event time is the value of the group's OWN
    /// `EventTimeFeatureName`, never the order or the names one request happens to carry.
    ///
    /// A `ValueAsStringList` — what a collection-typed feature carries — is held as the
    /// JSON array of its values, so a list column keeps every value it was written with and
    /// one element is distinguishable from a list of one.
    #[must_use]
    pub fn of(
        definition: &::service_common::feature_store_groups::FeatureGroupRecord,
        record: &[crate::ops::types::FeatureValue],
    ) -> Self {
        let features = record
            .iter()
            .map(|feature| {
                let value = match (&feature.value_as_string, &feature.value_as_string_list) {
                    (Some(value), _) => value.clone(),
                    (None, Some(values)) => {
                        ::serde_json::to_string(values).unwrap_or_default()
                    }
                    (None, None) => String::new(),
                };
                (feature.feature_name.clone(), value)
            })
            .collect();
        let event_time = record
            .iter()
            .find(|feature| feature.feature_name == definition.event_time_feature_name)
            .and_then(|feature| feature.value_as_string.as_deref())
            .and_then(crate::state::parse_event_time_secs)
            .and_then(seconds_since_epoch);
        Self {
            features,
            event_time,
            is_deleted: false,
        }
    }

    /// The DELETED RECORD MARKER one deletion appends to the offline store: "a record with
    /// the same RecordIdentifer as the original, but with is_deleted value set to True,
    /// EventTime set to the delete input EventTime, and other feature values set to null"
    /// (`DeleteRecord`).
    ///
    /// The two columns the marker carries are therefore the group's OWN
    /// `RecordIdentifierFeatureName` and `EventTimeFeatureName` — read from the group's
    /// definition, as every other read of a record of this data plane is — and the other
    /// features of the record carry no value at all, which is what `null` is in the row.
    /// The marker is partitioned by the deletion's own event time, so it lands beside the
    /// record it deletes when that is the hour the deletion names.
    #[must_use]
    pub fn delete_marker(
        definition: &::service_common::feature_store_groups::FeatureGroupRecord,
        record_identifier: &str,
        event_time: &str,
    ) -> Self {
        let mut features = vec![(
            definition.record_identifier_feature_name.clone(),
            record_identifier.to_owned(),
        )];
        // A group whose identifier feature IS its event-time feature would otherwise be
        // written with one column twice; the two values are then the one column's.
        if definition.event_time_feature_name != definition.record_identifier_feature_name {
            features.push((
                definition.event_time_feature_name.clone(),
                event_time.to_owned(),
            ));
        }
        Self {
            features,
            event_time: crate::state::parse_event_time_secs(event_time)
                .and_then(seconds_since_epoch),
            is_deleted: true,
        }
    }
}

/// `secs` seconds after the epoch as an instant, and `None` for a value no instant can
/// state.
///
/// An event-time feature may carry ANY number of seconds since the epoch — the value is a
/// `Fractional` or an ISO-8601 string from the request — so a magnitude beyond what a
/// duration or an instant holds yields no instant at all rather than failing the ingestion
/// it arrived on: such a record is partitioned by the instant it was written at, as a record
/// carrying no event time is.
fn seconds_since_epoch(secs: f64) -> Option<SystemTime> {
    let magnitude = ::std::time::Duration::try_from_secs_f64(secs.abs()).ok()?;
    if secs < 0.0 {
        SystemTime::UNIX_EPOCH.checked_sub(magnitude)
    } else {
        SystemTime::UNIX_EPOCH.checked_add(magnitude)
    }
}

/// What became of one record's replication into the offline store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Replicated {
    /// The object was written, at this key of the offline location.
    Written(String),
    /// Nothing was written: IAM answered that the feature group's role may not write the
    /// object. The ingestion itself still succeeded — the delivery is the service's own
    /// asynchronous work, and the group's `OfflineStoreStatus` is what reports the block
    /// (the control plane derives it from the same permission question).
    Withheld,
    /// Nothing was written and the reason is not a permission: the location could not be
    /// read, the bytes could not be laid out, or Amazon S3 refused the write (no such
    /// bucket, say). Traced, never reported to the ingesting client, which AWS answers
    /// before the replication is made at all.
    Failed(String),
}

/// Append one row of an ingestion or a deletion into the feature group's offline store.
///
/// The write is made UNDER the group's own role, so it is authorized against that role's
/// permissions first ([`::service_common::feature_store_offline_store::write_is_withheld`])
/// and skipped where they withhold it: a client that revokes the role's `s3:PutObject` sees
/// nothing retained and the group reported `Blocked`, which is exactly what it sees in AWS.
///
/// Amazon S3 is reached the way any emulated service reaches another: the AWS SDK over the
/// emulator's own in-process callback ([`::service_common::s3_client`]), so no socket is
/// opened and nothing outside this process is reachable from it.
pub async fn replicate(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    group_name: &str,
    delivery: &OfflineStoreDelivery,
    record: &OfflineRecord,
    written_at: SystemTime,
) -> Replicated {
    let Some(location) = OfflineStoreLocation::parse(&delivery.resolved_output_s3_uri) else {
        return Replicated::Failed(format!(
            "the offline store of feature group '{group_name}' is at \
             '{}', which names no Amazon S3 location",
            delivery.resolved_output_s3_uri
        ));
    };
    // The partition is the record's own event time, which is what the offline store is
    // partitioned by; a record carrying no event time has only the instant it was ingested
    // at.
    let key = ::service_common::feature_store_offline_store::record_object_key(
        &location,
        record.event_time.unwrap_or(written_at),
        written_at,
        OBJECT_SUFFIX,
    );
    if ::service_common::feature_store_offline_store::write_is_withheld(
        callback,
        region,
        &delivery.role_arn,
        &location,
        &key,
    )
    .await
    {
        ::tracing::debug!(
            feature_group = group_name,
            role = delivery.role_arn,
            "the feature group's role may not write the offline store, so the record is \
             not replicated"
        );
        return Replicated::Withheld;
    }
    let body = match record_parquet_bytes(group_name, record, written_at) {
        Ok(body) => body,
        Err(error) => return Replicated::Failed(error),
    };
    let written = ::service_common::s3_client::s3_client(callback, region)
        .put_object()
        .bucket(&location.bucket)
        .key(&key)
        .body(::aws_sdk_s3::primitives::ByteStream::from(body))
        .send()
        .await;
    match written {
        Ok(_) => Replicated::Written(key),
        Err(error) => {
            let reported = ::aws_sdk_s3::error::ProvideErrorMetadata::code(&error)
                .unwrap_or("the write failed")
                .to_string();
            ::tracing::debug!(
                feature_group = group_name,
                bucket = location.bucket,
                key = key,
                error = reported,
                "the record could not be written to the offline store"
            );
            Replicated::Failed(reported)
        }
    }
}

/// The bytes of the Parquet file holding `record`: one row group, one row, one OPTIONAL leaf
/// per column.
///
/// Every feature column is a UTF-8 string, which is what this data plane holds a feature
/// value as (`ValueAsString`), and the three columns Feature Store adds are the two
/// timestamps of the write and the delete marker. `Err` carries what the Parquet writer
/// refused, which the caller reports rather than writing a truncated object.
pub fn record_parquet_bytes(
    group_name: &str,
    record: &OfflineRecord,
    written_at: SystemTime,
) -> Result<Vec<u8>, String> {
    let mut fields = Vec::with_capacity(record.features.len() + 3);
    for (name, _) in &record.features {
        fields.push(::std::sync::Arc::new(text_column(name)?));
    }
    for name in [WRITE_TIME_COLUMN, API_INVOCATION_TIME_COLUMN] {
        fields.push(::std::sync::Arc::new(timestamp_column(name)?));
    }
    fields.push(::std::sync::Arc::new(boolean_column(IS_DELETED_COLUMN)?));
    let schema = SchemaType::group_type_builder(group_name)
        .with_fields(fields)
        .build()
        .map_err(|error| error.to_string())?;
    // SNAPPY, the compression the offline store's Parquet data is written with.
    let properties = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .build();
    let mut writer = SerializedFileWriter::new(
        Vec::new(),
        ::std::sync::Arc::new(schema),
        ::std::sync::Arc::new(properties),
    )
    .map_err(|error| error.to_string())?;
    {
        // The definition level of the single row: 1, i.e. the value is present. Every leaf
        // is OPTIONAL, so a null would be a 0 — and this row carries a value for each.
        let present: [i16; 1] = [1];
        let millis = millis_since_epoch(written_at);
        let mut row_group = writer.next_row_group().map_err(|error| error.to_string())?;
        for (_, value) in &record.features {
            let mut column = next_column(&mut row_group, group_name)?;
            column
                .typed::<ByteArrayType>()
                .write_batch(
                    &[ByteArray::from(value.as_bytes().to_vec())],
                    Some(&present),
                    None,
                )
                .map_err(|error| error.to_string())?;
            column.close().map_err(|error| error.to_string())?;
        }
        // `write_time` and `api_invocation_time`: the instant the replication was made and
        // the instant the ingesting call was served. This emulator replicates within the
        // call it serves, so the two are the one instant.
        for _ in 0..2 {
            let mut column = next_column(&mut row_group, group_name)?;
            column
                .typed::<Int64Type>()
                .write_batch(&[millis], Some(&present), None)
                .map_err(|error| error.to_string())?;
            column.close().map_err(|error| error.to_string())?;
        }
        // `is_deleted`: true only of a DELETE MARKER; an ingested record is not one.
        let mut column = next_column(&mut row_group, group_name)?;
        column
            .typed::<BoolType>()
            .write_batch(&[record.is_deleted], Some(&present), None)
            .map_err(|error| error.to_string())?;
        column.close().map_err(|error| error.to_string())?;
        row_group.close().map_err(|error| error.to_string())?;
    }
    writer
        .into_inner()
        .map_err(|error| error.to_string())
        .map(|buffer| buffer.to_vec())
}

/// The next column of the row group, or the error naming the schema that lost one.
fn next_column<'group>(
    row_group: &'group mut ::parquet::file::writer::SerializedRowGroupWriter<'_, Vec<u8>>,
    group_name: &str,
) -> Result<::parquet::file::writer::SerializedColumnWriter<'group>, String> {
    row_group
        .next_column()
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the Parquet schema of feature group '{group_name}' lost a column"))
}

/// One OPTIONAL UTF-8 column: a feature value this data plane holds as text.
fn text_column(name: &str) -> Result<SchemaType, String> {
    SchemaType::primitive_type_builder(name, PhysicalType::BYTE_ARRAY)
        .with_repetition(Repetition::OPTIONAL)
        .with_converted_type(ConvertedType::UTF8)
        .build()
        .map_err(|error| error.to_string())
}

/// One OPTIONAL millisecond-timestamp column: `write_time` and `api_invocation_time`, which
/// the offline store holds as timestamps rather than as text.
fn timestamp_column(name: &str) -> Result<SchemaType, String> {
    SchemaType::primitive_type_builder(name, PhysicalType::INT64)
        .with_repetition(Repetition::OPTIONAL)
        .with_converted_type(ConvertedType::TIMESTAMP_MILLIS)
        .build()
        .map_err(|error| error.to_string())
}

/// One OPTIONAL boolean column: `is_deleted`.
fn boolean_column(name: &str) -> Result<SchemaType, String> {
    SchemaType::primitive_type_builder(name, PhysicalType::BOOLEAN)
        .with_repetition(Repetition::OPTIONAL)
        .build()
        .map_err(|error| error.to_string())
}

/// `at` in milliseconds since the epoch, which is the unit the two timestamp columns above
/// declare.
fn millis_since_epoch(at: SystemTime) -> i64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::parquet::file::reader::FileReader;

    /// The definition of a feature group whose event-time feature is `event_time`.
    fn definition() -> ::service_common::feature_store_groups::FeatureGroupRecord {
        ::service_common::feature_store_groups::FeatureGroupRecord {
            name: "customers".to_owned(),
            arn: "arn:aws:sagemaker:us-east-1:123456789012:feature-group/customers".to_owned(),
            record_identifier_feature_name: "customer_id".to_owned(),
            event_time_feature_name: "event_time".to_owned(),
            online_store_enabled: true,
            offline_store: None,
            default_ttl_seconds: None,
        }
    }

    /// One posted feature value.
    fn feature(name: &str, value: &str) -> crate::ops::types::FeatureValue {
        crate::ops::types::FeatureValue {
            feature_name: name.to_owned(),
            value_as_string: Some(value.to_owned()),
            value_as_string_list: None,
        }
    }

    /// RULE: the object is a Parquet file holding one row, with one column per posted
    /// feature and the three columns Feature Store adds to every offline record.
    #[test]
    fn a_replicated_record_is_a_parquet_file_of_the_posted_record() {
        let record = OfflineRecord::of(
            &definition(),
            &[
                feature("customer_id", "C1"),
                feature("event_time", "2026-06-01T00:00:00Z"),
                crate::ops::types::FeatureValue {
                    feature_name: "tags".to_owned(),
                    value_as_string: None,
                    value_as_string_list: Some(vec!["a".to_owned(), "b".to_owned()]),
                },
            ],
        );
        let bytes = record_parquet_bytes("customers", &record, SystemTime::UNIX_EPOCH)
            .expect("the record lays out as a Parquet file");
        assert!(
            !bytes.is_empty(),
            "case one ingested record: expected a non-empty object, got {} bytes",
            bytes.len()
        );
        let reader = ::parquet::file::reader::SerializedFileReader::new(::bytes::Bytes::from(bytes))
            .expect("the written object is a Parquet file");
        let metadata = reader.metadata().file_metadata().clone();
        let columns: Vec<String> = metadata
            .schema_descr()
            .columns()
            .iter()
            .map(|column| column.name().to_string())
            .collect();
        assert_eq!(
            columns,
            vec![
                "customer_id",
                "event_time",
                "tags",
                WRITE_TIME_COLUMN,
                API_INVOCATION_TIME_COLUMN,
                IS_DELETED_COLUMN
            ],
            "case one ingested record: expected the posted features and the three added \
             columns, got {columns:?}"
        );
        assert_eq!(
            metadata.num_rows(),
            1,
            "case one ingested record: expected one row, got {}",
            metadata.num_rows()
        );
        let row = reader
            .get_row_iter(None)
            .expect("the file's rows are readable")
            .next()
            .expect("the file holds a row")
            .expect("the row is readable");
        let rendered = row.to_string();
        for expected in ["C1", "2026-06-01T00:00:00Z", "[\"a\",\"b\"]"] {
            assert!(
                rendered.contains(expected),
                "case one ingested record: expected {expected:?} in the row, got {rendered}"
            );
        }
    }

    /// RULE: a deletion's row is the delete marker — the group's own record identifier and
    /// the deletion's event time, `is_deleted` true — while an ingestion's row is not
    /// marked deleted.
    #[test]
    fn a_deletions_row_is_the_delete_marker_of_the_identified_record() {
        // (case, the row, whether it must be marked deleted, the values it must carry)
        let rows: [(&str, OfflineRecord, bool, Vec<&str>); 3] = [
            (
                "an ingested record",
                OfflineRecord::of(
                    &definition(),
                    &[
                        feature("customer_id", "C1"),
                        feature("event_time", "2026-06-01T00:00:00Z"),
                    ],
                ),
                false,
                vec!["C1", "2026-06-01T00:00:00Z"],
            ),
            (
                "a deletion of that record",
                OfflineRecord::delete_marker(&definition(), "C1", "2026-06-02T00:00:00Z"),
                true,
                vec!["C1", "2026-06-02T00:00:00Z"],
            ),
            (
                "a deletion of a record that was never ingested",
                OfflineRecord::delete_marker(&definition(), "C9", "1780272000"),
                true,
                vec!["C9", "1780272000"],
            ),
        ];
        for (case, row, marked, expected_values) in rows {
            let bytes = record_parquet_bytes("customers", &row, SystemTime::UNIX_EPOCH)
                .expect("the row lays out as a Parquet file");
            let reader =
                ::parquet::file::reader::SerializedFileReader::new(::bytes::Bytes::from(bytes))
                    .expect("the written object is a Parquet file");
            let rendered = reader
                .get_row_iter(None)
                .expect("the file's rows are readable")
                .next()
                .expect("the file holds a row")
                .expect("the row is readable")
                .to_string();
            let expected_marker = format!("{IS_DELETED_COLUMN}: {marked}");
            assert!(
                rendered.contains(&expected_marker),
                "case {case:?}: expected {expected_marker:?} in the row, got {rendered}"
            );
            for expected in expected_values {
                assert!(
                    rendered.contains(expected),
                    "case {case:?}: expected {expected:?} in the row, got {rendered}"
                );
            }
        }
    }

    /// RULE: the row's event time is the value of the group's OWN event-time feature, and a
    /// record carrying none — or one carrying a value no instant can state — has no event
    /// time at all, so the write's own instant is what partitions it.
    #[test]
    fn the_event_time_of_a_row_is_the_groups_own_feature() {
        // (case, the posted record, the instant it belongs to)
        let rows: [(&str, Vec<crate::ops::types::FeatureValue>, Option<SystemTime>); 5] = [
            (
                "an ISO-8601 event time",
                vec![
                    feature("customer_id", "C1"),
                    feature("event_time", "2026-06-01T00:00:00Z"),
                ],
                Some(SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_780_272_000)),
            ),
            (
                "an event time in seconds since the epoch",
                vec![
                    feature("customer_id", "C1"),
                    feature("event_time", "1780272000"),
                ],
                Some(SystemTime::UNIX_EPOCH + ::std::time::Duration::from_secs(1_780_272_000)),
            ),
            (
                "a record carrying no event time",
                vec![feature("customer_id", "C1")],
                None,
            ),
            (
                "an event time no instant can state",
                vec![
                    feature("customer_id", "C1"),
                    feature("event_time", "1e300"),
                ],
                None,
            ),
            (
                "an event time before the epoch",
                vec![
                    feature("customer_id", "C1"),
                    feature("event_time", "-86400"),
                ],
                Some(SystemTime::UNIX_EPOCH - ::std::time::Duration::from_secs(86_400)),
            ),
        ];
        for (case, posted, expected) in rows {
            let read = OfflineRecord::of(&definition(), &posted).event_time;
            assert_eq!(
                read, expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
