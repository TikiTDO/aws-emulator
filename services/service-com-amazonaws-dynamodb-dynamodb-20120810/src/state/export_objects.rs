// SPDX-License-Identifier: CC0-1.0

//! The objects one table export WRITES, and the write into the object store that holds
//! them.
//!
//! "Exports table data to an S3 bucket" (the `ExportTableToPointInTime` doc): what an
//! export leaves behind is the exported item data itself, under a path of its own —
//! `<S3Prefix>/AWSDynamoDB/<ExportId>/` — together with the manifest that says what was
//! written and the digests of each. A caller then LISTS and READS those objects with
//! Amazon S3's own operations, so they have to be objects that service really holds,
//! which is why they are recorded through its own store on this same engine
//! ([`::service_com_amazonaws_s3_amazons3::state`]) rather than reported as written.
//!
//! The DATA is the table's own: the items the table held at the instant the export read
//! it, rendered in the format the request asked for. Nothing is synthesised — an export
//! of a table holding no item writes an empty data file and a manifest that says so.
//!
//! What one export can write is bounded by what the table can hold ([`MAX_ITEMS_PER_TABLE`]
//! and [`MAX_TABLE_SIZE_BYTES`], which admission control holds every item write to) and by
//! the object store's own ceilings, which refuse an object past them — so the write below
//! reports what that store really took rather than growing the process without end.

use super::*;

/// The `version` the export's own manifest carries: the version of the OUTPUT FORMAT a
/// consumer of these objects branches on, as DynamoDB's export output stamps it.
pub const EXPORT_MANIFEST_VERSION: &str = "2020-06-30";

/// The folder DynamoDB writes an export's objects under, below the prefix the request
/// named: `<S3Prefix>/AWSDynamoDB/<ExportId>/`.
pub const EXPORT_OBJECT_FOLDER: &str = "AWSDynamoDB";

/// The `FailureCode` of an export whose destination bucket the object store holds none
/// of, with the message S3 states for that condition.
pub const EXPORT_FAILURE_NO_SUCH_BUCKET: &str = "S3NoSuchBucket";
pub const EXPORT_FAILURE_NO_SUCH_BUCKET_MESSAGE: &str = "The specified bucket does not exist";

/// The `FailureCode` of an export the object store could not take the objects of — this
/// emulator's own store running out rather than anything the request got wrong, so the
/// message says so.
pub const EXPORT_FAILURE_WRITE_REFUSED: &str = "InternalServerError";
pub const EXPORT_FAILURE_WRITE_REFUSED_MESSAGE: &str =
    "This emulator's object store could not hold the exported objects";

/// How many items ONE export renders, at most.
///
/// This emulator's own ceiling on the work a single call can ask for, not an AWS quota:
/// DynamoDB states no bound on what a table may export. An export reads every item the
/// table holds and renders it, so without a bound of its own one call's transient work
/// would be whatever a table has been filled with ([`MAX_ITEMS_PER_TABLE`]). A request
/// past it is refused BEFORE anything is rendered, with the modelled
/// `LimitExceededException` and [`export_too_large_message`].
pub const MAX_EXPORTED_ITEMS: i64 = 100_000;

/// How many bytes of ITEM DATA one export renders, at most — the same ceiling measured
/// over the data rather than the item count, since one large item costs what many small
/// ones do. A fraction of what the table store itself may hold
/// ([`MAX_TABLE_SIZE_BYTES`]), because this is the work of a single call.
pub const MAX_EXPORTED_BYTES: i64 = 128 * 1024 * 1024;

/// How an export of a table holding more than one call will render is refused.
///
/// The message names THE EMULATOR as what ran out and says the service itself imposes no
/// such limit, the way this store words its other own-exhaustion refusals, rather than
/// sending a client to fix input that is not wrong.
pub fn export_too_large_message(table: &str) -> String {
    format!(
        "This emulator cannot export table {table}: one export renders at most \
         {MAX_EXPORTED_ITEMS} items and {MAX_EXPORTED_BYTES} bytes of item data. DynamoDB \
         itself imposes no such limit; export a smaller table."
    )
}

