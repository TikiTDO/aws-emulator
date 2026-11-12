// SPDX-License-Identifier: CC0-1.0

//! A configuration set's Kinesis Data Firehose event destination MOVES DATA: a message sent
//! through that set lands its email events as records on the stream the destination names,
//! and the stream's own Amazon S3 destination then writes them as objects a client reads
//! back.
//!
//! The rule under test: for each event an accepted message produces, every event destination
//! of the set the message named that is `Enabled` and whose `MatchingEventTypes` names that
//! event's type receives one notification of it, and every other destination receives
//! nothing. A `KinesisFirehoseDestination` receives it as a record put on the stream its
//! `DeliveryStreamArn` names ("You can use Amazon Kinesis Data Firehose to stream data to
//! other services, such as Amazon S3" — `KinesisFirehoseDestination`), so what a client
//! finally observes is the stream's delivered object, not an answer this service made up
//! about it.
//!
//! Only a cross-service suite can ask this: the flow spans four services over ONE engine
//! exactly as `core/router/src/lib.rs` mounts them — SESv2 for the identity, the set, the
//! destination and the send, IAM for the roles the publish and the delivery are made under,
//! Firehose for the stream, and Amazon S3 for the objects. A crate-local test only ever sees
//! one side of each call.
//!
//! The rows differ along what the rule generalises over: which event types the destination
//! matches (the acceptance alone, the acceptance with the delivery, a failure alone), what
//! the recipient of the message does with it (an ordinary mailbox, which takes it, and the
//! mailbox simulator's bounce address, which refuses it permanently), how many messages are
//! sent through the set, and whether the destination is enabled at all. Every row asserts
//! the records the destination must receive AND that it received no record of an event type
//! it does not match, so no row can pass by publishing everything.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::{element, elements, form_encode};

/// The Region every call below is signed for, and the one every ARN here names.
const REGION: &str = "us-east-1";

/// IAM's API version, which an `awsQuery` body carries beside its `Action`.
const IAM_API_VERSION: &str = "2010-05-08";

/// The bucket every stream here delivers into.
const BUCKET: &str = "ses-event-publication";

/// The role Firehose assumes to write a delivered object into that bucket.
const DELIVERY_ROLE: &str = "ses-firehose-delivery";

/// The role SESv2 sends a configuration set's events under
/// (`KinesisFirehoseDestination$IamRoleArn`).
const PUBLISHING_ROLE: &str = "ses-event-publishing";

/// The verified sending identity every message below is sent from, and an address inside
/// it.
const IDENTITY: &str = "publication.example.com";
const SENDER: &str = "events@publication.example.com";

/// The mailbox simulator's address for a recipient that refuses a message permanently:
/// "bounce@simulator.amazonses.com" is answered by SES itself, which is what makes the
/// failure deterministic.
const BOUNCE_MAILBOX: &str = "bounce@simulator.amazonses.com";

/// The trust policy the delivery role carries: Firehose assumes it to deliver.
const DELIVERY_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"#,
    r#""firehose.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The trust policy the publishing role carries: SES sends the set's events under it.
const PUBLISHING_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"#,
    r#""ses.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The permissions each role needs: the record write into the stream
/// (`KinesisFirehoseDestination$IamRoleArn`, the role SES "uses to send email events to the
/// Amazon Kinesis Data Firehose stream") and the object write the delivery itself makes.
const ROLE_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"#,
    r#"["firehose:PutRecord","firehose:PutRecordBatch","s3:PutObject"],"Resource":"*"}]}"#
);

