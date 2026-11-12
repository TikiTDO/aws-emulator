// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Data Catalog's EXPORT configuration:
//! `PutDataCatalogExportConfiguration` and `GetDataCatalogExportConfiguration`.
//!
//! The rules under test:
//!
//! * The export configuration is ONE record per catalog that a write replaces whole and a
//!   read reports, and a catalog no write has configured reports the export as off
//!   (`DISABLED`, status `DISABLED`) with no instants rather than missing an entity. A read
//!   reports: it never changes what a later read sees.
//! * A write that CHANGES the setting leaves the catalog in that direction's transitional
//!   status (`ENABLING`/`DISABLING`) until the clock reaches the due instant the write
//!   stored, and the settled status (`ENABLED`/`DISABLED`) from it. The due instant is
//!   stored, so nothing sleeps for the span.
//! * The metadata is exported into ONE table bucket per account and Region, which S3 TABLES
//!   owns: enabling the export brings that bucket into existence there, the read names it
//!   only while that service holds it, and a write naming an encryption the bucket already
//!   there does not use is refused with `ConflictException` (HTTP 400) — the bucket's
//!   encryption is the bucket's own.
//! * A retry carrying the `ClientToken` of the write already stored is that write, not a
//!   second one.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: which
//! published `ExportSetting` the write asks for, which published `SseAlgorithm` it names and
//! whether a key comes with it, which side of the due instant the read lands on, how the
//! bucket already there came to exist, and whether a client token is carried.
//!
//! Every Glue call goes over the wire through the shared [`crate::wire`] harness, and every
//! S3 Tables call over that service's own restJson1 route, so what is asserted is HTTP
//! behaviour and not a direct call into a handler.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, EXPORT_CONFIGURATION_SETTLES_AFTER, EXPORT_TABLE_BUCKET_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, error_code, error_message, ok, ACCOUNT_ID, REGION};

/// The published values of the modelled `ExportSetting` enum, which are the only values
/// `PutDataCatalogExportConfiguration$ExportSetting` accepts.
///
/// Source: `smithy_query(query="shape", shape="ExportSetting")` on this crate's model, whose
/// `values` are exactly these two.
const PUBLISHED_EXPORT_SETTINGS: [&str; 2] = ["ENABLED", "DISABLED"];

/// The published values of the modelled `ExportStatus` enum: the statuses a read may report.
///
/// Source: `smithy_query(query="shape", shape="ExportStatus")`.
const PUBLISHED_EXPORT_STATUSES: [&str; 5] =
    ["ENABLING", "ENABLED", "DISABLING", "DISABLED", "FAILED"];

/// The server-side encryption algorithms an export configuration may name.
///
/// Source: the documentation of `ExportEncryptionConfiguration$SseAlgorithm` — "Valid values
/// are `AES256` and `aws:kms`".
const PUBLISHED_SSE_ALGORITHMS: [&str; 2] = ["AES256", "aws:kms"];

/// Two KMS key ARNs of the form the modelled `ExportEncryptionConfiguration$KmsKeyArn`
/// pattern accepts (`^arn:aws[-a-z0-9]*:kms:[-a-z0-9]*:[0-9]{12}:key/.+$`), differing in key
/// id so a key a refusal or a read turns on cannot be a constant.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/1234abcd-12ab-34cd-56ef-1234567890ab";
const OTHER_KMS_KEY_ARN: &str =
    "arn:aws:kms:us-east-1:000000000000:key/fedcba98-7654-3210-fedc-ba9876543210";

/// The whole `Message` a write disagreeing with the table bucket already there is refused
/// with — written out here rather than read from the crate's own builder, so this suite pins
/// the wording and the JSON document it travels in.
const BUCKET_ENCRYPTION_CONFLICT: &str = "{\"Message\":\"The existing table bucket aws-catalog \
                                          uses a different encryption configuration than the \
                                          request.\"}";

