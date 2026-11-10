// SPDX-License-Identifier: CC0-1.0

//! What a successful EventBridge Pipes call ANSWERS on the wire when it has nothing to
//! report, driven over `MultiServiceRouter::dispatch` — the one path a socket-served
//! request takes — so what is asserted is what an AWS SDK client sees.
//!
//! RULE: an operation whose modelled output structure declares NO member leaves its caller
//! nothing to parse, so its success is HTTP 204 No Content with zero body bytes, stating the
//! length it has; an operation whose output carries members keeps the 200 the protocol
//! defaults to, with that body. In this model the members-less outputs are exactly
//! `TagResourceResponse` and `UntagResourceResponse`
//! (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse")` — both
//! `"members": {}`; every other operation's response shape declares members), and neither
//! operation states an `@http` code of its own
//! (`smithy_query(query="operation",operation="TagResource,UntagResource")` reports
//! `code: 200, codeDefaulted: true`).
//!
//! The rule is about a CLASS of answer rather than one recorded request, so the rows below
//! differ along what it generalises over: how many pairs a write carries, whether a removed
//! key is stored or was never there, the key and value sizes the modelled `TagKey` (1..128)
//! and `TagValue` (0..256) bounds admit, the alphabet, and how often the same call is
//! repeated. The status is asserted for every row, and the write is read back through
//! `ListTagsForResource` and `DescribePipe` so the status is an answer about a real
//! mutation rather than a refusal dressed as a success.
//!
//! What the two operations DO to the tag set — the merge, the removal, the 50-tag bound and
//! the 404 — is owned by `tests/all/pipe_tagging.rs`; this suite owns the status and the
//! framing alone. The harness is the one `pipe_lifecycle.rs` and `pipe_tagging.rs` own.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{call_raw, create, describe, router, PAST_SETTLE, REGION};
use crate::pipe_tagging::{
    assert_tags_agree, create_body, encode, pipe_arn, tag_http, tag_map, untag_http,
    TAG_MUTATION_STATUS,
};

/// The status every operation whose output carries members answers on success: the
/// protocol's default, which none of this model's `@http` bindings overrides.
const WITH_CONTENT_STATUS: u16 = 200;

/// The length a no-content answer states for the body it does not carry. The recorded
/// no-content answers of this class carry that header, and a client that frames a bodyless
/// message by its stated length needs it. An answer served over HTTP/1.1 reaches the client
/// without it — the HTTP writer the vendored listener is built on drops `content-length`
/// from a status that cannot carry a body — so this is the framing the in-process response
/// carries, which is every path a service crate owns.
const NO_CONTENT_LENGTH: &str = "0";

/// The `TagKey` length maximum and the `TagValue` one. Source: the `TagKey` / `TagValue`
/// shapes in `smithy-model/com-amazonaws-pipes-pipes.json`.
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// The most tags one request may carry. Source: `TagMap`'s `@length(max: 50)` in the same
/// model.
const MAX_TAGS: usize = 50;

/// `TagResource` over the shared request path (`pipe_tagging::tag_http`), with the row's
/// pairs as the `tags` member.
async fn tag_raw(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> http::Response<bytes::Bytes> {
    let mut map = Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), json!(value));
    }
    tag_http(router, resource_arn, &Value::Object(map)).await
}

/// `UntagResource` over the shared request path (`pipe_tagging::untag_http`), naming the
/// row's keys.
async fn untag_raw(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[String],
) -> http::Response<bytes::Bytes> {
    let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
    untag_http(router, resource_arn, &borrowed).await
}

/// The body bytes of one response, as text for a failure message.
fn body_text(response: &http::Response<bytes::Bytes>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

/// Assert one mutation answered the no-content success: the status, zero body bytes, and
/// the length such an answer states.
fn assert_no_content(case: &str, operation: &str, response: &http::Response<bytes::Bytes>) {
    let status = response.status().as_u16();
    assert_eq!(
        status,
        TAG_MUTATION_STATUS,
        "case {case:?}/{operation}: a members-less output must answer \
         {TAG_MUTATION_STATUS}, got {status} with {:?}",
        body_text(response)
    );
    assert!(
        response.body().is_empty(),
        "case {case:?}/{operation}: a no-content answer carries no body bytes, got {:?}",
        body_text(response)
    );
    let framing = response
        .headers()
        .get("content-length")
        .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string());
    assert_eq!(
        framing.as_deref(),
        Some(NO_CONTENT_LENGTH),
        "case {case:?}/{operation}: a no-content answer states the length it has, got \
         {framing:?}"
    );
}

/// Assert one answer carries content: its own success status and a JSON body with members.
fn assert_with_content(case: &str, operation: &str, response: &http::Response<bytes::Bytes>) {
    let status = response.status().as_u16();
    assert_eq!(
        status,
        WITH_CONTENT_STATUS,
        "case {case:?}/{operation}: an output carrying members keeps \
         {WITH_CONTENT_STATUS}, got {status} with {:?}",
        body_text(response)
    );
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    let members = parsed.as_object().map(Map::len).unwrap_or_default();
    assert!(
        members > 0,
        "case {case:?}/{operation}: an answer carrying members must carry them, got {:?}",
        body_text(response)
    );
}

