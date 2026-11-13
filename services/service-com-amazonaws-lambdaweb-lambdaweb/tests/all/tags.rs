// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a web function's tags: `TagResource`, `UntagResource` and
//! `ListTags`, plus the tags a create applies.
//!
//! Every assertion goes over the wire through the same [`Emulator`] harness the
//! `web_function_lifecycle` suite uses: a request is built, dispatched through the
//! production router, and only the HTTP status, the `x-amzn-errortype` header and
//! the JSON body are inspected. Nothing reaches into the service's state struct, so
//! a tag map that is not computed from persisted state cannot pass.
//!
//! The tests are tables: each row is one input along the axis the rule under test
//! generalises over (how many keys, which characters a key or value is built from,
//! how long it is, whether the key is already stored, how a key misses, which
//! operation carries it), and the single new key is only one row.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::web_function_lifecycle::{build_config, percent_encoded, role_arn, Emulator, Reply};

/// The region every test signs with: the Region the function, and therefore its
/// ARN, is created in.
const REGION: &str = "us-east-1";

/// The longest tag key the model accepts: `TagKey` is `@length(1..128)` (source:
/// `smithy_query(query="shape", shape="TagKey")`).
const TAG_KEY_MAX_CHARS: usize = 128;

/// The longest tag value the model accepts: `Tags`' value target is
/// `@length(0..256)` (source: `smithy_query(query="shape", shape="Tags")`).
const TAG_VALUE_MAX_CHARS: usize = 256;

/// The most tags one web function holds: `Tags` is `@length(0..200)`, and
/// `ListTagsResponse$tags` targets it, so a function cannot hold more tags than
/// its own reader can report (source: `smithy_query(query="shape", shape="Tags")`).
const MAX_TAGS_PER_FUNCTION: usize = 200;

/// The whole message every tag path refuses a key set it may not apply with: ONE
/// fixed sentence covering the rules it lists, with no part taken from the
/// request — written out here rather than taken from the handler's helper, so the
/// test pins the wording.
const INVALID_TAGS_MESSAGE: &str = "One or more of your tags are invalid. Please ensure none of \
     your tag keys start with \"aws:\", none are empty, and there are no duplicate tag keys.";

/// A tag map as the JSON body `TagResource` carries (`tags` is `jsonName: "Tags"`).
fn tags_body(tags: &BTreeMap<String, String>) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in tags {
        map.insert(key.clone(), json!(value));
    }
    json!({ "Tags": Value::Object(map) })
}

/// `POST /tags/{resource}` with `tags`.
async fn tag_resource(emulator: &Emulator, resource: &str, tags: &BTreeMap<String, String>) -> Reply {
    emulator
        .send(
            "POST",
            &format!("/tags/{}", percent_encoded(resource)),
            REGION,
            Some(tags_body(tags)),
        )
        .await
}

/// `DELETE /tags/{resource}?tagKeys=...` — one `tagKeys` parameter per key, the way
/// an `@httpQuery` list travels.
async fn untag_resource(emulator: &Emulator, resource: &str, keys: &[String]) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encoded(key)))
        .collect::<Vec<_>>()
        .join("&");
    emulator
        .send(
            "DELETE",
            &format!("/tags/{}?{query}", percent_encoded(resource)),
            REGION,
            None,
        )
        .await
}

/// `GET /tags/{resource}`.
async fn list_tags(emulator: &Emulator, resource: &str) -> Reply {
    emulator
        .send(
            "GET",
            &format!("/tags/{}", percent_encoded(resource)),
            REGION,
            None,
        )
        .await
}

/// The tag map a reader reported, as an ordered map so two answers compare
/// regardless of the order the protocol rendered them in.
fn reported_tags(case: &str, reply: &Reply) -> BTreeMap<String, String> {
    assert_eq!(
        reply.status, 200,
        "case {case}: ListTags expected 200, got {} with {:?}",
        reply.status, reply.body
    );
    let object = reply
        .field("Tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case}: expected a Tags map, got {:?}", reply.body));
    object
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A tag map from `(key, value)` pairs.
fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Create a web function with one revision and the supplied tags, returning its
/// `functionArn` — the `resource` the tag operations address it by.
async fn create_function(
    emulator: &Emulator,
    name: &str,
    tags: Option<&BTreeMap<String, String>>,
) -> String {
    let mut body = json!({
        "functionName": name,
        "revisionConfig": {
            "buildConfig": build_config("python3.13", "demo-bucket", "app.zip", None),
            "serviceConfig": { "executionRoleArn": role_arn(name) },
        },
    });
    if let Some(tags) = tags {
        body["tags"] = tags_body(tags)["Tags"].clone();
    }
    let created = emulator.send("PUT", "/web-functions", REGION, Some(body)).await;
    assert_eq!(
        created.status, 202,
        "case {name:?}: CreateWebFunction expected 202, got {} with {:?}",
        created.status, created.body
    );
    created
        .string("functionArn")
        .unwrap_or_else(|| panic!("case {name:?}: the create reports a functionArn"))
}

