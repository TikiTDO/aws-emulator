// SPDX-License-Identifier: CC0-1.0

//! How this service FRAMES an answer that carries no content, driven over the wire.
//!
//! RULE: a success at the no-content status carries no body and STATES the length it has
//! — `content-length: 0` — while every answer that DOES carry content keeps the body its
//! renderer wrote and never states that length for it.
//!
//! Every operation of this service whose success code is that status is a tagging write
//! whose output is `smithy.api#Unit`: `CreateTags` (POST /v1/tags/{ResourceArn}, code
//! 204) and `DeleteTags` (DELETE /v1/tags/{ResourceArn}, code 204), which is what
//! `smithy_query(query="operation", operation="CreateTags,DeleteTags")` reports for this
//! model; no other operation of it answers anything but 200. The status itself comes from
//! `service_common::no_content_responses::NO_CONTENT_STATUS` rather than being written out
//! here.
//!
//! The table spans what the rule generalises over: both operations that answer at that
//! status, crossed with every kind of resource this service lets a caller tag, with the
//! tag sets a write can carry (one tag, several, an empty value, a key that replaces one
//! already there, keys that are not there at all) and a repeat of the same write, so the
//! framing is pinned however much or little the write actually changed. The contrast rows
//! are the answers that carry content: the tag reader, the resource's own describe, a
//! create, and the modelled refusals at 404 and 400.
//!
//! What the socket finally delivers is the HTTP writer's decision, not this crate's: over
//! HTTP/1.1 that writer leaves the framing header off a no-content answer, which is below
//! anything a service crate states.

use serde_json::{json, Value};
use service_common::no_content_responses::NO_CONTENT_STATUS;

use crate::broker_lifecycle::{create_body, string_at, Mq};
use crate::configuration_name::configuration_body;

/// The length an answer that carries no content states for the body it does not have.
const NO_CONTENT_LENGTH: &str = "0";

/// The `content-length` an answer states, if any.
fn content_length(headers: &http::HeaderMap) -> Option<String> {
    headers
        .get("content-length")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
}

/// Every kind of resource of this service a caller can create and tag.
#[derive(Debug, Clone, Copy)]
enum Taggable {
    Broker,
    Configuration,
}

/// The ARN of a freshly created resource of this kind, and the path its own describe
/// reads it back from.
async fn create_taggable(mq: &Mq, kind: Taggable, name: &str) -> (String, String) {
    let (path, body, arn_pointer, id_pointer) = match kind {
        Taggable::Broker => ("/v1/brokers", create_body(name), "/brokerArn", "/brokerId"),
        Taggable::Configuration => (
            "/v1/configurations",
            configuration_body(name),
            "/arn",
            "/id",
        ),
    };
    let (status, created) = mq.call("POST", path, Some(body)).await;
    assert_eq!(
        status, 200,
        "setup {kind:?} {name:?}: expected the resource to be created, got {status} {created:?}"
    );
    let arn = string_at(&created, arn_pointer);
    let id = string_at(&created, id_pointer);
    let describe_path = match kind {
        Taggable::Broker => format!("/v1/brokers/{id}"),
        Taggable::Configuration => format!("/v1/configurations/{id}"),
    };
    (arn, describe_path)
}

/// What one row of the no-content table asks of the service: a write, and the tags or
/// keys it carries.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// `CreateTags` with these key/value pairs.
    Create(&'static [(&'static str, &'static str)]),
    /// `DeleteTags` naming these keys.
    Delete(&'static [&'static str]),
}

/// One row: a label, and the write it makes.
struct NoContentCase {
    name: &'static str,
    write: Write,
}

/// Rows differing along the operation, the tag count, the value form, and whether the
/// write changes anything at all.
const NO_CONTENT_CASES: &[NoContentCase] = &[
    NoContentCase {
        name: "CreateTags, one tag",
        write: Write::Create(&[("team", "payments")]),
    },
    NoContentCase {
        name: "CreateTags, several tags",
        write: Write::Create(&[("team", "payments"), ("env", "prod"), ("owner", "ops")]),
    },
    NoContentCase {
        name: "CreateTags, an empty value",
        write: Write::Create(&[("marker", "")]),
    },
    NoContentCase {
        name: "CreateTags, a key already on the resource",
        write: Write::Create(&[("team", "search")]),
    },
    NoContentCase {
        name: "CreateTags, no tag at all",
        write: Write::Create(&[]),
    },
    NoContentCase {
        name: "DeleteTags, one key that is there",
        write: Write::Delete(&["team"]),
    },
    NoContentCase {
        name: "DeleteTags, several keys",
        write: Write::Delete(&["env", "owner", "marker"]),
    },
    NoContentCase {
        name: "DeleteTags, a key that was never set",
        write: Write::Delete(&["never-set"]),
    },
];

