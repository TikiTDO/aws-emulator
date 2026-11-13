// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for how this service frames an answer that carries NO body.
//!
//! RULE under test: a response with no body states its length — `content-length: 0` —
//! rather than leaving the framing header off; a response that carries a body is framed
//! from the body it carries. Every operation whose output is `Unit` answers with no
//! body (`smithy_query(query="digest")`), so the rule is
//! asserted over each implemented one of them rather than over the one operation a
//! recording happened to cover. WHICH STATUS each of them answers is a separate
//! question, pinned in `tests/all/empty_result_statuses.rs`.
//!
//! The last case serves the router on a real socket, where the framing the service
//! states is not the last word: see its own note.
//!
//! Every other case drives the real wire path in process: a signed `restJson1` request
//! through `MultiServiceRouter::dispatch`, exactly as the production router serves it,
//! and exactly what the in-process front door (the inter-service callback path, the SDK
//! connector) delivers to its caller. The harness (`Emulator`, `Answer`, …) is the one
//! `registry_schema_lifecycle` owns — `tests/all/*.rs` are modules of ONE test crate.

use serde_json::json;

use crate::registry_schema_lifecycle::{
    create_registry, registry_uri, schema_uri, Answer, Emulator, REGION,
};

/// The header a response with no body carries, and the length it states.
const EMPTY_BODY_LENGTH: &str = "0";

/// Assert that `answer` is an empty-bodied success, framed with its length.
fn assert_empty_and_framed(case: &str, answer: &Answer, expected_status: u16) {
    assert_eq!(
        answer.status, expected_status,
        "case {case}: expected {expected_status}, got {} {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.raw.is_empty(),
        "case {case}: this answer carries no body, got {:?}",
        String::from_utf8_lossy(&answer.raw)
    );
    assert_eq!(
        answer.header("content-length"),
        Some(EMPTY_BODY_LENGTH),
        "case {case}: an answer with no body must state its length, got {:?}",
        answer.header("content-length")
    );
}

/// RULE: every operation that answers with no body frames that answer with
/// `content-length: 0`.
#[tokio::test]
async fn an_answer_with_no_body_states_its_length() {
    let emulator = Emulator::new();

    // Rows: one per implemented operation whose output is `Unit`, each reached after
    // the state it deletes was created, and each in the two shapes its resource comes
    // in — a schema deleted from a registry of its own, and the registry itself.
    // `DeleteSchemaVersion` is implemented and
    // its own suite (`schema_version_deletion.rs`) carries its framing row, because the
    // version it deletes has to be minted first; `UntagResource` and `TagResource` are
    // implemented and their own suites (`resource_tag_removal.rs`,
    // `resource_tag_application.rs`) carry their framing rows for the same reason — the
    // resource whose tags they write has to exist first. `DeleteResourcePolicy` answers with no
    // body too, but at 200 rather than 204, and its row lives with the other statuses in
    // `empty_result_statuses.rs`.
    for (case, registry, schemas) in [
        ("one schema", "framing-one", vec!["only-schema"]),
        (
            "several schemas",
            "framing-many",
            vec!["first-schema", "second-schema", "third-schema"],
        ),
        ("no schema at all", "framing-empty", vec![]),
    ] {
        create_registry(&emulator, registry).await;
        for schema in &schemas {
            let created = emulator
                .call(
                    "POST",
                    &schema_uri(registry, schema),
                    Some(json!({"Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3"})),
                )
                .await;
            assert_eq!(
                created.status, 201,
                "case {case}/{schema}: expected 201 from CreateSchema, got {} {:?}",
                created.status, created.body
            );
        }

        for schema in &schemas {
            let deleted = emulator
                .call("DELETE", &schema_uri(registry, schema), None)
                .await;
            assert_empty_and_framed(&format!("{case}/DeleteSchema/{schema}"), &deleted, 204);
        }

        let deleted = emulator.call("DELETE", &registry_uri(registry), None).await;
        assert_empty_and_framed(&format!("{case}/DeleteRegistry"), &deleted, 204);
    }
}

