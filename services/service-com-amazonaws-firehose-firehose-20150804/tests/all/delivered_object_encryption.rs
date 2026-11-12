// SPDX-License-Identifier: CC0-1.0

//! The encryption at rest a DELIVERED object is stored under.
//!
//! The rule under test: an object a stream's buffer is delivered as is stored encrypted
//! under the key its destination's `EncryptionConfiguration` names — the `AWSKMSKeyARN` of
//! its `KMSEncryptionConfig`, "the encryption key" (`KMSEncryptionConfig`) — so a read of
//! that object reports server-side encryption with KMS and that very key. A destination
//! that names no key (one overriding encryption with `NoEncryptionConfig`, and one naming
//! no `EncryptionConfiguration` at all, whose documented default is that same no
//! encryption) delivers an object that asks for no encryption of its own, so the
//! destination bucket's own default encryption configuration decides what it is stored
//! under, exactly as for any other write to that bucket.
//!
//! The cases differ along what the rule generalises over: which destination member carries
//! the Amazon S3 location (a plain destination and an extended one), which of the
//! `EncryptionConfiguration` members the destination names, the FORM of the key it names
//! (a key ARN and an alias ARN) and its value, the bucket's own default encryption
//! configuration (none, SSE-KMS under a key of its own), and the `CompressionFormat` the
//! body is encoded with — the encryption of an object is not the encoding of its bytes.
//!
//! Every read is the one a client makes: `HEAD /<bucket>/<key>` for the encryption the
//! object was stored under, and `GET /<bucket>/<key>` for its bytes.

use std::io::Read;

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::ROLE_ARN;
use crate::destination_delivery::Emulator;

/// The header reporting the server-side encryption algorithm an object is stored under
/// (`GetObjectOutput$ServerSideEncryption`).
const ALGORITHM_HEADER: &str = "x-amz-server-side-encryption";

/// The header reporting the KMS key an object stored with SSE-KMS is encrypted with
/// (`GetObjectOutput$SSEKMSKeyId`).
const KEY_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// Server-side encryption with KMS keys, the `ServerSideEncryption` value an object
/// encrypted under a named KMS key is stored with.
const SSE_KMS: &str = "aws:kms";

/// Server-side encryption with Amazon S3 managed keys, what a bucket no
/// `PutBucketEncryption` ever configured encrypts a write naming no encryption with.
const SSE_S3: &str = "AES256";

/// A customer-managed key, named as a key ARN — the first form
/// `KMSEncryptionConfig$AWSKMSKeyARN`'s pattern admits
/// (`^arn:.*:kms:[a-zA-Z0-9\-]+:\d{12}:(key|alias)/[a-zA-Z_0-9+=,.@\-_/]+$`,
/// `smithy_query(query="shape",shape="KMSEncryptionConfig")`).
const KEY_ARN: &str = "arn:aws:kms:us-west-2:000000000000:key/2f1c4e0e-9a6b-4a2b-9c9c-5b9f0d7a1e11";

/// Another customer-managed key ARN, so a case that names it reports it and not the one
/// another case named.
const OTHER_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/7d3b6a55-1f44-4e37-8d0f-2c0a9b8e5c22";

/// A customer-managed key named as an ALIAS ARN — the second form that pattern admits.
const KEY_ALIAS_ARN: &str = "arn:aws:kms:us-west-2:000000000000:alias/firehose-delivery";

/// The key a BUCKET's own default encryption configuration names, which is what a write
/// naming no encryption of its own is stored under.
const BUCKET_DEFAULT_KEY_ARN: &str =
    "arn:aws:kms:us-west-2:000000000000:key/b1a9f6c2-3d55-4f88-9e11-04c7a2d6f300";

/// The XML namespace a client's writer puts on an Amazon S3 request payload.
const XMLNS: &str = "http://s3.amazonaws.com/doc/2006-03-01/";