/// One object an export writes: a key in the destination bucket and the bytes stored
/// under it.
#[derive(Debug, Clone)]
pub struct ExportedObject {
    /// The object's key, under the export's own path.
    pub key: String,
    /// The object's bytes.
    pub body: Vec<u8>,
}

/// What one export's write into the object store really stored.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExportWrite {
    /// How many of the objects offered the store took.
    pub objects: usize,
    /// How many bytes of them it took.
    pub bytes: usize,
}

/// The path every object of one export is written under:
/// `<S3Prefix>/AWSDynamoDB/<ExportId>/`, and `AWSDynamoDB/<ExportId>/` for an export that
/// named no prefix — "The Amazon S3 bucket prefix to use as the file name and path of the
/// exported snapshot" (`$S3Prefix`), with the export's own id below it, which is what
/// keeps two exports into one bucket apart.
pub fn export_object_prefix(s3_prefix: Option<&str>, export_id: &str) -> String {
    match s3_prefix.map(|prefix| prefix.trim_matches('/')) {
        Some(prefix) if !prefix.is_empty() => {
            format!("{prefix}/{EXPORT_OBJECT_FOLDER}/{export_id}/")
        }
        _ => format!("{EXPORT_OBJECT_FOLDER}/{export_id}/"),
    }
}

/// The objects `export` writes for the items it read, in the order they are written.
///
/// The set is the one a DynamoDB export leaves in the bucket: the marker that says the
/// task started, the data file holding the items, the manifest of data files and the
/// summary manifest — each manifest beside the digest file that covers it, so a consumer
/// can check what it read.
pub fn exported_objects(export: &Export, items: &[Item]) -> Vec<ExportedObject> {
    let prefix = export_object_prefix(export.request.s3_prefix.as_deref(), export.export_id());
    let data_key = format!("{prefix}data/{}.json.gz", export.export_id());
    let data = data_file_body(items);
    let files_key = format!("{prefix}manifest-files.json");
    let files = manifest_files_document(items.len(), &data_key, &data);
    let summary_key = format!("{prefix}manifest-summary.json");
    let summary = manifest_summary_document(export, items.len(), &files_key);
    vec![
        // "_started": the marker DynamoDB writes when the task begins.
        ExportedObject {
            key: format!("{prefix}_started"),
            body: Vec::new(),
        },
        ExportedObject {
            key: data_key,
            body: data,
        },
        ExportedObject {
            key: format!("{files_key}.md5"),
            body: ::service_com_amazonaws_s3_amazons3::state::content_md5(files.as_bytes())
                .into_bytes(),
        },
        ExportedObject {
            key: files_key,
            body: files.into_bytes(),
        },
        ExportedObject {
            key: format!("{summary_key}.md5"),
            body: ::service_com_amazonaws_s3_amazons3::state::content_md5(summary.as_bytes())
                .into_bytes(),
        },
        ExportedObject {
            key: summary_key,
            body: summary.into_bytes(),
        },
    ]
}

/// The data file an export writes: one line per item, each the item as DynamoDB JSON
/// under the `Item` member, GZIP-compressed — the `.json.gz` object a `DYNAMODB_JSON`
/// export leaves in the bucket.
fn data_file_body(items: &[Item]) -> Vec<u8> {
    let mut text = String::new();
    for item in items {
        text.push_str(&::serde_json::json!({ "Item": item_json(item) }).to_string());
        text.push('\n');
    }
    gzipped(text.as_bytes())
}

