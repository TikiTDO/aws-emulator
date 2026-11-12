// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `PutAuditEvents`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with, against the single shared engine.
//!
//! Each test is a TABLE: the rule under test is asserted over a set of inputs that
//! differ along what the rule generalises over (batch size and payload shape, checksum
//! form, payload size at and past the documented limit, ARN form, id multiplicity,
//! external id), not only over the one case the bootstrap issue cited.
//!
//! The harness below is this crate's one test client; a sibling suite that drives the
//! same operation reaches it as `crate::put_audit_events::{Harness, ev, channel_arn}`
//! rather than building a second one.

use std::collections::HashSet;
use std::sync::Arc;

use service_com_amazonaws_cloudtraildata_cloudtraildataservice::state::{
    self, Channel, MAX_EVENT_DATA_BYTES, MAX_RETAINED_BYTES_PER_CHANNEL,
    MAX_RETAINED_EVENTS_PER_CHANNEL,
};
use service_com_amazonaws_cloudtraildata_cloudtraildataservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The access key id a call signs with unless a case names another: a key neither STS nor
/// IAM holds a record for, which is the position the account's own published credential is
/// in.
pub const DEFAULT_ACCESS_KEY_ID: &str = "AKIAEMULATORTEST";

pub const REGION: &str = "us-east-1";

/// Routes the request to this service: `resolve_route_key` reads the signing name out
/// of the SigV4 credential scope, and the key id is what names the principal the request
/// speaks for. Signature verification is off in these tests.
fn sigv4_auth(access_key_id: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential={access_key_id}/20260101/{REGION}/cloudtrail-data/aws4_request, \
         SignedHeaders=host, Signature=0"
    )
}

/// A channel ARN in the emulator's own account, as the CloudTrail control plane would
/// mint it: `arn:aws:cloudtrail:<region>:<account>:channel/<id>`.
pub fn channel_arn(channel_id: &str) -> String {
    engine::emulator_arn("cloudtrail", REGION, &format!("channel/{channel_id}"))
}

/// The checksum the model documents for `AuditEvent$eventDataChecksum`:
/// `printf %s $eventdata | openssl dgst -binary -sha256 | base64`. The codec is the
/// SDK's own standard-alphabet base64 (`aws_smithy_types::base64`).
fn base64_sha256(data: &str) -> String {
    use sha2::Digest as _;
    aws_smithy_types::base64::encode(sha2::Sha256::digest(data.as_bytes()))
}

/// True when `value` has the form the model gives `AuditEventResultEntry$eventID`
/// (`@pattern ^[-_A-Za-z0-9]+$`, `@length 1..128`).
fn is_modelled_event_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// A valid audit-event JSON object whose encoded length is exactly `len` bytes, for
/// exercising the payload-size bound.
fn json_object_of_len(len: usize) -> String {
    let prefix = r#"{"eventSource":"custom.app","eventName":"Login","pad":""#;
    let suffix = r#""}"#;
    assert!(
        len >= prefix.len() + suffix.len(),
        "json_object_of_len({len}): too short to build a JSON object"
    );
    format!(
        "{prefix}{}{suffix}",
        "p".repeat(len - prefix.len() - suffix.len())
    )
}

/// One entry of an `auditEvents` batch.
#[derive(Debug, Clone)]
pub struct Ev {
    id: String,
    data: String,
    checksum: Option<String>,
}

pub fn ev(id: impl Into<String>, data: impl Into<String>) -> Ev {
    Ev {
        id: id.into(),
        data: data.into(),
        checksum: None,
    }
}

impl Ev {
    /// `AuditEvent$id` as this entry carries it.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// `AuditEvent$eventData` as this entry carries it.
    pub fn data(&self) -> &str {
        &self.data
    }

    /// `AuditEvent$eventDataChecksum`, when this entry carries one.
    pub fn checksum(&self) -> Option<&str> {
        self.checksum.as_deref()
    }

    /// Carry an explicit `eventDataChecksum`.
    pub fn with_checksum(mut self, checksum: impl Into<String>) -> Self {
        self.checksum = Some(checksum.into());
        self
    }

    /// Carry the correct base64-SHA256 checksum of this event's own `eventData`.
    fn signed(self) -> Self {
        let checksum = base64_sha256(&self.data);
        self.with_checksum(checksum)
    }