/// What an empty answer looks like on the WIRE, where the service's framing is not the
/// last word: the HTTP/1.1 writer this emulator is served by removes `content-length`
/// from a `204`, because RFC 7230 §3.3.2 forbids it there ("A server MUST NOT send a
/// Content-Length header field in any response with a status code of 1xx or 204"), and
/// that writer is a dependency of the vendored HTTP layer rather than anything this
/// service decides. So this case pins what the socket DOES answer — the status and the
/// absence of a body — while the cases above pin the framing the service itself states,
/// which is what the in-process front door (the inter-service callback path and the SDK
/// connector, both of which serve `dispatch`'s response unchanged) delivers.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_answer_on_the_wire_carries_the_status_and_no_body() {
    let emulator = Emulator::new();
    let registry = "wire-registry";
    create_registry(&emulator, registry).await;
    let schema = "wire-schema";
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );

    // The same router, serving the same store, on a loopback port.
    let (endpoint, _served) = emulator_server::serve_on_ephemeral_port(emulator.router()).await;
    let port: u16 = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));

    let path = schema_uri(registry, schema);
    let answer = tokio::task::spawn_blocking(move || raw_request(port, "DELETE", &path))
        .await
        .expect("the socket exchange runs to completion");
    assert!(
        answer.starts_with("HTTP/1.1 204"),
        "expected a 204 from DeleteSchema on the wire, got {answer:?}"
    );
    let (head, body) = answer
        .split_once("\r\n\r\n")
        .unwrap_or_else(|| panic!("a response head and body, got {answer:?}"));
    assert!(
        body.is_empty(),
        "case DeleteSchema: a 204 carries no body, got {body:?}"
    );
    assert!(
        !head.to_ascii_lowercase().contains("transfer-encoding"),
        "case DeleteSchema: an empty answer is not chunked, got {head:?}"
    );

    // And the schema really is gone, so the 204 reported a deletion rather than being
    // an empty answer to nothing.
    let described = emulator.call("GET", &schema_uri(registry, schema), None).await;
    assert_eq!(
        described.status, 404,
        "after the wire delete the schema must be gone, got {} {:?}",
        described.status, described.body
    );
}

/// One request written straight onto a socket, with everything the server answered
/// read back as text. Shared with the sibling suite that pins the STATUS a bodiless
/// answer carries (`empty_result_statuses.rs`), so one socket exchange serves both.
///
/// `connection: close` so the server closes the socket after answering and the read
/// below terminates on EOF rather than on a parse of the framing this very case is
/// about.
pub(crate) fn raw_request(port: u16, method: &str, path: &str) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "{method} {path} HTTP/1.1\r\n\
         host: localhost\r\n\
         authorization: AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20240101/{REGION}/schemas/aws4_request, \
         SignedHeaders=host, Signature=deadbeef\r\n\
         content-length: 0\r\n\
         connection: close\r\n\
         \r\n"
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

/// RULE: only an EMPTY body is framed by the service — an answer that carries a body
/// is framed from the body it carries, so it never claims a length that is not its
/// own.
#[tokio::test]
async fn an_answer_with_a_body_is_framed_from_the_body_it_carries() {
    let emulator = Emulator::new();
    let registry = "framed-registry";
    create_registry(&emulator, registry).await;
    let schema = "framed-schema";
    let created = emulator
        .call(
            "POST",
            &schema_uri(registry, schema),
            Some(json!({"Content": r#"{"openapi":"3.0.0"}"#, "Type": "OpenApi3"})),
        )
        .await;
    assert_eq!(
        created.status, 201,
        "expected 201 from CreateSchema, got {} {:?}",
        created.status, created.body
    );

    // Rows: a create (201), a read (200), a listing (200), and a refusal (404) — a
    // body on the success path and a body on the error path.
    for (case, answer) in [
        ("CreateSchema", created),
        (
            "DescribeSchema",
            emulator.call("GET", &schema_uri(registry, schema), None).await,
        ),
        (
            "ListSchemas",
            emulator
                .call("GET", &format!("{}/schemas", registry_uri(registry)), None)
                .await,
        ),
        (
            "DescribeSchema of a schema that does not exist",
            emulator
                .call("GET", &schema_uri(registry, "no-such-schema"), None)
                .await,
        ),
    ] {
        assert!(
            !answer.raw.is_empty(),
            "case {case}: this answer carries a body"
        );
        if let Some(stated) = answer.header("content-length") {
            assert_eq!(
                stated,
                answer.raw.len().to_string(),
                "case {case}: a stated length must be the body's own, got {stated:?} for \
                 {} bytes",
                answer.raw.len()
            );
        }
    }
}
