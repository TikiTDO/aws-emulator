// SPDX-License-Identifier: CC0-1.0

//! What a CLIENT of the offline store sees after a write of this data plane, read the way a
//! client reads it: `ListObjectsV2` under the prefix the feature group was configured with,
//! then `HeadObject` on what it lists.
//!
//! The rule under test: every write of this data plane into a feature group the SageMaker
//! control plane gave an `OfflineStoreConfig` APPENDS one non-empty object under the
//! configured `S3StorageConfig.S3Uri` prefix, under the group's own `RoleArn` — an ingestion
//! appends the record ("If a new record's EventTime is greater, the new record is written to
//! both the OnlineStore and OfflineStore. Otherwise, the record is a historic record and it
//! is written only to the OfflineStore", `PutRecord`) and a deletion appends the deleted
//! record marker ("In both cases, Feature Store appends the deleted record marker to the
//! OfflineStore. The deleted record marker is a record with the same RecordIdentifer as the
//! original, but with is_deleted value set to True, EventTime set to the delete input
//! EventTime", `DeleteRecord`) — except the one deletion the model refuses, "HardDelete
//! returns EventTime: 400 ValidationException … No delete record marker is written to the
//! OfflineStore."
//!
//! The four services the rule spans are mounted on ONE shared engine, exactly as the router
//! mounts them: this data plane, the SageMaker control plane that defines the group, IAM
//! which holds the role the write is made under, and Amazon S3 which holds the objects.
//! Every call goes through the real front door.

use std::sync::Arc;

use parquet::file::reader::FileReader;
use quick_xml::events::Event;
use quick_xml::Reader;
use serde_json::{json, Value};
use smithy_server_runtime::{http, MultiServiceRouter};

use service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService;

/// The Region every request here is signed for.
const REGION: &str = "us-east-1";

/// The host the requests name; the emulator serves one endpoint for every service.
const HOST: &str = "s3.us-east-1.amazonaws.com";

/// The group's record-identifier feature.
const IDENTIFIER_FEATURE: &str = "customer_id";

/// The group's event-time feature.
const EVENT_TIME_FEATURE: &str = "event_time";

/// A feature carrying the record's payload, so one object's bytes can be told from another's.
const PAYLOAD_FEATURE: &str = "payload";

/// The name of the role's one inline policy.
const OFFLINE_POLICY: &str = "offline-store-write";

/// The IAM action an object write is authorized under: "you must have the `s3:PutObject`
/// permission" (Amazon S3 `PutObject`).
const WRITE_ACTION: &str = "s3:PutObject";

/// The `is_deleted` column of an offline row, as a Parquet row renders it.
const DELETED_MARKER: &str = "is_deleted: true";

/// The `is_deleted` column of an ingested row.
const NOT_DELETED_MARKER: &str = "is_deleted: false";

/// The event time the seeded record of every case carries.
const SEEDED_EVENT_TIME: &str = "2026-06-01T07:08:09Z";

