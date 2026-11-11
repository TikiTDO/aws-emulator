// SPDX-License-Identifier: CC0-1.0

//! Delivery to an HTTP endpoint destination, end to end.
//!
//! The rule under test: a stream whose destination is an HTTP endpoint has its buffered
//! records DELIVERED to the URL that destination's `EndpointConfiguration` names once the
//! buffer is due — "Firehose buffers records before delivering them to the destination"
//! (`PutRecord`) — as one POST of the envelope the delivery protocol
//! `HttpEndpointConfiguration$Url` points at documents (`requestId`, `timestamp`, and a
//! `records` array of base64 `data` items), carrying the `RequestConfiguration`'s
//! `ContentEncoding` and `CommonAttributes` and the configured `AccessKey`. Only a `2xx`
//! answer acknowledges the delivery; anything else leaves the records buffered for as long
//! as `RetryOptions` allows and then parks them in the destination's Amazon S3 backup
//! ("only the documents that Firehose could not deliver to the specified HTTP endpoint
//! destination (`FailedDataOnly`)" — `S3BackupMode`), so nothing a put accepted is lost.
//!
//! The stream is created and the records are put over the real `awsJson1_1` wire, and the
//! endpoint is an in-process transport that records what it was sent: the request this
//! emulator renders is the request the endpoint receives, with nothing between them. The
//! parking case is driven by the emulator's own delivery clock rather than by the test's
//! transport, so what it proves is what a producer that makes no further call observes.

use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime};

use aws_smithy_runtime_api::client::http::{
    HttpConnector, HttpConnectorFuture, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::{HttpRequest, HttpResponse};
use aws_smithy_runtime_api::http::StatusCode;
use aws_smithy_types::body::SdkBody;
use serde_json::{json, Value};
use service_com_amazonaws_firehose_firehose_20150804::state::delivery::{
    deliver_due, DeliveryHop, Extractions,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_stream_lifecycle::{PAST_THE_TRANSITION, REGION, ROLE_ARN};

/// How long a delivery is waited for before a case fails: far longer than the emulator's
/// delivery span, so a slow machine does not fail a case the clock would have served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the delivery is driven, and its outcome read, while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The bucket the destinations below back failed records up to.
const BACKUP_BUCKET: &str = "firehose-http-backup";

/// `BucketARN` of [`BACKUP_BUCKET`], in the form that member's pattern requires.
const BACKUP_BUCKET_ARN: &str = "arn:aws:s3:::firehose-http-backup";

/// The `firehose:error-output-type` an HTTP endpoint destination's failed records are
/// written under, and the prefix they get when the backup configures no
/// `ErrorOutputPrefix`.
const FAILED_OUTPUT_TYPE: &str = "http-endpoint-failed";

/// The `errorCode`s the error-output document of a record an HTTP endpoint destination did
/// not take carries: the endpoint answered without acknowledging, or the request could not
/// be made at all.
const FAILED_ERROR_CODES: &[&str] = &[
    "HttpEndpoint.DestinationException",
    "HttpEndpoint.ConnectionFailed",
];

// ---------------------------------------------------------------------------
// The endpoint
// ---------------------------------------------------------------------------

/// One delivery request an endpoint was sent.
#[derive(Debug, Clone)]
struct Captured {
    method: String,
    uri: String,
    /// Header names lowercased, which is how HTTP compares them.
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The request's JSON envelope, un-gzipped first when the request says it is gzipped,
    /// or `None` when the body is neither — what an endpoint reading its own input sees,
    /// with no case to fail.
    fn envelope_if_json(&self) -> Option<Value> {
        let body = match self.header("content-encoding") {
            Some("gzip") => {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(&self.body[..])
                    .read_to_end(&mut decoded)
                    .ok()?;
                decoded
            }
            Some(_) => return None,
            None => self.body.clone(),
        };
        serde_json::from_slice(&body).ok()
    }

    /// The request's JSON envelope, un-gzipped first when the request says it is gzipped.
    fn envelope(&self, case: &str) -> Value {
        let body = match self.header("content-encoding") {
            Some("gzip") => {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(&self.body[..])
                    .read_to_end(&mut decoded)
                    .unwrap_or_else(|error| {
                        panic!("case {case:?}: the gzipped body must decode, got {error}")
                    });
                decoded
            }
            encoding => {
                assert_eq!(
                    encoding, None,
                    "case {case:?}: only gzip is a content encoding this destination asks for"
                );
                self.body.clone()
            }
        };
        serde_json::from_slice(&body).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the delivery request body must be JSON, got {error} for {:?}",
                String::from_utf8_lossy(&body)
            )
        })
    }
}