/// One pair as owned strings.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// A tag map from pairs.
fn tags<const N: usize>(pairs: [(String, String); N]) -> BTreeMap<String, String> {
    pairs.into_iter().collect()
}

fn keys(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_string()).collect()
}

/// One row of the status table: the pairs `TagResource` writes onto a pipe that already
/// carries `stored`, and the keys `UntagResource` then names.
struct MutationCase {
    what: &'static str,
    stored: BTreeMap<String, String>,
    written: BTreeMap<String, String>,
    removed: Vec<String>,
}

/// The table. Rows differ in how many pairs the write carries (one, several, every one
/// `TagMap` admits), whether the removal names a stored key or one that was never there,
/// the key and value sizes the modelled bounds admit, and the alphabet.
fn mutation_table() -> Vec<MutationCase> {
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    vec![
        MutationCase {
            what: "one pair onto an untagged pipe, and the same key removed",
            stored: BTreeMap::new(),
            written: tags([pair("env", "prod")]),
            removed: keys(&["env"]),
        },
        MutationCase {
            what: "several pairs, one replacing a stored value, one of them removed",
            stored: tags([pair("env", "dev")]),
            written: tags([
                pair("env", "prod"),
                pair("team", "core"),
                pair("owner", "on-call"),
            ]),
            removed: keys(&["env"]),
        },
        MutationCase {
            what: "a write that only replaces a value, and a removal naming absent keys",
            stored: tags([pair("team", "core")]),
            written: tags([pair("team", "platform")]),
            removed: keys(&["absent-a", "absent-b"]),
        },
        MutationCase {
            what: "a key at the TagKey maximum and a value at the TagValue maximum",
            stored: BTreeMap::new(),
            written: tags([(long_key.clone(), long_value)]),
            removed: vec![long_key],
        },
        MutationCase {
            what: "an empty value, the TagValue minimum, beside a one-character key",
            stored: BTreeMap::new(),
            written: tags([pair("retired", ""), pair("k", "short")]),
            removed: keys(&["retired"]),
        },
        MutationCase {
            what: "keys and values carrying punctuation and non-ascii letters",
            stored: tags([pair("cost:center", "eu-west/1")]),
            written: tags([
                pair("cost:center", "eu-west/42"),
                pair("équipe", "naïve 製造"),
                pair("ключ", "значение"),
            ]),
            removed: keys(&["équipe", "ключ"]),
        },
        MutationCase {
            what: "every pair TagMap admits written and every one of them removed",
            stored: BTreeMap::new(),
            written: (0..MAX_TAGS)
                .map(|n| pair(&format!("key-{n:03}"), &format!("value-{n:03}")))
                .collect(),
            removed: (0..MAX_TAGS).map(|n| format!("key-{n:03}")).collect(),
        },
    ]
}

/// A `CreatePipe` body carrying `stored` as its `Tags`, or none when the row starts
/// untagged (`TagMap` is `@length(min: 1)`, so an empty map is not a legal member value).
fn created_with(stored: &BTreeMap<String, String>) -> Value {
    if stored.is_empty() {
        return create_body(None);
    }
    let mut map = Map::new();
    for (key, value) in stored {
        map.insert(key.clone(), json!(value));
    }
    create_body(Some(&Value::Object(map)))
}

#[tokio::test]
async fn every_successful_tag_mutation_answers_no_content() {
    let router = router();
    let table = mutation_table();
    assert!(
        table.len() >= 5,
        "the status table must span the class, got {} rows",
        table.len()
    );

    for (index, case) in table.iter().enumerate() {
        let name = format!("nocontent{index}");
        let created = create(&router, &name, created_with(&case.stored)).await;
        assert_eq!(
            created.status, WITH_CONTENT_STATUS,
            "case {:?}: CreatePipe must succeed, got {created:?}",
            case.what
        );
        let arn = pipe_arn(&name);

        // Repeated: the status does not depend on whether the mutation changed anything,
        // so the second, no-op call answers exactly the same way.
        for attempt in 1..=2 {
            let response = tag_raw(&router, &arn, &case.written).await;
            assert_no_content(case.what, &format!("TagResource attempt {attempt}"), &response);
        }

        // The write landed, which is what makes the status an answer about a real
        // mutation: both readers of the pipe's tags report the merged map.
        let mut expected = case.stored.clone();
        expected.extend(case.written.clone());
        assert_tags_agree(&router, case.what, &name, &expected).await;

        for attempt in 1..=2 {
            let response = untag_raw(&router, &arn, &case.removed).await;
            assert_no_content(
                case.what,
                &format!("UntagResource attempt {attempt}"),
                &response,
            );
        }

        for key in &case.removed {
            expected.remove(key);
        }
        assert_tags_agree(&router, &format!("{} (removed)", case.what), &name, &expected).await;
    }
}

