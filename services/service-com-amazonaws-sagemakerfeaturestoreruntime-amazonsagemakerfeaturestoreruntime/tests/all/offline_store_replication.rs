// SPDX-License-Identifier: CC0-1.0

//! The OfflineStore side of the Feature Store data plane, driven over the wire.
//!
//! The rule under test: a record ingested into a feature group the SageMaker control plane
//! gave an `OfflineStoreConfig` is WRITTEN into that store — "If a new record's EventTime
//! is greater, the new record is written to both the OnlineStore and OfflineStore.
//! Otherwise, the record is a historic record and it is written only to the OfflineStore"
//! (`PutRecord`) — as an object under the output path the group resolved
//! (`S3StorageConfig$ResolvedOutputS3Uri`, "The S3 path where offline records are
//! written"), partitioned by the record's own event time, and the write is made under the
//! group's `RoleArn` ("the IAM execution role used to persist data into the OfflineStore"),
//! so a role whose permissions withhold `s3:PutObject` there retains nothing and the group
//! reports `OfflineStoreStatus` `Blocked` with a `BlockedReason` until the permission is
//! restored.
//!
//! The four services the rule spans are mounted on ONE shared engine, exactly as the router
//! mounts them: this data plane, the SageMaker control plane that defines the group, IAM
//! which holds the role and answers what it may do, and Amazon S3 which holds the objects.
//! Every call goes through the real front door — the control-plane calls as `awsJson1_1`
//! with `X-Amz-Target`, the ingestions as this service's own restJson1 requests, the IAM
//! calls as `awsQuery` form posts and the destination reads as the path-style Amazon S3
//! requests a client sends.

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

/// A feature carrying the record's payload, so an object's bytes can be told apart from a
/// placeholder.
const PAYLOAD_FEATURE: &str = "payload";

