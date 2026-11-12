// SPDX-License-Identifier: CC0-1.0

//! The CloudWatch Logs pipeline of a Firehose destination, from the put that feeds it to the
//! object it delivers.
//!
//! The rule under test, in two halves:
//!
//! * WHO may feed such a stream. A stream whose destination has data processing enabled with
//!   a processor of the CloudWatch Logs pipeline takes records from CloudWatch Logs — "Only
//!   requests from CloudWatch Logs are supported when CloudWatch Logs decompression is
//!   enabled" (`InvalidSourceException`) — so a put made BY that service is accepted, and
//!   one made by any other producer is refused with HTTP 400. Which producer a put came from
//!   is what the credential it presents says: CloudWatch Logs forwards a subscription
//!   filter's log events as its own service principal, acting for the log group they were
//!   ingested into, and records that under the access key id it signs the call with
//!   (`service_common::service_principal_calls`).
//! * WHAT the delivered object then holds. `Decompression` inflates each record out of the
//!   gzip frame CloudWatch Logs delivers it in, and `CloudWatchLogProcessing` with
//!   `DataMessageExtraction` reduces each inflated envelope to the `message` of each log
//!   event it carries, one per line — dropping an envelope that carries no log event of the
//!   account, and writing a record it cannot read to the destination's error output instead.
//!
//! The rows span what the rule generalises over: which parts of the pipeline the destination
//! enables (none, both but disabled, decompression alone, the whole pipeline), how many
//! events an envelope carries, which kind of envelope it is, a record the pipeline cannot
//! read, and both ingestion operations.

use std::io::Write;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::delivery_stream_lifecycle::{error_code, BUCKET_ARN, REGION, ROLE_ARN};
use crate::destination_delivery::Emulator;
use crate::refusal_messages::error_message;

/// The bucket every row delivers into, the one [`BUCKET_ARN`] names — each row stands up
/// its own emulator, so one name serves them all.
const BUCKET: &str = "demo-bucket";

/// Where a row's delivered objects go, and where the records its processing could not read
/// go instead.
const DATA_PREFIX: &str = "data/";
const ERROR_PREFIX: &str = "failed/";

/// The access key id every request of this suite presents, which is the one the harness
/// signs with: a put is a CloudWatch Logs put exactly while the shared directory holds a
/// record of a service-principal call under it.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";

/// The service principal CloudWatch Logs forwards a subscription filter's events as.
const LOGS_SERVICE_PRINCIPAL: &str = "logs.amazonaws.com";

/// How long a delivery is waited for before a row fails: far longer than the emulator's own
/// delivery span.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// How often the destination is polled while waiting for a delivery.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Comfortably past the window a buffer created with `IntervalInSeconds` 0 is held for
/// (`state::delivery::FULL_BUFFER_DELIVERY_SPAN`), so a
/// read that finds nothing is reading a delivery that was never owed rather than one the
/// clock has not reached.
const PAST_THE_DELIVERY_WINDOW: Duration = Duration::from_millis(2_000);

/// `body` in one gzip frame — the framing a CloudWatch Logs subscription payload arrives in
/// ("they are Base64 encoded and compressed with the GZIP format").
fn gzip(body: &str) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(body.as_bytes())
        .expect("writing to a Vec cannot fail");
    encoder.finish().expect("the frame finishes over a Vec")
}

/// One CloudWatch Logs envelope document of `message_type`, carrying one log event per
/// message.
fn envelope(message_type: &str, messages: &[&str]) -> String {
    json!({
        "messageType": message_type,
        "owner": engine::emulator_account_id(),
        "logGroup": "/aws/emulator/pipeline",
        "logStream": "instance-one",
        "subscriptionFilters": ["to-firehose"],
        "logEvents": messages
            .iter()
            .enumerate()
            .map(|(ordinal, message)| json!({
                "id": format!("{ordinal:0>56}"),
                "timestamp": 1_767_225_600_000_i64,
                "message": message,
            }))
            .collect::<Vec<Value>>(),
    })
    .to_string()
}