/// The four services on one shared engine.
struct Aws {
    router: MultiServiceRouter,
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One SageMaker control-plane call, as a client makes it.
    async fn sagemaker(&self, case: &str, target: &str, body: &Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("SageMaker.{target}"))
            .header("authorization", authorization("sagemaker"))
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from {target}, got {status} with {answered}"
        );
        answered
    }

    /// One request to this data plane, answering its status and parsed body.
    async fn data_plane(&self, method: &str, uri: &str, body: Option<Value>) -> (u16, Value) {
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", HOST)
            .header("authorization", authorization("sagemaker"))
            .body(
                body.map(|value| bytes::Bytes::from(value.to_string()))
                    .unwrap_or_default(),
            )
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = serde_json::from_slice::<Value>(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// `PUT /FeatureGroup/<group>` — one `PutRecord` that must be served.
    async fn put_record(&self, case: &str, group: &str, identifier: &str, payload: &str) {
        let body = json!({
            "Record": [
                { "FeatureName": IDENTIFIER_FEATURE, "ValueAsString": identifier },
                { "FeatureName": EVENT_TIME_FEATURE, "ValueAsString": SEEDED_EVENT_TIME },
                { "FeatureName": PAYLOAD_FEATURE, "ValueAsString": payload },
            ],
        });
        let (status, answered) = self
            .data_plane("PUT", &format!("/FeatureGroup/{group}"), Some(body))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an empty HTTP 200 from PutRecord, got {status} with \
             {answered}"
        );
    }

    /// `DELETE /FeatureGroup/<group>?…` — one `DeleteRecord`, answering its status.
    async fn delete_record(&self, group: &str, query: &str) -> (u16, Value) {
        self.data_plane("DELETE", &format!("/FeatureGroup/{group}?{query}"), None)
            .await
    }

    /// `GET /FeatureGroup/<group>?…` — one `GetRecord`.
    async fn get_record(&self, group: &str, identifier: &str) -> (u16, Value) {
        self.data_plane(
            "GET",
            &format!("/FeatureGroup/{group}?RecordIdentifierValueAsString={identifier}"),
            None,
        )
        .await
    }

    /// One path-style Amazon S3 request, answering its status, its headers and its body.
    async fn s3(&self, method: &str, path: &str) -> (u16, http::HeaderMap, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::new())
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (parts.status.as_u16(), parts.headers, body)
    }

    /// `PUT /<bucket>` — Amazon S3's `CreateBucket`.
    async fn create_bucket(&self, bucket: &str) {
        let (status, _, body) = self.s3("PUT", &format!("/{bucket}")).await;
        assert_eq!(
            status,
            200,
            "the bucket {bucket} must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// The keys `ListObjectsV2` reports under `prefix` of `bucket`, sorted — the read a
    /// client makes to find what the offline store holds.
    async fn keys_under(&self, bucket: &str, prefix: &str) -> Vec<String> {
        let path = if prefix.is_empty() {
            format!("/{bucket}?list-type=2")
        } else {
            format!("/{bucket}?list-type=2&prefix={prefix}")
        };
        let (status, _, body) = self.s3("GET", &path).await;
        assert_eq!(
            status,
            200,
            "listing {bucket} under {prefix:?} must be served, got {status} {}",
            String::from_utf8_lossy(&body)
        );
        let document = String::from_utf8_lossy(&body).into_owned();
        let mut reader = Reader::from_str(&document);
        let mut keys = Vec::new();
        let mut element = String::new();
        loop {
            match reader
                .read_event()
                .unwrap_or_else(|error| panic!("the listing must be well-formed XML: {error}"))
            {
                Event::Start(start) => {
                    element = String::from_utf8_lossy(start.name().as_ref()).into_owned();
                }
                Event::Text(text) => {
                    if element == "Key" {
                        keys.push(text.xml10_content().unwrap_or_default().into_owned());
                    }
                }
                Event::Eof => break,
                _ => {}
            }
        }
        keys.sort();
        keys
    }

    /// The `ContentLength` `HeadObject` reports for `key` — "The size of the body in bytes".
    async fn content_length(&self, case: &str, bucket: &str, key: &str) -> u64 {
        let (status, headers, _) = self.s3("HEAD", &format!("/{bucket}/{key}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: HeadObject on {key:?} must be served, got {status}"
        );
        headers
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or_else(|| {
                panic!("case {case:?}: HeadObject on {key:?} must report a ContentLength")
            })
    }

    /// The bytes `bucket` holds at `key`, read through `GetObject`.
    async fn object(&self, case: &str, bucket: &str, key: &str) -> Vec<u8> {
        let (status, _, body) = self.s3("GET", &format!("/{bucket}/{key}")).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the object {key:?} must be served, got {status}"
        );
        body.to_vec()
    }

    /// One IAM call (`awsQuery`) that must be served.
    async fn iam(&self, case: &str, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "iam.amazonaws.com")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "case {case:?}: {action} must be served, got {status} with {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// Hold the role `name` in IAM, trusted by the SageMaker service principal, granted the
    /// object write on `resource`, and answer its ARN.
    async fn create_granted_role(&self, case: &str, name: &str, resource: &str) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "sagemaker.amazonaws.com" },
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            case,
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": WRITE_ACTION,
                "Resource": resource,
            }],
        })
        .to_string();
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", OFFLINE_POLICY.to_string()),
                ("PolicyDocument", document),
            ],
        )
        .await;
        format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
    }

    /// Create the feature group `group` with the offline store `s3_uri` names, and answer
    /// what `DescribeFeatureGroup` reports for it.
    async fn create_feature_group(
        &self,
        case: &str,
        group: &str,
        s3_uri: &str,
        role_arn: &str,
    ) -> Value {
        let body = json!({
            "FeatureGroupName": group,
            "RecordIdentifierFeatureName": IDENTIFIER_FEATURE,
            "EventTimeFeatureName": EVENT_TIME_FEATURE,
            "FeatureDefinitions": [
                { "FeatureName": IDENTIFIER_FEATURE, "FeatureType": "String" },
                { "FeatureName": EVENT_TIME_FEATURE, "FeatureType": "String" },
                { "FeatureName": PAYLOAD_FEATURE, "FeatureType": "String" },
            ],
            "OnlineStoreConfig": { "EnableOnlineStore": true },
            // `DisableGlueTableCreation`: this suite manages no Glue table.
            "OfflineStoreConfig": {
                "S3StorageConfig": { "S3Uri": s3_uri },
                "DisableGlueTableCreation": true,
            },
            "RoleArn": role_arn,
        });
        self.sagemaker(case, "CreateFeatureGroup", &body).await;
        self.sagemaker(
            case,
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": group }),
        )
        .await
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key
/// the router claims a request by. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// Percent-encode one `application/x-www-form-urlencoded` component with the maintained
/// RFC 3986 encoder, leaving only the unreserved characters literal — so a policy document's
/// braces and quotes survive the round trip.
fn form_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// The `S3StorageConfig.ResolvedOutputS3Uri` a describe reports, as the KEY it names inside
/// its bucket.
fn resolved_key(described: &Value) -> String {
    let uri = described
        .pointer("/OfflineStoreConfig/S3StorageConfig/ResolvedOutputS3Uri")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let rest = uri.strip_prefix("s3://").unwrap_or(uri);
    rest.split_once('/')
        .map(|(_bucket, key)| key.to_string())
        .unwrap_or_default()
}

/// The `OfflineStoreStatus.Status` a describe reports.
fn offline_status(described: &Value) -> Option<&str> {
    described
        .pointer("/OfflineStoreStatus/Status")
        .and_then(Value::as_str)
}

/// The rows an offline object holds, rendered as text.
fn parquet_rows(bytes: Vec<u8>) -> Vec<String> {
    let reader = parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(bytes))
        .expect("the written object must be a Parquet file");
    reader
        .get_row_iter(None)
        .expect("the object's rows must be readable")
        .map(|row| row.expect("each row must be readable").to_string())
        .collect()
}