/// `bytes` GZIP-compressed, which is how an export's data file is stored.
fn gzipped(bytes: &[u8]) -> Vec<u8> {
    use ::std::io::Write;

    let mut encoder =
        ::flate2::write::GzEncoder::new(Vec::new(), ::flate2::Compression::default());
    // Writing into a `Vec` cannot fail for want of room, and `finish` is what yields the
    // compressed bytes; a failure here would mean the compressor itself, so the
    // uncompressed bytes are not substituted for them — the data file is `.gz`.
    if encoder.write_all(bytes).is_err() {
        return Vec::new();
    }
    encoder.finish().unwrap_or_default()
}

/// The manifest of data files: one line per data file, naming the items it holds, its
/// digest, its entity tag and its key — the `manifest-files.json` object a consumer reads
/// to find the data.
fn manifest_files_document(items: usize, data_key: &str, data: &[u8]) -> String {
    let etag = ::service_com_amazonaws_s3_amazons3::state::object_etag(data);
    format!(
        "{}\n",
        ::serde_json::json!({
            "itemCount": items,
            "md5Checksum": ::service_com_amazonaws_s3_amazons3::state::content_md5(data),
            "etag": ::service_com_amazonaws_s3_amazons3::state::unquoted_etag(&etag),
            "dataFileS3Key": data_key,
        })
    )
}

/// The summary manifest: what was exported, from which table and at which point in time,
/// where the data files are listed and how much was written — the
/// `manifest-summary.json` object `ExportDescription$ExportManifest` names.
fn manifest_summary_document(export: &Export, items: usize, files_key: &str) -> String {
    ::serde_json::json!({
        "version": EXPORT_MANIFEST_VERSION,
        "exportArn": export.arn,
        "startTime": rendered_instant(export.started_at),
        "endTime": rendered_instant(export.completes_at),
        "tableArn": table_arn(&export.region, &export.table_name),
        "tableId": export.table_id,
        "exportTime": rendered_instant(export.export_time),
        "s3Bucket": export.request.s3_bucket,
        "s3Prefix": export.request.s3_prefix,
        "s3SseAlgorithm": export.request.s3_sse_algorithm,
        "s3SseKmsKeyId": export.request.s3_sse_kms_key_id,
        "manifestFilesS3Key": files_key,
        "billedSizeBytes": export.billed_size_bytes,
        "itemCount": items,
        "outputFormat": export.request.export_format,
    })
    .to_string()
}

