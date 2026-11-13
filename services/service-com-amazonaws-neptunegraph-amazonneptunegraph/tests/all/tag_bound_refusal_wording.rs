// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal of a tag bound, driven over the wire (`restJson1`)
//! through the same router the emulator serves from.
//!
//! The rule every assertion here is about: a constraint violation this service finds in a
//! handler is reported in the one form it reports a violation the model-driven walk in
//! front of the handler found — the count of broken constraints, then one clause per
//! violation naming the member path, the value in the form that class of violation states
//! it, and the requirement the value failed. A bound measured on what a record holds
//! after the call is the SAME bound the model puts on a request, so the two must read
//! identically.
//!
//! The tables below span what that rule generalises over: which side of the boundary
//! caught the violation, the operation that caught it (four of them, both the ones that
//! take tags at create time and the one that writes them later), which half of a tag
//! entry carried the broken bound (the map's entry count, a key's length, a value's
//! length), how the record reached the bound (what was stored, what the request repeated
//! and what it added), and how far past the bound the input was.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::state::{
    MAX_TAGS, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, PROVISIONING_SPAN,
};
use service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the test's SigV4 credential scope names.
const REGION: &str = "us-east-1";

/// The smallest `provisionedMemory` the model allows (`range 16..24576`), which is all a
/// graph these tests create needs.
const MIN_PROVISIONED_MEMORY: i64 = 16;

/// An IAM role ARN matching the modelled `RoleArn` pattern
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)(/[\w+=,.@-]+)+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/NeptuneTagBoundRole";

/// An emulator holding only this service, on its own engine, so each test starts from an
/// empty store.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one `restJson1` request and return its status and parsed JSON body.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |value| {
        serde_json::to_vec(&value).expect("a JSON request body serializes")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{REGION}/neptune-graph/\
                 aws4_request, SignedHeaders=host, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(payload))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Percent-encode one path segment, so an ARN fits the non-greedy `{resourceArn}` label
/// of `/tags/{resourceArn}` as a single segment.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The wire error code of a failure body (`__type`), without its namespace.
fn error_code(body: &Value) -> String {
    body["__type"]
        .as_str()
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The `message` member of a failure body.
fn message(body: &Value) -> String {
    body["message"].as_str().unwrap_or_default().to_string()
}

/// The envelope one broken constraint is reported in, restated here independently of the
/// service's own builder.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a map whose entry count is past its bound fails: the count in the form a
/// collection states its size, the member the map sits at, and the bound the model
/// publishes for it (`smithy_query(query="shape",shape="TagMap")` -> `@length(0..50)`).
fn entry_count_clause(total: usize) -> String {
    format!(
        "Value '{total} entries' at 'tags' failed to satisfy constraint: Member must have \
         length less than or equal to {MAX_TAGS}"
    )
}

/// The clause a tag key longer than its bound fails: reported against the member the map
/// sits at, listing everything a key of that map has to satisfy, and — a length bound
/// being what it is — stating the requirements without echoing the key
/// (`smithy_query(query="shape",shape="TagKey")` -> `@length(1..128)`,
/// `@pattern("^[a-zA-Z+-=._:/]+$")`, the pattern reported without its anchors as this
/// service reports every pattern).
fn key_length_clause() -> String {
    format!(
        "Value at 'tags' failed to satisfy constraint: Map keys must satisfy constraint: \
         [Member must have length less than or equal to {MAX_TAG_KEY_CHARS}, Member must \
         have length greater than or equal to 1, Member must satisfy regular expression \
         pattern: [a-zA-Z+-=._:/]+]"
    )
}

/// The clause a tag value longer than its bound fails, from its member path on: reported
/// at the ENTRY's own path, which is where the walk in front of the handler states a
/// constraint an entry's value broke (`smithy_query(query="shape",shape="TagValue")` ->
/// `@length(0..256)`). Only the value half of the clause differs between the two sides —
/// the walk echoes the oversized value, elided, where a handler of this service withholds
/// it — so the rows below pin everything from the member path on.
fn value_clause_tail(key: &str) -> String {
    format!(
        "at 'tags.{key}' failed to satisfy constraint: Member must have length less than \
         or equal to {MAX_TAG_VALUE_CHARS}"
    )
}

/// `count` distinct tags whose keys all start with `prefix`, for the entry-count rows.
fn many_tags(prefix: &str, count: usize) -> BTreeMap<String, String> {
    (0..count)
        .map(|index| (format!("{prefix}{index}"), format!("value{index}")))
        .collect()
}

/// `POST /graphs` with `tags`, returning the created graph's ARN.
async fn create_graph(
    router: &MultiServiceRouter,
    name: &str,
    tags: &BTreeMap<String, String>,
) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/graphs",
        Some(json!({
            "graphName": name,
            "provisionedMemory": MIN_PROVISIONED_MEMORY,
            "tags": tags,
        })),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {name:?}: expected 201 from CreateGraph, got {status}: {created}"
    );
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name:?}: no arn in {created}"))
        .to_string()
}