/// One row of the configured-location table.
struct Location {
    case: &'static str,
    /// The bucket the offline store lives in; a row's own, so its listing holds only its own
    /// objects.
    bucket: &'static str,
    /// The feature group the record is ingested into.
    group: &'static str,
    /// The `S3Uri` the create names, with `{bucket}` standing for the row's bucket.
    s3_uri: &'static str,
    /// The prefix a client lists under, which is the one it configured.
    prefix: &'static str,
    /// The payload the ingested record carries, which decides the object's size.
    payload: &'static str,
}

/// RULE: an ingestion into a feature group with an `OfflineStoreConfig` appends ONE non-empty
/// object under the prefix the group was configured with, so a client that lists that prefix
/// and heads what it lists finds a body with bytes in it — whatever shape the configured
/// location has and whatever the record carries.
///
/// The rows differ along what the rule generalises over: the SHAPE of the configured location
/// (one prefix segment, several, none at all, a prefix written with a trailing delimiter, the
/// `https://` spelling the member's pattern admits) and the SIZE of the ingested record.
#[tokio::test]
async fn an_ingested_record_is_a_non_empty_object_under_the_configured_prefix() {
    let rows: [Location; 5] = [
        Location {
            case: "one prefix segment",
            bucket: "features-one-segment",
            group: "offlineone",
            s3_uri: "s3://{bucket}/offline",
            prefix: "offline",
            payload: "first",
        },
        Location {
            case: "a nested prefix",
            bucket: "features-nested",
            group: "offlinenested",
            s3_uri: "s3://{bucket}/offline/store/v1",
            prefix: "offline/store/v1",
            payload: "first",
        },
        Location {
            case: "a bucket with no prefix at all",
            bucket: "features-root",
            group: "offlineroot",
            s3_uri: "s3://{bucket}",
            prefix: "",
            payload: "first",
        },
        Location {
            case: "a prefix written with a trailing delimiter",
            bucket: "features-trailing",
            group: "offlinetrailing",
            s3_uri: "s3://{bucket}/offline/",
            prefix: "offline",
            payload: "first",
        },
        Location {
            case: "the https spelling of the location, with a record of several kilobytes",
            bucket: "features-https",
            group: "offlinehttps",
            s3_uri: "https://{bucket}.s3.us-east-1.amazonaws.com/offline",
            prefix: "offline",
            // A payload far longer than the key, so a body of the record's own size is
            // distinguishable from a placeholder of a fixed one.
            payload: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
        },
    ];

    for row in rows {
        let case = row.case;
        let aws = Aws::new();
        aws.create_bucket(row.bucket).await;
        let role_arn = aws
            .create_granted_role(
                case,
                &format!("{}-role", row.group),
                &format!("arn:aws:s3:::{}/*", row.bucket),
            )
            .await;
        let s3_uri = row.s3_uri.replace("{bucket}", row.bucket);
        let described = aws
            .create_feature_group(case, row.group, &s3_uri, &role_arn)
            .await;
        assert_eq!(
            offline_status(&described),
            Some("Active"),
            "case {case:?}: a granted role's offline store is Active, got {described}"
        );
        assert!(
            aws.keys_under(row.bucket, row.prefix).await.is_empty(),
            "case {case:?}: expected nothing under the prefix before the ingestion"
        );

        aws.put_record(case, row.group, "C1", row.payload).await;

        // What a client sees: a listing of the prefix it configured, then a head of each key
        // it lists.
        let keys = aws.keys_under(row.bucket, row.prefix).await;
        assert_eq!(
            keys.len(),
            1,
            "case {case:?}: expected one object under {:?}, got {keys:?}",
            row.prefix
        );
        let key = &keys[0];
        let expected_prefix = {
            let resolved = resolved_key(&described);
            if resolved.is_empty() {
                String::new()
            } else {
                format!("{resolved}/")
            }
        };
        assert!(
            key.starts_with(&expected_prefix),
            "case {case:?}: expected the object under the resolved path {expected_prefix:?}, \
             got {key:?}"
        );
        let length = aws.content_length(case, row.bucket, key).await;
        assert!(
            length > 0,
            "case {case:?}: expected a ContentLength above zero for {key:?}, got {length}"
        );
        let body = aws.object(case, row.bucket, key).await;
        assert_eq!(
            u64::try_from(body.len()).unwrap_or_default(),
            length,
            "case {case:?}: expected the headed length to be the body's, got {} bytes",
            body.len()
        );
        let held = parquet_rows(body);
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected the object to hold the one ingested record, got {held:?}"
        );
        for expected in ["C1", SEEDED_EVENT_TIME, row.payload, NOT_DELETED_MARKER] {
            assert!(
                held[0].contains(expected),
                "case {case:?}: expected the row to carry {expected:?}, got {:?}",
                held[0]
            );
        }

        // The status is unchanged by the write it permitted.
        let described = aws
            .sagemaker(
                case,
                "DescribeFeatureGroup",
                &json!({ "FeatureGroupName": row.group }),
            )
            .await;
        assert_eq!(
            offline_status(&described),
            Some("Active"),
            "case {case:?}: expected the offline store still Active, got {described}"
        );
    }
}

