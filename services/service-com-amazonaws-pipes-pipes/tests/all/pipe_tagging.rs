// SPDX-License-Identifier: CC0-1.0

//! The EventBridge Pipes tagging slice, end to end over the wire.
//!
//! Every assertion goes through `MultiServiceRouter::dispatch` — the one path a
//! socket-served request takes — so what is tested is the HTTP behaviour of
//! `ListTagsForResource`/`TagResource`/`UntagResource` and their agreement with
//! `CreatePipe`/`DescribePipe`, not a Rust helper. The harness (`router`, `call`,
//! `create`, `describe`, `Reply`) is the one `pipe_lifecycle` already owns.
//!
//! The rule under test: the tags of a pipe are ONE persisted map on the pipe record,
//! addressed by that record's ARN — `TagResource` merges keys into it (a new key is
//! added, a key already present has its value replaced), `UntagResource` removes the
//! keys it names and nothing else, and `ListTagsForResource` and `DescribePipe` both
//! report exactly that map (`ListTagsForResource` always as a map, empty when the pipe
//! carries no tag; `DescribePipe` as the record member, absent when it carries none).
//! Each test's rows span what the rule generalises over (the number of tags, the key and
//! value charsets and lengths, which keys a call names, whether a key is present) rather
//! than one recorded request.
//!
//! The rules a tag's content must satisfy (the published `TagKey`/`TagValue` length
//! bounds, the documented tag character set, and the prefix AWS reserves for its own tags)
//! have their own suite in `tests/all/tag_key_rules.rs`, and how an unresolvable
//! `resourceArn` is reported has one in `tests/all/tagging_refusals.rs`; the helpers below
//! are `pub(crate)` so those suites reach this same one request path rather than copying
//! it.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_lifecycle::{
    call, call_raw, create, describe, minimal_body, reply_of, router, Reply, REGION,
};

/// The uppercase hex digits a percent-escape is written with (RFC 3986 §2.1).
const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// The most tags one pipe carries. Source: the `TagResource` documentation in
/// `smithy-model/com-amazonaws-pipes-pipes.json` ("You can associate as many as 50 tags
/// with a pipe."), the same bound as `TagMap`'s `@length(max: 50)`.
const MAX_TAGS: usize = 50;

/// The `TagKey` length bound (1..128) and the `TagValue` one (0..256). Source: the
/// `TagKey` / `TagValue` shapes in `smithy-model/com-amazonaws-pipes-pipes.json`.
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// The status a successful `TagResource` / `UntagResource` answers: no content, because
/// `TagResourceResponse` and `UntagResourceResponse` declare no member
/// (`smithy_query(query="shape",shape="TagResourceResponse,UntagResourceResponse")`).
/// What such an answer carries — no body at all — is pinned in
/// `tests/all/tag_mutation_no_content.rs`.
pub(crate) const TAG_MUTATION_STATUS: u16 = 204;

/// The ARN `CreatePipe` mints for `name` in the region every request here is signed for.
pub(crate) fn pipe_arn(name: &str) -> String {
    format!(
        "arn:aws:pipes:{REGION}:{}:pipe/{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// Percent-encode `value` the way an AWS SDK encodes a non-greedy `@httpLabel` or a query
/// value: every byte outside RFC 3986's unreserved set (`A-Za-z0-9` and `-._~`) becomes
/// `%XX`, so the ARN's `:` and `/` arrive escaped and `/tags/{resourceArn}` stays one
/// path segment.
pub(crate) fn encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(*byte));
            }
            other => {
                encoded.push('%');
                encoded.push(char::from(HEX_DIGITS[usize::from(other >> 4)]));
                encoded.push(char::from(HEX_DIGITS[usize::from(other & 0x0f)]));
            }
        }
    }
    encoded
}

pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Reply {
    call(
        router,
        "GET",
        &format!("/tags/{}", encode(resource_arn)),
        None,
        REGION,
    )
    .await
}

pub(crate) async fn tag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &Value,
) -> Reply {
    reply_of(tag_http(router, resource_arn, tags).await)
}

/// `POST /tags/{resourceArn}` — `TagResource`, as the WHOLE response, so a suite can assert
/// the framing (the status line, the body bytes and the headers) and not only the members a
/// body parses into. [`tag`] is this plus that parse, so there is ONE request path for this
/// operation.
pub(crate) async fn tag_http(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &Value,
) -> http::Response<bytes::Bytes> {
    call_raw(
        router,
        "POST",
        &format!("/tags/{}", encode(resource_arn)),
        Some(json!({ "tags": tags })),
        REGION,
    )
    .await
}