/// Read the graph at `path` until it reports `AVAILABLE`, which is the status a call
/// that reads the graph beside the tags it stores needs it in. It waits for the
/// CONDITION rather than for a span, with a deadline generous beside the one the
/// transition is owed on.
async fn wait_until_available(router: &MultiServiceRouter, path: &str) {
    for _ in 0..40 {
        let (status, body) = call(router, "GET", path, None).await;
        assert_eq!(
            status, 200,
            "case {path}: expected 200 from the read, got {status}: {body}"
        );
        if body["status"] == json!("AVAILABLE") {
            return;
        }
        tokio::time::sleep(PROVISIONING_SPAN / 4).await;
    }
    let (_, body) = call(router, "GET", path, None).await;
    panic!(
        "case {path}: never became AVAILABLE, last status {}",
        body["status"]
    );
}

/// `POST /tags/{resourceArn}` with a `tags` map.
async fn tag(
    router: &MultiServiceRouter,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/tags/{}", encode(arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

/// Assert one refusal: 400, `ValidationException`, and the whole message.
fn assert_refusal(case: &str, status: u16, body: &Value, expected: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(body),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {body}"
    );
    let actual = message(body);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {expected:?}, got {actual:?}"
    );
}

#[tokio::test]
async fn the_tag_entry_bound_reads_the_same_whichever_side_of_the_boundary_caught_it() {
    let router = emulator();

    // (case, how many tags the resource already holds, how many of the stored keys the
    // request repeats, how many keys it adds) — the rows differ in what was stored, what
    // the request repeats (which replaces a value and adds no entry) and how far past the
    // bound the merge lands.
    let rows: [(&str, usize, usize, usize); 5] = [
        ("one key past a resource at the bound", MAX_TAGS, 0, 1),
        ("five keys past a resource at the bound", MAX_TAGS, 0, 5),
        (
            "a full request onto a resource at the bound",
            MAX_TAGS,
            0,
            MAX_TAGS,
        ),
        ("a half-full resource and a full request", 26, 0, 25),
        (
            "a resource at the bound, re-tagged and grown at once",
            MAX_TAGS,
            10,
            3,
        ),
    ];

    for (index, (case, stored, repeated, added)) in rows.iter().enumerate() {
        let held = many_tags("stored", *stored);
        let arn = create_graph(&router, &format!("merge-graph-{index}"), &held).await;

        // What the request carries: keys the resource already holds take a new value and
        // add no entry, so only the fresh keys grow the record.
        let mut sent: BTreeMap<String, String> = held
            .keys()
            .take(*repeated)
            .map(|key| (key.clone(), "replaced".to_string()))
            .collect();
        sent.extend(many_tags("added", *added));

        let total = stored + added;
        let (status, body) = tag(&router, &arn, &sent).await;
        assert_refusal(case, status, &body, &one_violation(&entry_count_clause(total)));

        // The SAME count broken on the other side of the boundary: one request carrying
        // that many entries is refused by the model-driven walk, and a caller cannot tell
        // the two refusals apart.
        let (boundary_status, boundary_body) = call(
            &router,
            "POST",
            "/graphs",
            Some(json!({
                "graphName": format!("boundary-graph-{index}"),
                "provisionedMemory": MIN_PROVISIONED_MEMORY,
                "tags": many_tags("sent", total),
            })),
        )
        .await;
        let boundary_case = format!("{case}: the same count refused at the boundary");
        assert_refusal(
            &boundary_case,
            boundary_status,
            &boundary_body,
            &one_violation(&entry_count_clause(total)),
        );
    }
}