/// One row of the merge table: what makes it different, and the tags it applies.
struct MergeCase {
    case: &'static str,
    tags: BTreeMap<String, String>,
}

/// The tag maps a `TagResource` call must accept, spanning the axes the merge rule
/// generalises over: how many keys, whether a key is already stored, the value's
/// emptiness and length, and the characters a key and a value are built from.
fn merge_cases() -> Vec<MergeCase> {
    // Every character class the model's `TagKey` pattern admits —
    // `^[a-zA-Z0-9 _\.:/=+\-@]+$` — and the value pattern the same set plus the
    // empty string (source: `smithy_query(query="shape", shape="Tags")`).
    let every_class = "Az09 _.:/=+-@";
    vec![
        MergeCase {
            case: "one new key",
            tags: map(&[("team", "payments")]),
        },
        MergeCase {
            case: "a key that is already stored, with a different value",
            tags: map(&[("env", "staging")]),
        },
        MergeCase {
            case: "the same key again with the value it already has",
            tags: map(&[("env", "staging")]),
        },
        MergeCase {
            case: "an empty value",
            tags: map(&[("cost-center", "")]),
        },
        MergeCase {
            case: "every character class the key and value patterns admit",
            tags: map(&[(every_class, every_class)]),
        },
        MergeCase {
            case: "the longest key and the longest value the model allows",
            tags: map(&[(
                &"k".repeat(TAG_KEY_MAX_CHARS),
                &"v".repeat(TAG_VALUE_MAX_CHARS),
            )]),
        },
        MergeCase {
            case: "eight keys at once, one of them already stored",
            tags: (0..7)
                .map(|index| (format!("batch-{index}"), format!("value-{index}")))
                .chain([("owner".to_string(), "platform".to_string())])
                .collect(),
        },
        MergeCase {
            case: "an empty tag map, which the model admits (Tags length 0..200)",
            tags: BTreeMap::new(),
        },
    ]
}

#[tokio::test]
async fn tag_resource_merges_the_keys_it_carries_and_leaves_every_other_key_alone() {
    // RULE: `TagResource` MERGES: a key the request names takes the value it
    // carries, a key already stored under that name is overwritten, and every
    // stored key the request does not name is left exactly as it was — and
    // `ListTags` reports that one stored map.
    let emulator = Emulator::new();
    let initial = map(&[("env", "prod"), ("owner", "team-a")]);
    let arn = create_function(&emulator, "tag-merge", Some(&initial)).await;

    // The tags a create applied are the map the reader starts from.
    assert_eq!(
        reported_tags("the tags the create applied", &list_tags(&emulator, &arn).await),
        initial,
        "case the tags the create applied: ListTags reports the create's tags"
    );

    let mut expected = initial;
    for MergeCase { case, tags } in merge_cases() {
        let tagged = tag_resource(&emulator, &arn, &tags).await;
        assert_eq!(
            tagged.status, 204,
            "case {case}: TagResource expected 204, got {} with {:?}",
            tagged.status, tagged.body
        );
        expected.extend(tags.clone());

        // Read it back, twice: a read reports the stored map and moves nothing.
        for round in 1..=2 {
            let got = reported_tags(case, &list_tags(&emulator, &arn).await);
            assert_eq!(
                got, expected,
                "case {case} read {round}: expected the merged map {expected:?}, got {got:?}"
            );
        }
    }
}