/// The no-content status belongs to the two members-less outputs alone: every other
/// operation of this service answers its own success status with the body its output
/// declares, so the change is not a blanket 204.
#[tokio::test]
async fn every_operation_whose_output_carries_members_keeps_its_own_status() {
    let router = router();
    let name = "contentstatus";
    let created = call_raw(
        &router,
        "POST",
        &format!("/v1/pipes/{name}"),
        Some(create_body(Some(&json!({"env": "prod"})))),
        REGION,
    )
    .await;
    assert_with_content("the create every other row is read off", "CreatePipe", &created);
    let arn = pipe_arn(name);

    // The contrast is drawn against a real no-content answer, so both halves of the rule
    // are asserted against the same pipe.
    assert_no_content(
        "the mutation the contrast is drawn against",
        "TagResource",
        &tag_raw(&router, &arn, &tags([pair("team", "core")])).await,
    );

    // The create's state move is settled first: a lifecycle call arriving during a move in
    // flight is refused with the modelled 409, which is a different rule
    // (`tests/all/pipe_lifecycle_conflicts.rs`).
    tokio::time::sleep(PAST_SETTLE).await;

    // Every reader, then each lifecycle writer in an order no row refuses: the pipe rests
    // RUNNING after the create, so it is stopped, started and updated in turn, with the
    // move each one starts settled before the next arrives.
    let readers: Vec<(&str, &str, String, Option<Value>)> = vec![
        ("DescribePipe", "GET", format!("/v1/pipes/{name}"), None),
        ("ListPipes", "GET", "/v1/pipes".to_string(), None),
        (
            "ListTagsForResource",
            "GET",
            format!("/tags/{}", encode(&arn)),
            None,
        ),
    ];
    for (operation, method, uri, body) in readers {
        let response = call_raw(&router, method, &uri, body, REGION).await;
        assert_with_content("a reader of the pipe", operation, &response);
    }

    let writers: Vec<(&str, &str, String, Option<Value>)> = vec![
        ("StopPipe", "POST", format!("/v1/pipes/{name}/stop"), None),
        ("StartPipe", "POST", format!("/v1/pipes/{name}/start"), None),
        (
            "UpdatePipe",
            "PUT",
            format!("/v1/pipes/{name}"),
            Some(json!({ "RoleArn": "arn:aws:iam::000000000000:role/pipes-role" })),
        ),
        ("DeletePipe", "DELETE", format!("/v1/pipes/{name}"), None),
    ];
    for (operation, method, uri, body) in writers {
        let response = call_raw(&router, method, &uri, body, REGION).await;
        assert_with_content("a lifecycle writer", operation, &response);
        tokio::time::sleep(PAST_SETTLE).await;
    }
}

/// A REFUSED tag mutation is not a no-content answer: the status belongs to the success
/// path alone, so a refusal keeps its modelled status and carries the error envelope a
/// client parses.
#[tokio::test]
async fn a_refused_tag_mutation_is_not_a_no_content_answer() {
    let router = router();
    let name = "refusednocontent";
    let created = create(&router, name, create_body(Some(&json!({"env": "prod"})))).await;
    assert_eq!(
        created.status, WITH_CONTENT_STATUS,
        "the fixture pipe must be created, got {created:?}"
    );
    let stored = tag_map(&json!({"env": "prod"}));

    // (the case, the whole response, the status the refusal carries). The rows differ along
    // which operation is refused and which rule refuses it: an ARN that resolves to no
    // pipe, and a tag whose key is past the published `TagKey` bound.
    let absent = pipe_arn("refusednocontentghost");
    let over_long = "k".repeat(MAX_TAG_KEY_LEN + 1);
    let rows: Vec<(&str, http::Response<bytes::Bytes>, u16)> = vec![
        (
            "TagResource on an ARN no pipe carries",
            tag_raw(&router, &absent, &tags([pair("env", "prod")])).await,
            404,
        ),
        (
            "UntagResource on an ARN no pipe carries",
            untag_raw(&router, &absent, &keys(&["env"])).await,
            404,
        ),
        (
            "TagResource with a key past the TagKey bound",
            tag_raw(&router, &pipe_arn(name), &tags([(over_long.clone(), "x".to_string())])).await,
            400,
        ),
        (
            "UntagResource with a key past the TagKey bound",
            untag_raw(&router, &pipe_arn(name), &[over_long]).await,
            400,
        ),
    ];

    for (case, response, expected) in rows {
        let status = response.status().as_u16();
        assert_eq!(
            status, expected,
            "case {case:?}: a refusal keeps its modelled status, got {status} with {:?}",
            body_text(&response)
        );
        assert_ne!(
            status, TAG_MUTATION_STATUS,
            "case {case:?}: the no-content status belongs to the success path alone"
        );
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        assert!(
            parsed.as_object().is_some_and(|members| !members.is_empty()),
            "case {case:?}: a refusal carries its error shape, got {:?}",
            body_text(&response)
        );
    }

    // And nothing the refusals touched was written: the pipe still carries what it was
    // created with.
    assert_tags_agree(&router, "after the refusals", name, &stored).await;
    let described = describe(&router, name).await;
    assert_eq!(
        described.status, WITH_CONTENT_STATUS,
        "the fixture pipe must still be there, got {described:?}"
    );
}