/// How long a condition the clock owes is waited for before a case fails: far longer than
/// the spans this emulator chose for a stream becoming `ACTIVE`, an identity's verification
/// and a buffer's delivery, so a loaded machine does not fail a case the clock would have
/// served.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How often a condition is re-read while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One row: the destination's matching event types, what is sent through the set, and the
/// records the destination must then receive.
struct Row {
    case: &'static str,
    /// Names this row's stream, prefix, configuration set and destination, so no row reads
    /// another's records.
    slug: &'static str,
    /// `EventDestinationDefinition$MatchingEventTypes`.
    matching_event_types: &'static [&'static str],
    /// `EventDestinationDefinition$Enabled`.
    enabled: bool,
    /// One recipient per message the row sends through the set.
    recipients: &'static [&'static str],
    /// The `eventType` of each record the destination must receive, ascending — the
    /// published spelling of an event, which is not the modelled `EventType` value
    /// (`SEND` -> `Send`).
    expected: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "two messages through a destination matching the acceptance",
        slug: "fh-send",
        matching_event_types: &["SEND"],
        enabled: true,
        recipients: &["first@example.net", "second@example.net"],
        expected: &["Send", "Send"],
    },
    Row {
        case: "one message through a destination matching the acceptance and the delivery",
        slug: "fh-send-delivery",
        matching_event_types: &["SEND", "DELIVERY"],
        enabled: true,
        recipients: &["third@example.net"],
        expected: &["Delivery", "Send"],
    },
    Row {
        case: "one message to a mailbox that refuses it, matching the failure alone",
        slug: "fh-bounce",
        matching_event_types: &["BOUNCE"],
        enabled: true,
        recipients: &[BOUNCE_MAILBOX],
        expected: &["Bounce"],
    },
    Row {
        case: "a destination that is off, matching the acceptance",
        slug: "fh-off",
        matching_event_types: &["SEND"],
        enabled: false,
        recipients: &["fourth@example.net"],
        expected: &[],
    },
];

/// The subject of message `index` of the row `slug`: distinct per row and per message, so an
/// assertion names exactly which message's event it did or did not find.
fn subject(slug: &str, index: usize) -> String {
    format!("{slug} message {index}")
}

/// The emulator the suite drives: the four services the flow spans, on ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's concern, not any of these services'.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The signature
/// is never checked: this router runs with authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One request to the service `signing_name` serves, at the endpoint host it answers on.
async fn send(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> (u16, Vec<u8>) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host)
        .header("authorization", authorization(signing_name));
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (response.status().as_u16(), response.body().to_vec())
}

/// One SESv2 call: `restJson1` under the `ses` credential scope, at the `email` endpoint.
async fn sesv2(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value, String) {
    let payload = match &body {
        Some(value) => value.to_string().into_bytes(),
        None => Vec::new(),
    };
    let (status, answered) = send(
        router,
        "ses",
        &format!("email.{REGION}.amazonaws.com"),
        method,
        path,
        &[("content-type", "application/json")],
        payload,
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    let json = serde_json::from_str(&text).unwrap_or(Value::Null);
    (status, json, text)
}

/// One SESv2 call that must be answered 200.
async fn sesv2_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Value {
    let (status, json, text) = sesv2(router, method, path, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: SESv2 {method} {path} must succeed, got {status} {text}"
    );
    json
}

/// One Firehose call that must succeed: `awsJson1_1`, `POST /` with the operation in
/// `X-Amz-Target`.
async fn firehose_ok(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, answered) = send(
        router,
        "firehose",
        &format!("firehose.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", &format!("Firehose_20150804.{operation}")),
        ],
        body.to_string().into_bytes(),
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: Firehose {operation} must succeed, got {status} {text}"
    );
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