#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys_and_ignores_the_rest() {
    // RULE: `UntagResource` removes exactly the keys it names and passes over any
    // key the function does not carry — the operation is `@idempotent`, so naming a
    // key that was never set, or repeating a call, leaves the same stored map. Tag
    // keys are compared verbatim, so a key differing only in case names a
    // different key.
    let emulator = Emulator::new();
    let stored = map(&[
        ("env", "prod"),
        ("owner", "team-a"),
        ("cost-center", ""),
        ("Az09 _.:/=+-@", "every-class"),
    ]);
    let arn = create_function(&emulator, "tag-removal", Some(&stored)).await;

    // Each row: what makes it different, and the keys it asks to remove.
    let cases: Vec<(&str, Vec<String>)> = vec![
        ("a key that was never set", vec!["never-set".to_string()]),
        (
            "a key differing from a stored one only in case",
            vec!["ENV".to_string()],
        ),
        (
            "a key that is stored, named on its own",
            vec!["owner".to_string()],
        ),
        (
            "the same stored key again, after it is gone",
            vec!["owner".to_string()],
        ),
        (
            "a stored key and a key that was never set together",
            vec!["cost-center".to_string(), "also-never-set".to_string()],
        ),
        (
            "a key built from every character class the pattern admits",
            vec!["Az09 _.:/=+-@".to_string()],
        ),
        (
            "every key the function still carries, plus one it never had",
            vec!["env".to_string(), "gone".to_string()],
        ),
        (
            "every key again, when the function carries none at all",
            vec!["env".to_string(), "owner".to_string()],
        ),
    ];

    let mut expected = stored;
    for (case, keys) in cases {
        for round in 1..=2 {
            let untagged = untag_resource(&emulator, &arn, &keys).await;
            assert_eq!(
                untagged.status, 204,
                "case {case:?} call {round}: UntagResource expected 204, got {} with {:?}",
                untagged.status, untagged.body
            );
            for key in &keys {
                expected.remove(key);
            }
            let got = reported_tags(case, &list_tags(&emulator, &arn).await);
            assert_eq!(
                got, expected,
                "case {case:?} call {round}: expected {expected:?} to be left, got {got:?}"
            );
        }
    }
    assert!(
        expected.is_empty(),
        "case the end of the removal table: every key was asked for, got {expected:?}"
    );
}

