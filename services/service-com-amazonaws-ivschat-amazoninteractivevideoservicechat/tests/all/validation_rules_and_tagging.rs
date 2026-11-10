// SPDX-License-Identifier: CC0-1.0

//! The three request-shaped rules this service answers by, driven end-to-end over the
//! router the production entrypoint serves:
//!
//! 1. a request naming a resource that does not exist is refused with
//!    `ResourceNotFoundException` whose `message` is built from the request as
//!    `Resource: <the identifier the caller sent> not found` — one rule, every operation
//!    that takes an identifier;
//! 2. a value outside a member's modelled `@range` is refused with `ValidationException`
//!    whose message names the operation, the member, the violated bound and the submitted
//!    value in decimal form, while a value ON either bound is accepted and honoured;
//! 3. the `/tags/{resourceArn}` trio accepts every ARN form this service's resources
//!    carry — the published `@pattern`'s single-character resource-type class matches
//!    none of them — and tags attach, list and untag per key.
//!
//! Every table row is a different input along what the rule generalises over (the
//! operation, the member, the bound, the identifier, the tag set, its size and its keys),
//! never a rename of one case.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::state::NO_CONTENT_SUCCESS_STATUS;

/// The account every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";
/// The region this suite signs for.
const REGION: &str = "us-west-2";
/// The status a successful tag mutation answers: it conveys nothing but the status, so it
/// is `204 No Content` with no body, which `empty_success_answers.rs` pins as the rule.
/// Read off the service crate, so the status has one home here too.
const NO_CONTENT_STATUS: u16 = NO_CONTENT_SUCCESS_STATUS;

/// One emulator, one shared engine: exactly what the router hands every handler.
///
/// `pub(crate)` because `empty_success_answers.rs` drives the same router through the same
/// front door: one harness and one encoder for both suites, rather than a second copy that
/// has to be kept in step.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Pin authentication off so the suite does not depend on the process setting.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::EmulatedService::new(
                engine,
            ),
        );
        Self { router }
    }

    /// One request over the wire, answered as the status, the RAW body bytes and the length
    /// the answer states — raw, because whether a body is there at all, and how it is
    /// framed, is what `empty_success_answers.rs` asserts.
    pub(crate) async fn call_raw(&self, method: &str, uri: &str, body: &str) -> Answer {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVSCHAT/20260101/{REGION}/ivschat/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("http://localhost{uri}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(bytes::Bytes::from(body.to_owned()))
            .expect("a valid request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        // Every response header, lowercased: the error document suite reads the
        // protocol's discriminator and the request id off them.
        let headers: BTreeMap<String, String> = response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    String::from_utf8_lossy(value.as_bytes()).to_string(),
                )
            })
            .collect();
        Answer {
            status,
            body: response.into_body().to_vec(),
            content_length,
            headers,
        }
    }

    /// One request over the wire. `uri` is a path (plus query), so the REST-bound tagging
    /// operations are reachable beside the RPC-style ones.
    pub(crate) async fn call(&self, method: &str, uri: &str, body: &str) -> (u16, Value) {
        let answer = self.call_raw(method, uri, body).await;
        let parsed = if answer.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&answer.body).unwrap_or(Value::Null)
        };
        (answer.status, parsed)
    }

    /// One of the operations the model binds to `POST /<Operation>`.
    pub(crate) async fn post(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.call("POST", &format!("/{operation}"), &body.to_string())
            .await
    }

    pub(crate) async fn create_room(&self, body: &Value) -> Value {
        let (status, created) = self.post("CreateRoom", body).await;
        assert_eq!(
            status, 200,
            "case {body}: CreateRoom must answer 200, got {status} {created}"
        );
        created
    }

    /// `POST /tags/{resourceArn}`.
    async fn tag(&self, resource_arn: &str, tags: &Value) -> (u16, Value) {
        self.call(
            "POST",
            &format!("/tags/{}", encode_label(resource_arn)),
            &json!({ "tags": tags }).to_string(),
        )
        .await
    }

    /// `DELETE /tags/{resourceArn}?tagKeys=…` — `tagKeys` is an `@httpQuery` list.
    async fn untag(&self, resource_arn: &str, tag_keys: &[&str]) -> (u16, Value) {
        let query: Vec<String> = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", encode_label(key)))
            .collect();
        self.call(
            "DELETE",
            &format!("/tags/{}?{}", encode_label(resource_arn), query.join("&")),
            "",
        )
        .await
    }

    /// `GET /tags/{resourceArn}`.
    async fn list_tags(&self, resource_arn: &str) -> (u16, Value) {
        self.call("GET", &format!("/tags/{}", encode_label(resource_arn)), "")
            .await
    }
}