/// An emulator with Glue AND S3 Tables mounted on one shared engine, plus that engine: the
/// table bucket the export writes into is an S3 Tables resource, so that service has to be
/// mounted for the account to hold one.
fn emulator_with_s3_tables() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_s3tables_s3tablebuckets::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// The ARN S3 Tables holds a table bucket of `name` under, in [`REGION`].
fn bucket_arn(name: &str) -> String {
    format!("arn:aws:s3tables:{REGION}:{ACCOUNT_ID}:bucket/{name}")
}

/// An `@httpLabel` is ONE path segment, so an ARN's `:` and `/` travel percent-encoded or the
/// request addresses a different route.
fn encode(value: &str) -> String {
    value.replace(':', "%3A").replace('/', "%2F")
}

/// Make one restJson1 call to the S3 TABLES service mounted beside Glue, and return its HTTP
/// status and parsed body. The credential scope names that service's signing name, which is
/// what routes the request to it.
async fn s3tables_call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| value.to_string().into_bytes());
    let authorization = format!(
        "AWS4-HMAC-SHA256 Credential=AKIDGLUETEST/20260101/{REGION}/s3tables/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    );
    let request = smithy_server_runtime::http::Request::builder()
        .method(method)
        .uri(format!("https://s3tables.{REGION}.amazonaws.com{path}"))
        .header("authorization", authorization)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("build the request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// `CreateTableBucket` through S3 Tables' own API: how a CALLER puts a table bucket of `name`
/// in the account, under `encryption` when it names one.
async fn create_table_bucket(
    router: &MultiServiceRouter,
    name: &str,
    encryption: Option<&Value>,
) -> (u16, Value) {
    let mut request = Map::new();
    request.insert("name".to_string(), json!(name));
    if let Some(encryption) = encryption {
        request.insert("encryptionConfiguration".to_string(), encryption.clone());
    }
    s3tables_call(router, "PUT", "/buckets", Some(&Value::Object(request))).await
}

/// `GetTableBucket` through S3 Tables' own API: the read that says whether that service holds
/// the bucket `arn` names.
async fn get_table_bucket(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    s3tables_call(router, "GET", &format!("/buckets/{}", encode(arn)), None).await
}

/// `GetTableBucketEncryption` through S3 Tables' own API.
async fn get_table_bucket_encryption(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    s3tables_call(
        router,
        "GET",
        &format!("/buckets/{}/encryption", encode(arn)),
        None,
    )
    .await
}

/// The `PutDataCatalogExportConfiguration` body for one row: the setting, and the encryption
/// configuration and client token where the row carries them.
fn put_body(setting: &str, encryption: Option<&Value>, client_token: Option<&str>) -> Value {
    let mut request = Map::new();
    request.insert("ExportSetting".to_string(), json!(setting));
    if let Some(encryption) = encryption {
        request.insert("EncryptionConfiguration".to_string(), encryption.clone());
    }
    if let Some(token) = client_token {
        request.insert("ClientToken".to_string(), json!(token));
    }
    Value::Object(request)
}

/// An `ExportEncryptionConfiguration` naming `algorithm`, and `key` where there is one.
fn encryption(algorithm: &str, key: Option<&str>) -> Value {
    let mut configuration = Map::new();
    configuration.insert("SseAlgorithm".to_string(), json!(algorithm));
    if let Some(key) = key {
        configuration.insert("KmsKeyArn".to_string(), json!(key));
    }
    Value::Object(configuration)
}

/// The export configuration a read reports.
async fn get_configuration(router: &MultiServiceRouter) -> Value {
    ok(router, "GetDataCatalogExportConfiguration", &json!({})).await
}

/// Pull the stored configuration's instants back by `elapsed`, so a suite can observe the
/// status the clock owes it without sleeping for the span.
fn rewind(engine: &Arc<engine::InMemoryEngine>, elapsed: Duration) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let configuration = store
        .catalogs
        .get_mut(ACCOUNT_ID)
        .and_then(|catalog| catalog.export_configuration.as_mut())
        .expect("the catalog holds an export configuration");
    let back = |at: SystemTime| at.checked_sub(elapsed).expect("an instant in range");
    configuration.created_at = back(configuration.created_at);
    configuration.updated_at = back(configuration.updated_at);
    configuration.settled_at = back(configuration.settled_at);
}

/// A catalog no write has configured reports the export as off, and the read reports rather
/// than writes: repeated reads answer the same thing and leave the next read alone.
#[tokio::test]
async fn an_unconfigured_catalog_reports_the_export_as_off() {
    let (router, _engine) = emulator_with_s3_tables();

    let expected = json!({ "ExportSetting": "DISABLED", "Status": "DISABLED" });
    for case in ["the first read", "the second read", "the third read"] {
        let reported = get_configuration(&router).await;
        assert_eq!(
            reported, expected,
            "case {case}: expected {expected}, got {reported}"
        );
    }

    // Nothing was configured, so nothing created the export's table bucket either: the
    // service that owns table buckets holds none.
    let (status, body) = get_table_bucket(&router, &bucket_arn(EXPORT_TABLE_BUCKET_NAME)).await;
    assert_eq!(
        status, 404,
        "case an unconfigured catalog: S3 Tables must hold no export bucket, got {status} with \
         {body}"
    );
}

/// One row of the round-trip table: the case name, the setting written, the encryption
/// configuration written, and the status the read must report straight after the write.
struct RoundTrip {
    case: &'static str,
    setting: &'static str,
    encryption: Option<Value>,
    status: &'static str,
}

/// The rows the round-trip rule is asserted over: both published settings, with no encryption
/// configuration, with each published algorithm, and with a key beside the keyed one.
fn round_trip_rows() -> Vec<RoundTrip> {
    vec![
        RoundTrip {
            case: "enabled with no encryption configuration",
            setting: "ENABLED",
            encryption: None,
            status: "ENABLING",
        },
        RoundTrip {
            case: "enabled under S3-managed keys",
            setting: "ENABLED",
            encryption: Some(encryption("AES256", None)),
            status: "ENABLING",
        },
        RoundTrip {
            case: "enabled under a KMS key",
            setting: "ENABLED",
            encryption: Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            status: "ENABLING",
        },
        RoundTrip {
            case: "disabled with no encryption configuration",
            setting: "DISABLED",
            encryption: None,
            // The catalog was not exporting, so a write asking for the export to be off
            // changes nothing and its status is settled from the start.
            status: "DISABLED",
        },
        RoundTrip {
            case: "disabled under a KMS key",
            setting: "DISABLED",
            encryption: Some(encryption("aws:kms", Some(OTHER_KMS_KEY_ARN))),
            status: "DISABLED",
        },
    ]
}

/// What the write persists is what the read reports: the setting and the encryption
/// configuration come back unchanged, beside the status the write left the catalog in and the
/// instants of the write itself.
#[tokio::test]
async fn a_written_export_configuration_is_reported_back_by_the_read() {
    for row in round_trip_rows() {
        let (router, _engine) = emulator_with_s3_tables();
        let request = put_body(row.setting, row.encryption.as_ref(), None);

        let written = ok(&router, "PutDataCatalogExportConfiguration", &request).await;
        assert_eq!(
            written["ExportSetting"],
            json!(row.setting),
            "case {case}: the write must report the setting it stored, got {written}",
            case = row.case
        );
        assert_eq!(
            written.get("EncryptionConfiguration").cloned(),
            row.encryption,
            "case {case}: the write must report the encryption configuration it stored, got \
             {written}",
            case = row.case
        );

        let reported = get_configuration(&router).await;
        assert_eq!(
            reported["ExportSetting"],
            json!(row.setting),
            "case {case}: expected ExportSetting {setting}, got {reported}",
            case = row.case,
            setting = row.setting
        );
        assert_eq!(
            reported["Status"],
            json!(row.status),
            "case {case}: expected Status {status}, got {reported}",
            case = row.case,
            status = row.status
        );
        assert_eq!(
            reported.get("EncryptionConfiguration").cloned(),
            row.encryption,
            "case {case}: expected EncryptionConfiguration {expected:?}, got {reported}",
            case = row.case,
            expected = row.encryption
        );
        let created_at = reported.get("CreatedAt").cloned().unwrap_or(Value::Null);
        let updated_at = reported.get("UpdatedAt").cloned().unwrap_or(Value::Null);
        assert!(
            created_at.is_number() && updated_at.is_number(),
            "case {case}: a written configuration carries the instants of its writes, got \
             {reported}",
            case = row.case
        );

        // Stored, not computed per call: a second read answers the same record.
        let again = get_configuration(&router).await;
        assert_eq!(
            again, reported,
            "case {case}: two reads of one configuration must agree; got {reported} then {again}",
            case = row.case
        );
    }
}

/// Every published `ExportSetting` is covered by the round-trip table, and every status it
/// reports is one the model publishes.
#[tokio::test]
async fn the_table_spans_every_published_export_setting() {
    for setting in PUBLISHED_EXPORT_SETTINGS {
        assert!(
            round_trip_rows().iter().any(|row| row.setting == setting),
            "case {setting}: no row of the round-trip table writes this published setting"
        );
    }
    for row in round_trip_rows() {
        assert!(
            PUBLISHED_EXPORT_STATUSES.contains(&row.status),
            "case {case}: {status} is not a published ExportStatus",
            case = row.case,
            status = row.status
        );
    }
}

/// A write that changes the setting leaves the catalog in that direction's transitional
/// status until the clock reaches the due instant the write stored, and the settled status
/// from it.
///
/// The rows differ along which side of the due instant the read lands on, including the
/// instant itself and one step short of it.
#[tokio::test]
async fn the_transitional_status_settles_when_the_clock_reaches_the_due_instant() {
    // (case, how far the stored instants are pulled back, the status the read must report)
    let cases: Vec<(&str, Duration, &str)> = vec![
        ("no time elapsed", Duration::ZERO, "ENABLING"),
        (
            "halfway to the due instant",
            EXPORT_CONFIGURATION_SETTLES_AFTER / 2,
            "ENABLING",
        ),
        (
            "one millisecond short of the due instant",
            EXPORT_CONFIGURATION_SETTLES_AFTER - Duration::from_millis(1),
            "ENABLING",
        ),
        (
            "exactly at the due instant",
            EXPORT_CONFIGURATION_SETTLES_AFTER,
            "ENABLED",
        ),
        (
            "well past the due instant",
            EXPORT_CONFIGURATION_SETTLES_AFTER * 4,
            "ENABLED",
        ),
    ];

    for (case, elapsed, expected) in cases {
        let (router, engine) = emulator_with_s3_tables();
        ok(
            &router,
            "PutDataCatalogExportConfiguration",
            &put_body("ENABLED", Some(&encryption("AES256", None)), None),
        )
        .await;
        rewind(&engine, elapsed);

        let reported = get_configuration(&router).await;
        assert_eq!(
            reported["Status"],
            json!(expected),
            "case {case}: expected Status {expected}, got {reported}"
        );

        // The status is derived from the stored instant, so reading it twice reports the same
        // thing and moves nothing.
        let again = get_configuration(&router).await;
        assert_eq!(
            again["Status"],
            json!(expected),
            "case {case}: a read must not move the due instant, got {again}"
        );
    }

    // Turning the export back off starts the transition in the other direction, from the
    // settled status the clock had already reached.
    let (router, engine) = emulator_with_s3_tables();
    ok(
        &router,
        "PutDataCatalogExportConfiguration",
        &put_body("ENABLED", None, None),
    )
    .await;
    rewind(&engine, EXPORT_CONFIGURATION_SETTLES_AFTER);
    ok(
        &router,
        "PutDataCatalogExportConfiguration",
        &put_body("DISABLED", None, None),
    )
    .await;

    let disabling = get_configuration(&router).await;
    assert_eq!(
        disabling["Status"],
        json!("DISABLING"),
        "case turning the export off: expected Status DISABLING, got {disabling}"
    );
    rewind(&engine, EXPORT_CONFIGURATION_SETTLES_AFTER);
    let disabled = get_configuration(&router).await;
    assert_eq!(
        disabled["Status"],
        json!("DISABLED"),
        "case turning the export off: expected Status DISABLED once due, got {disabled}"
    );
}

/// One row of the bucket table: what the write asks for, and what the service that OWNS
/// table buckets must hold afterwards — the algorithm the bucket is protected by and the key
/// it carries, or no bucket at all.
struct BucketCase {
    case: &'static str,
    setting: &'static str,
    encryption: Option<Value>,
    bucket_algorithm: Option<&'static str>,
    bucket_key: Option<&'static str>,
}

/// Enabling the export brings the table bucket it writes into existence IN S3 TABLES, under
/// the encryption the write asked for, and the read names that bucket — while a write that
/// asks for the export to be off creates none, so the read names none.
#[tokio::test]
async fn enabling_the_export_creates_the_table_bucket_in_the_service_that_owns_it() {
    let cases = vec![
        BucketCase {
            case: "enabled with no encryption configuration",
            setting: "ENABLED",
            encryption: None,
            // The bucket takes no configuration of its own, so its owning service reports
            // its own default for it.
            bucket_algorithm: Some("AES256"),
            bucket_key: None,
        },
        BucketCase {
            case: "enabled under S3-managed keys",
            setting: "ENABLED",
            encryption: Some(encryption("AES256", None)),
            bucket_algorithm: Some("AES256"),
            bucket_key: None,
        },
        BucketCase {
            case: "enabled under a KMS key",
            setting: "ENABLED",
            encryption: Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            bucket_algorithm: Some("aws:kms"),
            bucket_key: Some(KMS_KEY_ARN),
        },
        BucketCase {
            case: "disabled",
            setting: "DISABLED",
            encryption: None,
            bucket_algorithm: None,
            bucket_key: None,
        },
    ];

    let arn = bucket_arn(EXPORT_TABLE_BUCKET_NAME);
    for BucketCase {
        case,
        setting,
        encryption: configuration,
        bucket_algorithm,
        bucket_key,
    } in cases
    {
        let expected = bucket_algorithm.map(|algorithm| (algorithm, bucket_key));
        let (router, _engine) = emulator_with_s3_tables();
        ok(
            &router,
            "PutDataCatalogExportConfiguration",
            &put_body(setting, configuration.as_ref(), None),
        )
        .await;

        let reported = get_configuration(&router).await;
        let (status, bucket) = get_table_bucket(&router, &arn).await;
        match expected {
            None => {
                assert_eq!(
                    status, 404,
                    "case {case}: no bucket must be created, got {status} with {bucket}"
                );
                assert!(
                    reported.get("S3TableBucketArn").is_none(),
                    "case {case}: the read must name no bucket while there is none, got \
                     {reported}"
                );
            }
            Some((algorithm, key)) => {
                assert_eq!(
                    status, 200,
                    "case {case}: the owning service must hold the bucket, got {status} with \
                     {bucket}"
                );
                assert_eq!(
                    bucket["name"],
                    json!(EXPORT_TABLE_BUCKET_NAME),
                    "case {case}: expected the export's bucket, got {bucket}"
                );
                assert_eq!(
                    bucket["type"],
                    json!("aws"),
                    "case {case}: no caller asked for this bucket, so it is one Amazon Web \
                     Services made on the account's behalf, got {bucket}"
                );
                assert_eq!(
                    reported["S3TableBucketArn"],
                    json!(arn),
                    "case {case}: expected the read to name {arn}, got {reported}"
                );

                let (encryption_status, encryption_body) =
                    get_table_bucket_encryption(&router, &arn).await;
                assert_eq!(
                    encryption_status, 200,
                    "case {case}: the bucket's encryption must read back, got \
                     {encryption_status} with {encryption_body}"
                );
                let configuration = &encryption_body["encryptionConfiguration"];
                assert_eq!(
                    configuration["sseAlgorithm"],
                    json!(algorithm),
                    "case {case}: expected sseAlgorithm {algorithm}, got {encryption_body}"
                );
                assert_eq!(
                    configuration.get("kmsKeyArn").and_then(Value::as_str),
                    key,
                    "case {case}: expected kmsKeyArn {key:?}, got {encryption_body}"
                );
            }
        }
    }
}

/// A write whose `EncryptionConfiguration` is not the encryption the table bucket already
/// there uses is refused with `ConflictException` (HTTP 400), and stores nothing — whether
/// that bucket was created by an earlier write of this configuration or by a caller of S3
/// Tables itself.
#[tokio::test]
async fn a_write_disagreeing_with_the_existing_table_bucket_is_refused() {
    // (case, the encryption the bucket already there was created with, the encryption the
    //  write names, whether the write is refused)
    let cases: Vec<(&str, Option<Value>, Option<Value>, bool)> = vec![
        (
            "a different algorithm",
            Some(encryption("AES256", None)),
            Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            true,
        ),
        (
            "the same algorithm under a different key",
            Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            Some(encryption("aws:kms", Some(OTHER_KMS_KEY_ARN))),
            true,
        ),
        (
            "a key where the bucket uses S3-managed keys",
            Some(encryption("AES256", None)),
            Some(encryption("AES256", Some(KMS_KEY_ARN))),
            true,
        ),
        (
            "a keyed algorithm against a bucket nothing configured",
            None,
            Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            true,
        ),
        (
            "the encryption the bucket already uses",
            Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            false,
        ),
        (
            "the default a bucket nothing configured carries",
            None,
            Some(encryption("AES256", None)),
            false,
        ),
        (
            // A write that names no encryption asks for nothing, so it disagrees with
            // nothing the bucket holds.
            "no encryption configuration at all",
            Some(encryption("aws:kms", Some(KMS_KEY_ARN))),
            None,
            false,
        ),
    ];

    for (case, bucket_encryption, requested, refused) in cases {
        let (router, _engine) = emulator_with_s3_tables();

        // The bucket already there, created by a CALLER of the service that owns it: the
        // export's own bucket name, with the encryption the row says.
        let bucket_configuration = bucket_encryption.as_ref().map(|configuration| {
            let mut lowered = Map::new();
            lowered.insert(
                "sseAlgorithm".to_string(),
                configuration["SseAlgorithm"].clone(),
            );
            if let Some(key) = configuration.get("KmsKeyArn") {
                lowered.insert("kmsKeyArn".to_string(), key.clone());
            }
            Value::Object(lowered)
        });
        let (created, body) = create_table_bucket(
            &router,
            EXPORT_TABLE_BUCKET_NAME,
            bucket_configuration.as_ref(),
        )
        .await;
        assert_eq!(
            created, 200,
            "case {case}: the bucket already there must be created, got {created} with {body}"
        );

        let (status, body) = call(
            &router,
            "PutDataCatalogExportConfiguration",
            &put_body("ENABLED", requested.as_ref(), None),
        )
        .await;
        if refused {
            assert_eq!(
                status, 400,
                "case {case}: expected the write to be refused with 400, got {status} with {body}"
            );
            assert!(
                error_code(&body).contains("ConflictException"),
                "case {case}: expected ConflictException, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, BUCKET_ENCRYPTION_CONFLICT,
                "case {case}: expected {BUCKET_ENCRYPTION_CONFLICT}, got {message:?}"
            );

            // A refused write stores nothing: the catalog is still unconfigured.
            let reported = get_configuration(&router).await;
            assert_eq!(
                reported["ExportSetting"],
                json!("DISABLED"),
                "case {case}: a refused write must store nothing, got {reported}"
            );
            assert!(
                reported.get("CreatedAt").is_none(),
                "case {case}: a refused write must leave the catalog unconfigured, got {reported}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case}: expected the write to be served, got {status} with {body}"
            );
            let reported = get_configuration(&router).await;
            assert_eq!(
                reported["ExportSetting"],
                json!("ENABLED"),
                "case {case}: the served write must be stored, got {reported}"
            );
            assert_eq!(
                reported.get("EncryptionConfiguration").cloned(),
                requested,
                "case {case}: expected the encryption configuration written, got {reported}"
            );
        }
    }
}

/// A retry carrying the `ClientToken` of the write already stored is that write: it is
/// answered with the stored configuration and leaves the record where it is, while a write
/// under a NEW token is a second write and moves it.
#[tokio::test]
async fn a_retry_under_the_stored_client_token_is_the_write_it_retries() {
    let (router, engine) = emulator_with_s3_tables();
    let token = "export-configuration-token-1";
    let first = ok(
        &router,
        "PutDataCatalogExportConfiguration",
        &put_body("ENABLED", Some(&encryption("AES256", None)), Some(token)),
    )
    .await;

    // Settled, so a second write of the SAME setting would be visible by the instants alone
    // if it were applied.
    rewind(&engine, EXPORT_CONFIGURATION_SETTLES_AFTER);
    let before = get_configuration(&router).await;

    let retry = ok(
        &router,
        "PutDataCatalogExportConfiguration",
        &put_body("ENABLED", Some(&encryption("AES256", None)), Some(token)),
    )
    .await;
    assert_eq!(
        retry, first,
        "case a retry under the stored token: expected {first}, got {retry}"
    );
    let after = get_configuration(&router).await;
    assert_eq!(
        after, before,
        "case a retry under the stored token: the record must not move; got {before} then {after}"
    );

    // A new token is a new write: the record is replaced, so its UpdatedAt moves while its
    // CreatedAt stays where the first write put it.
    let written = ok(
        &router,
        "PutDataCatalogExportConfiguration",
        &put_body(
            "ENABLED",
            Some(&encryption("AES256", None)),
            Some("export-configuration-token-2"),
        ),
    )
    .await;
    assert_eq!(
        written["ExportSetting"],
        json!("ENABLED"),
        "case a write under a new token: expected the setting written, got {written}"
    );
    let replaced = get_configuration(&router).await;
    assert_eq!(
        replaced["CreatedAt"], before["CreatedAt"],
        "case a write under a new token: CreatedAt is the FIRST write's instant, got {replaced}"
    );
    assert_ne!(
        replaced["UpdatedAt"], before["UpdatedAt"],
        "case a write under a new token: UpdatedAt must move, got {replaced}"
    );
}

/// An `SseAlgorithm` outside the published pair names an encryption the service does not
/// perform, so the write is refused with `InvalidInputException` (HTTP 400) and stores
/// nothing; each published value is served.
#[tokio::test]
async fn an_sse_algorithm_outside_the_published_pair_is_refused() {
    for algorithm in PUBLISHED_SSE_ALGORITHMS {
        let (router, _engine) = emulator_with_s3_tables();
        let key = (algorithm == "aws:kms").then_some(KMS_KEY_ARN);
        let written = ok(
            &router,
            "PutDataCatalogExportConfiguration",
            &put_body("ENABLED", Some(&encryption(algorithm, key)), None),
        )
        .await;
        assert_eq!(
            written["EncryptionConfiguration"]["SseAlgorithm"],
            json!(algorithm),
            "case {algorithm}: a published algorithm must be served, got {written}"
        );
    }

    // One step past the published list: a value neither of them, including the same values
    // in another case and the empty string.
    for unpublished in ["AES128", "aws:kms:dsse", "aes256", "AWS:KMS", ""] {
        let (router, _engine) = emulator_with_s3_tables();
        ok(
            &router,
            "PutDataCatalogExportConfiguration",
            &put_body("ENABLED", Some(&encryption("AES256", None)), None),
        )
        .await;

        let (status, body) = call(
            &router,
            "PutDataCatalogExportConfiguration",
            &put_body("DISABLED", Some(&encryption(unpublished, None)), None),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {unpublished:?}: an algorithm outside the published pair must be refused with \
             400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("InvalidInputException"),
            "case {unpublished:?}: expected InvalidInputException, got {body}"
        );
        assert!(
            error_message(&body).contains(unpublished),
            "case {unpublished:?}: the refusal is built from the request, so it must name the \
             value it refused, got {message:?}",
            message = error_message(&body)
        );

        // The refused write stored nothing: the configuration the earlier write left stands.
        let reported = get_configuration(&router).await;
        assert_eq!(
            reported["ExportSetting"],
            json!("ENABLED"),
            "case {unpublished:?}: a refused write must leave the stored configuration alone, \
             got {reported}"
        );
    }
}