pub(crate) async fn untag(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[&str],
) -> Reply {
    reply_of(untag_http(router, resource_arn, tag_keys).await)
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`, as the whole response, for the
/// same reason [`tag_http`] exists. `tagKeys` is an `@httpQuery` list, so each key travels
/// as its own repeated parameter.
pub(crate) async fn untag_http(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tag_keys: &[&str],
) -> http::Response<bytes::Bytes> {
    let query = tag_keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<_>>()
        .join("&");
    call_raw(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encode(resource_arn)),
        None,
        REGION,
    )
    .await
}

async fn delete_pipe(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "DELETE", &format!("/v1/pipes/{name}"), None, REGION).await
}

/// A `CreatePipe` body carrying the required members plus `Tags` when the case has any.
pub(crate) fn create_body(tags: Option<&Value>) -> Value {
    let mut body = minimal_body(
        "arn:aws:sqs:us-west-2:000000000000:orders",
        "arn:aws:lambda:us-west-2:000000000000:function:handler",
    );
    if let Some(tags) = tags {
        body.as_object_mut()
            .expect("the CreatePipe body is an object")
            .insert("Tags".to_string(), tags.clone());
    }
    body
}

/// A JSON tag object as an order-independent map (`Value::Null` — an absent member —
/// reads as no tags at all).
pub(crate) fn tag_map(tags: &Value) -> BTreeMap<String, String> {
    tags.as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// `n` distinct tags, `key0..key{n-1}` -> `value0..value{n-1}`.
fn generated_tags(n: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..n {
        map.insert(format!("key{index}"), json!(format!("value{index}")));
    }
    Value::Object(map)
}

/// Assert that both readers of the pipe's tags agree with `expected`.
pub(crate) async fn assert_tags_agree(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    expected: &BTreeMap<String, String>,
) {
    let listed = list_tags(router, &pipe_arn(name)).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource must answer 200, got {listed:?}"
    );
    assert_eq!(
        &tag_map(listed.member("tags")),
        expected,
        "case {case:?}: ListTagsForResource must report the stored tags {expected:?}, got {:?}",
        tag_map(listed.member("tags"))
    );
    if expected.is_empty() {
        // The tagging layer reports the map either way, so an untagged pipe reports an
        // EMPTY one rather than no member — `TagMap`'s `@length(min: 1)` bounds what a
        // request may send, not what a report carries.
        assert_eq!(
            listed.member("tags"),
            &json!({}),
            "case {case:?}: an untagged pipe must report an empty tags map, got {:?}",
            listed.member("tags")
        );
        // `DescribePipe` reports the pipe RECORD, whose `Tags` member is reported the same
        // way the tagging layer reports its own: an EMPTY map for a pipe with no tags. See
        // `tests/all/pipe_reported_configuration.rs`.
        assert_eq!(
            describe(router, name).await.member("Tags"),
            &json!({}),
            "case {case:?}: an untagged pipe's record must carry an empty Tags map"
        );
    }

    let described = describe(router, name).await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: DescribePipe must answer 200, got {described:?}"
    );
    assert_eq!(
        &tag_map(described.member("Tags")),
        expected,
        "case {case:?}: DescribePipe must report the same map as ListTagsForResource \
         ({expected:?}), got {:?}",
        tag_map(described.member("Tags"))
    );
}

/// RULE: `ListTagsForResource` reports the tags persisted on the pipe its `resourceArn`
/// names — for a freshly created pipe, exactly the `Tags` `CreatePipe` stored.
///
/// The rows differ along the number of tags (none, one, every one `TagMap` allows), the
/// value's emptiness, the charset of both key and value, and the published length bounds
/// of `TagKey` and `TagValue`.
#[tokio::test]
async fn list_tags_for_resource_reports_the_tags_create_pipe_stored() {
    let router = router();
    let long_key = "k".repeat(MAX_TAG_KEY_LEN);
    let long_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let rows: Vec<(&str, Option<Value>)> = vec![
        ("no-tags-at-all", None),
        ("one-tag", Some(json!({"env": "prod"}))),
        // `TagValue` is `@length(min: 0)`: an empty value is a value, not an absence.
        ("empty-value", Some(json!({"env": ""}))),
        (
            // Multi-byte LETTERS, which the documented tag character set admits and
            // which must round-trip byte-identically; what it does NOT admit has its
            // own table in `tag_key_rules`.
            "unicode-key-and-value",
            Some(json!({"équipe": "日本語 製造", "ключ": "значение"})),
        ),
        (
            "key-and-value-at-their-length-bounds",
            Some(json!({ long_key.clone(): long_value.clone() })),
        ),
        ("every-tag-tagmap-allows", Some(generated_tags(MAX_TAGS))),
    ];

    for (index, (case, tags)) in rows.iter().enumerate() {
        let name = format!("createtags{index}");
        let created = create(&router, &name, create_body(tags.as_ref())).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );
        let expected = tags.as_ref().map(tag_map).unwrap_or_default();
        assert_tags_agree(&router, case, &name, &expected).await;
    }
}

/// RULE: `TagResource` MERGES its map into the pipe's: a key the pipe does not carry is
/// added, a key it does carry has its value replaced, and a key the request does not name
/// is left alone — so repeating the same call leaves the same tags (the operation is
/// `@idempotent`).
///
/// The rows differ along whether the pipe starts tagged, whether the request's keys are
/// new, already present or a mix, and the value's charset and emptiness.
#[tokio::test]
async fn tag_resource_merges_into_the_tags_the_pipe_already_carries() {
    let router = router();
    let rows: Vec<(&str, Option<Value>, Value, Value)> = vec![
        (
            "first-tags-on-an-untagged-pipe",
            None,
            json!({"env": "prod"}),
            json!({"env": "prod"}),
        ),
        (
            "new-key-is-appended",
            Some(json!({"env": "prod"})),
            json!({"team": "core"}),
            json!({"env": "prod", "team": "core"}),
        ),
        (
            "present-key-has-its-value-replaced",
            Some(json!({"env": "prod"})),
            json!({"env": "staging"}),
            json!({"env": "staging"}),
        ),
        (
            "replace-and-append-together",
            Some(json!({"env": "prod", "team": "core"})),
            json!({"env": "dev", "owner": "ops"}),
            json!({"env": "dev", "owner": "ops", "team": "core"}),
        ),
        (
            "replacing-with-an-empty-value-keeps-the-key",
            Some(json!({"env": "prod"})),
            json!({"env": ""}),
            json!({"env": ""}),
        ),
        (
            "unicode-value-round-trips",
            Some(json!({"env": "prod"})),
            json!({"名前": "パイプ 製造", "env": "pród"}),
            json!({"名前": "パイプ 製造", "env": "pród"}),
        ),
        (
            "every-tag-tagmap-allows-in-one-call",
            None,
            generated_tags(MAX_TAGS),
            generated_tags(MAX_TAGS),
        ),
    ];

    for (index, (case, initial, request, merged)) in rows.iter().enumerate() {
        let name = format!("mergetags{index}");
        let created = create(&router, &name, create_body(initial.as_ref())).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );

        let tagged = tag(&router, &pipe_arn(&name), request).await;
        assert_eq!(
            tagged.status, TAG_MUTATION_STATUS,
            "case {case:?}: TagResource must answer {TAG_MUTATION_STATUS}, got {tagged:?}"
        );
        let expected = tag_map(merged);
        assert_tags_agree(&router, case, &name, &expected).await;

        // `@idempotent`: the same call again states the same end state.
        let again = tag(&router, &pipe_arn(&name), request).await;
        assert_eq!(
            again.status, TAG_MUTATION_STATUS,
            "case {case:?}: a repeated TagResource must answer {TAG_MUTATION_STATUS}, got \
             {again:?}"
        );
        assert_tags_agree(&router, &format!("{case} (repeated)"), &name, &expected).await;
    }
}

/// RULE: `UntagResource` removes the keys it names and only those; naming a key the pipe
/// does not carry removes nothing and fails nothing, so a repeated call leaves the same
/// tags behind.
///
/// The rows differ along how many keys are named, whether each named key is present, and
/// whether every key is removed.
#[tokio::test]
async fn untag_resource_removes_only_the_keys_it_names() {
    let router = router();
    let rows: Vec<(&str, Value, Vec<String>, Value)> = vec![
        (
            "one-of-three-keys",
            json!({"env": "prod", "team": "core", "owner": "ops"}),
            vec!["team".to_string()],
            json!({"env": "prod", "owner": "ops"}),
        ),
        (
            "a-key-the-pipe-does-not-carry-is-a-noop",
            json!({"env": "prod"}),
            vec!["absent".to_string()],
            json!({"env": "prod"}),
        ),
        (
            "present-and-absent-keys-mixed",
            json!({"env": "prod", "team": "core"}),
            vec!["team".to_string(), "absent".to_string()],
            json!({"env": "prod"}),
        ),
        (
            "a-unicode-key",
            json!({"名前": "パイプ", "env": "prod"}),
            vec!["名前".to_string()],
            json!({"env": "prod"}),
        ),
        (
            "every-key-leaves-the-pipe-untagged",
            json!({"env": "prod", "team": "core"}),
            vec!["env".to_string(), "team".to_string()],
            json!({}),
        ),
        (
            // `TagKeyList` is `@length(max: 50)`: the longest legal key list, naming one
            // stored key and 49 the pipe never carried.
            "every-key-tagkeylist-allows-one-of-them-stored",
            generated_tags(3),
            std::iter::once("key1".to_string())
                .chain((0..MAX_TAGS - 1).map(|index| format!("missing{index}")))
                .collect(),
            json!({"key0": "value0", "key2": "value2"}),
        ),
    ];

    for (index, (case, initial, keys, remaining)) in rows.iter().enumerate() {
        let name = format!("untagtags{index}");
        let created = create(&router, &name, create_body(Some(initial))).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );

        let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();

        let untagged = untag(&router, &pipe_arn(&name), &borrowed).await;
        assert_eq!(
            untagged.status, TAG_MUTATION_STATUS,
            "case {case:?}: UntagResource must answer {TAG_MUTATION_STATUS}, got {untagged:?}"
        );
        let expected = tag_map(remaining);
        assert_tags_agree(&router, case, &name, &expected).await;

        // `@idempotent`: every named key is already gone, so the second call changes
        // nothing.
        let again = untag(&router, &pipe_arn(&name), &borrowed).await;
        assert_eq!(
            again.status, TAG_MUTATION_STATUS,
            "case {case:?}: a repeated UntagResource must answer {TAG_MUTATION_STATUS}, got \
             {again:?}"
        );
        assert_tags_agree(&router, &format!("{case} (repeated)"), &name, &expected).await;
    }
}

/// RULE: a tagging call addresses the pipe whose ARN is its `resourceArn`; when that ARN is
/// of this vendor but this account holds no such pipe — it was never created, it has been
/// deleted, or the ARN names another region or another account — every one of the three
/// operations answers 404 `NotFoundException`. (An ARN of ANOTHER vendor is a different
/// refusal, 400 `BadRequestException`, pinned in `tests/all/tagging_refusals.rs`.)
#[tokio::test]
async fn tagging_an_arn_no_pipe_carries_is_not_found() {
    let router = router();
    // One live pipe, so the 404s below are about the ARN and not an empty store.
    let live = "tagnotfoundlive";
    let created = create(&router, live, create_body(Some(&json!({"env": "prod"})))).await;
    assert_eq!(
        created.status, 200,
        "the live pipe must be created, got {created:?}"
    );

    // A pipe that existed and was deleted: its ARN stops naming anything.
    let deleted = "tagnotfounddeleted";
    let created = create(&router, deleted, create_body(Some(&json!({"env": "prod"})))).await;
    assert_eq!(
        created.status, 200,
        "the pipe to delete must be created, got {created:?}"
    );
    let removed = delete_pipe(&router, deleted).await;
    assert_eq!(
        removed.status, 200,
        "DeletePipe must succeed, got {removed:?}"
    );

    let other_account = format!("arn:aws:pipes:{REGION}:210987654321:pipe/{live}");
    let rows: Vec<(&str, String)> = vec![
        ("never-created", pipe_arn("tagnotfoundmissing")),
        ("deleted-pipe", pipe_arn(deleted)),
        (
            "another-region",
            format!(
                "arn:aws:pipes:eu-central-1:{}:pipe/{live}",
                engine::EMULATOR_ACCOUNT_ID
            ),
        ),
        ("another-account", other_account),
    ];

    for (case, resource_arn) in rows {
        let listed = list_tags(&router, &resource_arn).await;
        assert_eq!(
            (listed.status, listed.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: ListTagsForResource on {resource_arn} must be a 404 \
             NotFoundException, got {listed:?}"
        );

        let tagged = tag(&router, &resource_arn, &json!({"env": "prod"})).await;
        assert_eq!(
            (tagged.status, tagged.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: TagResource on {resource_arn} must be a 404 NotFoundException, \
             got {tagged:?}"
        );

        let untagged = untag(&router, &resource_arn, &["env"]).await;
        assert_eq!(
            (untagged.status, untagged.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: UntagResource on {resource_arn} must be a 404 NotFoundException, \
             got {untagged:?}"
        );
    }

    // The refused calls wrote nothing: the live pipe still carries what it was created
    // with, and the deleted one is gone from DescribePipe too.
    assert_tags_agree(
        &router,
        "live-pipe-after-the-404s",
        live,
        &tag_map(&json!({"env": "prod"})),
    )
    .await;
    let described = describe(&router, deleted).await;
    assert_eq!(
        described.status, 404,
        "the deleted pipe must stay deleted, got {described:?}"
    );
}

/// RULE: a pipe carries at most 50 tags (the `TagResource` doc and `TagMap`'s
/// `@length(max: 50)`), and the bound is on the tags the pipe ENDS UP with — so a request
/// within `TagMap`'s own bound is still refused when the merge would overflow it, and a
/// refused call leaves the stored tags untouched.
///
/// The rows sit at the bound and one past it, from below (49 + 1) and from on it
/// (50 + a replacement, 50 + a new key).
#[tokio::test]
async fn a_pipe_carries_at_most_the_published_tag_maximum() {
    let router = router();

    // At the bound from below: 49 stored + 1 new = 50.
    let below = "tagcapbelow";
    let initial = generated_tags(MAX_TAGS - 1);
    let created = create(&router, below, create_body(Some(&initial))).await;
    assert_eq!(
        created.status, 200,
        "case \"49-plus-1\": CreatePipe must succeed, got {created:?}"
    );
    let tagged = tag(&router, &pipe_arn(below), &json!({"extra": "one"})).await;
    assert_eq!(
        tagged.status, TAG_MUTATION_STATUS,
        "case \"49-plus-1\": a merge landing exactly on the {MAX_TAGS}-tag bound must \
         succeed, got {tagged:?}"
    );
    let mut expected = tag_map(&initial);
    expected.insert("extra".to_string(), "one".to_string());
    assert_tags_agree(&router, "49-plus-1", below, &expected).await;

    // On the bound: replacing a stored key's value keeps the count at 50, so it is fine.
    let replaced = tag(&router, &pipe_arn(below), &json!({"key0": "replaced"})).await;
    assert_eq!(
        replaced.status, TAG_MUTATION_STATUS,
        "case \"50-replace-existing\": replacing a value does not grow the map, got \
         {replaced:?}"
    );
    expected.insert("key0".to_string(), "replaced".to_string());
    assert_tags_agree(&router, "50-replace-existing", below, &expected).await;

    // One past the bound: a 51st key is refused, and nothing is written.
    let overflow = tag(&router, &pipe_arn(below), &json!({"onetoomany": "x"})).await;
    assert_eq!(
        (overflow.status, overflow.error_code.as_str()),
        (400, "ValidationException"),
        "case \"50-plus-1\": a merge past the {MAX_TAGS}-tag bound must be a 400 \
         ValidationException, got {overflow:?}"
    );
    assert_tags_agree(&router, "50-plus-1 (unchanged)", below, &expected).await;

    // One past the bound inside a SINGLE request: `TagMap` itself is `@length(max: 50)`,
    // so the boundary validator refuses 51 tags before the handler runs.
    let past = "tagcappast";
    let created = create(&router, past, create_body(None)).await;
    assert_eq!(
        created.status, 200,
        "case \"51-in-one-call\": CreatePipe must succeed, got {created:?}"
    );
    let too_many = tag(&router, &pipe_arn(past), &generated_tags(MAX_TAGS + 1)).await;
    assert_eq!(
        (too_many.status, too_many.error_code.as_str()),
        (400, "ValidationException"),
        "case \"51-in-one-call\": {} tags in one TagResource must be a 400 \
         ValidationException, got {too_many:?}",
        MAX_TAGS + 1
    );
    assert_tags_agree(&router, "51-in-one-call (unchanged)", past, &BTreeMap::new()).await;
}
