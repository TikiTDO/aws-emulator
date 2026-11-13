// SPDX-License-Identifier: CC0-1.0

//! The SUCCESS STATUS and the framing of this service's tag mutations, `TagResource` and
//! `UntagResource`, driven over the real router so what is asserted is what a signed AWS
//! SDK request observes.
//!
//! RULE: a successful tag mutation answers 204 No Content with zero body bytes, stating
//! the length such an answer has (`content-length: 0`). Both mutations declare an output
//! structure with NO MEMBERS
//! (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse",
//! model_path="services/service-com-amazonaws-emrserverless-awstoledowebservice/smithy-model/com-amazonaws-emrserverless-awstoledowebservice.json")`)
//! and neither states an `@http` code, so there is nothing for a caller to parse and the
//! service reports that with the status that says so rather than with the 200 the protocol
//! leaves defaulted. Every other operation keeps its own success status: the readers and
//! the writers whose output carries members answer 200, and so do this service's other
//! members-less outputs (`StartApplication`, `StopApplication`, `DeleteApplication`).
//!
//! The rule covers the whole class of tag mutations rather than one request shape, so the
//! rows below vary along what it generalises over — the resource class the ARN names, how
//! many pairs the write carries, whether its keys are new or already stored, whether a
//! removed key is stored or absent, the key and value sizes the modelled `TagKey` and
//! `TagValue` bounds admit, the alphabet, and how often the same call is repeated — and the
//! status, the body bytes and the framing are asserted for every row.
//!
//! What the two operations DO to the tag set — the merge, the removal, the bound and the
//! 404 — is owned by `resource_tags.rs`; this suite owns the status alone, and asserts the
//! stored set only far enough to know the status is an answer about a real mutation.
//!
//! The last case is a PROBE of what a real socket then delivers: the status and the absent
//! body reach a socket client, while the one framing header does not — the HTTP/1.1 writer
//! the vendored transport serves on removes `content-length` from a 204, which RFC 9110
//! §15.3.5 forbids there, and that writer is below anything this crate decides. The header
//! this service STATES is what the in-process front door delivers (the inter-service
//! callback path and the SDK connector both serve the router's response unchanged), which
//! is what the rows above assert.

use std::collections::BTreeMap;
use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_emrserverless_awstoledowebservice::EmulatedService;

use super::application_lifecycle::{
    call, call_http, create_application, created_application, delete_application, emulator,
    get_application, list_applications, minimal_create, minimal_job_run, start_application,
    start_job_run, stop_application, urlencode, wait_for_transition, Answer,
};
use super::session_lifecycle::{minimal_session, session_application, start_session};

/// The status a successful tag mutation answers with. See the module docs: the model
/// leaves the code defaulted, so 204 is the recorded behaviour of the service and this
/// suite is what pins it.
const NO_CONTENT_STATUS: u16 = 204;

/// A no-content answer states the length it has: zero. The status carries no entity, so the
/// length is a fixed statement about the answer rather than a count of a body.
const NO_CONTENT_LENGTH: Option<&str> = Some("0");

/// The status every other operation of this service answers a success with.
const WITH_CONTENT_STATUS: u16 = 200;

/// The longest key and value the modelled `TagKey` / `TagValue` admit
/// (`smithy_query(query="shape",shape="TagKey,TagValue")`: `@length(min: 1, max: 128)` and
/// `@length(min: 0, max: 256)`).
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// Every character beyond the alphanumerics that `TagKey`'s pattern
/// `^[A-Za-z0-9 /_.:=+@-]+$` admits, in one key — and the same set is `TagValue`'s.
const PUNCTUATION: &str = "a Z9 /_.:=+@-";

fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

fn tags_of<const N: usize>(pairs: [(String, String); N]) -> BTreeMap<String, String> {
    pairs.into_iter().collect()
}

/// The JSON object a tag set spells, which is how a `TagMap` travels on the wire.
fn tag_body(tags: &BTreeMap<String, String>) -> Value {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), json!(value));
    }
    Value::Object(map)
}