/// One case: a destination configuration, the destination bucket's own default encryption,
/// and the encryption a read of the delivered object must report.
struct Row {
    case: &'static str,
    /// Whether the records are accepted through `PutRecordBatch` — two of them — rather
    /// than through `PutRecord`. Both accept into the one buffer the delivery is of.
    via_batch: bool,
    /// The `CreateDeliveryStream` member the Amazon S3 location is carried in.
    member: &'static str,
    /// The `EncryptionConfiguration` the destination names, or `None` when it names none.
    encryption: Option<Value>,
    /// The `CompressionFormat` the destination names, or `None` for the documented default
    /// (`UNCOMPRESSED`), where the delivered bytes are the record's own.
    compression: Option<&'static str>,
    /// The bucket's own default encryption configuration: an `SSEAlgorithm` and the
    /// `KMSMasterKeyID` qualifying it, or `None` to leave the bucket as created.
    bucket_default: Option<(&'static str, Option<&'static str>)>,
    /// The `x-amz-server-side-encryption` the delivered object reports.
    algorithm: &'static str,
    /// The `x-amz-server-side-encryption-aws-kms-key-id` it reports, or `None` when it
    /// reports none.
    key: Option<&'static str>,
}

/// The `EncryptionConfiguration` naming `key` as the destination's encryption key.
fn kms_encryption(key: &str) -> Value {
    json!({ "KMSEncryptionConfig": { "AWSKMSKeyARN": key } })
}

/// The cases.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an extended destination naming a key ARN",
            via_batch: false,
            member: "ExtendedS3DestinationConfiguration",
            encryption: Some(kms_encryption(KEY_ARN)),
            compression: None,
            bucket_default: None,
            algorithm: SSE_KMS,
            key: Some(KEY_ARN),
        },
        Row {
            case: "an extended destination naming an alias ARN",
            via_batch: true,
            member: "ExtendedS3DestinationConfiguration",
            encryption: Some(kms_encryption(KEY_ALIAS_ARN)),
            compression: None,
            bucket_default: None,
            algorithm: SSE_KMS,
            key: Some(KEY_ALIAS_ARN),
        },
        Row {
            case: "a plain destination naming a key ARN",
            via_batch: true,
            member: "S3DestinationConfiguration",
            encryption: Some(kms_encryption(OTHER_KEY_ARN)),
            compression: None,
            bucket_default: None,
            algorithm: SSE_KMS,
            key: Some(OTHER_KEY_ARN),
        },
        Row {
            case: "an extended destination overriding encryption with NoEncryption",
            via_batch: false,
            member: "ExtendedS3DestinationConfiguration",
            encryption: Some(json!({ "NoEncryptionConfig": "NoEncryption" })),
            compression: None,
            bucket_default: None,
            algorithm: SSE_S3,
            key: None,
        },
        Row {
            case: "an extended destination naming no encryption configuration",
            via_batch: true,
            member: "ExtendedS3DestinationConfiguration",
            encryption: None,
            compression: None,
            bucket_default: None,
            algorithm: SSE_S3,
            key: None,
        },
        Row {
            case: "a plain destination naming no encryption configuration",
            via_batch: false,
            member: "S3DestinationConfiguration",
            encryption: None,
            compression: None,
            bucket_default: None,
            algorithm: SSE_S3,
            key: None,
        },
        Row {
            case: "a destination naming no key, into a bucket whose default is SSE-KMS",
            via_batch: true,
            member: "ExtendedS3DestinationConfiguration",
            encryption: Some(json!({ "NoEncryptionConfig": "NoEncryption" })),
            compression: None,
            bucket_default: Some((SSE_KMS, Some(BUCKET_DEFAULT_KEY_ARN))),
            algorithm: SSE_KMS,
            key: Some(BUCKET_DEFAULT_KEY_ARN),
        },
        Row {
            case: "a destination naming a key, into a bucket whose default names another",
            via_batch: false,
            member: "ExtendedS3DestinationConfiguration",
            encryption: Some(kms_encryption(KEY_ARN)),
            compression: None,
            bucket_default: Some((SSE_KMS, Some(BUCKET_DEFAULT_KEY_ARN))),
            algorithm: SSE_KMS,
            key: Some(KEY_ARN),
        },
        Row {
            case: "a destination naming a key and GZIP compression",
            via_batch: false,
            member: "ExtendedS3DestinationConfiguration",
            encryption: Some(kms_encryption(KEY_ARN)),
            compression: Some("GZIP"),
            bucket_default: None,
            algorithm: SSE_KMS,
            key: Some(KEY_ARN),
        },
    ]
}