/// A request id that is not the one of any request this emulator renders: an 8-4-4-4-12
/// GUID, so an answer naming it is a well-formed answer to ANOTHER request.
const ANOTHER_REQUEST_ID: &str = "00000000-0000-4000-8000-000000000000";

/// What the body of the endpoint's answer says about the delivery request it answers.
///
/// The delivery protocol asks a destination to answer one JSON document naming the request
/// it is answering, and to report a request it could not process in that document's own
/// `errorMessage` member rather than in the status line. These are the forms an endpoint's
/// answer can take around that: the acknowledgment, and each way an answer can fail to be
/// one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Answer {
    /// The acknowledgment: this request's own id.
    EchoesTheRequestId,
    /// This request's id beside the `timestamp` the documented answer also carries.
    EchoesTheRequestIdAndTimestamp,
    /// This request's id beside an `errorMessage` that reports nothing.
    EchoesTheRequestIdAndNoError,
    /// This request's id beside the error the destination reports for it.
    ReportsAnError,
    /// [`ANOTHER_REQUEST_ID`], so the answer belongs to a different request.
    NamesAnotherRequest,
    /// A JSON object naming no request at all.
    NamesNoRequest,
    /// A body that is not JSON.
    NotJson,
    /// No body at all.
    Nothing,
}

/// An HTTP endpoint that records every delivery request it is sent and answers each with
/// one status and one form of answer — the in-process transport a delivery is made over.
#[derive(Debug, Clone)]
struct Sink {
    status: u16,
    answer: Answer,
    captured: Arc<Mutex<Vec<Captured>>>,
}

impl Sink {
    /// An endpoint answering `status` with the acknowledgment the protocol asks for.
    fn answering(status: u16) -> Self {
        Self::answering_with(status, Answer::EchoesTheRequestId)
    }