/// One row of the deletion table.
struct Deletion {
    case: &'static str,
    /// The `DeletionMode` the request carries, as the query string spells it (empty for the
    /// defaulted mode).
    mode: &'static str,
    /// The `TargetStores` the request names, as the query string spells it (empty for none).
    target_stores: &'static str,
    /// The record the deletion names.
    identifier: &'static str,
    /// The `EventTime` the deletion carries.
    event_time: &'static str,
    /// The status the deletion must answer.
    status: u16,
    /// The partition the appended marker must land in, and `None` for a deletion that
    /// appends none.
    partition: Option<&'static str>,
    /// Whether the seeded record is still served by the OnlineStore afterwards.
    online_record_remains: bool,
}

/// RULE: a deletion of this data plane APPENDS the deleted record marker — the same record
/// identifier, the deletion's own `EventTime`, `is_deleted` true — as one more non-empty
/// object under the configured prefix, in the partition of the event time it carries, for
/// every deletion the service accepts in either deletion mode; the one deletion it refuses
/// (a `HardDelete` whose `EventTime` is older than the record's) appends nothing, and neither
/// does a deletion aimed at the OnlineStore alone.
///
/// The rows differ along what the rule generalises over: the deletion MODE (defaulted,
/// explicit `SoftDelete`, `HardDelete`), where the deletion's `EventTime` falls relative to
/// the record's (after, equal, before) and how it is spelled (ISO-8601, seconds since the
/// epoch), which stores it is aimed at, and whether the named record was ever ingested.
#[tokio::test]
async fn a_deletion_appends_the_deleted_record_marker_to_the_configured_prefix() {
    // The partition of the seeded record itself, which the ingested object occupies.
    const SEEDED_PARTITION: &str = "year=2026/month=06/day=01/hour=07";

    let rows: [Deletion; 10] = [
        Deletion {
            case: "a defaulted deletion after the record's event time",
            mode: "",
            target_stores: "",
            identifier: "C1",
            event_time: "2026-06-02T08:00:00Z",
            status: 200,
            partition: Some("year=2026/month=06/day=02/hour=08"),
            online_record_remains: false,
        },
        Deletion {
            case: "an explicit SoftDelete",
            mode: "&DeletionMode=SoftDelete",
            target_stores: "",
            identifier: "C1",
            event_time: "2026-06-02T08:00:00Z",
            status: 200,
            partition: Some("year=2026/month=06/day=02/hour=08"),
            online_record_remains: false,
        },
        Deletion {
            case: "a HardDelete",
            mode: "&DeletionMode=HardDelete",
            target_stores: "",
            identifier: "C1",
            event_time: "2026-06-02T08:00:00Z",
            status: 200,
            partition: Some("year=2026/month=06/day=02/hour=08"),
            online_record_remains: false,
        },
        Deletion {
            case: "a HardDelete at exactly the record's own event time",
            mode: "&DeletionMode=HardDelete",
            target_stores: "",
            identifier: "C1",
            event_time: SEEDED_EVENT_TIME,
            status: 200,
            partition: Some(SEEDED_PARTITION),
            online_record_remains: false,
        },
        Deletion {
            case: "an event time spelled as seconds since the epoch",
            mode: "",
            target_stores: "",
            identifier: "C1",
            // 2026-06-02T00:00:00Z, which is after the seeded record's event time.
            event_time: "1780358400",
            status: 200,
            partition: Some("year=2026/month=06/day=02/hour=00"),
            online_record_remains: false,
        },
        Deletion {
            case: "a SoftDelete whose event time is older than the record's",
            mode: "&DeletionMode=SoftDelete",
            target_stores: "",
            identifier: "C1",
            event_time: "2026-01-01T00:00:00Z",
            status: 200,
            // "For SoftDelete, the existing (not deleted) record remains in the OnlineStore,
            // though the delete record marker is still written to the OfflineStore."
            partition: Some("year=2026/month=01/day=01/hour=00"),
            online_record_remains: true,
        },
        Deletion {
            case: "a HardDelete whose event time is older than the record's",
            mode: "&DeletionMode=HardDelete",
            target_stores: "",
            identifier: "C1",
            event_time: "2026-01-01T00:00:00Z",
            // "HardDelete returns EventTime: 400 ValidationException … No delete record
            // marker is written to the OfflineStore."
            status: 400,
            partition: None,
            online_record_remains: true,
        },
        Deletion {
            case: "a deletion aimed at the OfflineStore alone",
            mode: "",
            target_stores: "&TargetStores=OfflineStore",
            identifier: "C1",
            event_time: "2026-06-02T08:00:00Z",
            status: 200,
            partition: Some("year=2026/month=06/day=02/hour=08"),
            online_record_remains: true,
        },
        Deletion {
            case: "a deletion aimed at the OnlineStore alone",
            mode: "",
            target_stores: "&TargetStores=OnlineStore",
            identifier: "C1",
            event_time: "2026-06-02T08:00:00Z",
            status: 200,
            partition: None,
            online_record_remains: false,
        },
        Deletion {
            case: "a deletion of a record that was never ingested",
            mode: "",
            target_stores: "",
            identifier: "C9",
            event_time: "2026-06-02T08:00:00Z",
            status: 200,
            partition: Some("year=2026/month=06/day=02/hour=08"),
            online_record_remains: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let bucket = "features-deleted";
        let group = "offlinedeleted";
        let prefix = "offline";
        let aws = Aws::new();
        aws.create_bucket(bucket).await;
        let role_arn = aws
            .create_granted_role(
                case,
                &format!("{group}-role"),
                &format!("arn:aws:s3:::{bucket}/*"),
            )
            .await;
        let described = aws
            .create_feature_group(case, group, &format!("s3://{bucket}/{prefix}"), &role_arn)
            .await;
        let resolved = resolved_key(&described);

        aws.put_record(case, group, "C1", "first").await;
        let ingested = aws.keys_under(bucket, prefix).await;
        assert_eq!(
            ingested.len(),
            1,
            "case {case:?}: the seeded ingestion must be retained, got {ingested:?}"
        );

        let (status, answered) = aws
            .delete_record(
                group,
                &format!(
                    "RecordIdentifierValueAsString={}&EventTime={}{}{}",
                    row.identifier, row.event_time, row.mode, row.target_stores
                ),
            )
            .await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {} from DeleteRecord, got {status} with {answered}",
            row.status
        );

        let keys = aws.keys_under(bucket, prefix).await;
        let Some(partition) = row.partition else {
            assert_eq!(
                keys, ingested,
                "case {case:?}: expected no marker appended, got {keys:?}"
            );
            assert_online_record(&aws, case, group, row.online_record_remains).await;
            continue;
        };
        assert_eq!(
            keys.len(),
            2,
            "case {case:?}: expected the marker appended beside the ingested record, got \
             {keys:?}"
        );
        let marker = keys
            .iter()
            .find(|key| !ingested.contains(key))
            .unwrap_or_else(|| panic!("case {case:?}: expected a key the ingestion did not write"));
        let expected_prefix = format!("{resolved}/{partition}/");
        assert!(
            marker.starts_with(&expected_prefix) && marker.ends_with(".parquet"),
            "case {case:?}: expected a Parquet marker under {expected_prefix:?}, got {marker:?}"
        );
        let length = aws.content_length(case, bucket, marker).await;
        assert!(
            length > 0,
            "case {case:?}: expected a ContentLength above zero for {marker:?}, got {length}"
        );
        let held = parquet_rows(aws.object(case, bucket, marker).await);
        assert_eq!(
            held.len(),
            1,
            "case {case:?}: expected the marker to hold one row, got {held:?}"
        );
        for expected in [row.identifier, row.event_time, DELETED_MARKER] {
            assert!(
                held[0].contains(expected),
                "case {case:?}: expected the marker row to carry {expected:?}, got {:?}",
                held[0]
            );
        }
        // The marker is a marker, not a copy of the record: the ingested payload is not in it.
        assert!(
            !held[0].contains("first"),
            "case {case:?}: expected the marker to carry no feature values of the record, got \
             {:?}",
            held[0]
        );
        // The ingested record's own object is untouched: the offline store is append-only.
        let ingested_row = parquet_rows(aws.object(case, bucket, &ingested[0]).await);
        assert!(
            ingested_row
                .first()
                .is_some_and(|row| row.contains(NOT_DELETED_MARKER) && row.contains("first")),
            "case {case:?}: expected the ingested object left as written, got {ingested_row:?}"
        );

        assert_online_record(&aws, case, group, row.online_record_remains).await;
    }
}

/// Assert whether the OnlineStore still serves the seeded record `C1`: a deleted record is
/// answered without a `Record` at all.
async fn assert_online_record(aws: &Aws, case: &str, group: &str, remains: bool) {
    let (status, answered) = aws.get_record(group, "C1").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRecord 200, got {status} with {answered}"
    );
    let served = answered.get("Record").is_some_and(|record| !record.is_null());
    assert_eq!(
        served, remains,
        "case {case:?}: expected the online record {}, got {answered}",
        if remains { "to remain" } else { "deleted" }
    );
}