/// The row's write, as the status, headers and body bytes the dispatch answered with.
async fn perform(mq: &Mq, arn: &str, write: Write) -> (u16, http::HeaderMap, bytes::Bytes) {
    match write {
        Write::Create(pairs) => {
            let tags = pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), json!(value)))
                .collect::<serde_json::Map<String, Value>>();
            mq.call_raw(
                "POST",
                &format!("/v1/tags/{arn}"),
                Some(json!({"tags": Value::Object(tags)})),
            )
            .await
        }
        Write::Delete(keys) => {
            let query = keys
                .iter()
                .map(|key| format!("tagKeys={key}"))
                .collect::<Vec<String>>()
                .join("&");
            mq.call_raw("DELETE", &format!("/v1/tags/{arn}?{query}"), None)
                .await
        }
    }
}

/// Assert the whole no-content framing of one answer.
fn assert_no_content(case: &str, status: u16, headers: &http::HeaderMap, body: &[u8]) {
    assert_eq!(
        status, NO_CONTENT_STATUS,
        "case {case:?}: expected the modelled no-content status {NO_CONTENT_STATUS}, got \
         {status} {:?}",
        String::from_utf8_lossy(body)
    );
    assert!(
        body.is_empty(),
        "case {case:?}: an answer at that status carries no body, got {:?}",
        String::from_utf8_lossy(body)
    );
    assert_eq!(
        content_length(headers).as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {case:?}: expected the answer to state content-length {NO_CONTENT_LENGTH}, got \
         {:?}",
        content_length(headers)
    );
}

/// RULE, no-content half: every tagging write of this service answers the no-content
/// status with no body and states the length it has — for both operations, on every kind
/// of resource, and however much the write changed.
#[tokio::test]
async fn every_tagging_write_states_the_length_of_the_body_it_does_not_carry() {
    let mq = Mq::new("eu-west-2");

    for (index, kind) in [Taggable::Broker, Taggable::Configuration]
        .into_iter()
        .enumerate()
    {
        for (row, case) in NO_CONTENT_CASES.iter().enumerate() {
            let label = format!("{kind:?}: {}", case.name);
            let (arn, _) =
                create_taggable(&mq, kind, &format!("framing-{index}-{row}")).await;

            // A row whose write replaces or removes a key needs that key on the resource
            // first, so the write is the one the row describes rather than a no-op.
            let (status, _code, seeded) = mq
                .call_with_code(
                    "POST",
                    &format!("/v1/tags/{arn}"),
                    Some(json!({"tags": {"team": "payments", "env": "prod", "owner": "ops",
                                         "marker": ""}})),
                )
                .await;
            assert_eq!(
                status, NO_CONTENT_STATUS,
                "setup {label:?}: the seeding write must be served, got {status} {seeded:?}"
            );

            let (status, headers, body) = perform(&mq, &arn, case.write).await;
            assert_no_content(&label, status, &headers, &body);

            // The same write again: the framing is the status's, not the change's.
            let (status, headers, body) = perform(&mq, &arn, case.write).await;
            assert_no_content(&format!("{label} (repeated)"), status, &headers, &body);
        }
    }
}

/// What a real socket delivers for one of those writes, as opposed to what the service
/// serializes: the framing headers of an answer that carries no content belong to the HTTP
/// writer this emulator is served by, which leaves the length off a 204 — so the boundary
/// between what this crate states and what reaches a client is a fact in the suite rather
/// than a claim in a comment. What is required of it either way is that the answer never
/// MIS-frames: the status, no body, no chunked framing, and no length other than the one
/// such an answer has.
#[tokio::test(flavor = "multi_thread")]
async fn on_the_wire_a_tagging_write_is_the_status_with_no_body() {
    let engine = std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_mq_mq::EmulatedService::new(engine));

    let create = http::Request::builder()
        .method("POST")
        .uri("https://mq.us-east-1.amazonaws.com/v1/configurations")
        .header("authorization", crate::broker_lifecycle::authorization("us-east-1"))
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(
            serde_json::to_vec(&configuration_body("framing-wire")).expect("a request body"),
        ))
        .expect("the test request is well formed");
    let created = router.dispatch(create).await;
    assert_eq!(
        created.status().as_u16(),
        200,
        "setup: the fixture configuration must be created, got {} {:?}",
        created.status().as_u16(),
        String::from_utf8_lossy(created.body())
    );
    let created: Value =
        serde_json::from_slice(created.body()).expect("CreateConfiguration answers JSON");
    let arn = string_at(&created, "/arn");

    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(router).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));
    // The whole ARN is ONE path segment, so its `:` and `/` travel percent-encoded.
    let path = format!("/v1/tags/{}", arn.replace(':', "%3A").replace('/', "%2F"));
    let answer = tokio::task::spawn_blocking(move || raw_create_tags(port, &path))
        .await
        .expect("the socket exchange runs to completion");

    assert!(
        answer.starts_with(&format!("HTTP/1.1 {NO_CONTENT_STATUS}")),
        "case CreateTags on the wire: expected a {NO_CONTENT_STATUS} on the wire, got {answer:?}"
    );
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answer:?}"));
    assert!(
        body.is_empty(),
        "case CreateTags on the wire: an answer at that status carries no body, got {body:?}"
    );
    let head = head.to_ascii_lowercase();
    assert!(
        !head.contains("transfer-encoding"),
        "case CreateTags on the wire: an answer with no content is not chunked, got {head:?}"
    );
    // Whatever the writer decides about the header itself, the only length it may state
    // for such an answer is the one it has.
    let stated = head
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .map(|value| value.trim().to_string());
    assert!(
        stated.as_deref().is_none_or(|value| value == NO_CONTENT_LENGTH),
        "case CreateTags on the wire: a stated length must be {NO_CONTENT_LENGTH}, got {stated:?}"
    );
}