#[tokio::test]
async fn a_key_in_the_reserved_namespace_is_refused_and_changes_no_tag() {
    // RULE: a tag key beginning with the reserved `aws:` prefix is not the caller's
    // to create, change or delete: every tag path refuses the whole request with a
    // 400 `ValidationException`, states the one fixed tag-validation sentence —
    // which carries no part of the request, so every row below reads identically —
    // and applies none of the keys it carried. The rows span how the reserved key
    // arrives — alone, mixed with valid keys, with different suffixes — and which
    // path carries it. Keys that merely start with the letters `aws` are NOT in that
    // namespace and are accepted.
    let emulator = Emulator::new();
    let stored = map(&[("env", "prod")]);
    let arn = create_function(&emulator, "tag-reserved", Some(&stored)).await;

    // The reserved keys, each a legal `TagKey` for the boundary validator (the
    // pattern `^[a-zA-Z0-9 _\.:/=+\-@]+$` admits `:`), so the handler is what
    // refuses them.
    let reserved: [&str; 4] = [
        "aws:",
        "aws:a",
        "aws:cloudformation:stack-name",
        "aws:created-by",
    ];

    for key in reserved {
        // Alone, and mixed with keys that are perfectly valid on their own.
        let payloads: Vec<(&str, BTreeMap<String, String>)> = vec![
            ("the reserved key alone", map(&[(key, "value")])),
            (
                "the reserved key among valid keys",
                map(&[("team", "payments"), (key, "value"), ("tier", "gold")]),
            ),
        ];
        for (shape, tags) in payloads {
            let refused = tag_resource(&emulator, &arn, &tags).await;
            assert_eq!(
                refused.status, 400,
                "case TagResource with {key:?} ({shape}): expected HTTP 400, got {} with {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error(),
                "ValidationException",
                "case TagResource with {key:?} ({shape}): expected ValidationException, got {:?}",
                refused.body
            );
            assert_eq!(
                refused.string("message").as_deref(),
                Some(INVALID_TAGS_MESSAGE),
                "case TagResource with {key:?} ({shape}): expected the tag-validation sentence, \
                 naming neither the key nor its member path, got {:?}",
                refused.body
            );
            let got = reported_tags("after the refused tag call", &list_tags(&emulator, &arn).await);
            assert_eq!(
                got, stored,
                "case TagResource with {key:?} ({shape}): a refused call applies no tags, \
                 got {got:?}"
            );
        }

        // The removal path refuses it the same way, and removes nothing.
        let refused = untag_resource(&emulator, &arn, &[key.to_string(), "env".to_string()]).await;
        assert_eq!(
            refused.status, 400,
            "case UntagResource with {key:?}: expected HTTP 400, got {} with {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error(),
            "ValidationException",
            "case UntagResource with {key:?}: expected ValidationException, got {:?}",
            refused.body
        );
        // The removal path states the same sentence: it names neither the key nor
        // the position it arrived at.
        assert_eq!(
            refused.string("message").as_deref(),
            Some(INVALID_TAGS_MESSAGE),
            "case UntagResource with {key:?}: expected the tag-validation sentence, got {:?}",
            refused.body
        );
        let got = reported_tags("after the refused untag call", &list_tags(&emulator, &arn).await);
        assert_eq!(
            got, stored,
            "case UntagResource with {key:?}: a refused call removes no tags, got {got:?}"
        );

        // And so does the create path, which creates nothing at all.
        let create = emulator
            .send(
                "PUT",
                "/web-functions",
                REGION,
                Some(json!({
                    "functionName": "tag-reserved-create",
                    "tags": { key: "value" },
                })),
            )
            .await;
        assert_eq!(
            create.status, 400,
            "case CreateWebFunction with {key:?}: expected HTTP 400, got {} with {:?}",
            create.status, create.body
        );
        assert_eq!(
            create.string("message").as_deref(),
            Some(INVALID_TAGS_MESSAGE),
            "case CreateWebFunction with {key:?}: expected the same tag-validation sentence as \
             the tagging path, got {:?}",
            create.body
        );
        let absent = list_tags(
            &emulator,
            &arn.replace("tag-reserved", "tag-reserved-create"),
        )
        .await;
        assert_eq!(
            absent.status, 404,
            "case CreateWebFunction with {key:?}: a refused create creates nothing, got {} \
             with {:?}",
            absent.status, absent.body
        );
    }

    // A key that is not in the reserved namespace — it only starts with the same
    // letters, or has the prefix somewhere other than the start — is accepted.
    let accepted = map(&[
        ("aws", "no-colon"),
        ("awsome", "not-the-prefix"),
        ("my-aws:key", "the-prefix-is-not-at-the-start"),
    ]);
    let tagged = tag_resource(&emulator, &arn, &accepted).await;
    assert_eq!(
        tagged.status, 204,
        "case keys that only resemble the reserved prefix: expected 204, got {} with {:?}",
        tagged.status, tagged.body
    );
    let mut expected = stored;
    expected.extend(accepted);
    let got = reported_tags("keys that only resemble the reserved prefix", &list_tags(&emulator, &arn).await);
    assert_eq!(
        got, expected,
        "case keys that only resemble the reserved prefix: expected {expected:?}, got {got:?}"
    );
}

#[tokio::test]
async fn a_resource_that_names_no_stored_function_is_not_found_on_every_tag_path() {
    // RULE: the tag operations address a web function by the `web-function/<name>`
    // segment of the ARN they carry — the same resolution every other reader in
    // this crate applies — so an ARN whose segment names no stored function is the
    // modelled not-found, on every one of them. The rows differ along how that name
    // misses.
    let emulator = Emulator::new();
    let held = create_function(&emulator, "tag-arn-resolution", None).await;

    let absent: Vec<(&str, String)> = vec![
        (
            "a function that was never created",
            held.replace("tag-arn-resolution", "never-created"),
        ),
        (
            "a name that differs only in case",
            held.replace("tag-arn-resolution", "Tag-Arn-Resolution"),
        ),
        (
            "a name that extends the stored one",
            held.replace("tag-arn-resolution", "tag-arn-resolution-2"),
        ),
        (
            "a name that is a prefix of the stored one",
            held.replace("tag-arn-resolution", "tag-arn"),
        ),
    ];

    let applied = map(&[("env", "prod")]);
    for (case, arn) in &absent {
        for (operation, reply) in [
            ("TagResource", tag_resource(&emulator, arn, &applied).await),
            (
                "UntagResource",
                untag_resource(&emulator, arn, &["env".to_string()]).await,
            ),
            ("ListTags", list_tags(&emulator, arn).await),
        ] {
            assert_eq!(
                reply.status, 404,
                "case {case:?} ({operation} on {arn}): expected HTTP 404, got {} with {:?}",
                reply.status, reply.body
            );
            assert_eq!(
                reply.error(),
                "ResourceNotFoundException",
                "case {case:?} ({operation} on {arn}): expected ResourceNotFoundException, \
                 got {:?}",
                reply.body
            );
            assert_eq!(
                reply
                    .field("resourceType")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                "WebFunction",
                "case {case:?} ({operation} on {arn}): the refusal names the resource that is \
                 missing, got {:?}",
                reply.body
            );
        }
    }

    // The function that WAS created exists and carries no tags, which is a
    // different answer from not existing: an EMPTY map (`Tags` is length 0..200).
    assert_eq!(
        reported_tags("a function carrying no tags", &list_tags(&emulator, &held).await),
        BTreeMap::new(),
        "case a function carrying no tags: expected an empty map"
    );
}