/// The `Decompression` processor, naming the format CloudWatch Logs frames its payload in.
/// Source: `smithy_query(query="shape", shape="ProcessorType,ProcessorParameterName")`.
fn decompression() -> Value {
    json!({
        "Type": "Decompression",
        "Parameters": [{ "ParameterName": "CompressionFormat", "ParameterValue": "GZIP" }],
    })
}

/// The `CloudWatchLogProcessing` processor, with its extraction switch on.
fn message_extraction() -> Value {
    json!({
        "Type": "CloudWatchLogProcessing",
        "Parameters": [{ "ParameterName": "DataMessageExtraction", "ParameterValue": "true" }],
    })
}

/// A `ProcessingConfiguration` over `processors`, enabled or not.
fn processing(enabled: bool, processors: Vec<Value>) -> Value {
    json!({ "Enabled": enabled, "Processors": processors })
}

/// Which ingestion operation a row puts its records through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Put {
    One,
    Batch,
}

/// One row: a destination's processing, the records CloudWatch Logs puts, and what the
/// delivery then holds.
struct Row {
    case: &'static str,
    /// The destination's `ProcessingConfiguration`, or `None` for one that configures none.
    processing: Option<Value>,
    /// The records the put carries, in order.
    records: Vec<Vec<u8>>,
    put: Put,
    /// The bytes every delivered object under [`DATA_PREFIX`] holds, concatenated in key
    /// order. Empty for a row whose records the processing left nothing of.
    delivered: Vec<u8>,
    /// How many records are written to [`ERROR_PREFIX`] instead.
    failed: usize,
    /// Whether a put from an ordinary API producer is refused on this destination.
    refuses_api_producer: bool,
}

fn rows() -> Vec<Row> {
    let one = envelope("DATA_MESSAGE", &["ERROR disk full"]);
    let two = envelope("DATA_MESSAGE", &["ERROR disk full", "WARN disk filling"]);
    let control = envelope(
        "CONTROL_MESSAGE",
        &["CWL CONTROL MESSAGE: Checking health of destination Firehose."],
    );
    vec![
        Row {
            case: "a destination configuring no data processing keeps the frames",
            processing: None,
            records: vec![gzip(&one), gzip(&two)],
            put: Put::Batch,
            delivered: [gzip(&one), gzip(&two)].concat(),
            failed: 0,
            refuses_api_producer: false,
        },
        Row {
            case: "the whole pipeline configured but disabled keeps the frames",
            processing: Some(processing(
                false,
                vec![decompression(), message_extraction()],
            )),
            records: vec![gzip(&two)],
            put: Put::One,
            delivered: gzip(&two),
            failed: 0,
            refuses_api_producer: false,
        },
        Row {
            case: "decompression alone delivers the envelope documents",
            processing: Some(processing(true, vec![decompression()])),
            records: vec![gzip(&one), gzip(&two)],
            put: Put::Batch,
            delivered: format!("{one}{two}").into_bytes(),
            failed: 0,
            refuses_api_producer: true,
        },
        Row {
            case: "the whole pipeline delivers each event's message on its own line",
            processing: Some(processing(
                true,
                vec![decompression(), message_extraction()],
            )),
            records: vec![gzip(&two)],
            put: Put::One,
            delivered: b"ERROR disk full\nWARN disk filling\n".to_vec(),
            failed: 0,
            refuses_api_producer: true,
        },
        Row {
            case: "a control message carries no log event to extract and is dropped",
            processing: Some(processing(
                true,
                vec![decompression(), message_extraction()],
            )),
            records: vec![gzip(&control), gzip(&one), gzip(&control)],
            put: Put::Batch,
            delivered: b"ERROR disk full\n".to_vec(),
            failed: 0,
            refuses_api_producer: true,
        },
        Row {
            case: "a record the pipeline cannot read goes to the error output",
            processing: Some(processing(
                true,
                vec![decompression(), message_extraction()],
            )),
            records: vec![b"not a gzip frame".to_vec(), gzip(&one)],
            put: Put::Batch,
            delivered: b"ERROR disk full\n".to_vec(),
            failed: 1,
            refuses_api_producer: true,
        },
        Row {
            case: "a frame that is not an envelope document goes to the error output",
            processing: Some(processing(
                true,
                vec![decompression(), message_extraction()],
            )),
            records: vec![gzip("plain text, not a document"), gzip(&one)],
            put: Put::Batch,
            delivered: b"ERROR disk full\n".to_vec(),
            failed: 1,
            refuses_api_producer: true,
        },
        Row {
            case: "every record of a put the pipeline drops leaves no object at all",
            processing: Some(processing(
                true,
                vec![decompression(), message_extraction()],
            )),
            records: vec![gzip(&control)],
            put: Put::One,
            delivered: Vec::new(),
            failed: 0,
            refuses_api_producer: true,
        },
    ]
}