/// One `CreateTags` request written straight onto a socket, with everything the server
/// answered read back as text.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the very framing this case is about.
fn raw_create_tags(port: u16, path: &str) -> String {
    use std::io::{Read, Write};

    let payload = json!({"tags": {"team": "payments"}}).to_string();
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         host: mq.us-east-1.amazonaws.com\r\n\
         authorization: {}\r\n\
         content-type: application/json\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{payload}",
        crate::broker_lifecycle::authorization("us-east-1"),
        payload.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("write the request");
    let mut answer = Vec::new();
    stream
        .read_to_end(&mut answer)
        .expect("read the whole answer");
    String::from_utf8_lossy(&answer).to_string()
}

/// RULE, carries-content half: an answer with a document in it keeps that document and
/// never states the no-content length — the tag reader, the resource's own describe, a
/// create, and the modelled refusals.
#[tokio::test]
async fn an_answer_that_carries_content_is_left_as_its_renderer_wrote_it() {
    let mq = Mq::new("us-east-1");
    let (arn, describe_path) = create_taggable(&mq, Taggable::Broker, "framing-content").await;
    let (status, _headers, _body) = mq
        .call_raw(
            "POST",
            &format!("/v1/tags/{arn}"),
            Some(json!({"tags": {"team": "payments"}})),
        )
        .await;
    assert_eq!(
        status, NO_CONTENT_STATUS,
        "setup: the write must be served, got {status}"
    );

    // Rows: a reader, a resource read, a create, an ARN that names nothing (404) and a
    // tag map past the documented quota (400) — one per status class this service answers
    // with a document.
    let quota_breaking_tags = (0..=service_com_amazonaws_mq_mq::state::MAX_TAGS)
        .map(|index| (format!("key-{index}"), json!(format!("value-{index}"))))
        .collect::<serde_json::Map<String, Value>>();
    let rows: Vec<(&str, u16, (u16, http::HeaderMap, bytes::Bytes))> = vec![
        (
            "ListTags",
            200,
            mq.call_raw("GET", &format!("/v1/tags/{arn}"), None).await,
        ),
        (
            "DescribeBroker",
            200,
            mq.call_raw("GET", &describe_path, None).await,
        ),
        (
            "CreateConfiguration",
            200,
            mq.call_raw(
                "POST",
                "/v1/configurations",
                Some(configuration_body("framing-created")),
            )
            .await,
        ),
        (
            "CreateTags on an ARN that names nothing",
            404,
            mq.call_raw(
                "POST",
                "/v1/tags/arn:aws:mq:us-east-1:000000000000:broker:absent:b-0",
                Some(json!({"tags": {"team": "payments"}})),
            )
            .await,
        ),
        (
            "DeleteTags on an ARN that names nothing",
            404,
            mq.call_raw(
                "DELETE",
                "/v1/tags/arn:aws:mq:us-east-1:000000000000:broker:absent:b-0?tagKeys=team",
                None,
            )
            .await,
        ),
        (
            "CreateTags past the tag quota",
            400,
            mq.call_raw(
                "POST",
                &format!("/v1/tags/{arn}"),
                Some(json!({"tags": Value::Object(quota_breaking_tags)})),
            )
            .await,
        ),
    ];

    for (case, expected_status, (status, headers, body)) in rows {
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {:?}",
            String::from_utf8_lossy(&body)
        );
        assert!(
            !body.is_empty(),
            "case {case:?}: an answer at {status} carries its document, got an empty body"
        );
        assert_ne!(
            content_length(&headers).as_deref(),
            Some(NO_CONTENT_LENGTH),
            "case {case:?}: an answer carrying {} body bytes must not state content-length \
             {NO_CONTENT_LENGTH}",
            body.len()
        );
        // Whatever length such an answer states is the length it carries.
        if let Some(stated) = content_length(&headers) {
            assert_eq!(
                stated,
                body.len().to_string(),
                "case {case:?}: a stated length must be the body's own {}, got {stated:?}",
                body.len()
            );
        }
    }
}