/// One answer as a suite that cares about FRAMING reads it: the status, the raw body bytes,
/// the length the answer states and the headers it carries.
pub(crate) struct Answer {
    pub(crate) status: u16,
    pub(crate) body: Vec<u8>,
    pub(crate) content_length: Option<String>,
    pub(crate) headers: BTreeMap<String, String>,
}

impl Answer {
    /// The body as text, for an assertion message.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// One response header, by its lowercased name.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// Percent-encode one URI path segment or query value the way an SDK does: an ARN carries
/// `:` and `/`, and `/tags/{resourceArn}` is a single (non-greedy) label, so everything
/// outside RFC 3986's unreserved set is encoded and survives the server's decode.
pub(crate) fn encode_label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' => c.to_string(),
            other => other
                .to_string()
                .bytes()
                .map(|byte| format!("%{byte:02X}"))
                .collect(),
        })
        .collect()
}

fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `message` an error payload carries, under either casing a protocol may render it.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or("<no message>")
        .to_string()
}

fn str_field(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("member {member} missing from {body}"))
        .to_string()
}

fn tags_of(body: &Value, member: &str) -> BTreeMap<String, String> {
    body.get(member)
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .map(|(key, value)| {
                    (
                        key.clone(),
                        value.as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A `Time` member as epoch seconds, accepting either rendering the codec may emit.
fn seconds(body: &Value, member: &str) -> i64 {
    let raw = body
        .get(member)
        .unwrap_or_else(|| panic!("member {member} missing from {body}"));
    if let Some(epoch) = raw.as_f64() {
        return epoch as i64;
    }
    let text = raw
        .as_str()
        .unwrap_or_else(|| panic!("member {member} must be an epoch or ISO 8601 value, got {raw}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("member {member} = {text:?} must parse as ISO 8601: {e}"))
        .secs()
}

fn now_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_secs(),
    )
    .expect("the clock fits in an i64")
}

/// A room ARN for a room that was never created.
fn absent_room_arn(id: &str) -> String {
    format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:room/{id}")
}

/// The one message rule every not-found refusal in this service follows.
fn not_found_message(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

#[tokio::test]
async fn a_not_found_refusal_names_the_identifier_the_caller_sent() {
    let emulator = Emulator::new();
    // A real room exists throughout, so each 404 is about the identifier the request
    // carried and not about an empty store.
    let live = emulator.create_room(&json!({ "name": "live" })).await;
    let live_arn = str_field(&live, "arn");

    // A room that WAS created and then deleted: the same rule has to cover an identifier
    // that was valid a moment ago.
    let doomed = emulator.create_room(&json!({ "name": "doomed" })).await;
    let deleted_arn = str_field(&doomed, "arn");
    let (status, body) = emulator
        .post("DeleteRoom", &json!({ "identifier": deleted_arn }))
        .await;
    assert_eq!(
        status, 204,
        "DeleteRoom must answer its modelled 204, got {status} {body}"
    );

    // Rows differ in HOW the identifier fails to name a room: never created, created and
    // deleted, a different id length, and a well-formed ARN in another region.
    let identifiers = vec![
        ("never created", absent_room_arn("neverCreated1")),
        ("created then deleted", deleted_arn.clone()),
        ("a twelve-character id", absent_room_arn("abc123ABC456")),
        ("an id carrying a dash", absent_room_arn("room-7")),
        (
            "another region",
            format!("arn:aws:ivschat:eu-west-2:{ACCOUNT}:room/elsewhere01"),
        ),
    ];

    for (case, identifier) in &identifiers {
        // Every operation that takes a room identifier states the refusal the same way.
        let room_calls: Vec<(&str, Value)> = vec![
            ("GetRoom", json!({ "identifier": identifier })),
            (
                "UpdateRoom",
                json!({ "identifier": identifier, "name": "renamed" }),
            ),
            ("DeleteRoom", json!({ "identifier": identifier })),
            (
                "CreateChatToken",
                json!({ "roomIdentifier": identifier, "userId": "u" }),
            ),
        ];
        for (operation, request) in room_calls {
            let (status, body) = emulator.post(operation, &request).await;
            assert_eq!(
                status, 404,
                "case {case} {operation}: expected HTTP 404, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case} {operation}: expected ResourceNotFoundException, got {body}"
            );
            assert_eq!(
                error_message(&body),
                not_found_message(identifier),
                "case {case} {operation}: expected the service's not-found message rule, got {body}"
            );
            assert_eq!(
                body.get("resourceType").and_then(Value::as_str),
                Some("ROOM"),
                "case {case} {operation}: expected the modelled resourceType, got {body}"
            );
        }

        // And the tagging trio, which names the resource by ARN rather than by identifier.
        let (status, body) = emulator.list_tags(identifier).await;
        assert_eq!(
            status, 404,
            "case {case} ListTagsForResource: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            not_found_message(identifier),
            "case {case} ListTagsForResource: expected the not-found message rule, got {body}"
        );
        let (status, body) = emulator.tag(identifier, &json!({ "env": "x" })).await;
        assert_eq!(
            status, 404,
            "case {case} TagResource: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            not_found_message(identifier),
            "case {case} TagResource: expected the not-found message rule, got {body}"
        );
        let (status, body) = emulator.untag(identifier, &["env"]).await;
        assert_eq!(
            status, 404,
            "case {case} UntagResource: expected HTTP 404, got {status} {body}"
        );
        assert_eq!(
            error_message(&body),
            not_found_message(identifier),
            "case {case} UntagResource: expected the not-found message rule, got {body}"
        );
    }

    // The live room is untouched by all of that: the refusals were about the identifier.
    let (status, body) = emulator
        .post("GetRoom", &json!({ "identifier": live_arn }))
        .await;
    assert_eq!(
        status, 200,
        "the room that does exist still reads back, got {status} {body}"
    );
}

/// The message rule a range refusal follows: the operation, the member, the violated
/// bound and the submitted value, both numbers in decimal form.
fn range_message(operation: &str, member: &str, bound_kind: &str, bound: i32, found: i32) -> String {
    format!("{operation}.{member} is required to have a {bound_kind} value of {bound}.0, found {found}.0")
}

#[tokio::test]
async fn a_value_outside_a_modelled_range_is_refused_with_the_services_message_rule() {
    let emulator = Emulator::new();
    let room = emulator.create_room(&json!({ "name": "ranges" })).await;
    let arn = str_field(&room, "arn");

    // One row per range-constrained request member this service declares, on each side of
    // its published bounds. Bounds from
    // `smithy_query(query="operation", operation="CreateChatToken,ListRooms,CreateRoom,UpdateRoom")`:
    // SessionDurationInMinutes 1..180, MaxRoomResults 1..50,
    // RoomMaxMessageRatePerSecond 1..100, RoomMaxMessageLength 1..500.
    let cases: Vec<(&str, &str, &str, i32, i32, Value)> = vec![
        (
            "CreateChatToken",
            "sessionDurationInMinutes",
            "maximum",
            180,
            181,
            json!({ "roomIdentifier": arn, "userId": "u", "sessionDurationInMinutes": 181 }),
        ),
        (
            "CreateChatToken",
            "sessionDurationInMinutes",
            "minimum",
            1,
            0,
            json!({ "roomIdentifier": arn, "userId": "u", "sessionDurationInMinutes": 0 }),
        ),
        (
            "CreateChatToken",
            "sessionDurationInMinutes",
            "maximum",
            180,
            1_440,
            json!({ "roomIdentifier": arn, "userId": "u", "sessionDurationInMinutes": 1440 }),
        ),
        (
            "ListRooms",
            "maxResults",
            "maximum",
            50,
            51,
            json!({ "maxResults": 51 }),
        ),
        (
            "ListRooms",
            "maxResults",
            "minimum",
            1,
            0,
            json!({ "maxResults": 0 }),
        ),
        (
            "ListRooms",
            "maxResults",
            "minimum",
            1,
            -3,
            json!({ "maxResults": -3 }),
        ),
        (
            "CreateRoom",
            "maximumMessageRatePerSecond",
            "maximum",
            100,
            101,
            json!({ "maximumMessageRatePerSecond": 101 }),
        ),
        (
            "CreateRoom",
            "maximumMessageRatePerSecond",
            "minimum",
            1,
            0,
            json!({ "maximumMessageRatePerSecond": 0 }),
        ),
        (
            "CreateRoom",
            "maximumMessageLength",
            "maximum",
            500,
            501,
            json!({ "maximumMessageLength": 501 }),
        ),
        (
            "CreateRoom",
            "maximumMessageLength",
            "minimum",
            1,
            0,
            json!({ "maximumMessageLength": 0 }),
        ),
        (
            "UpdateRoom",
            "maximumMessageRatePerSecond",
            "maximum",
            100,
            101,
            json!({ "identifier": arn, "maximumMessageRatePerSecond": 101 }),
        ),
        (
            "UpdateRoom",
            "maximumMessageLength",
            "minimum",
            1,
            0,
            json!({ "identifier": arn, "maximumMessageLength": 0 }),
        ),
    ];

    for (operation, member, bound_kind, bound, found, request) in cases {
        let case = format!("{operation}.{member} = {found}");
        let (status, body) = emulator.post(operation, &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            error_message(&body),
            range_message(operation, member, bound_kind, bound, found),
            "case {case:?}: expected the service's range message rule, got {body}"
        );
        assert_eq!(
            body.get("reason").and_then(Value::as_str),
            Some("FIELD_VALIDATION_FAILED"),
            "case {case:?}: expected the modelled reason, got {body}"
        );
        assert_eq!(
            body.pointer("/fieldList/0/name").and_then(Value::as_str),
            Some(format!("{operation}.{member}").as_str()),
            "case {case:?}: expected fieldList to name the member, got {body}"
        );
    }
}

#[tokio::test]
async fn a_value_on_a_modelled_bound_is_accepted_and_honoured_rather_than_clamped() {
    let emulator = Emulator::new();

    // Each published bound of each range-constrained member, accepted and answered back.
    for (case, request, member, expected) in [
        (
            "rate at its minimum",
            json!({ "maximumMessageRatePerSecond": 1 }),
            "maximumMessageRatePerSecond",
            1,
        ),
        (
            "rate at its maximum",
            json!({ "maximumMessageRatePerSecond": 100 }),
            "maximumMessageRatePerSecond",
            100,
        ),
        (
            "length at its minimum",
            json!({ "maximumMessageLength": 1 }),
            "maximumMessageLength",
            1,
        ),
        (
            "length at its maximum",
            json!({ "maximumMessageLength": 500 }),
            "maximumMessageLength",
            500,
        ),
    ] {
        let created = emulator.create_room(&request).await;
        assert_eq!(
            created.get(member).and_then(Value::as_i64),
            Some(i64::from(expected)),
            "case {case:?}: CreateRoom must honour the value on the bound, got {created}"
        );
        let arn = str_field(&created, "arn");
        let (status, updated) = emulator
            .post(
                "UpdateRoom",
                &json!({ "identifier": arn, member: expected }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateRoom must accept the value on the bound, got {status} {updated}"
        );
        assert_eq!(
            updated.get(member).and_then(Value::as_i64),
            Some(i64::from(expected)),
            "case {case:?}: UpdateRoom must honour the value on the bound, got {updated}"
        );
    }

    // A session duration on either bound runs for exactly the minutes asked for.
    let room = emulator.create_room(&json!({ "name": "bounds" })).await;
    let arn = str_field(&room, "arn");
    for minutes in [1_i64, 60, 179, 180] {
        let before = now_secs();
        let (status, token) = emulator
            .post(
                "CreateChatToken",
                &json!({ "roomIdentifier": arn, "userId": "u", "sessionDurationInMinutes": minutes }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {minutes} minutes: expected HTTP 200, got {status} {token}"
        );
        let session = seconds(&token, "sessionExpirationTime");
        let expected = before + minutes * 60;
        assert!(
            (session - expected).abs() <= 5,
            "case {minutes} minutes: expected a session ending near {expected}, got {session}"
        );
    }

    // A page size on either bound is the page size served, not a clamped one.
    for index in 0..3 {
        emulator
            .create_room(&json!({ "name": format!("page{index}") }))
            .await;
    }
    // The store's own count, read with no page size at all, is what the upper bound has to
    // serve in one page.
    let (_, every) = emulator.post("ListRooms", &json!({})).await;
    let total = every
        .get("rooms")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or_default();
    assert!(
        (2..=50).contains(&total),
        "the suite must leave between 2 and 50 rooms for the page-size rows, got {total}"
    );
    for (max_results, expected_len) in [(1_usize, 1_usize), (50, total)] {
        let (status, listed) = emulator
            .post("ListRooms", &json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            status, 200,
            "case maxResults {max_results}: expected HTTP 200, got {status} {listed}"
        );
        let rooms = listed
            .get("rooms")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            rooms, expected_len,
            "case maxResults {max_results}: expected {expected_len} rooms on the page, got {listed}"
        );
    }
}

#[tokio::test]
async fn tags_attach_to_a_room_arn_and_read_back_through_every_reporting_operation() {
    let emulator = Emulator::new();

    // Rows differ in the tag set's size, in its keys and values, and in whether a key the
    // room already carries is overwritten or a new one added.
    let cases: Vec<(&str, Value, Value, BTreeMap<String, String>)> = vec![
        (
            "one key on an untagged room",
            json!({}),
            json!({ "env": "sim2real" }),
            BTreeMap::from([("env".to_string(), "sim2real".to_string())]),
        ),
        (
            "two keys on an untagged room",
            json!({}),
            json!({ "env": "sim2real", "keep": "me" }),
            BTreeMap::from([
                ("env".to_string(), "sim2real".to_string()),
                ("keep".to_string(), "me".to_string()),
            ]),
        ),
        (
            "an empty tag set changes nothing",
            json!({ "tags": { "created": "with" } }),
            json!({}),
            BTreeMap::from([("created".to_string(), "with".to_string())]),
        ),
        (
            "a key the room already carries is updated",
            json!({ "tags": { "env": "old", "owner": "team" } }),
            json!({ "env": "new" }),
            BTreeMap::from([
                ("env".to_string(), "new".to_string()),
                ("owner".to_string(), "team".to_string()),
            ]),
        ),
        (
            "keys outside [a-z]: dots, dashes and an empty value",
            json!({}),
            json!({ "cost.center": "c-12", "Name": "A Room", "blank": "" }),
            BTreeMap::from([
                ("cost.center".to_string(), "c-12".to_string()),
                ("Name".to_string(), "A Room".to_string()),
                ("blank".to_string(), String::new()),
            ]),
        ),
        (
            "a full set of ten keys",
            json!({}),
            Value::Object(
                (0..10)
                    .map(|index| (format!("key{index}"), Value::String(format!("value{index}"))))
                    .collect(),
            ),
            (0..10)
                .map(|index| (format!("key{index}"), format!("value{index}")))
                .collect(),
        ),
    ];

    for (case, create_body, tags, expected) in cases {
        let room = emulator.create_room(&create_body).await;
        let arn = str_field(&room, "arn");

        let (status, body) = emulator.tag(&arn, &tags).await;
        assert_eq!(
            status, NO_CONTENT_STATUS,
            "case {case:?}: TagResource on an ARN CreateRoom minted must answer \
             {NO_CONTENT_STATUS}, got {status} {body}"
        );

        let (status, listed) = emulator.list_tags(&arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListTagsForResource must answer 200, got {status} {listed}"
        );
        let actual = tags_of(&listed, "tags");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} from ListTagsForResource, got {actual:?}"
        );

        // The same tag set, read back through the room itself: one stored set, not two.
        let (status, read) = emulator
            .post("GetRoom", &json!({ "identifier": arn }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetRoom must answer 200, got {status} {read}"
        );
        let from_room = tags_of(&read, "tags");
        assert_eq!(
            from_room, expected,
            "case {case:?}: expected GetRoom to report {expected:?}, got {from_room:?}"
        );

        // A read leaves the tag set alone, so a second read answers the same.
        let (_, again) = emulator.list_tags(&arn).await;
        assert_eq!(
            tags_of(&again, "tags"),
            expected,
            "case {case:?}: a read must not change what a later caller sees, got {again}"
        );
    }
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_keys_it_names() {
    let emulator = Emulator::new();

    // Rows differ in how many of the resource's keys are named, and in whether a named key
    // is one the resource carries at all.
    let cases: Vec<(&str, Vec<&str>, BTreeMap<String, String>)> = vec![
        (
            "one of three keys",
            vec!["env"],
            BTreeMap::from([
                ("keep".to_string(), "me".to_string()),
                ("owner".to_string(), "team".to_string()),
            ]),
        ),
        (
            "two of three keys",
            vec!["env", "owner"],
            BTreeMap::from([("keep".to_string(), "me".to_string())]),
        ),
        ("every key", vec!["env", "keep", "owner"], BTreeMap::new()),
        (
            "a key the resource does not carry",
            vec!["absent"],
            BTreeMap::from([
                ("env".to_string(), "sim2real".to_string()),
                ("keep".to_string(), "me".to_string()),
                ("owner".to_string(), "team".to_string()),
            ]),
        ),
        (
            "a key it carries and one it does not",
            vec!["env", "absent"],
            BTreeMap::from([
                ("keep".to_string(), "me".to_string()),
                ("owner".to_string(), "team".to_string()),
            ]),
        ),
    ];

    for (case, keys, expected) in cases {
        let room = emulator
            .create_room(&json!({
                "tags": { "env": "sim2real", "keep": "me", "owner": "team" }
            }))
            .await;
        let arn = str_field(&room, "arn");

        let (status, body) = emulator.untag(&arn, &keys).await;
        assert_eq!(
            status, NO_CONTENT_STATUS,
            "case {case:?}: UntagResource must answer {NO_CONTENT_STATUS}, got {status} {body}"
        );
        let (_, listed) = emulator.list_tags(&arn).await;
        let actual = tags_of(&listed, "tags");
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {expected:?} to remain, got {actual:?}"
        );

        // Removing the same keys again removes nothing more: the call is `@idempotent`.
        let (status, body) = emulator.untag(&arn, &keys).await;
        assert_eq!(
            status, NO_CONTENT_STATUS,
            "case {case:?}: a repeated UntagResource must answer {NO_CONTENT_STATUS}, got \
             {status} {body}"
        );
        let (_, listed) = emulator.list_tags(&arn).await;
        let actual = tags_of(&listed, "tags");
        assert_eq!(
            actual, expected,
            "case {case:?}: a repeated UntagResource must leave {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_resource_is_not_left_holding_more_tags_than_the_modelled_set_can_report() {
    let emulator = Emulator::new();

    // The modelled `Tags` map is `@length(0..50)` — the same shape every operation reports
    // a resource's tags through — so 50 is the most one resource can carry.
    let full: Value = Value::Object(
        (0..50)
            .map(|index| (format!("key{index:02}"), Value::String(index.to_string())))
            .collect(),
    );
    let room = emulator.create_room(&json!({ "tags": full })).await;
    let arn = str_field(&room, "arn");

    // Rows differ in how many keys past the ceiling the request would add, and in whether
    // the keys it names are new or already there.
    for (case, tags, expected_status) in [
        (
            "one key the resource already carries",
            json!({ "key00": "updated" }),
            NO_CONTENT_STATUS,
        ),
        ("every key it already carries", full.clone(), NO_CONTENT_STATUS),
        ("one new key past the ceiling", json!({ "key50": "one too many" }), 400),
        (
            "several new keys past the ceiling",
            json!({ "key50": "a", "key51": "b", "key52": "c" }),
            400,
        ),
    ] {
        let (status, body) = emulator.tag(&arn, &tags).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body}"
            );
        }
        // Either way the resource still carries exactly 50 keys: a refusal wrote nothing.
        let (_, listed) = emulator.list_tags(&arn).await;
        let count = tags_of(&listed, "tags").len();
        assert_eq!(
            count, 50,
            "case {case:?}: expected the resource to still carry 50 tags, got {count}"
        );
    }
}

#[tokio::test]
async fn the_tagging_operations_accept_an_ivschat_resource_arn_and_refuse_a_malformed_one() {
    let emulator = Emulator::new();
    let room = emulator.create_room(&json!({ "name": "arns" })).await;
    let arn = str_field(&room, "arn");

    // ACCEPTED: an ARN whose resource-type segment is a real ivschat resource type. The
    // published `@pattern` admits only a ONE-character type, which neither of these is, so
    // a boundary that enforced it verbatim would answer 400 for both. A room ARN resolves,
    // so each operation answers its own success status — the two tag MUTATIONS the
    // no-content 204, the report that carries the `tags` map a 200; a
    // logging-configuration ARN is well-formed but names nothing this store holds, so it
    // answers the modelled 404 — never a 400.
    let logging_arn = format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:logging-configuration/abc123ABC456");
    for (case, resource_arn, expected_write, expected_report) in [
        (
            "a room ARN CreateRoom minted",
            arn.clone(),
            NO_CONTENT_STATUS,
            200_u16,
        ),
        ("a logging-configuration ARN", logging_arn, 404, 404),
    ] {
        let (status, body) = emulator
            .tag(&resource_arn, &json!({ "env": "sim2real" }))
            .await;
        assert_eq!(
            status, expected_write,
            "case {case:?}: expected HTTP {expected_write} from TagResource, got {status} {body}"
        );
        let (status, body) = emulator.list_tags(&resource_arn).await;
        assert_eq!(
            status, expected_report,
            "case {case:?}: expected HTTP {expected_report} from ListTagsForResource, got \
             {status} {body}"
        );
        let (status, body) = emulator.untag(&resource_arn, &["env"]).await;
        assert_eq!(
            status, expected_write,
            "case {case:?}: expected HTTP {expected_write} from UntagResource, got {status} {body}"
        );
    }

    // REFUSED: rows differ in WHICH part of the member's contract the value breaks — not
    // an ARN at all, another service's ARN, a region segment outside the modelled class, a
    // resource type outside it, and an ARN past the modelled 128-character length.
    let long_id = "a".repeat(120);
    let refused = vec![
        ("not an ARN at all", "not-an-arn".to_string()),
        (
            "another service's ARN",
            format!("arn:aws:ivs:{REGION}:{ACCOUNT}:room/abc123ABC456"),
        ),
        (
            "an uppercase region segment",
            format!("arn:aws:ivschat:US-WEST-2:{ACCOUNT}:room/abc123ABC456"),
        ),
        (
            "a resource type outside the modelled class",
            format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:Room/abc123ABC456"),
        ),
        (
            "past the modelled length bound",
            format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:room/{long_id}"),
        ),
    ];
    for (case, resource_arn) in refused {
        for (operation, answer) in [
            ("TagResource", emulator.tag(&resource_arn, &json!({})).await),
            ("ListTagsForResource", emulator.list_tags(&resource_arn).await),
            ("UntagResource", emulator.untag(&resource_arn, &["env"]).await),
        ] {
            let (status, body) = answer;
            assert_eq!(
                status, 400,
                "case {case:?} {operation}: expected HTTP 400, got {status} {body}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?} {operation}: expected ValidationException, got {body}"
            );
            assert!(
                error_message(&body)
                    .starts_with(&format!("{operation}.resourceArn ")),
                "case {case:?} {operation}: the refusal must name the operation and the \
                 member, got {body}"
            );
            assert_eq!(
                body.pointer("/fieldList/0/name").and_then(Value::as_str),
                Some(format!("{operation}.resourceArn").as_str()),
                "case {case:?} {operation}: expected fieldList to name the member, got {body}"
            );
        }
    }

    // The length clause is the modelled bound, not one this service invented.
    let (_, body) = emulator
        .tag(
            &format!("arn:aws:ivschat:{REGION}:{ACCOUNT}:room/{long_id}"),
            &json!({}),
        )
        .await;
    assert!(
        error_message(&body).contains("is required to have a maximum length of 128"),
        "an over-long ARN must be refused on the modelled 128-character bound, got {body}"
    );

    // And the room the suite started from is still tagless-but-readable: a refusal wrote
    // nothing.
    let (status, listed) = emulator.list_tags(&arn).await;
    assert_eq!(
        status, 200,
        "the room ARN still lists its tags, got {status} {listed}"
    );
}