/// The directory of in-flight service-principal calls this emulator holds.
fn in_flight_calls(
    emulator: &Emulator,
) -> engine::ServiceState<service_common::service_principal_calls::ServicePrincipalCalls> {
    emulator
        .engine()
        .state::<service_common::service_principal_calls::ServicePrincipalCalls>()
}

/// Record that the calls signed with [`ACCESS_KEY_ID`] are CloudWatch Logs forwarding a
/// subscription filter's log events, for as long as the returned guard lives.
fn as_cloudwatch_logs(emulator: &Emulator) -> CloudWatchLogsCaller<'_> {
    let now = std::time::SystemTime::now();
    in_flight_calls(emulator).lock().remember(
        ACCESS_KEY_ID,
        service_common::service_principal_calls::ServicePrincipalCall {
            principal: LOGS_SERVICE_PRINCIPAL.to_string(),
            source_arn: format!(
                "arn:aws:logs:{REGION}:{}:log-group:/aws/emulator/pipeline",
                engine::emulator_account_id()
            ),
            source_account: engine::emulator_account_id().to_string(),
            expires_at: now + Duration::from_secs(60),
        },
        now,
    );
    CloudWatchLogsCaller { emulator }
}

/// Holds the record above for one put, and forgets it again — so the SAME harness makes an
/// ordinary API put once it is dropped.
struct CloudWatchLogsCaller<'e> {
    emulator: &'e Emulator,
}

impl Drop for CloudWatchLogsCaller<'_> {
    fn drop(&mut self) {
        in_flight_calls(self.emulator).lock().forget(ACCESS_KEY_ID);
    }
}