/// A record a put accepted is delivered as an object encrypted under the key its
/// destination's `EncryptionConfiguration` names, and under the destination bucket's own
/// default encryption when it names none.
#[tokio::test]
async fn a_delivered_object_is_stored_under_the_encryption_its_destination_configures() {
    let emulator = Emulator::new();
    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let bucket = format!("delivery-encryption-{index}");
        let stream = format!("delivery-encryption-stream-{index}");
        let marker = format!("encrypted-record-{index}");
        // What this case puts: two records through `PutRecordBatch`, or one through
        // `PutRecord`. A delivery holds the buffer's records in arrival order, so what the
        // delivered object must hold is their data concatenated.
        let records: Vec<Vec<u8>> = if row.via_batch {
            vec![
                format!("{marker}-a").into_bytes(),
                format!("{marker}-b").into_bytes(),
            ]
        } else {
            vec![marker.clone().into_bytes()]
        };
        let delivered = records.concat();

        emulator.create_bucket(&bucket).await;
        if let Some((algorithm, key)) = row.bucket_default {
            put_bucket_encryption(&emulator, &bucket, algorithm, key).await;
        }

        let mut config = json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": format!("arn:aws:s3:::{bucket}"),
            // The shortest wait `IntervalInSeconds` admits (its published range is 0..900):
            // these cases are about the key a delivered object is encrypted under, not
            // about how long its buffer is held.
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        });
        if let Some(encryption) = &row.encryption {
            config["EncryptionConfiguration"] = encryption.clone();
        }
        if let Some(compression) = row.compression {
            config["CompressionFormat"] = json!(compression);
        }
        emulator
            .create_active_stream(&stream, row.member, config)
            .await;

        let (status, answered) = if row.via_batch {
            emulator.put_batch(&stream, &records).await
        } else {
            emulator
                .put_record(&stream, records.concat().as_slice())
                .await
        };
        assert_eq!(status, 200, "case {case:?}: the put must succeed, got {answered:?}");

        let keys = emulator.delivered_keys(case, &bucket, 1).await;
        let key = keys.first().unwrap_or_else(|| {
            panic!("case {case:?}: expected a delivered object, got {keys:?}");
        });

        let read = emulator.s3("HEAD", &format!("/{bucket}/{key}")).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: reading {key} must succeed, got status {}",
            read.status
        );
        let algorithm = read.header(ALGORITHM_HEADER);
        assert_eq!(
            algorithm.as_deref(),
            Some(row.algorithm),
            "case {case:?}: expected {ALGORITHM_HEADER} {:?}, got {algorithm:?}",
            row.algorithm
        );
        let reported_key = read.header(KEY_HEADER);
        assert_eq!(
            reported_key.as_deref(),
            row.key,
            "case {case:?}: expected {KEY_HEADER} {:?}, got {reported_key:?}",
            row.key
        );

        // The delivery is still a delivery of the records' own bytes: an encryption
        // configuration decides what the object is stored UNDER, not what it holds.
        let body = emulator.object(&bucket, key).await;
        let held = match row.compression {
            // `GZIP`, so what the object holds is the records' bytes gzipped: read back
            // through the same maintained decoder the rest of this crate encodes with.
            Some("GZIP") => {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(body.as_slice())
                    .read_to_end(&mut decoded)
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: the delivered object must be a gzip stream: {error}")
                    });
                decoded
            }
            Some(format) => panic!("case {case:?}: no decoder for {format}"),
            None => body,
        };
        assert_eq!(
            held,
            delivered,
            "case {case:?}: expected the delivered object to hold {:?}, got {:?}",
            String::from_utf8_lossy(&delivered),
            String::from_utf8_lossy(&held)
        );
    }
}

/// `PUT /<bucket>?encryption` — Amazon S3's `PutBucketEncryption`, giving `bucket` one
/// default encryption rule.
async fn put_bucket_encryption(
    emulator: &Emulator,
    bucket: &str,
    algorithm: &str,
    key: Option<&str>,
) {
    let mut payload = format!(
        "<ServerSideEncryptionConfiguration xmlns=\"{XMLNS}\"><Rule>\
         <ApplyServerSideEncryptionByDefault><SSEAlgorithm>{algorithm}</SSEAlgorithm>"
    );
    if let Some(key) = key {
        payload.push_str(&format!("<KMSMasterKeyID>{key}</KMSMasterKeyID>"));
    }
    payload.push_str(
        "</ApplyServerSideEncryptionByDefault></Rule></ServerSideEncryptionConfiguration>",
    );
    let answer = emulator
        .signed("s3", "PUT", &format!("/{bucket}?encryption"), payload.as_bytes())
        .await;
    assert_eq!(
        answer.status,
        200,
        "default encryption on {bucket}: got {}",
        String::from_utf8_lossy(&answer.body)
    );
}