    /// An endpoint answering `status` with `answer`.
    fn answering_with(status: u16, answer: Answer) -> Self {
        Self {
            status,
            answer,
            captured: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn transport(&self) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }

    fn captured(&self) -> Vec<Captured> {
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl HttpConnector for Sink {
    fn call(&self, request: HttpRequest) -> HttpConnectorFuture {
        let captured = Captured {
            method: request.method().to_string(),
            uri: request.uri().to_string(),
            headers: request
                .headers()
                .iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
                .collect(),
            body: request.body().bytes().unwrap_or_default().to_vec(),
        };
        // The acknowledgment the delivery protocol asks a destination for names the
        // request it acknowledges, which an endpoint reads out of the envelope it was
        // sent — un-gzipped first when the request says it is gzipped.
        let request_id = captured
            .envelope_if_json()
            .and_then(|envelope| {
                envelope
                    .get("requestId")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        self.captured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(captured);
        let answered_at = epoch_millis(SystemTime::now());
        let body = match self.answer {
            Answer::EchoesTheRequestId => json!({ "requestId": request_id }).to_string(),
            Answer::EchoesTheRequestIdAndTimestamp => {
                json!({ "requestId": request_id, "timestamp": answered_at }).to_string()
            }
            Answer::EchoesTheRequestIdAndNoError => {
                json!({ "requestId": request_id, "errorMessage": Value::Null }).to_string()
            }
            Answer::ReportsAnError => json!({
                "requestId": request_id,
                "timestamp": answered_at,
                "errorMessage": "the destination could not process this batch",
            })
            .to_string(),
            Answer::NamesAnotherRequest => {
                json!({ "requestId": ANOTHER_REQUEST_ID, "timestamp": answered_at }).to_string()
            }
            Answer::NamesNoRequest => json!({ "timestamp": answered_at }).to_string(),
            Answer::NotJson => "accepted".to_string(),
            Answer::Nothing => String::new(),
        };
        let status = StatusCode::try_from(self.status).expect("the sink's status is a status");
        let response = HttpResponse::new(status, SdkBody::from(body));
        HttpConnectorFuture::new(Box::pin(async move { Ok(response) }))
    }
}

// ---------------------------------------------------------------------------
// The emulator
// ---------------------------------------------------------------------------

/// Firehose and Amazon S3 on one shared engine, exactly as the router mounts them: the
/// destination's backup is the Amazon S3 service's own store.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router, engine }
    }

    /// Call one Firehose operation and require it to succeed.
    async fn firehose_ok(&self, operation: &str, body: Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "127.0.0.1:9297")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Firehose_20150804.{operation}"))
            .header("authorization", authorization("firehose"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let answered: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        assert_eq!(status, 200, "{operation} must succeed, got {answered:?}");
        answered
    }

    /// `PUT /<bucket>` — Amazon S3's `CreateBucket`, so the destination's backup exists.
    ///
    /// The create names its `LocationConstraint`: a create that names none can only mean
    /// us-east-1, the Region the unspecified constraint stands for, so S3 refuses one sent
    /// to any other Region's endpoint and this harness signs for [`REGION`].
    async fn create_bucket(&self, bucket: &str) {
        let payload = format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        );
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("https://127.0.0.1:9297/{bucket}"))
            .header("host", "127.0.0.1:9297")
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::from(payload))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "create bucket {bucket}: got {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// Create a stream whose only destination is the HTTP endpoint `config` describes, and
    /// answer its ARN. It is `CREATING` on return: a put is accepted only once it is
    /// `ACTIVE`, which [`Emulator::await_active_streams`] waits for ONCE for every stream a
    /// case created.
    async fn create_endpoint_stream(&self, stream: &str, config: Value) -> String {
        let created = self
            .firehose_ok(
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "HttpEndpointDestinationConfiguration": config,
                }),
            )
            .await;
        created["DeliveryStreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("CreateDeliveryStream must report an ARN, got {created:?}"))
            .to_string()
    }

    /// Wait out the emulator's chosen `CREATING` -> `ACTIVE` span once, so every stream a
    /// case created is accepting data. One wait for a whole table rather than one per row.
    async fn await_active_streams(&self) {
        tokio::time::sleep(PAST_THE_TRANSITION).await;
    }

    /// `PutRecordBatch` with one record per entry of `payloads`.
    async fn put_batch(&self, stream: &str, payloads: &[&[u8]]) {
        let records: Vec<Value> = payloads
            .iter()
            .map(|data| json!({ "Data": aws_smithy_types::base64::encode(data) }))
            .collect();
        let answered = self
            .firehose_ok(
                "PutRecordBatch",
                json!({ "DeliveryStreamName": stream, "Records": records }),
            )
            .await;
        assert_eq!(
            answered["FailedPutCount"].as_i64(),
            Some(0),
            "every record of the batch must be accepted, got {answered:?}"
        );
    }

    /// The data of every record still buffered for `stream`, in arrival order.
    fn buffered(&self, stream: &str) -> Vec<Vec<u8>> {
        let state = self.engine.state::<
            service_com_amazonaws_firehose_firehose_20150804::ComAmazonawsFirehoseFirehose20150804State,
        >();
        let store = state.lock();
        store
            .streams
            .get(stream)
            .map(|stream| {
                stream
                    .records
                    .iter()
                    .map(|record| record.data.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Every object the backup bucket holds, keyed by key — what the Amazon S3 service
    /// stores, which `GetObject` serves verbatim.
    fn backup_objects(&self) -> BTreeMap<String, Vec<u8>> {
        let Some(state) = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
        else {
            return BTreeMap::new();
        };
        let store = state.lock();
        store
            .bucket(BACKUP_BUCKET)
            .map(|bucket| {
                bucket
                    .objects()
                    .map(|(key, object)| (key.to_string(), object.body.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Drive every delivery this store owes over `transport` until `ready` holds, or fail
    /// naming `case` and what did not happen.
    ///
    /// The emulator's own delivery pump is attempting the same buffer over the transport of
    /// the request that started it — and nothing this emulator serves answers at the case's
    /// endpoint URL, so each of the pump's attempts fails and holds the next attempt off by
    /// the retry span. Clearing that backoff right before each attempt is what lets the
    /// case's own transport be the one that reaches the endpoint; the records themselves are
    /// untouched.
    async fn deliver_until(
        &self,
        case: &str,
        stream: &str,
        transport: &SharedHttpConnector,
        expected: &str,
        mut ready: impl FnMut() -> bool,
    ) {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            self.clear_retry_backoff(stream);
            deliver_due(
                &*self.engine,
                SystemTime::now(),
                Extractions::Run,
                // The case's own transport, and no emulator to put a delivery's
                // authorization question to: this suite drives the endpoint it serves
                // itself.
                &DeliveryHop::over_transport(transport.clone()),
            )
            .await;
            if ready() {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: {expected} within {DELIVERY_DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Forget that a failed delivery attempt asked for the next one to be held off. See
    /// [`Emulator::deliver_until`].
    fn clear_retry_backoff(&self, stream: &str) {
        let state = self.engine.state::<
            service_com_amazonaws_firehose_firehose_20150804::ComAmazonawsFirehoseFirehose20150804State,
        >();
        let mut store = state.lock();
        if let Some(stream) = store.streams.get_mut(stream) {
            stream.delivery_retry_at = None;
        }
    }

    /// Wait for `ready` to hold without driving a delivery, so what makes it hold is the
    /// emulator's own delivery clock.
    async fn wait_until(&self, case: &str, expected: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "case {case:?}: {expected} within {DELIVERY_DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key
/// the router claims a request by. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The destination configuration of an HTTP endpoint at `url`, with the backup every row
/// shares.
///
/// `retry_seconds` is `RetryOptions$DurationInSeconds` and the buffering hints are the
/// smallest the members admit, so a case does not wait longer than the emulator's own
/// delivery span.
fn endpoint_configuration(url: &str, retry_seconds: i64) -> Value {
    json!({
        "EndpointConfiguration": { "Url": url, "Name": "the-sink" },
        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 60 },
        "RetryOptions": { "DurationInSeconds": retry_seconds },
        "S3BackupMode": "FailedDataOnly",
        "S3Configuration": {
            "RoleARN": ROLE_ARN,
            "BucketARN": BACKUP_BUCKET_ARN,
            "Prefix": "delivered/",
        },
    })
}

/// The URL of the endpoint one case delivers to. Nothing this emulator serves answers at
/// it, so the only transport that can acknowledge a delivery to it is the case's own
/// [`Sink`].
fn endpoint_url(case: &str) -> String {
    format!("https://sink.firehose.test/{case}/records")
}

// ---------------------------------------------------------------------------
// The envelope
// ---------------------------------------------------------------------------

/// One row of the envelope table: how the destination's request is configured, and the
/// records put into the stream.
struct EnvelopeRow {
    case: &'static str,
    /// `RequestConfiguration$ContentEncoding`, or `None` to configure no
    /// `RequestConfiguration` at all.
    content_encoding: Option<&'static str>,
    access_key: Option<&'static str>,
    common_attributes: &'static [(&'static str, &'static str)],
    payloads: &'static [&'static [u8]],
}

#[tokio::test]
async fn the_delivery_request_to_an_http_endpoint_carries_the_documented_envelope() {
    // Each row varies what the rule generalises over: how many records the buffer holds
    // and what they carry (text, JSON, arbitrary bytes, an empty blob — `Data`'s length is
    // 0 to 1,024,000), whether the body is gzipped, whether an access key is configured,
    // and how many common attributes are.
    let rows = [
        EnvelopeRow {
            case: "one-record",
            content_encoding: None,
            access_key: None,
            common_attributes: &[],
            payloads: &[b"the-first-record"],
        },
        EnvelopeRow {
            case: "three-records-with-attributes",
            content_encoding: Some("NONE"),
            access_key: Some("sink-access-key"),
            common_attributes: &[("environment", "test"), ("tenant", "acme")],
            payloads: &[b"alpha", b"beta", b"gamma"],
        },
        EnvelopeRow {
            case: "gzipped",
            content_encoding: Some("GZIP"),
            access_key: Some("another-access-key"),
            common_attributes: &[("source", "integration-suite")],
            payloads: &[b"{\"event\":\"one\"}", b"{\"event\":\"two\"}"],
        },
        EnvelopeRow {
            case: "binary-and-empty-records",
            content_encoding: Some("NONE"),
            access_key: None,
            common_attributes: &[],
            payloads: &[&[0x00, 0x9f, 0x92, 0x96, 0xff], b"", b"\n\t ascii \r\n"],
        },
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;

    // Every row's stream is created first, so the `CREATING` -> `ACTIVE` span is waited out
    // once for the whole table.
    let mut cases = Vec::new();
    for row in rows {
        let sink = Sink::answering(200);
        let url = endpoint_url(row.case);
        let mut config = endpoint_configuration(&url, 60);
        if let Some(encoding) = row.content_encoding {
            config["RequestConfiguration"] = json!({ "ContentEncoding": encoding });
        }
        if !row.common_attributes.is_empty() {
            let attributes: Vec<Value> = row
                .common_attributes
                .iter()
                .map(|(name, value)| {
                    json!({ "AttributeName": name, "AttributeValue": value })
                })
                .collect();
            config["RequestConfiguration"]["CommonAttributes"] = json!(attributes);
        }
        if let Some(access_key) = row.access_key {
            config["EndpointConfiguration"]["AccessKey"] = json!(access_key);
        }
        let stream_arn = emulator.create_endpoint_stream(row.case, config).await;
        cases.push((row, sink, url, stream_arn));
    }
    emulator.await_active_streams().await;

    for (row, sink, url, stream_arn) in cases {
        let transport = sink.transport();
        let before = epoch_millis(SystemTime::now());
        emulator.put_batch(row.case, row.payloads).await;
        emulator
            .deliver_until(
                row.case,
                row.case,
                &transport,
                "the endpoint is sent the buffer",
                || !sink.captured().is_empty(),
            )
            .await;
        let after = epoch_millis(SystemTime::now());

        let captured = sink.captured();
        let request = captured
            .first()
            .unwrap_or_else(|| panic!("case {:?}: a delivery request was captured", row.case));
        assert_eq!(
            request.method, "POST",
            "case {:?}: a delivery request is a POST, got {:?}",
            row.case, request.method
        );
        assert_eq!(
            request.uri, url,
            "case {:?}: the delivery goes to the configured Url, got {:?}",
            row.case, request.uri
        );
        assert_eq!(
            request.header("content-type"),
            Some("application/json"),
            "case {:?}: the envelope is JSON, got {:?}",
            row.case,
            request.header("content-type")
        );
        assert_eq!(
            request.header("x-amz-firehose-protocol-version"),
            Some("1.0"),
            "case {:?}: the request names the delivery protocol version, got {:?}",
            row.case,
            request.header("x-amz-firehose-protocol-version")
        );
        assert_eq!(
            request.header("x-amz-firehose-source-arn"),
            Some(stream_arn.as_str()),
            "case {:?}: the request names the stream the records came from, got {:?}",
            row.case,
            request.header("x-amz-firehose-source-arn")
        );
        assert_eq!(
            request.header("x-amz-firehose-access-key"),
            row.access_key,
            "case {:?}: the request carries the configured AccessKey and nothing else, got {:?}",
            row.case,
            request.header("x-amz-firehose-access-key")
        );
        let expected_encoding = (row.content_encoding == Some("GZIP")).then_some("gzip");
        assert_eq!(
            request.header("content-encoding"),
            expected_encoding,
            "case {:?}: the body is encoded as ContentEncoding asks, got {:?}",
            row.case,
            request.header("content-encoding")
        );
        let expected_attributes = row.common_attributes.iter().fold(
            serde_json::Map::new(),
            |mut common, (name, value)| {
                common.insert((*name).to_string(), json!(value));
                common
            },
        );
        let carried_attributes = request
            .header("x-amz-firehose-common-attributes")
            .map(|header| {
                serde_json::from_str::<Value>(header).unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: the common attributes header must be JSON, got {error} for \
                         {header:?}",
                        row.case
                    )
                })
            });
        if row.common_attributes.is_empty() {
            assert_eq!(
                carried_attributes, None,
                "case {:?}: a destination configuring no common attributes carries none, got \
                 {carried_attributes:?}",
                row.case
            );
        } else {
            assert_eq!(
                carried_attributes,
                Some(json!({ "commonAttributes": expected_attributes })),
                "case {:?}: the request carries the configured common attributes, got \
                 {carried_attributes:?}",
                row.case
            );
        }

        let envelope = request.envelope(row.case);
        let request_id = envelope["requestId"].as_str().unwrap_or_default();
        assert!(
            !request_id.is_empty(),
            "case {:?}: the envelope carries a requestId, got {envelope:?}",
            row.case
        );
        assert_eq!(
            request.header("x-amz-firehose-request-id"),
            Some(request_id),
            "case {:?}: the header and the envelope name one request, got {:?}",
            row.case,
            request.header("x-amz-firehose-request-id")
        );
        let timestamp = envelope["timestamp"].as_u64().unwrap_or_default();
        assert!(
            (before..=after).contains(&timestamp),
            "case {:?}: the envelope's timestamp is when the delivery was made, expected it \
             within {before}..={after}, got {timestamp}",
            row.case
        );
        let delivered: Vec<Vec<u8>> = envelope["records"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: the envelope carries records, got {envelope:?}", row.case))
            .iter()
            .map(|record| {
                let data = record["data"].as_str().unwrap_or_default();
                aws_smithy_types::base64::decode(data).unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: every record's data is base64, got {error} for {data:?}",
                        row.case
                    )
                })
            })
            .collect();
        let expected: Vec<Vec<u8>> = row.payloads.iter().map(|data| data.to_vec()).collect();
        assert_eq!(
            delivered, expected,
            "case {:?}: the envelope carries every buffered record in arrival order, got \
             {delivered:?}",
            row.case
        );
        assert!(
            emulator.buffered(row.case).is_empty(),
            "case {:?}: an acknowledged delivery leaves the buffer empty, got {:?}",
            row.case,
            emulator.buffered(row.case)
        );
        assert!(
            emulator.backup_objects().is_empty(),
            "case {:?}: a delivered buffer writes nothing to the failed-data backup, got {:?}",
            row.case,
            emulator.backup_objects().keys().collect::<Vec<_>>()
        );
    }
}

// ---------------------------------------------------------------------------
// The acknowledgment
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_a_success_status_acknowledges_a_delivery_to_an_http_endpoint() {
    // Each row: the status the endpoint answers with, and whether that acknowledges the
    // delivery. The whole 2xx class acknowledges and every other class does not, so the
    // rows sit on both sides of each boundary of it. The retry window is wide here, so a
    // delivery that is not acknowledged is one the records are still waiting for.
    let rows = [
        (200u16, true),
        (201, true),
        (204, true),
        (299, true),
        (199, false),
        (301, false),
        (400, false),
        (404, false),
        (500, false),
        (503, false),
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    let payloads: &[&[u8]] = &[b"first", b"second"];

    // Every row's stream is created first, so the `CREATING` -> `ACTIVE` span is waited out
    // once for the whole table.
    let mut cases = Vec::new();
    for (status, acknowledges) in rows {
        let case = format!("status-{status}");
        let sink = Sink::answering(status);
        let config = endpoint_configuration(&endpoint_url(&case), 60);
        emulator.create_endpoint_stream(&case, config).await;
        cases.push((case, sink, acknowledges));
    }
    emulator.await_active_streams().await;

    for (case, sink, acknowledges) in cases {
        let transport = sink.transport();
        emulator.put_batch(&case, payloads).await;
        emulator
            .deliver_until(
                &case,
                &case,
                &transport,
                "the endpoint is sent the buffer",
                || !sink.captured().is_empty(),
            )
            .await;

        if acknowledges {
            emulator
                .deliver_until(&case, &case, &transport, "the buffer is handed over", || {
                    emulator.buffered(&case).is_empty()
                })
                .await;
        } else {
            // The records are neither delivered nor lost: they stay buffered for the
            // retry window. The buffer is taken out of the store for the duration of an
            // attempt, so this waits for the restored buffer rather than reading once.
            let expected: Vec<Vec<u8>> = payloads.iter().map(|data| data.to_vec()).collect();
            emulator
                .wait_until(&case, "the records are back in the buffer", || {
                    emulator.buffered(&case) == expected
                })
                .await;
            assert!(
                emulator.backup_objects().is_empty(),
                "case {case:?}: nothing is parked in the backup while the retry window is \
                 open, got {:?}",
                emulator.backup_objects().keys().collect::<Vec<_>>()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The answer
// ---------------------------------------------------------------------------

/// One row of the acknowledgment table: how the endpoint answers a delivery request, and
/// whether that answer hands the buffer over.
struct AnswerRow {
    case: &'static str,
    status: u16,
    answer: Answer,
    acknowledges: bool,
}

#[tokio::test]
async fn only_an_answer_that_acknowledges_this_request_hands_a_buffer_over() {
    // Each row varies the answer's body around the one document the delivery protocol asks
    // a destination for: the request's own id, optionally beside the `timestamp` and the
    // `errorMessage` that document also carries. A success status is on BOTH sides of the
    // rule, so what the rows separate is the answer and not the status. The retry window is
    // wide, so a buffer an answer did not hand over is one the records are still waiting
    // for.
    let rows = [
        AnswerRow {
            case: "acknowledged",
            status: 200,
            answer: Answer::EchoesTheRequestId,
            acknowledges: true,
        },
        AnswerRow {
            case: "acknowledged-with-timestamp",
            status: 200,
            answer: Answer::EchoesTheRequestIdAndTimestamp,
            acknowledges: true,
        },
        AnswerRow {
            case: "acknowledged-with-no-error",
            status: 200,
            answer: Answer::EchoesTheRequestIdAndNoError,
            acknowledges: true,
        },
        AnswerRow {
            case: "acknowledged-at-201",
            status: 201,
            answer: Answer::EchoesTheRequestId,
            acknowledges: true,
        },
        AnswerRow {
            case: "reports-an-error",
            status: 200,
            answer: Answer::ReportsAnError,
            acknowledges: false,
        },
        AnswerRow {
            case: "reports-an-error-at-202",
            status: 202,
            answer: Answer::ReportsAnError,
            acknowledges: false,
        },
        AnswerRow {
            case: "names-another-request",
            status: 200,
            answer: Answer::NamesAnotherRequest,
            acknowledges: false,
        },
        AnswerRow {
            case: "names-no-request",
            status: 200,
            answer: Answer::NamesNoRequest,
            acknowledges: false,
        },
        AnswerRow {
            case: "answers-what-is-not-json",
            status: 200,
            answer: Answer::NotJson,
            acknowledges: false,
        },
        AnswerRow {
            case: "answers-no-body",
            status: 200,
            answer: Answer::Nothing,
            acknowledges: false,
        },
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    let payloads: &[&[u8]] = &[b"first", b"second"];

    // Every row's stream is created first, so the `CREATING` -> `ACTIVE` span is waited out
    // once for the whole table.
    let mut cases = Vec::new();
    for row in rows {
        let sink = Sink::answering_with(row.status, row.answer);
        let config = endpoint_configuration(&endpoint_url(row.case), 60);
        emulator.create_endpoint_stream(row.case, config).await;
        cases.push((row, sink));
    }
    emulator.await_active_streams().await;

    for (row, sink) in cases {
        let transport = sink.transport();
        emulator.put_batch(row.case, payloads).await;
        emulator
            .deliver_until(
                row.case,
                row.case,
                &transport,
                "the endpoint is sent the buffer",
                || !sink.captured().is_empty(),
            )
            .await;

        if row.acknowledges {
            emulator
                .deliver_until(
                    row.case,
                    row.case,
                    &transport,
                    "the buffer is handed over",
                    || emulator.buffered(row.case).is_empty(),
                )
                .await;
        } else {
            // The records are neither delivered nor lost: they stay buffered for the
            // retry window. The buffer is taken out of the store for the duration of an
            // attempt, so this waits for the restored buffer rather than reading once.
            let expected: Vec<Vec<u8>> = payloads.iter().map(|data| data.to_vec()).collect();
            emulator
                .wait_until(row.case, "the records are back in the buffer", || {
                    emulator.buffered(row.case) == expected
                })
                .await;
            assert!(
                emulator.backup_objects().is_empty(),
                "case {:?}: nothing is parked in the backup while the retry window is open, \
                 got {:?}",
                row.case,
                emulator.backup_objects().keys().collect::<Vec<_>>()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The backup
// ---------------------------------------------------------------------------

#[tokio::test]
async fn records_an_http_endpoint_never_took_are_parked_in_the_destination_backup() {
    // Each row: the `ErrorOutputPrefix` the backup configures, and the prefix the parked
    // object is then written under — the configured one, or the error output type itself
    // when none is configured. The endpoint of each row is a URL nothing in this emulator
    // serves and the retry window is zero (`DurationInSeconds`'s own minimum), so the
    // delivery is attempted once, fails, and the records are parked by the emulator's own
    // delivery clock without any further call.
    let rows = [
        ("no-error-prefix", None, format!("{FAILED_OUTPUT_TYPE}/")),
        (
            "configured-error-prefix",
            Some("failed/"),
            "failed/".to_string(),
        ),
        (
            "error-prefix-with-output-type",
            Some("failed/!{firehose:error-output-type}/"),
            format!("failed/{FAILED_OUTPUT_TYPE}/"),
        ),
    ];

    let emulator = Emulator::new();
    emulator.create_bucket(BACKUP_BUCKET).await;
    let payloads: &[&[u8]] = &[b"one", b"two", b"three"];

    // Every row's stream is created first, so the `CREATING` -> `ACTIVE` span is waited out
    // once for the whole table.
    for (case, error_output_prefix, _) in &rows {
        let mut config = endpoint_configuration(&endpoint_url(case), 0);
        if let Some(prefix) = error_output_prefix {
            config["S3Configuration"]["ErrorOutputPrefix"] = json!(prefix);
        }
        emulator.create_endpoint_stream(case, config).await;
    }
    emulator.await_active_streams().await;

    for (case, _, expected_prefix) in rows {
        emulator.put_batch(case, payloads).await;
        emulator
            .wait_until(case, "the records are parked in the backup", || {
                emulator
                    .backup_objects()
                    .keys()
                    .any(|key| key.starts_with(&expected_prefix))
            })
            .await;

        let parked = emulator.backup_objects();
        let (key, body) = parked
            .iter()
            .find(|(key, _)| key.starts_with(&expected_prefix))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: an object under {expected_prefix:?} holds the failed \
                     records, got {:?}",
                    parked.keys().collect::<Vec<_>>()
                )
            });
        assert!(
            key.starts_with(&expected_prefix),
            "case {case:?}: the failed records are written under {expected_prefix:?}, got {key:?}"
        );
        // One error-output document per record, each naming how the delivery failed and
        // carrying the record's own bytes, so nothing of a parked record is lost.
        let documents: Vec<Value> = String::from_utf8_lossy(body)
            .lines()
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: every parked line is a JSON document, got {error}")
                })
            })
            .collect();
        assert_eq!(
            documents.len(),
            payloads.len(),
            "case {case:?}: one document per failed record, got {documents:?}"
        );
        for (document, payload) in documents.iter().zip(payloads) {
            let code = document["errorCode"].as_str().unwrap_or_default();
            assert!(
                FAILED_ERROR_CODES.contains(&code),
                "case {case:?}: the document names how the HTTP endpoint delivery failed, \
                 expected one of {FAILED_ERROR_CODES:?}, got {code:?}"
            );
            let raw = document["rawData"].as_str().unwrap_or_default();
            let decoded = aws_smithy_types::base64::decode(raw).unwrap_or_else(|error| {
                panic!("case {case:?}: rawData is base64, got {error} for {raw:?}")
            });
            assert_eq!(
                decoded,
                payload.to_vec(),
                "case {case:?}: the document carries the record's own bytes, got {decoded:?}"
            );
            assert!(
                document["arrivalTimestamp"].as_u64().unwrap_or_default() > 0,
                "case {case:?}: the document says when the record arrived, got {document:?}"
            );
        }
        assert!(
            emulator.buffered(case).is_empty(),
            "case {case:?}: a parked buffer is no longer waiting for the endpoint, got {:?}",
            emulator.buffered(case)
        );
    }
}

/// `at` as epoch milliseconds, the resolution the envelope's `timestamp` carries.
fn epoch_millis(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis()
        .try_into()
        .expect("the instant fits in a u64 of milliseconds")
}