/// Every delivered object under `prefix`, concatenated in key order, once `want` of them are
/// there — or a failure naming the row once [`DELIVERY_DEADLINE`] has passed.
async fn delivered(emulator: &Emulator, case: &str, prefix: &str, want: usize) -> Vec<u8> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let mut keys = emulator.keys_under(BUCKET, prefix).await;
        keys.sort();
        if keys.len() >= want {
            let mut body = Vec::new();
            for key in &keys {
                body.extend_from_slice(&emulator.object(BUCKET, key).await);
            }
            return body;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {want} object(s) under {prefix:?}, got {keys:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[tokio::test]
async fn the_pipeline_takes_a_cloudwatch_logs_put_and_delivers_what_its_processors_leave() {
    for row in rows() {
        let case = row.case;
        let emulator = Emulator::new();
        emulator.create_bucket(BUCKET).await;
        let mut destination = json!({
            "RoleARN": ROLE_ARN,
            "BucketARN": BUCKET_ARN,
            "Prefix": DATA_PREFIX,
            "ErrorOutputPrefix": ERROR_PREFIX,
            // The shortest buffering the hints admit, so one put is one object.
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
        });
        if let Some(processing) = &row.processing {
            destination["ProcessingConfiguration"] = processing.clone();
        }
        emulator
            .create_active_stream("pipeline", "ExtendedS3DestinationConfiguration", destination)
            .await;

        // An ordinary API producer: accepted only where the destination enables none of the
        // pipeline, and refused with the modelled `InvalidSourceException` where it does.
        let (status, answered) = emulator.put_record("pipeline", b"an api record").await;
        if row.refuses_api_producer {
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "InvalidSourceException"),
                "case {case:?}: an API put must be refused, got {status} {answered}"
            );
            assert_eq!(
                error_message(&answered),
                format!(
                    "Put to Firehose failed for AccountId: {}, FirehoseName: pipeline \
                     because the request is not originating from CloudWatch.",
                    engine::emulator_account_id()
                ),
                "case {case:?}: expected the data plane's own refusal, got {answered}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {case:?}: a destination enabling none of the pipeline takes an API \
                 put, got {status} {answered}"
            );
        }

        // The same records, put BY CloudWatch Logs: taken whatever the destination enables.
        {
            let _caller = as_cloudwatch_logs(&emulator);
            let (status, answered) = match row.put {
                Put::One => {
                    let mut last = (0, Value::Null);
                    for record in &row.records {
                        last = emulator.put_record("pipeline", record).await;
                        assert_eq!(
                            last.0, 200,
                            "case {case:?}: a CloudWatch Logs put must be taken, got {} {}",
                            last.0, last.1
                        );
                    }
                    last
                }
                Put::Batch => emulator.put_batch("pipeline", &row.records).await,
            };
            assert_eq!(
                status, 200,
                "case {case:?}: a CloudWatch Logs put must be taken, got {status} {answered}"
            );
            if row.put == Put::Batch {
                assert_eq!(
                    answered["FailedPutCount"],
                    json!(0),
                    "case {case:?}: every record of the batch was taken, got {answered}"
                );
            }
        }

        // What the destination's processing left of them. A row the API put above was
        // ACCEPTED for carries that record in the same buffer, so its delivered object holds
        // it beside the row's own records.
        let want_data = usize::from(!row.delivered.is_empty() || !row.refuses_api_producer);
        if want_data > 0 {
            let body = delivered(&emulator, case, DATA_PREFIX, want_data).await;
            let expected = if row.refuses_api_producer {
                row.delivered.clone()
            } else {
                [b"an api record".to_vec(), row.delivered.clone()].concat()
            };
            assert_eq!(
                String::from_utf8_lossy(&body),
                String::from_utf8_lossy(&expected),
                "case {case:?}: expected the delivered object to hold {:?}, got {:?}",
                String::from_utf8_lossy(&expected),
                String::from_utf8_lossy(&body)
            );
        } else {
            // Nothing was left to deliver, so the destination takes no object at all: an
            // empty one is not something a delivery writes. Read after the buffer's own
            // window, so this is the absence of a delivery rather than one still owed.
            tokio::time::sleep(PAST_THE_DELIVERY_WINDOW).await;
            let keys = emulator.keys_under(BUCKET, "").await;
            assert!(
                keys.is_empty(),
                "case {case:?}: expected no object at all, got {keys:?}"
            );
        }

        if row.failed > 0 {
            let body = delivered(&emulator, case, ERROR_PREFIX, 1).await;
            let text = String::from_utf8_lossy(&body).to_string();
            let documents = text.lines().count();
            assert_eq!(
                documents, row.failed,
                "case {case:?}: expected {} error-output document(s), got {text:?}",
                row.failed
            );
            assert!(
                text.contains("\"errorCode\":\"DataProcessing.ProcessingFailed\"")
                    && text.contains("\"rawData\":"),
                "case {case:?}: an error-output document names the failure and carries the \
                 record's own bytes, got {text:?}"
            );
        }
    }
}