/// The IAM action an object write is authorized under, as the rule names it.
const WRITE_ACTION: &str = "s3:PutObject";

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
        let answered: Value =
            serde_json::from_slice(response.body()).unwrap_or(Value::Null);
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
    async fn put_record(&self, case: &str, group: &str, record: Value) {
        let (status, answered) = self
            .data_plane("PUT", &format!("/FeatureGroup/{group}"), Some(record))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected an empty HTTP 200 from PutRecord, got {status} with \
             {answered}"
        );
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

    /// One path-style Amazon S3 request.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let (parts, body) = response.into_parts();
        (parts.status.as_u16(), body)
    }

    /// `PUT /<bucket>` — Amazon S3's `CreateBucket`.
    async fn create_bucket(&self, bucket: &str) {
        let (status, body) = self.s3("PUT", &format!("/{bucket}"), b"").await;
        assert_eq!(
            status,
            200,
            "the bucket {bucket} must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Every key `bucket` holds, read through `ListObjectsV2`.
    async fn keys(&self, bucket: &str) -> Vec<String> {
        let (status, body) = self.s3("GET", &format!("/{bucket}?list-type=2"), b"").await;
        assert_eq!(
            status,
            200,
            "listing {bucket} must be served, got {status} {}",
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
        keys
    }

    /// The bytes `bucket` holds at `key`, read through `GetObject`.
    async fn object(&self, bucket: &str, key: &str) -> Vec<u8> {
        let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            status,
            200,
            "the object {key} must be served, got {status} {}",
            String::from_utf8_lossy(&body)
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

    /// Hold the role `name` in IAM, trusted by the SageMaker service principal, and answer
    /// its ARN.
    async fn create_role(&self, case: &str, name: &str) -> String {
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
        format!(
            "arn:aws:iam::{}:role/{name}",
            engine::emulator_account_id()
        )
    }

    /// Grant the role `name` the object write on `resource`, as its one inline policy.
    async fn grant_write(&self, case: &str, name: &str, resource: &str) {
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
    }

    /// Take that grant away again, the way a client revokes a permission.
    async fn revoke_write(&self, case: &str, name: &str) {
        self.iam(
            case,
            "DeleteRolePolicy",
            &[
                ("RoleName", name.to_string()),
                ("PolicyName", OFFLINE_POLICY.to_string()),
            ],
        )
        .await;
    }

    /// Create the feature group `group`, with the offline store `s3_uri` names where the
    /// case has one, and answer what `DescribeFeatureGroup` reports for it.
    async fn create_feature_group(
        &self,
        case: &str,
        group: &str,
        online: bool,
        offline: Option<(&str, &str)>,
    ) -> Value {
        let mut body = json!({
            "FeatureGroupName": group,
            "RecordIdentifierFeatureName": IDENTIFIER_FEATURE,
            "EventTimeFeatureName": EVENT_TIME_FEATURE,
            "FeatureDefinitions": [
                { "FeatureName": IDENTIFIER_FEATURE, "FeatureType": "String" },
                { "FeatureName": EVENT_TIME_FEATURE, "FeatureType": "String" },
                { "FeatureName": PAYLOAD_FEATURE, "FeatureType": "String" },
            ],
        });
        let object = body.as_object_mut().expect("a JSON object body");
        if online {
            object.insert(
                "OnlineStoreConfig".to_string(),
                json!({ "EnableOnlineStore": true }),
            );
        }
        if let Some((s3_uri, role_arn)) = offline {
            object.insert(
                "OfflineStoreConfig".to_string(),
                // `DisableGlueTableCreation`: this suite manages no Glue table, which is
                // what the recorded client asks for too.
                json!({
                    "S3StorageConfig": { "S3Uri": s3_uri },
                    "DisableGlueTableCreation": true,
                }),
            );
            object.insert("RoleArn".to_string(), json!(role_arn));
        }
        self.sagemaker(case, "CreateFeatureGroup", &body).await;
        self.describe_feature_group(case, group).await
    }

    /// `DescribeFeatureGroup` for `group`.
    async fn describe_feature_group(&self, case: &str, group: &str) -> Value {
        self.sagemaker(
            case,
            "DescribeFeatureGroup",
            &json!({ "FeatureGroupName": group }),
        )
        .await
    }
}

/// The name of the role's one inline policy.
const OFFLINE_POLICY: &str = "offline-store-write";

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
/// RFC 3986 encoder, leaving only the unreserved characters literal — so a policy
/// document's braces and quotes survive the round trip.
fn form_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// One record, as a `PutRecord` body carries it.
fn record(identifier: &str, event_time: &str, payload: &str) -> Value {
    json!({
        "Record": [
            { "FeatureName": IDENTIFIER_FEATURE, "ValueAsString": identifier },
            { "FeatureName": EVENT_TIME_FEATURE, "ValueAsString": event_time },
            { "FeatureName": PAYLOAD_FEATURE, "ValueAsString": payload },
        ],
    })
}

/// The `S3StorageConfig.ResolvedOutputS3Uri` a describe reports, as the KEY it names inside
/// its bucket (the output path below the bucket).
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

/// The `OfflineStoreStatus` a describe reports, as `(Status, BlockedReason)`.
fn offline_status(described: &Value) -> (Option<String>, Option<String>) {
    let status = described.get("OfflineStoreStatus");
    (
        status
            .and_then(|held| held.get("Status"))
            .and_then(Value::as_str)
            .map(str::to_string),
        status
            .and_then(|held| held.get("BlockedReason"))
            .and_then(Value::as_str)
            .map(str::to_string),
    )
}

/// The rows a Parquet object holds, rendered as text — enough to assert that the object
/// carries the record that was ingested rather than a placeholder.
fn parquet_rows(bytes: Vec<u8>) -> Vec<String> {
    let reader =
        parquet::file::reader::SerializedFileReader::new(bytes::Bytes::from(bytes))
            .expect("the written object must be a Parquet file");
    reader
        .get_row_iter(None)
        .expect("the object's rows must be readable")
        .map(|row| row.expect("each row must be readable").to_string())
        .collect()
}

/// One row of the replication table.
struct Row {
    case: &'static str,
    /// The feature group the record is ingested into.
    group: &'static str,
    /// The bucket its offline store lives in; a row's own, so its listing holds only its
    /// own objects.
    bucket: &'static str,
    /// The `S3Uri` the create names, or `None` for a group with no offline store at all.
    s3_uri: Option<&'static str>,
    /// The resource the role's inline policy grants the object write on, or `None` for a
    /// role carrying no policy at all.
    grant: Option<&'static str>,
    /// Whether the group also keeps an OnlineStore.
    online: bool,
    /// The record's event time.
    event_time: &'static str,
    /// The partition the written object's key must carry, derived from that event time.
    partition: &'static str,
    /// Whether the record must be retained in the offline store.
    replicated: bool,
    /// The `OfflineStoreStatus.Status` the describe must report, and `None` for a group
    /// with no offline store.
    status: Option<&'static str>,
}

/// RULE: a record ingested into a feature group with an `OfflineStoreConfig` is written as
/// an object under the group's resolved output path, in the partition of its own event
/// time, when the group's role is permitted the object write there — and nothing is written,
/// with the group reporting `Blocked`, when it is not.
///
/// The rows differ along what the rule generalises over: the SHAPE of the configured
/// location (a bucket with a prefix, a nested prefix, a bucket with no prefix), WHICH
/// resource the role's grant names (the whole bucket, exactly the configured prefix, another
/// bucket, no policy at all), WHICH stores the group keeps (both, offline only, online only)
/// and the record's EVENT TIME, which decides the partition.
#[tokio::test]
async fn an_ingested_record_is_replicated_into_the_offline_store_its_role_may_write() {
    let rows = [
        Row {
            case: "granted on the whole bucket",
            group: "offlinegranted",
            bucket: "features-granted",
            s3_uri: Some("s3://features-granted/offline"),
            grant: Some("arn:aws:s3:::features-granted/*"),
            online: true,
            event_time: "2026-06-01T07:08:09Z",
            partition: "year=2026/month=06/day=01/hour=07",
            replicated: true,
            status: Some("Active"),
        },
        Row {
            case: "granted on the configured prefix only, with a nested prefix",
            group: "offlinenested",
            bucket: "features-nested",
            s3_uri: Some("s3://features-nested/offline/store/"),
            grant: Some("arn:aws:s3:::features-nested/offline/store/*"),
            online: true,
            event_time: "2027-01-01T00:00:00Z",
            partition: "year=2027/month=01/day=01/hour=00",
            replicated: true,
            status: Some("Active"),
        },
        Row {
            case: "a location with no prefix at all",
            group: "offlinerootlocation",
            bucket: "features-root",
            s3_uri: Some("s3://features-root"),
            grant: Some("arn:aws:s3:::features-root/*"),
            online: true,
            event_time: "2026-02-28T23:59:59Z",
            partition: "year=2026/month=02/day=28/hour=23",
            replicated: true,
            status: Some("Active"),
        },
        Row {
            case: "a group that keeps no online store",
            group: "offlineonly",
            bucket: "features-offline-only",
            s3_uri: Some("s3://features-offline-only/offline"),
            grant: Some("arn:aws:s3:::features-offline-only/*"),
            online: false,
            event_time: "2026-06-01T07:08:09Z",
            partition: "year=2026/month=06/day=01/hour=07",
            replicated: true,
            status: Some("Active"),
        },
        Row {
            case: "a role carrying no policy at all",
            group: "offlineungranted",
            bucket: "features-ungranted",
            s3_uri: Some("s3://features-ungranted/offline"),
            grant: None,
            online: true,
            event_time: "2026-06-01T07:08:09Z",
            partition: "year=2026/month=06/day=01/hour=07",
            replicated: false,
            status: Some("Blocked"),
        },
        Row {
            case: "a role granted on another bucket",
            group: "offlineelsewhere",
            bucket: "features-elsewhere",
            s3_uri: Some("s3://features-elsewhere/offline"),
            grant: Some("arn:aws:s3:::somewhere-else/*"),
            online: true,
            event_time: "2026-06-01T07:08:09Z",
            partition: "year=2026/month=06/day=01/hour=07",
            replicated: false,
            status: Some("Blocked"),
        },
        Row {
            case: "a group with no offline store",
            group: "onlineonly",
            bucket: "features-online-only",
            s3_uri: None,
            grant: Some("arn:aws:s3:::features-online-only/*"),
            online: true,
            event_time: "2026-06-01T07:08:09Z",
            partition: "year=2026/month=06/day=01/hour=07",
            replicated: false,
            status: None,
        },
    ];

    for row in rows {
        let aws = Aws::new();
        let case = row.case;
        aws.create_bucket(row.bucket).await;
        let role_name = format!("{}-role", row.group);
        let role_arn = aws.create_role(case, &role_name).await;
        if let Some(resource) = row.grant {
            aws.grant_write(case, &role_name, resource).await;
        }
        let described = aws
            .create_feature_group(
                case,
                row.group,
                row.online,
                row.s3_uri.map(|s3_uri| (s3_uri, role_arn.as_str())),
            )
            .await;

        // The status the group reports, and the reason a blocked one is reported with.
        let (status, reason) = offline_status(&described);
        assert_eq!(
            status.as_deref(),
            row.status,
            "case {case:?}: expected OfflineStoreStatus {:?}, got {described}",
            row.status
        );
        if row.status == Some("Blocked") {
            let reason = reason.unwrap_or_default();
            for expected in [role_arn.as_str(), WRITE_ACTION] {
                assert!(
                    reason.contains(expected),
                    "case {case:?}: expected the blocked reason to name {expected:?}, got \
                     {reason:?}"
                );
            }
        } else {
            assert_eq!(
                reason, None,
                "case {case:?}: expected no BlockedReason, got {reason:?}"
            );
        }

        aws.put_record(case, row.group, record("C1", row.event_time, "first"))
            .await;

        let keys = aws.keys(row.bucket).await;
        if !row.replicated {
            assert!(
                keys.is_empty(),
                "case {case:?}: expected nothing retained in the offline store, got {keys:?}"
            );
        } else {
            assert_eq!(
                keys.len(),
                1,
                "case {case:?}: expected one replicated object, got {keys:?}"
            );
            let key = &keys[0];
            let expected_prefix = {
                let resolved = resolved_key(&described);
                if resolved.is_empty() {
                    format!("{}/", row.partition)
                } else {
                    format!("{resolved}/{}/", row.partition)
                }
            };
            assert!(
                key.starts_with(&expected_prefix) && key.ends_with(".parquet"),
                "case {case:?}: expected a Parquet object under {expected_prefix:?}, got \
                 {key:?}"
            );
            let body = aws.object(row.bucket, key).await;
            assert!(
                !body.is_empty(),
                "case {case:?}: expected a non-empty object at {key:?}"
            );
            let rows_held = parquet_rows(body);
            assert_eq!(
                rows_held.len(),
                1,
                "case {case:?}: expected the object to hold the one ingested record, got \
                 {rows_held:?}"
            );
            for expected in ["C1", row.event_time, "first"] {
                assert!(
                    rows_held[0].contains(expected),
                    "case {case:?}: expected the replicated row to carry {expected:?}, got \
                     {:?}",
                    rows_held[0]
                );
            }
        }

        // The ONLINE store is untouched by any of this: a group that keeps one still serves
        // the record it took, and a group that keeps none serves nothing.
        let (status, answered) = aws.get_record(row.group, "C1").await;
        if row.online {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the online store to serve the record, got {status} \
                 with {answered}"
            );
            let values: Vec<String> = answered["Record"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|feature| feature["ValueAsString"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                values.contains(&"first".to_string()),
                "case {case:?}: expected the ingested payload to read back, got {answered}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a group with no OnlineStore has no record to serve, got \
                 {status} with {answered}"
            );
        }
    }
}

/// RULE: the status is the permission IN FORCE, so a group whose role loses the object write
/// reports `Blocked` with a `BlockedReason` and retains nothing more, and reports `Active`
/// again — retaining what is ingested after it — once the permission is restored.
#[tokio::test]
async fn revoking_and_restoring_the_roles_permission_moves_the_offline_store_status() {
    let case = "a permission revoked and restored";
    let bucket = "features-revoked";
    let group = "offlinerevoked";
    let aws = Aws::new();
    aws.create_bucket(bucket).await;
    let role_name = format!("{group}-role");
    let role_arn = aws.create_role(case, &role_name).await;
    let resource = format!("arn:aws:s3:::{bucket}/*");
    aws.grant_write(case, &role_name, &resource).await;
    let described = aws
        .create_feature_group(
            case,
            group,
            true,
            Some((&format!("s3://{bucket}/offline"), role_arn.as_str())),
        )
        .await;
    assert_eq!(
        offline_status(&described).0.as_deref(),
        Some("Active"),
        "case {case:?}: a granted role's offline store is Active, got {described}"
    );

    aws.put_record(case, group, record("C1", "2026-06-01T07:08:09Z", "first"))
        .await;
    assert_eq!(
        aws.keys(bucket).await.len(),
        1,
        "case {case:?}: the first ingestion must be retained"
    );

    // The permission goes away. Nothing else changes: the group, the role and the records
    // already written stay exactly as they are.
    aws.revoke_write(case, &role_name).await;
    let described = aws.describe_feature_group(case, group).await;
    let (status, reason) = offline_status(&described);
    assert_eq!(
        status.as_deref(),
        Some("Blocked"),
        "case {case:?}: a revoked permission blocks the offline store, got {described}"
    );
    let reason = reason.unwrap_or_default();
    for expected in [role_arn.as_str(), WRITE_ACTION] {
        assert!(
            reason.contains(expected),
            "case {case:?}: expected the blocked reason to name {expected:?}, got {reason:?}"
        );
    }
    aws.put_record(case, group, record("C2", "2026-06-02T07:08:09Z", "withheld"))
        .await;
    let keys = aws.keys(bucket).await;
    assert_eq!(
        keys.len(),
        1,
        "case {case:?}: an ingestion the role may not write retains nothing, got {keys:?}"
    );

    // And back again.
    aws.grant_write(case, &role_name, &resource).await;
    let described = aws.describe_feature_group(case, group).await;
    let (status, reason) = offline_status(&described);
    assert_eq!(
        (status.as_deref(), reason),
        (Some("Active"), None),
        "case {case:?}: a restored permission returns the offline store to Active, got \
         {described}"
    );
    aws.put_record(case, group, record("C3", "2026-06-03T07:08:09Z", "again"))
        .await;
    let keys = aws.keys(bucket).await;
    assert_eq!(
        keys.len(),
        2,
        "case {case:?}: an ingestion made while the permission is back is retained, got \
         {keys:?}"
    );
}

/// RULE: the same replication is made by the BATCH write, whose entries are ingested exactly
/// as the same records written on their own — including a historic entry, which the
/// OfflineStore takes and the OnlineStore leaves standing.
#[tokio::test]
async fn a_batch_write_replicates_every_entry_including_a_historic_one() {
    let case = "a batch of three entries, one historic";
    let bucket = "features-batched";
    let group = "offlinebatched";
    let aws = Aws::new();
    aws.create_bucket(bucket).await;
    let role_name = format!("{group}-role");
    let role_arn = aws.create_role(case, &role_name).await;
    aws.grant_write(case, &role_name, &format!("arn:aws:s3:::{bucket}/*"))
        .await;
    let described = aws
        .create_feature_group(
            case,
            group,
            true,
            Some((&format!("s3://{bucket}/offline"), role_arn.as_str())),
        )
        .await;
    let resolved = resolved_key(&described);

    // The newest record of C1 is written first, so the third entry below is HISTORIC: the
    // online store keeps the newer values and the offline store takes the older record all
    // the same.
    let entries = json!({
        "Entries": [
            { "FeatureGroupName": group, "Record": record("C1", "2026-06-01T00:00:00Z", "newer")["Record"] },
            { "FeatureGroupName": group, "Record": record("C2", "2026-07-02T13:00:00Z", "other")["Record"] },
            { "FeatureGroupName": group, "Record": record("C1", "2026-01-01T00:00:00Z", "historic")["Record"] },
        ],
    });
    let (status, answered) = aws
        .data_plane("POST", "/BatchWriteRecord", Some(entries))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from BatchWriteRecord, got {status} with {answered}"
    );
    assert_eq!(
        (
            answered["Errors"].as_array().map(Vec::len),
            answered["UnprocessedEntries"].as_array().map(Vec::len)
        ),
        (Some(0), Some(0)),
        "case {case:?}: every entry must be written, got {answered}"
    );

    let keys = aws.keys(bucket).await;
    assert_eq!(
        keys.len(),
        3,
        "case {case:?}: expected one replicated object per entry, got {keys:?}"
    );
    // Each entry lands in the partition of its OWN event time, the historic one included.
    for partition in [
        "year=2026/month=06/day=01/hour=00",
        "year=2026/month=07/day=02/hour=13",
        "year=2026/month=01/day=01/hour=00",
    ] {
        let expected_prefix = format!("{resolved}/{partition}/");
        assert!(
            keys.iter().any(|key| key.starts_with(&expected_prefix)),
            "case {case:?}: expected an object under {expected_prefix:?}, got {keys:?}"
        );
    }

    // The online store kept the NEWER record of C1: the historic entry is written only to
    // the offline store.
    let (status, answered) = aws.get_record(group, "C1").await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the online store to serve C1, got {status} with {answered}"
    );
    let payload: Vec<String> = answered["Record"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|feature| feature["FeatureName"] == json!(PAYLOAD_FEATURE))
        .map(|feature| feature["ValueAsString"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        payload,
        vec!["newer".to_string()],
        "case {case:?}: the historic entry must not overwrite the online record, got \
         {answered}"
    );
}