/// `POST /tags/{resourceArn}` — `TagResource`, as the WHOLE response, so the status, the
/// body bytes and the headers can all be asserted. `urlencode` keeps an ARN carrying `:`
/// and `/` the single `{resourceArn}` label of the route.
async fn tag_http(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> ::http::Response<Bytes> {
    call_http(
        router,
        "POST",
        &format!("/tags/{}", urlencode(resource_arn)),
        Some(json!({ "tags": tag_body(tags) })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`, as the whole response.
/// `tagKeys` is an `@httpQuery` list, so each key travels as its own repeated parameter.
async fn untag_http(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[String],
) -> ::http::Response<Bytes> {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", urlencode(key)))
        .collect();
    call_http(
        router,
        "DELETE",
        &format!("/tags/{}?{}", urlencode(resource_arn), query.join("&")),
        None,
    )
    .await
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/tags/{}", urlencode(resource_arn)),
        None,
    )
    .await
}

/// The stored tag set `ListTagsForResource` reports, as a map.
async fn stored_tags(router: &MultiServiceRouter, resource_arn: &str) -> BTreeMap<String, String> {
    let listed = list_tags(router, resource_arn).await;
    assert_eq!(
        listed.status, WITH_CONTENT_STATUS,
        "ListTagsForResource expected {WITH_CONTENT_STATUS}, got {} with {:?}",
        listed.status, listed.body
    );
    listed
        .at("/tags")
        .as_object()
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

/// Assert one mutation answered the no-content success: the status, zero body bytes, and
/// the length such an answer states.
fn assert_no_content(case: &str, operation: &str, response: &::http::Response<Bytes>) {
    let status = response.status().as_u16();
    let body = response.body();
    assert_eq!(
        status,
        NO_CONTENT_STATUS,
        "case {case:?}/{operation}: expected {NO_CONTENT_STATUS}, got {status} with {:?}",
        String::from_utf8_lossy(body)
    );
    assert!(
        body.is_empty(),
        "case {case:?}/{operation}: a no-content answer carries no body bytes, got {:?}",
        String::from_utf8_lossy(body)
    );
    let framing = response
        .headers()
        .get("content-length")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string());
    assert_eq!(
        framing.as_deref(),
        NO_CONTENT_LENGTH,
        "case {case:?}/{operation}: a no-content answer states the length it has, got {framing:?}"
    );
}

/// One row of the status table: the pairs `TagResource` writes, and the keys
/// `UntagResource` then names.
struct StatusCase {
    what: &'static str,
    written: BTreeMap<String, String>,
    removed: Vec<String>,
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// The table. Rows differ in how many pairs the write carries (none, one, many), whether
/// its keys are new or already stored, whether the removal names a stored key or one that
/// was never there, the key and value sizes the modelled bounds admit, and the alphabet.
fn status_table() -> Vec<StatusCase> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    vec![
        StatusCase {
            what: "an empty tag map, and a removal naming a key never stored",
            written: BTreeMap::new(),
            removed: keys(&["never-stored"]),
        },
        StatusCase {
            what: "one pair written and the same key removed",
            written: tags_of([pair("env", "prod")]),
            removed: keys(&["env"]),
        },
        StatusCase {
            what: "three pairs written, one of them removed",
            written: tags_of([
                pair("team", "data"),
                pair("env", "staging"),
                pair("cost-center", "42"),
            ]),
            removed: keys(&["env"]),
        },
        StatusCase {
            what: "a write that only replaces a value, and a removal of absent keys",
            written: tags_of([pair("team", "platform")]),
            removed: keys(&["absent-a", "absent-b"]),
        },
        StatusCase {
            what: "a key and a value at their modelled maximum length",
            written: tags_of([(long_key.clone(), long_value)]),
            removed: vec![long_key],
        },
        StatusCase {
            what: "an empty value, the TagValue minimum, beside a one-character key",
            written: tags_of([pair("retain", ""), pair("k", "short")]),
            removed: keys(&["retain"]),
        },
        StatusCase {
            what: "the punctuation TagKey and TagValue admit",
            written: tags_of([(PUNCTUATION.to_string(), PUNCTUATION.to_string())]),
            removed: vec![PUNCTUATION.to_string()],
        },
        StatusCase {
            what: "fifty pairs written and every one of them removed",
            written: (0..50)
                .map(|n| pair(&format!("key-{n:03}"), &format!("value-{n:03}")))
                .collect(),
            removed: (0..50).map(|n| format!("key-{n:03}")).collect(),
        },
    ]
}

/// Every successful tag mutation answers the no-content status, on every resource class
/// this service tags, for every row of the table — and the mutation it reports really
/// landed.
#[tokio::test]
async fn every_successful_tag_mutation_answers_no_content() {
    let table = status_table();
    assert!(
        table.len() >= 5,
        "the status table must span the class, got {} rows",
        table.len()
    );

    let router = emulator();
    let application_id = created_application(&router, "tok-no-content").await;
    let application_arn = get_application(&router, &application_id)
        .await
        .at("/application/arn")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // A job run and an interactive session carry tags of their own, and their ARNs reach
    // the same route, so the status is asserted on all three classes.
    let submitted = start_job_run(&router, &application_id, minimal_job_run("job-no-content")).await;
    assert_eq!(
        submitted.status, WITH_CONTENT_STATUS,
        "StartJobRun expected {WITH_CONTENT_STATUS}, got {} with {:?}",
        submitted.status, submitted.body
    );
    let job_run_arn = submitted.at("/arn").as_str().unwrap_or_default().to_string();

    let (session_application_id, _) = session_application(&router, "tok-no-content-session").await;
    let started = start_session(
        &router,
        &session_application_id,
        minimal_session("ses-no-content"),
    )
    .await;
    assert_eq!(
        started.status, WITH_CONTENT_STATUS,
        "StartSession expected {WITH_CONTENT_STATUS}, got {} with {:?}",
        started.status, started.body
    );
    let session_arn = started.at("/arn").as_str().unwrap_or_default().to_string();

    for (class, resource_arn) in [
        ("application", application_arn),
        ("job run", job_run_arn),
        ("session", session_arn),
    ] {
        // The set the resource carries as the rows run, so each row's answer is checked
        // against what the mutation really left behind.
        let mut expected = stored_tags(&router, &resource_arn).await;

        for row in &table {
            let case = format!("{class}: {}", row.what);

            // Twice: the status does not depend on whether the call changed anything, so
            // the second, no-op write answers the same way.
            for attempt in 1..=2 {
                let response = tag_http(&router, &resource_arn, &row.written).await;
                assert_no_content(&case, &format!("TagResource attempt {attempt}"), &response);
            }
            expected.extend(row.written.clone());
            let reported = stored_tags(&router, &resource_arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the write must persist before its status means anything, \
                 got {reported:?}"
            );

            for attempt in 1..=2 {
                let response = untag_http(&router, &resource_arn, &row.removed).await;
                assert_no_content(&case, &format!("UntagResource attempt {attempt}"), &response);
            }
            for key in &row.removed {
                expected.remove(key);
            }
            let reported = stored_tags(&router, &resource_arn).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: the removal must persist too, got {reported:?}"
            );
        }
    }
}

/// The no-content status belongs to the two bodyless tag mutations alone: every other
/// operation of this service keeps its own success status, including the ones whose output
/// also declares no members, and a tag mutation that FAILS keeps its modelled error status
/// and its error body.
#[tokio::test]
async fn only_the_tag_mutations_answer_no_content() {
    let router = emulator();

    // A create, a read and a list: answers that carry members keep their 200 and their
    // body.
    let created = create_application(&router, minimal_create("tok-contrast")).await;
    assert_eq!(
        created.status, WITH_CONTENT_STATUS,
        "case \"CreateApplication\": expected {WITH_CONTENT_STATUS}, got {} with {:?}",
        created.status, created.body
    );
    let application_id = created
        .at("/applicationId")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let application_arn = created.at("/arn").as_str().unwrap_or_default().to_string();
    wait_for_transition();

    for (case, answer) in [
        ("GetApplication", get_application(&router, &application_id).await),
        ("ListApplications", list_applications(&router, "").await),
        ("ListTagsForResource", list_tags(&router, &application_arn).await),
    ] {
        assert_eq!(
            answer.status, WITH_CONTENT_STATUS,
            "case {case:?}: an answer carrying members keeps its own {WITH_CONTENT_STATUS}, \
             got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.body.is_object(),
            "case {case:?}: an answer carrying members must carry a body, got {:?}",
            answer.body
        );
    }

    // The contrast the rule is narrowed against: this service's OTHER members-less
    // outputs. `StartApplication`, `StopApplication` and `DeleteApplication` report
    // nothing either and keep the defaulted 200.
    let mutation = tag_http(&router, &application_arn, &tags_of([pair("env", "prod")])).await;
    assert_no_content("the mutation the contrast is drawn against", "TagResource", &mutation);

    let started = start_application(&router, &application_id).await;
    assert_eq!(
        started.status, WITH_CONTENT_STATUS,
        "case \"StartApplication\": this service's other bodyless successes keep the \
         protocol default, got {} with {:?}",
        started.status, started.body
    );
    wait_for_transition();
    let stopped = stop_application(&router, &application_id).await;
    assert_eq!(
        stopped.status, WITH_CONTENT_STATUS,
        "case \"StopApplication\": this service's other bodyless successes keep the \
         protocol default, got {} with {:?}",
        stopped.status, stopped.body
    );
    wait_for_transition();
    let deleted = delete_application(&router, &application_id).await;
    assert_eq!(
        deleted.status, WITH_CONTENT_STATUS,
        "case \"DeleteApplication\": this service's other bodyless successes keep the \
         protocol default, got {} with {:?}",
        deleted.status, deleted.body
    );

    // And a tag mutation the service REFUSES keeps the modelled failure status and the
    // error envelope: the narrowing is about the success alone.
    let unknown_arn = application_arn.replace(&application_id, "0000000000009999");
    for (case, answer) in [
        (
            "TagResource on an ARN naming no stored resource",
            call(
                &router,
                "POST",
                &format!("/tags/{}", urlencode(&unknown_arn)),
                Some(json!({ "tags": { "env": "prod" } })),
            )
            .await,
        ),
        (
            "UntagResource on an ARN naming no stored resource",
            call(
                &router,
                "DELETE",
                &format!("/tags/{}?tagKeys=env", urlencode(&unknown_arn)),
                None,
            )
            .await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: a refusal keeps its modelled status, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: a refusal carries its error shape, got {:?}",
            answer.body
        );
    }
}

/// This service, mounted on its own router and served on a loopback port, with the router
/// handed back so a fixture can be set up through the same front door.
async fn served() -> (u16, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The signature itself is not under test here; the framing of the answer is.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    let (endpoint, router) = emulator_server::serve_on_ephemeral_port(router).await;
    let port = endpoint
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .unwrap_or_else(|| panic!("an endpoint with a port, got {endpoint:?}"));
    (port, router)
}

/// One request written straight onto a socket, with everything the server answered read
/// back as text.
///
/// `connection: close` so the server closes the socket after answering and the read below
/// terminates on EOF rather than on a parse of the very framing this case is about.
fn raw_request(port: u16, method: &str, target: &str, body: &str) -> String {
    use std::io::{Read, Write};

    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect to the served port");
    let request = format!(
        "{method} {target} HTTP/1.1\r\n\
         host: 127.0.0.1:{port}\r\n\
         authorization: {SOCKET_AUTHORIZATION}\r\n\
         content-type: application/json\r\n\
         content-length: {}\r\n\
         connection: close\r\n\
         \r\n{body}",
        body.len()
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

/// A SigV4 credential scope naming this service's signing name, which is what routes a
/// socket request here; request authentication is off, so the signature is never checked.
const SOCKET_AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/emr-serverless/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The `content-length` a response head states, or `None` when it states none.
fn stated_length(head: &str) -> Option<String> {
    head.lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().to_string())
}

/// RULE, on the wire: a successful tag mutation is a 204 that carries no body and is not
/// chunked, for both mutations.
///
/// The one framing header is asserted only as far as a socket can show it: the HTTP/1.1
/// writer the vendored transport serves on removes `content-length` from a 204, below
/// anything this crate decides, so what the service STATES is pinned on the in-process
/// answer above and this case pins what a socket client then observes.
#[tokio::test(flavor = "multi_thread")]
async fn a_tag_mutation_on_the_wire_carries_the_status_and_no_body() {
    let (port, router) = served().await;
    let application_id = created_application(&router, "tok-wire").await;
    let application_arn = get_application(&router, &application_id)
        .await
        .at("/application/arn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let encoded = urlencode(&application_arn);

    // (case, method, request target, request body)
    let exchanges: Vec<(&str, &str, String, String)> = vec![
        (
            "TagResource",
            "POST",
            format!("/tags/{encoded}"),
            json!({ "tags": { "env": "prod" } }).to_string(),
        ),
        (
            "UntagResource",
            "DELETE",
            format!("/tags/{encoded}?tagKeys=env"),
            String::new(),
        ),
    ];

    for (case, method, target, body) in exchanges {
        let verb = method.to_string();
        let answer = tokio::task::spawn_blocking(move || raw_request(port, &verb, &target, &body))
            .await
            .expect("the socket exchange runs to completion");
        assert!(
            answer.starts_with("HTTP/1.1 204"),
            "case {case:?}: expected a 204 on the wire, got {answer:?}"
        );
        let (head, served_body) = answer
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("case {case:?}: a response head and body, got {answer:?}"));
        assert!(
            served_body.is_empty(),
            "case {case:?}: a no-content answer carries no body, got {served_body:?}"
        );
        assert!(
            !head.to_ascii_lowercase().contains("transfer-encoding"),
            "case {case:?}: a no-content answer is not chunked, got {head:?}"
        );
        assert_eq!(
            stated_length(head),
            None,
            "case {case:?}: the transport's writer states no length on a 204, got {head:?}"
        );
    }

    // The bodyless answers were real mutations: the tag was written and then removed.
    assert!(
        stored_tags(&router, &application_arn).await.is_empty(),
        "case 'the wire exchanges': the write and the removal must both have landed"
    );
}