/// One instant as the manifest carries it: the date-time form an AWS timestamp takes in a
/// document (`2020-11-04T07:28:34.028Z`).
fn rendered_instant(instant: SystemTime) -> String {
    super::reported_date(instant)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// One item as DynamoDB JSON: each attribute under its name, in name order so one item
/// always renders the same way whatever order the stored map iterates in.
pub fn item_json(item: &Item) -> ::serde_json::Value {
    let rendered: BTreeMap<&str, ::serde_json::Value> = item
        .iter()
        .map(|(name, value)| (name.as_str(), attribute_value_json(value)))
        .collect();
    ::serde_json::json!(rendered)
}

/// One attribute value as the JSON DynamoDB carries one in: a single-member object whose
/// name is the attribute's TYPE (`S`, `N`, `B`, `SS`, `NS`, `BS`, `M`, `L`, `NULL`,
/// `BOOL`) — the same form a `PutItem` request writes it in, so the exported data reads
/// back as the item was written.
///
/// Recursive over the nesting types, which is bounded: a stored attribute is at most
/// [`MAX_ATTRIBUTE_DEPTH`] levels deep, because a write deeper than that is refused
/// before it is stored ([`attribute_depth_exceeds`]).
pub fn attribute_value_json(value: &AttributeValue) -> ::serde_json::Value {
    /// One binary attribute's bytes as JSON carries them: base64, the encoding the wire
    /// form of a blob already uses.
    fn encoded(bytes: &[u8]) -> ::serde_json::Value {
        ::serde_json::Value::String(::aws_smithy_types::base64::encode(bytes))
    }

    match value {
        AttributeValue::S(text) => ::serde_json::json!({ "S": text }),
        AttributeValue::N(number) => ::serde_json::json!({ "N": number }),
        AttributeValue::B(bytes) => ::serde_json::json!({ "B": encoded(bytes) }),
        AttributeValue::Ss(members) => ::serde_json::json!({ "SS": members }),
        AttributeValue::Ns(members) => ::serde_json::json!({ "NS": members }),
        AttributeValue::Bs(members) => ::serde_json::json!({
            "BS": members.iter().map(|bytes| encoded(bytes)).collect::<Vec<_>>(),
        }),
        AttributeValue::M(members) => ::serde_json::json!({ "M": item_json(members) }),
        AttributeValue::L(members) => ::serde_json::json!({
            "L": members.iter().map(attribute_value_json).collect::<Vec<_>>(),
        }),
        AttributeValue::Null(is_null) => ::serde_json::json!({ "NULL": is_null }),
        AttributeValue::Bool(flag) => ::serde_json::json!({ "BOOL": flag }),
    }
}

/// Record `objects` in the destination bucket through Amazon S3's own store, answering
/// what that store took — or `None` where it holds no bucket under that name, which is
/// the destination an export cannot write to at all.
///
/// The object store is reached on the SAME engine this service is mounted on, so what
/// this writes is what that service's `ListObjectsV2` and `GetObject` then serve. A store
/// that refuses a write — its own object or byte ceiling — leaves that object unwritten
/// and its bytes out of the answer, so an export reports what was really stored rather
/// than claiming the whole of it.
pub fn write_exported_objects<Eng: Send + Sync + 'static>(
    engine: &Eng,
    bucket: &str,
    objects: &[ExportedObject],
) -> Option<ExportWrite> {
    let state = ::service_com_amazonaws_s3_amazons3::state::service_state(engine)?;
    let mut store = state.lock();
    // A destination the store holds no bucket under is one nothing can be written to, and
    // that is what the `None` here says to the caller.
    store.bucket(bucket)?;
    let mut written = ExportWrite::default();
    for object in objects {
        let encryption = store
            .bucket(bucket)
            .map(::service_com_amazonaws_s3_amazons3::state::Bucket::default_object_encryption)
            .unwrap_or_default();
        let stored = ::service_com_amazonaws_s3_amazons3::state::Object {
            // The entity tag is computed over the bytes stored, exactly as it is for an
            // object written through `PutObject`.
            etag: ::service_com_amazonaws_s3_amazons3::state::object_etag(&object.body),
            body: object.body.clone(),
            version_id: None,
            last_modified: SystemTime::now(),
            storage_class: ::service_com_amazonaws_s3_amazons3::state::DEFAULT_STORAGE_CLASS
                .to_string(),
            content_type: None,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            expires: None,
            website_redirect_location: None,
            metadata: ::std::collections::BTreeMap::new(),
            tags: Vec::new(),
            encryption,
            // An exported object carries no Object Lock retention, legal hold, restore or
            // annotation: each of those is placed on a stored version afterwards, by
            // Amazon S3's own operation for it.
            retention: None,
            legal_hold: None,
            restore: None,
            annotations: ::std::collections::BTreeMap::new(),
            // And no ACL of its own: it carries the one every object starts with, the
            // owner's `FULL_CONTROL`, which a reader derives.
            acl: None,
            // And no checksum: the export names no checksum algorithm, so a read of the
            // object reports none.
            checksum: None,
            // It is written whole rather than assembled from parts.
            parts: Vec::new(),
            // And nothing replicated it: S3's own store settles a replication record when
            // it records a write through its own data plane.
            replication: None,
        };
        if matches!(
            store.put_object(bucket, &object.key, stored),
            Some(
                ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Created
                    | ::service_com_amazonaws_s3_amazons3::state::ObjectWrite::Replaced
            )
        ) {
            written.objects += 1;
            written.bytes += object.body.len();
        }
    }
    Some(written)
}