#[tokio::test]
async fn a_functions_delete_takes_its_tags_with_it() {
    // RULE: tags live in the function's own record, so deleting the function
    // removes them with it — a function created again under the same name carries
    // none.
    let emulator = Emulator::new();
    let arn = create_function(&emulator, "tag-cascade", Some(&map(&[("env", "prod")]))).await;
    let tagged = tag_resource(&emulator, &arn, &map(&[("team", "payments")])).await;
    assert_eq!(
        tagged.status, 204,
        "case the tags to cascade: TagResource expected 204, got {} with {:?}",
        tagged.status, tagged.body
    );

    let deleted = emulator
        .send(
            "DELETE",
            &format!("/web-functions/{}", percent_encoded(&arn)),
            REGION,
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 204,
        "case the function's delete: expected 204, got {} with {:?}",
        deleted.status, deleted.body
    );

    let recreated = create_function(&emulator, "tag-cascade", None).await;
    assert_eq!(
        recreated, arn,
        "case the function created again: expected the same ARN, got {recreated:?}"
    );
    assert_eq!(
        reported_tags("the function created again", &list_tags(&emulator, &arn).await),
        BTreeMap::new(),
        "case the function created again: a function's delete takes its tags with it"
    );
}

#[tokio::test]
async fn a_merge_that_would_cross_the_tag_bound_is_refused_and_applies_no_tags() {
    // RULE: a function holds at most as many tags as `Tags` admits, because its own
    // reader reports that shape; a merge that would cross the bound is refused and
    // applies nothing, while a merge that only overwrites keys already stored stays
    // inside it. The rows are exactly the bound and one key past it.
    let emulator = Emulator::new();
    let arn = create_function(&emulator, "tag-bound", None).await;

    let full: BTreeMap<String, String> = (0..MAX_TAGS_PER_FUNCTION)
        .map(|index| (format!("key-{index:03}"), format!("value-{index:03}")))
        .collect();
    let filled = tag_resource(&emulator, &arn, &full).await;
    assert_eq!(
        filled.status, 204,
        "case exactly the bound ({MAX_TAGS_PER_FUNCTION} tags): expected 204, got {} with {:?}",
        filled.status, filled.body
    );
    assert_eq!(
        reported_tags("exactly the bound", &list_tags(&emulator, &arn).await),
        full,
        "case exactly the bound ({MAX_TAGS_PER_FUNCTION} tags): the whole map is stored"
    );

    // Overwriting keys that are already stored adds none, so it stays inside the
    // bound.
    let overwrite: BTreeMap<String, String> = full
        .keys()
        .take(5)
        .map(|key| (key.clone(), "overwritten".to_string()))
        .collect();
    let again = tag_resource(&emulator, &arn, &overwrite).await;
    assert_eq!(
        again.status, 204,
        "case overwriting stored keys at the bound: expected 204, got {} with {:?}",
        again.status, again.body
    );
    let mut expected = full.clone();
    expected.extend(overwrite);
    assert_eq!(
        reported_tags("overwriting stored keys at the bound", &list_tags(&emulator, &arn).await),
        expected,
        "case overwriting stored keys at the bound: the values moved and the count did not"
    );

    // One key past the bound is refused, and the stored map is untouched.
    let refused = tag_resource(&emulator, &arn, &map(&[("one-too-many", "value")])).await;
    assert_eq!(
        refused.status, 402,
        "case one key past the bound: expected HTTP 402, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error(),
        "ServiceQuotaExceededException",
        "case one key past the bound: expected ServiceQuotaExceededException, got {:?}",
        refused.body
    );
    assert_eq!(
        reported_tags("after the refused merge", &list_tags(&emulator, &arn).await),
        expected,
        "case one key past the bound: a refused merge applies no tags"
    );
}