#[tokio::test]
async fn an_oversized_tag_key_is_refused_the_same_way_by_every_operation_that_stores_tags() {
    let router = emulator();
    let host = create_graph(&router, "key-bound-host", &BTreeMap::new()).await;
    // One row below snapshots this graph, which is served only once the graph is settled,
    // so the rows measure the tag bound and not the source graph's status.
    wait_until_available(
        &router,
        &format!("/graphs/{}", host.rsplit('/').next().unwrap_or_default()),
    )
    .await;

    // Keys past the published bound: one character past it, far past it, and one whose
    // characters are multi-byte, because the bound counts characters rather than bytes.
    let oversized: [(&str, String); 4] = [
        ("one character past the bound", "k".repeat(MAX_TAG_KEY_CHARS + 1)),
        ("far past the bound", "k".repeat(MAX_TAG_KEY_CHARS * 3)),
        (
            "one character past the bound, in multi-byte characters",
            "\u{e9}".repeat(MAX_TAG_KEY_CHARS + 1),
        ),
        (
            "at the bound in characters but past it in bytes",
            "\u{e9}".repeat(MAX_TAG_KEY_CHARS),
        ),
    ];

    for (index, (case, key)) in oversized.iter().enumerate() {
        // A key of exactly the bound's length is accepted, so the rows below refuse the
        // length and not the member.
        let accepted_at_bound = key.chars().count() <= MAX_TAG_KEY_CHARS;
        let tags: BTreeMap<String, String> =
            [(key.clone(), "v".to_string())].into_iter().collect();

        // Every operation this suite drives a `tags` map through, each sent the same key:
        // three that store tags on a record they create, and the one that writes them to a
        // record that already exists.
        let requests: [(&str, &str, String, Value); 4] = [
            (
                "CreateGraph",
                "POST",
                "/graphs".to_string(),
                json!({
                    "graphName": format!("key-graph-{index}"),
                    "provisionedMemory": MIN_PROVISIONED_MEMORY,
                    "tags": tags,
                }),
            ),
            (
                "CreateGraphSnapshot",
                "POST",
                "/snapshots".to_string(),
                json!({
                    "graphIdentifier": host.rsplit('/').next().unwrap_or_default(),
                    "snapshotName": format!("key-snapshot-{index}"),
                    "tags": tags,
                }),
            ),
            (
                "CreateGraphUsingImportTask",
                "POST",
                "/importtasks".to_string(),
                json!({
                    "graphName": format!("key-import-graph-{index}"),
                    "source": "s3://example-bucket/data/",
                    "roleArn": ROLE_ARN,
                    "tags": tags,
                }),
            ),
            (
                "TagResource",
                "POST",
                format!("/tags/{}", encode(&host)),
                json!({ "tags": tags }),
            ),
        ];

        for (operation, method, path, body) in requests {
            let label = format!("{operation}: {case}");
            let (status, answer) = call(&router, method, &path, Some(body)).await;
            if accepted_at_bound {
                assert_ne!(
                    status, 400,
                    "case {label:?}: a key at the bound must not be refused, got {answer}"
                );
                continue;
            }
            assert_refusal(&label, status, &answer, &one_violation(&key_length_clause()));
        }
    }
}

/// A bound on a tag VALUE is reported at the entry it was broken at, in the same counting
/// envelope as every other refusal, and a value AT the bound is stored and reads back.
///
/// The model's own `@length` on `TagValue` reaches the walk in front of the handler, which
/// echoes the oversized value into the clause's value half where a handler of this service
/// withholds it, so the rows pin the whole clause from the member path on — the half both
/// sides state identically.
#[tokio::test]
async fn an_oversized_tag_value_is_refused_at_the_entry_it_was_broken_at() {
    let router = emulator();
    let arn = create_graph(&router, "value-bound-host", &BTreeMap::new()).await;

    // (case, the value's length in characters) — at the published bound, one past it, and
    // far past it.
    let rows: [(&str, usize); 3] = [
        ("at the bound", MAX_TAG_VALUE_CHARS),
        ("one character past the bound", MAX_TAG_VALUE_CHARS + 1),
        ("far past the bound", MAX_TAG_VALUE_CHARS * 4),
    ];

    for (case, length) in rows {
        let value = "x".repeat(length);
        let tags: BTreeMap<String, String> =
            [("v".to_string(), value.clone())].into_iter().collect();
        let (status, answer) = tag(&router, &arn, &tags).await;
        if length <= MAX_TAG_VALUE_CHARS {
            assert_eq!(
                status, 200,
                "case {case:?}: a value at the bound must be stored, got {status}: {answer}"
            );
            // Stored, not merely accepted: the record reads the value back.
            let (read_status, read) =
                call(&router, "GET", &format!("/tags/{}", encode(&arn)), None).await;
            assert_eq!(
                (read_status, read["tags"]["v"].as_str()),
                (200, Some(value.as_str())),
                "case {case:?}: expected the value at the bound to be stored, got \
                 {read_status}: {read}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {answer}"
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {answer}"
        );
        let actual = message(&answer);
        assert!(
            actual.starts_with("1 validation error detected: Value"),
            "case {case:?}: expected the one counting envelope, got {actual:?}"
        );
        let tail = value_clause_tail("v");
        assert!(
            actual.ends_with(&tail),
            "case {case:?}: expected a clause ending {tail:?}, got {actual:?}"
        );
    }
}