/// One `awsQuery` IAM action that must succeed, answering the XML document a query client
/// reads.
async fn iam_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    parameters: &[(&str, &str)],
) -> String {
    let mut body = format!("Action={action}&Version={IAM_API_VERSION}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = send(
        router,
        "iam",
        &format!("iam.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    )
    .await;
    let text = String::from_utf8_lossy(&answered).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: IAM {action} must succeed, got {status} {text}"
    );
    text
}

/// Create `role_name` with `trust` as its trust policy and the permissions both writes
/// need, answering the role ARN IAM minted for it.
async fn given_role(router: &MultiServiceRouter, role_name: &str, trust: &str) -> String {
    let created = iam_ok(
        router,
        "setup",
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    iam_ok(
        router,
        "setup",
        "PutRolePolicy",
        &[
            ("RoleName", role_name),
            ("PolicyName", "publication"),
            ("PolicyDocument", ROLE_POLICY),
        ],
    )
    .await;
    let arn = element(&created, "Arn");
    assert!(
        arn.ends_with(&format!(":role/{role_name}")),
        "setup: CreateRole({role_name}) must answer the ARN it minted, got {arn}"
    );
    arn
}

/// Every key the bucket holds under `prefix`, read through `ListObjectsV2`.
async fn keys_under(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<String> {
    let (status, body) = send(
        router,
        "s3",
        &format!("s3.{REGION}.amazonaws.com"),
        "GET",
        &format!("/{BUCKET}?list-type=2&prefix={prefix}"),
        &[],
        Vec::new(),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status, 200,
        "case {case:?}: listing {prefix:?} must succeed, got {status} {text}"
    );
    let mut keys = elements(&text, "Key");
    keys.sort();
    keys
}

/// Every record the bucket holds under `prefix`, in key order: a delivered object is the
/// records of one buffer concatenated, and each record is one JSON document on its own line.
async fn records_under(router: &MultiServiceRouter, case: &str, prefix: &str) -> Vec<Value> {
    let mut records = Vec::new();
    for key in keys_under(router, case, prefix).await {
        let (status, body) = send(
            router,
            "s3",
            &format!("s3.{REGION}.amazonaws.com"),
            "GET",
            &format!("/{BUCKET}/{key}"),
            &[],
            Vec::new(),
        )
        .await;
        let text = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "case {case:?}: reading {key:?} must succeed, got {status} {text}"
        );
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let record: Value = serde_json::from_str(line).unwrap_or_else(|error| {
                panic!(
                    "case {case:?}: every line of {key:?} must be a published event document, \
                     got {line:?}: {error}"
                )
            });
            records.push(record);
        }
    }
    records
}

/// The records under `prefix` once at least `want` of them are there — or a failure naming
/// the case once [`SETTLE_DEADLINE`] has passed.
async fn delivered_records(
    router: &MultiServiceRouter,
    case: &str,
    prefix: &str,
    want: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let records = records_under(router, case, prefix).await;
        if records.len() >= want {
            return records;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected at least {want} published event record(s) under {prefix:?}, \
             got {records:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until the stream `name` reports `ACTIVE`, the state a stream takes records in.
async fn given_active_stream(router: &MultiServiceRouter, case: &str, name: &str) {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = firehose_ok(
            router,
            case,
            "DescribeDeliveryStream",
            json!({"DeliveryStreamName": name}),
        )
        .await;
        let status = described["DeliveryStreamDescription"]["DeliveryStreamStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status == "ACTIVE" {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stream {name:?} must become ACTIVE, got {status:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until the sending identity reports that it is verified for sending, which is what a
/// send from an address inside it needs.
async fn given_verified_identity(router: &MultiServiceRouter) {
    sesv2_ok(
        router,
        "setup",
        "POST",
        "/v2/email/identities",
        Some(json!({"EmailIdentity": IDENTITY})),
    )
    .await;
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = sesv2_ok(
            router,
            "setup",
            "GET",
            &format!("/v2/email/identities/{IDENTITY}"),
            None,
        )
        .await;
        if read["VerifiedForSendingStatus"].as_bool() == Some(true) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the identity {IDENTITY:?} must become verified for sending, got {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `eventType` of each record, ascending, as the rows state what a destination receives.
fn event_types(records: &[Value]) -> Vec<String> {
    let mut types: Vec<String> = records
        .iter()
        .map(|record| {
            record["eventType"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    types.sort();
    types
}

#[tokio::test]
async fn a_firehose_event_destination_publishes_a_sets_events_into_its_stream() {
    let router = emulator();
    let delivery_role = given_role(&router, DELIVERY_ROLE, DELIVERY_TRUST).await;
    let publishing_role = given_role(&router, PUBLISHING_ROLE, PUBLISHING_TRUST).await;
    let (status, created_bucket) = send(
        &router,
        "s3",
        &format!("s3.{REGION}.amazonaws.com"),
        "PUT",
        &format!("/{BUCKET}"),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "setup: the destination bucket must be created, got {status} {}",
        String::from_utf8_lossy(&created_bucket)
    );
    given_verified_identity(&router).await;

    // Every row's stream, set and destination, created together so one wait covers the span
    // the creates armed.
    for row in ROWS {
        let case = row.case;
        let stream = format!("{}-stream", row.slug);
        let created = firehose_ok(
            &router,
            case,
            "CreateDeliveryStream",
            json!({
                "DeliveryStreamName": stream,
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": {
                    "RoleARN": delivery_role,
                    "BucketARN": format!("arn:aws:s3:::{BUCKET}"),
                    "Prefix": format!("{}/", row.slug),
                    "CompressionFormat": "UNCOMPRESSED",
                    // The shortest buffering the hints admit, so a row waits for this
                    // emulator's own delivery span and nothing longer.
                    "BufferingHints": {"SizeInMBs": 1, "IntervalInSeconds": 0},
                },
            }),
        )
        .await;
        let stream_arn = created["DeliveryStreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a DeliveryStreamARN, got {created}"))
            .to_string();

        sesv2_ok(
            &router,
            case,
            "POST",
            "/v2/email/configuration-sets",
            Some(json!({"ConfigurationSetName": format!("{}-set", row.slug)})),
        )
        .await;
        sesv2_ok(
            &router,
            case,
            "POST",
            &format!(
                "/v2/email/configuration-sets/{}-set/event-destinations",
                row.slug
            ),
            Some(json!({
                "EventDestinationName": format!("{}-destination", row.slug),
                "EventDestination": {
                    "Enabled": row.enabled,
                    "MatchingEventTypes": row.matching_event_types,
                    "KinesisFirehoseDestination": {
                        "IamRoleArn": publishing_role,
                        "DeliveryStreamArn": stream_arn,
                    },
                },
            })),
        )
        .await;
    }
    for row in ROWS {
        given_active_stream(&router, row.case, &format!("{}-stream", row.slug)).await;
    }

    // The sends: one message per recipient, each through its row's own set.
    let mut sent: BTreeMap<&str, Vec<(String, String)>> = BTreeMap::new();
    for row in ROWS {
        for (index, recipient) in row.recipients.iter().enumerate() {
            let subject = subject(row.slug, index);
            let answered = sesv2_ok(
                &router,
                row.case,
                "POST",
                "/v2/email/outbound-emails",
                Some(json!({
                    "FromEmailAddress": SENDER,
                    "Destination": {"ToAddresses": [recipient]},
                    "ConfigurationSetName": format!("{}-set", row.slug),
                    "Content": {"Simple": {
                        "Subject": {"Data": subject},
                        "Body": {"Text": {"Data": "the body of a published message"}},
                    }},
                })),
            )
            .await;
            let message_id = answered["MessageId"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: SendEmail must answer the MessageId it minted, got {answered}",
                        row.case
                    )
                })
                .to_string();
            sent.entry(row.slug)
                .or_default()
                .push((message_id, subject));
        }
    }

    // The enabled rows first: each must have received exactly the events its destination
    // matches, about the messages its own set carried.
    for row in ROWS.iter().filter(|row| !row.expected.is_empty()) {
        let case = row.case;
        let prefix = format!("{}/", row.slug);
        let records = delivered_records(&router, case, &prefix, row.expected.len()).await;
        assert_eq!(
            event_types(&records),
            row.expected,
            "case {case:?}: expected exactly the events the destination matches, got {records:?}"
        );
        let messages = sent.get(row.slug).cloned().unwrap_or_default();
        for record in &records {
            let message_id = record["mail"]["messageId"].as_str().unwrap_or_default();
            let subject = record["mail"]["commonHeaders"]["subject"]
                .as_str()
                .unwrap_or_default();
            assert!(
                messages
                    .iter()
                    .any(|(sent_id, sent_subject)| sent_id == message_id
                        && sent_subject == subject),
                "case {case:?}: a record must be about one of the messages this set sent \
                 ({messages:?}), got {record:?}"
            );
            assert_eq!(
                record["mail"]["tags"]["ses:configuration-set"],
                json!([format!("{}-set", row.slug)]),
                "case {case:?}: a published event names the configuration set it came from, got \
                 {record:?}"
            );
        }
    }

    // And the row whose destination is OFF received nothing, asserted only now: the rows
    // above sent at the same time and their records have already been delivered, so a
    // destination that was going to publish would have.
    for row in ROWS.iter().filter(|row| row.expected.is_empty()) {
        let prefix = format!("{}/", row.slug);
        let records = records_under(&router, row.case, &prefix).await;
        assert!(
            records.is_empty(),
            "case {:?}: a destination that is not enabled receives nothing, got {records:?}",
            row.case
        );
    }
}