    fn to_json(&self) -> serde_json::Value {
        let mut entry = serde_json::json!({ "id": self.id, "eventData": self.data });
        if let Some(checksum) = &self.checksum {
            entry["eventDataChecksum"] = serde_json::Value::String(checksum.clone());
        }
        entry
    }
}

/// The emulator's answer, as a client sees it.
pub struct Reply {
    pub status: u16,
    pub body: serde_json::Value,
}

impl Reply {
    /// The wire error code restJson1 carries in `__type` (empty on a success).
    pub fn error_code(&self) -> String {
        self.body
            .get("__type")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    fn member(&self, name: &str) -> Option<&Vec<serde_json::Value>> {
        self.body.get(name).and_then(serde_json::Value::as_array)
    }

    fn entries(&self, name: &str, value_member: &str) -> Vec<(String, String)> {
        self.member(name)
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let field = |m: &str| {
                            item.get(m)
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_string()
                        };
                        (field("id"), field(value_member))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `successful` as `(id, eventID)` pairs.
    fn successful(&self) -> Vec<(String, String)> {
        self.entries("successful", "eventID")
    }

    /// `failed` as `(id, errorCode)` pairs.
    fn failed(&self) -> Vec<(String, String)> {
        self.entries("failed", "errorCode")
    }
}

pub struct Harness {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Harness {
    pub fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The shared engine this service is mounted on, so a case can seed the stores the IAM
    /// and STS services hold their records in — the stores an authorization decision reads.
    pub fn engine(&self) -> &Arc<engine::InMemoryEngine> {
        &self.engine
    }

    /// Seed a channel, the way the CloudTrail control plane's `CreateChannel` will once
    /// it is implemented: into this service's store in the SHARED engine.
    pub fn register(&self, channel: Channel) {
        state::service_state(&*self.engine)
            .expect("the shared engine holds this service's state")
            .lock()
            .register_channel(channel);
    }

    /// The channel's stored ingest log as `(source id, event id)` pairs, oldest first —
    /// the state a later caller reads, not the response just rendered.
    pub fn ingest_log(&self, channel: &str) -> Vec<(String, String)> {
        let reference = state::parse_channel_ref(channel).expect("a channel reference");
        state::service_state(&*self.engine)
            .expect("the shared engine holds this service's state")
            .lock()
            .channel(&reference)
            .map(|c| {
                c.events()
                    .map(|e| (e.source_id.clone(), e.event_id.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// `(retained event count, retained eventData bytes)` for a channel.
    fn retention(&self, channel: &str) -> (usize, usize) {
        let reference = state::parse_channel_ref(channel).expect("a channel reference");
        state::service_state(&*self.engine)
            .expect("the shared engine holds this service's state")
            .lock()
            .channel(&reference)
            .map(|c| (c.retained_event_count(), c.retained_bytes()))
            .unwrap_or_default()
    }

    pub async fn put(&self, channel: &str, external_id: Option<&str>, events: &[Ev]) -> Reply {
        self.put_as(DEFAULT_ACCESS_KEY_ID, channel, external_id, events)
            .await
    }

    /// The same call, signed with `access_key_id` — what names the principal the request
    /// speaks for.
    pub async fn put_as(
        &self,
        access_key_id: &str,
        channel: &str,
        external_id: Option<&str>,
        events: &[Ev],
    ) -> Reply {
        let mut uri = format!("/PutAuditEvents?channelArn={channel}");
        if let Some(external_id) = external_id {
            uri.push_str("&externalId=");
            uri.push_str(external_id);
        }
        let body = serde_json::json!({
            "auditEvents": events.iter().map(Ev::to_json).collect::<Vec<_>>(),
        });
        let request = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("authorization", sigv4_auth(access_key_id))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("serialize the request body"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let body = serde_json::from_slice(response.body()).unwrap_or(serde_json::Value::Null);
        Reply { status, body }
    }
}

/// The ingest is STATEFUL: each accepted event is appended to the channel's log under a
/// fresh event id, and a later call sees everything the earlier calls stored, with the
/// event ids they returned unchanged. Spans batches of different size and payload shape.
#[tokio::test]
async fn ingest_accumulates_in_the_channel_across_calls() {
    let harness = Harness::new();
    let arn = channel_arn("0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    let batches: Vec<Vec<Ev>> = vec![
        // one event, flat payload
        vec![ev(
            "evt-1",
            r#"{"eventSource":"custom.app","eventName":"Login"}"#,
        )],
        // two events, one of them carrying a correct checksum
        vec![
            ev("evt-2", r#"{"eventSource":"custom.app","eventName":"Logout"}"#),
            ev(
                "evt-3",
                r#"{"eventSource":"custom.app","eventName":"Purchase","amount":42}"#,
            )
            .signed(),
        ],
        // three events, nested and unicode payloads, and a large one
        vec![
            ev(
                "evt_4",
                r#"{"eventSource":"custom.app","userIdentity":{"type":"Custom","principalId":"p-1"}}"#,
            ),
            ev("evt-5", r#"{"eventSource":"custom.app","note":"héllo wörld"}"#).signed(),
            ev("evt-6", json_object_of_len(64 * 1024)),
        ],
    ];

    let mut expected_log: Vec<(String, String)> = Vec::new();
    let mut all_event_ids: HashSet<String> = HashSet::new();

    for (case, batch) in batches.iter().enumerate() {
        let reply = harness.put(&arn, None, batch).await;
        assert_eq!(
            reply.status, 200,
            "case batch {case}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
        assert!(
            reply.member("successful").is_some() && reply.member("failed").is_some(),
            "case batch {case}: both successful and failed are @required and must always \
             be present, got {}",
            reply.body
        );
        let successful = reply.successful();
        assert!(
            reply.failed().is_empty(),
            "case batch {case}: expected no failures, got {:?}",
            reply.failed()
        );
        let returned_ids: Vec<&str> = successful.iter().map(|(id, _)| id.as_str()).collect();
        let submitted_ids: Vec<&str> = batch.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            returned_ids, submitted_ids,
            "case batch {case}: successful[].id must be the caller's own ids, in order, got \
             {returned_ids:?}"
        );
        for (id, event_id) in &successful {
            assert!(
                is_modelled_event_id(event_id),
                "case batch {case} event {id}: eventID must match the modelled form, got \
                 {event_id:?}"
            );
            assert!(
                all_event_ids.insert(event_id.clone()),
                "case batch {case} event {id}: eventID {event_id:?} was already minted for \
                 another event"
            );
            expected_log.push((id.clone(), event_id.clone()));
        }

        let stored = harness.ingest_log(&arn);
        assert_eq!(
            stored, expected_log,
            "case batch {case}: the channel's ingest log must hold every event accepted so \
             far, with the event ids already returned, got {stored:?}"
        );
    }

    assert_eq!(
        expected_log.len(),
        6,
        "all six submitted events must be stored, got {expected_log:?}"
    );
}

/// Per-event validation is a PARTIAL failure: each entry lands in `successful` or in
/// `failed` with the modelled error code for its own defect, in one and the same
/// response, and only accepted events are stored.
#[tokio::test]
async fn per_event_defects_are_reported_per_event() {
    let harness = Harness::new();
    let arn = channel_arn("1b2c3d4e-5f60-7182-93a4-b5c6d7e8f901");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    let good = r#"{"eventSource":"custom.app","eventName":"Login"}"#;
    // (case name, event, expected error code — None means the event is ingested)
    let rows: Vec<(&str, Ev, Option<&str>)> = vec![
        ("no checksum supplied", ev("ok-1", good), None),
        ("correct checksum", ev("ok-2", good).signed(), None),
        (
            "eventData exactly at the 1 MB limit",
            ev("ok-3", json_object_of_len(MAX_EVENT_DATA_BYTES)),
            None,
        ),
        (
            "checksum of different data",
            ev("bad-1", good).with_checksum(base64_sha256("something else")),
            Some("InvalidChecksum"),
        ),
        (
            "checksum missing its base64 padding",
            ev("bad-2", good).with_checksum(base64_sha256(good).trim_end_matches('=').to_string()),
            Some("InvalidChecksum"),
        ),
        (
            "empty checksum",
            ev("bad-3", good).with_checksum(""),
            Some("InvalidChecksum"),
        ),
        (
            "eventData is not JSON",
            ev("bad-4", "not json at all"),
            Some("InvalidData"),
        ),
        (
            "eventData is a JSON array",
            ev("bad-5", r#"[{"eventSource":"custom.app"}]"#),
            Some("InvalidData"),
        ),
        (
            "eventData is a JSON string",
            ev("bad-6", r#""custom.app""#),
            Some("InvalidData"),
        ),
        (
            "eventData is a truncated JSON object",
            ev("bad-7", r#"{"eventSource":"custom.app""#),
            Some("InvalidData"),
        ),
        (
            "eventData one byte past the 1 MB limit",
            ev("bad-8", json_object_of_len(MAX_EVENT_DATA_BYTES + 1)),
            Some("FieldTooLong"),
        ),
    ];

    let batch: Vec<Ev> = rows.iter().map(|(_, event, _)| event.clone()).collect();
    let reply = harness.put(&arn, None, &batch).await;
    assert_eq!(
        reply.status, 200,
        "per-event defects are partial failures, not operation errors: expected HTTP 200, \
         got {} with body {}",
        reply.status, reply.body
    );

    let successful = reply.successful();
    let failed = reply.failed();
    for (case, event, expected) in &rows {
        match expected {
            None => {
                assert!(
                    successful.iter().any(|(id, _)| id == &event.id),
                    "case {case:?}: expected {} in successful[], got successful {successful:?} \
                     and failed {failed:?}",
                    event.id
                );
            }
            Some(code) => {
                let actual = failed
                    .iter()
                    .find(|(id, _)| id == &event.id)
                    .map(|(_, code)| code.clone());
                assert_eq!(
                    actual.as_deref(),
                    Some(*code),
                    "case {case:?}: expected {} in failed[] with errorCode {code}, got \
                     {actual:?} (successful {successful:?})",
                    event.id
                );
            }
        }
    }

    // Every failed entry carries the @required errorMessage.
    for entry in reply.member("failed").expect("failed is @required") {
        let message = entry
            .get("errorMessage")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        assert!(
            !message.is_empty(),
            "every failed entry must carry an errorMessage, got {entry}"
        );
    }

    let stored: Vec<String> = harness
        .ingest_log(&arn)
        .into_iter()
        .map(|(source_id, _)| source_id)
        .collect();
    let expected_stored: Vec<&str> = rows
        .iter()
        .filter(|(_, _, expected)| expected.is_none())
        .map(|(_, event, _)| event.id.as_str())
        .collect();
    assert_eq!(
        stored, expected_stored,
        "only accepted events may be stored, got {stored:?}"
    );
}

/// Resolving the channel: a channel ARN that is not registered is `ChannelNotFound`,
/// and a value that is not a channel ARN is `InvalidChannelARN` — whether the member's
/// modelled `@pattern ^arn:.*$` rejects it at the boundary or the handler finds it names
/// no channel.
#[tokio::test]
async fn channel_resolution_answers_the_modelled_error_for_each_arn_form() {
    let harness = Harness::new();
    let registered = channel_arn("2c3d4e5f-6071-8293-a4b5-c6d7e8f90123");
    harness.register(Channel::from_arn(&registered, "Custom").expect("a channel ARN"));

    // (case name, channelArn, expected status, expected error code — "" means success)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        ("the registered channel ARN", registered.clone(), 200, ""),
        (
            "a well-formed channel ARN that was never registered",
            channel_arn("d0e1f203-1425-4637-8849-5a6b7c8d9e0f"),
            400,
            "ChannelNotFound",
        ),
        (
            "the registered channel id in another region",
            engine::emulator_arn(
                "cloudtrail",
                "us-west-2",
                "channel/2c3d4e5f-6071-8293-a4b5-c6d7e8f90123",
            ),
            400,
            "ChannelNotFound",
        ),
        (
            "another CloudTrail resource type",
            engine::emulator_arn("cloudtrail", REGION, "trail/my-trail"),
            400,
            "InvalidChannelARN",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::my-bucket".to_string(),
            400,
            "InvalidChannelARN",
        ),
        (
            "a channel ARN with an empty channel id",
            engine::emulator_arn("cloudtrail", REGION, "channel/"),
            400,
            "InvalidChannelARN",
        ),
        (
            "a channel resource without its id separator",
            engine::emulator_arn("cloudtrail", REGION, "channel"),
            400,
            "InvalidChannelARN",
        ),
        (
            "a channel ARN with no region",
            "arn:aws:cloudtrail::000000000000:channel/e1f20314-2536-4748-895a-6b7c8d9e0f10"
                .to_string(),
            400,
            "InvalidChannelARN",
        ),
        (
            "a channel ARN with a malformed account id",
            "arn:aws:cloudtrail:us-east-1:12345:channel/f2031425-3647-4859-8a6b-7c8d9e0f1021"
                .to_string(),
            400,
            "InvalidChannelARN",
        ),
        (
            "a channel ARN whose region is not an ARN field",
            "arn:aws:cloudtrail:US_EAST_1:000000000000:channel/03142536-4758-496a-8b7c-8d9e0f102132"
                .to_string(),
            400,
            "InvalidChannelARN",
        ),
        (
            "a channel ARN whose partition is not an ARN field",
            "arn:../x:cloudtrail:us-east-1:000000000000:channel/14253647-5869-4a7b-8c8d-9e0f10213243"
                .to_string(),
            400,
            "InvalidChannelARN",
        ),
        (
            "a bare arn prefix",
            "arn:".to_string(),
            400,
            "InvalidChannelARN",
        ),
        (
            // Refused by the member's modelled @pattern ^arn:.*$ at the boundary, and
            // answered under the error this service models for that fault.
            "a value that is not an ARN at all",
            "not-an-arn".to_string(),
            400,
            "InvalidChannelARN",
        ),
    ];

    for (case, channel, status, code) in &rows {
        let reply = harness
            .put(channel, None, &[ev("evt-1", r#"{"eventSource":"custom.app"}"#)])
            .await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got body {}",
            reply.body
        );
    }

    let stored = harness.ingest_log(&registered);
    assert_eq!(
        stored.len(),
        1,
        "only the request against the registered channel may store an event, got {stored:?}"
    );
}

/// A repeated `AuditEvent$id` fails the WHOLE request with `DuplicatedAuditEventId`,
/// whatever the batch's size or where the repeat sits, and persists nothing.
#[tokio::test]
async fn repeated_event_ids_fail_the_whole_request() {
    let harness = Harness::new();
    let arn = channel_arn("3d4e5f60-7182-93a4-b5c6-d7e8f9012345");
    harness.register(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));
    let data = r#"{"eventSource":"custom.app","eventName":"Login"}"#;

    // (case name, the batch's ids, expected status, expected error code)
    let rows: Vec<(&str, Vec<&'static str>, u16, &str)> = vec![
        (
            "two entries sharing one id",
            vec!["dup-1", "dup-1"],
            400,
            "DuplicatedAuditEventId",
        ),
        (
            "the repeat at the end of a longer batch",
            vec!["a-1", "b-1", "a-1"],
            400,
            "DuplicatedAuditEventId",
        ),
        (
            "every entry sharing one id",
            vec!["same", "same", "same"],
            400,
            "DuplicatedAuditEventId",
        ),
        ("distinct ids", vec!["uniq-1", "uniq-2"], 200, ""),
    ];

    let mut expected_stored = 0usize;
    for (case, ids, status, code) in &rows {
        let batch: Vec<Ev> = ids.iter().map(|id| ev(*id, data)).collect();
        let reply = harness.put(&arn, None, &batch).await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got body {}",
            reply.body
        );
        if *status == 200 {
            expected_stored += ids.len();
        }
        let stored = harness.ingest_log(&arn);
        assert_eq!(
            stored.len(),
            expected_stored,
            "case {case:?}: a refused request must persist nothing, got {stored:?}"
        );
    }
}

/// Ownership and the channel's external id: a channel whose resource policy includes an
/// external id requires the request to carry exactly that value, and a channel in
/// another account is not the caller's to ingest into — which authorization decides on
/// the ARN, before the registration is read at all (the table spanning that rule lives
/// in `tests/all/channel_arn_refusals.rs`).
#[tokio::test]
async fn ownership_and_external_id_decide_permission() {
    let harness = Harness::new();

    let open = channel_arn("4e5f6071-8293-a4b5-c6d7-e8f901234567");
    harness.register(Channel::from_arn(&open, "Custom").expect("a channel ARN"));

    let guarded = channel_arn("5f607182-93a4-b5c6-d7e8-f90123456789");
    harness.register(
        Channel::from_arn(&guarded, "Custom")
            .expect("a channel ARN")
            .requiring_external_id("secret-123"),
    );

    // A channel owned by another account. Naming it by ARN is refused by authorization,
    // ahead of everything this service knows about the registration.
    let foreign =
        "arn:aws:cloudtrail:us-east-1:111122223333:channel/25364758-697a-4b8c-8d9e-0f1021324354"
            .to_string();
    harness.register(Channel::from_arn(&foreign, "Custom").expect("a channel ARN"));

    // (case name, channel, externalId, expected status, expected error code)
    let rows: Vec<(&str, &str, Option<&str>, u16, &str)> = vec![
        ("no external id required, none sent", &open, None, 200, ""),
        (
            "no external id required, one sent anyway",
            &open,
            Some("ignored-123"),
            200,
            "",
        ),
        (
            "external id required and matching",
            &guarded,
            Some("secret-123"),
            200,
            "",
        ),
        (
            "external id required but absent",
            &guarded,
            None,
            400,
            "ChannelInsufficientPermission",
        ),
        (
            "external id required but different",
            &guarded,
            Some("wrong-value"),
            400,
            "ChannelInsufficientPermission",
        ),
        (
            "a channel in another account",
            &foreign,
            None,
            403,
            "AccessDeniedException",
        ),
        (
            "a channel in another account, external id sent",
            &foreign,
            Some("secret-123"),
            403,
            "AccessDeniedException",
        ),
    ];

    for (case, channel, external_id, status, code) in &rows {
        let reply = harness
            .put(
                channel,
                *external_id,
                &[ev("evt-1", r#"{"eventSource":"custom.app"}"#)],
            )
            .await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status}, got {} with body {}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            *code,
            "case {case:?}: expected error code {code:?}, got body {}",
            reply.body
        );
    }

    assert_eq!(
        harness.ingest_log(&foreign).len(),
        0,
        "a refused request must persist nothing on the foreign-owned channel"
    );
    assert_eq!(
        harness.ingest_log(&guarded).len(),
        1,
        "only the matching-external-id request may be ingested on the guarded channel"
    );
}

/// `channelArn` is documented as "The ARN or ID (the ARN suffix) of a channel", so both
/// forms resolve to the same registration; everything that is neither resolves to
/// nothing (which the handler answers with `InvalidChannelARN`).
#[test]
fn a_channel_resolves_by_its_arn_or_by_its_arn_suffix() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let arn = channel_arn("36475869-7a8b-4c9d-8e0f-102132435465");
    state::service_state(&*engine)
        .expect("the shared engine holds this service's state")
        .lock()
        .register_channel(Channel::from_arn(&arn, "Custom").expect("a channel ARN"));

    // (case name, input value, does it resolve to the registered channel?)
    let rows: Vec<(&str, String, bool)> = vec![
        ("the full ARN", arn.clone(), true),
        (
            "the ARN suffix (bare channel id)",
            "36475869-7a8b-4c9d-8e0f-102132435465".to_string(),
            true,
        ),
        (
            "another channel id",
            "4758697a-8b9c-4d0e-8f10-213243546576".to_string(),
            false,
        ),
        (
            "another account's ARN for the same id",
            "arn:aws:cloudtrail:us-east-1:111122223333:channel/36475869-7a8b-4c9d-8e0f-102132435465"
                .to_string(),
            false,
        ),
    ];

    let state = state::service_state(&*engine).expect("state");
    for (case, value, resolves) in &rows {
        let reference = state::parse_channel_ref(value)
            .unwrap_or_else(|| panic!("case {case:?}: {value:?} must parse as a channel ref"));
        let found = state.lock().channel(&reference).map(|c| c.arn.clone());
        assert_eq!(
            found.is_some(),
            *resolves,
            "case {case:?}: expected resolution {resolves}, got {found:?}"
        );
        if *resolves {
            assert_eq!(
                found.as_deref(),
                Some(arn.as_str()),
                "case {case:?}: must resolve to the registered channel"
            );
        }
    }

    // (case name, value that names no channel at all)
    let unresolvable = [
        ("a non-channel CloudTrail ARN", "arn:aws:cloudtrail:us-east-1:000000000000:trail/t"),
        ("another service's ARN", "arn:aws:s3:::bucket"),
        ("a channel ARN with no account", "arn:aws:cloudtrail:us-east-1::channel/abc"),
        ("an id that is not a channel id", "not-a-channel-id"),
        ("an id with an illegal character", "channel id!"),
        ("an empty value", ""),
        (
            "an ARN whose region field is blank",
            "arn:aws:cloudtrail: :000000000000:channel/c1",
        ),
        (
            "an ARN whose partition looks like a path",
            "arn:../x:cloudtrail:us-east-1:000000000000:channel/c1",
        ),
    ];
    for (case, value) in unresolvable {
        assert!(
            state::parse_channel_ref(value).is_none(),
            "case {case:?}: {value:?} names no channel and must not parse"
        );
    }

    // A second registration sharing the ARN suffix makes the bare-id form AMBIGUOUS:
    // it must resolve to nothing, deterministically, rather than to whichever channel
    // the hash map happens to yield first. Each full ARN still resolves to its own.
    let twin = "arn:aws:cloudtrail:us-east-1:111122223333:channel/36475869-7a8b-4c9d-8e0f-102132435465";
    state
        .lock()
        .register_channel(Channel::from_arn(twin, "Custom").expect("a channel ARN"));
    let bare = state::parse_channel_ref("36475869-7a8b-4c9d-8e0f-102132435465").expect("a channel ref");
    assert!(
        state.lock().channel(&bare).is_none(),
        "case \"ambiguous ARN suffix\": a suffix shared by two registrations must resolve to \
         nothing, got {:?}",
        state.lock().channel(&bare).map(|c| c.arn.clone())
    );
    for value in [arn.as_str(), twin] {
        let reference = state::parse_channel_ref(value).expect("a channel ref");
        let found = state.lock().channel(&reference).map(|c| c.arn.clone());
        assert_eq!(
            found.as_deref(),
            Some(value),
            "case {value:?}: a full ARN must still resolve to its own channel, got {found:?}"
        );
    }
}

/// The ingest log is BOUNDED: repeated calls cannot grow the store without limit, and
/// eviction is oldest-first, so what a channel retains is the newest window of events —
/// by count, and by retained `eventData` bytes.
#[tokio::test]
async fn the_ingest_log_is_bounded_by_its_retention_limits() {
    let harness = Harness::new();
    let small = r#"{"eventSource":"custom.app","eventName":"Login"}"#;

    // Count bound: 12 batches of 100 (the modelled maximum per request) = 1200 events.
    let by_count = channel_arn("58697a8b-9c0d-4e1f-8021-324354657687");
    harness.register(Channel::from_arn(&by_count, "Custom").expect("a channel ARN"));
    let batches = 12;
    for batch in 0..batches {
        let events: Vec<Ev> = (0..100)
            .map(|i| ev(format!("evt-{batch}-{i}"), small))
            .collect();
        let reply = harness.put(&by_count, None, &events).await;
        assert_eq!(
            reply.status, 200,
            "case count batch {batch}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
    }
    let (count, bytes) = harness.retention(&by_count);
    assert_eq!(
        count, MAX_RETAINED_EVENTS_PER_CHANNEL,
        "case \"count bound\": the log must retain exactly its event cap, got {count}"
    );
    assert!(
        bytes <= MAX_RETAINED_BYTES_PER_CHANNEL,
        "case \"count bound\": retained bytes {bytes} must stay inside the byte cap"
    );
    let log = harness.ingest_log(&by_count);
    assert_eq!(
        log.last().map(|(id, _)| id.as_str()),
        Some("evt-11-99"),
        "case \"count bound\": the newest event must be retained"
    );
    assert_eq!(
        log.first().map(|(id, _)| id.as_str()),
        Some("evt-2-0"),
        "case \"count bound\": eviction must be oldest-first (1200 ingested, cap \
         {MAX_RETAINED_EVENTS_PER_CHANNEL})"
    );

    // Byte bound: 20 single-event calls of 1 MB each, past the retained-byte cap.
    let by_bytes = channel_arn("697a8b9c-0d1e-4f20-8132-435465768798");
    harness.register(Channel::from_arn(&by_bytes, "Custom").expect("a channel ARN"));
    let calls = 20;
    for i in 0..calls {
        let event = ev(format!("big-{i}"), json_object_of_len(MAX_EVENT_DATA_BYTES));
        let reply = harness.put(&by_bytes, None, &[event]).await;
        assert_eq!(
            reply.status, 200,
            "case byte call {i}: expected HTTP 200, got {} with body {}",
            reply.status, reply.body
        );
    }
    let (count, bytes) = harness.retention(&by_bytes);
    assert!(
        bytes <= MAX_RETAINED_BYTES_PER_CHANNEL,
        "case \"byte bound\": retained bytes {bytes} must stay inside the byte cap \
         {MAX_RETAINED_BYTES_PER_CHANNEL}"
    );
    assert!(
        count > 0 && count < calls,
        "case \"byte bound\": {calls} ingested events of 1 MB must be trimmed to a smaller \
         retained window, got {count}"
    );
    let log = harness.ingest_log(&by_bytes);
    assert_eq!(
        log.last().map(|(id, _)| id.as_str()),
        Some("big-19"),
        "case \"byte bound\": the newest event must be retained"
    );
}
