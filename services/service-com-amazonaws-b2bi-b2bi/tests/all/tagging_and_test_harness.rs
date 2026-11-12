// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `TagResource`, the transformer `clientToken` contract and
//! `TestConversion`, driven over the wire (awsJson1_0, `x-amz-target: B2BI.<Operation>`)
//! through the same router the emulator boots, so every assertion is about what a real
//! SDK client observes.
//!
//! Each test states a RULE and asserts it over a table of inputs rather than one
//! recorded request: the rows differ along whatever the rule generalises over (the
//! resource kind an ARN addresses, the way an ARN fails to resolve, the document
//! format, the member a retried request differs in).
//!
//! The read side of tagging (`ListTagsForResource` reporting what a create stored, and
//! its ARN classification) is covered by `tagging.rs`; the transformer create/read
//! round-trip and the member-style exclusivity by `transformer_lifecycle.rs`; and
//! `TestMapping` by `test_mapping.rs`. This suite covers what those do not: the tag
//! WRITE path, the conflict a reused `clientToken` produces, and the `TestConversion`
//! harness.
//!
//! The wire harness (`call`, `error_code`, `b2bi_router`, the `given_*` builders) is the
//! one `onboarding_loop` already drives the emulator through.

use serde_json::{json, Value};
use service_com_amazonaws_b2bi_b2bi::state::{invalid_arn, INVALID_INPUT_ARN};
use smithy_server_runtime::MultiServiceRouter;

use crate::onboarding_loop::{
    b2bi_router, call, error_code, given_capability, given_partnership, given_profile,
    is_generated_id, text, ACCOUNT, SIGNING_REGION,
};

/// A well-formed b2bi ARN for `kind`/`id` in the region the tests sign for.
fn b2bi_arn(kind: &str, id: &str) -> String {
    format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:{kind}/{id}")
}

/// The `Tags` a response reports, as `(key, value)` pairs in report order.
fn tag_pairs(body: &Value) -> Vec<(String, String)> {
    body["Tags"]
        .as_array()
        .map(|tags| {
            tags.iter()
                .map(|tag| (text(tag, "Key"), text(tag, "Value")))
                .collect()
        })
        .unwrap_or_default()
}

/// A `Tags` / `tags` request member from `(key, value)` pairs. Member names are
/// `com.amazonaws.b2bi#Tag`'s own: `Key` and `Value`.
fn tags_json(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// Attach `tags` to `arn`.
async fn tag_resource(router: &MultiServiceRouter, arn: &str, tags: Value) -> (u16, Value) {
    call(
        router,
        "TagResource",
        json!({ "ResourceARN": arn, "Tags": tags }),
    )
    .await
}

/// Read the tags stored on `arn`.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> (u16, Value) {
    call(router, "ListTagsForResource", json!({ "ResourceARN": arn })).await
}

/// Create a transformer and return `(transformerId, transformerArn)`.
async fn given_transformer(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let (status, body) = call(
        router,
        "CreateTransformer",
        json!({ "name": name, "inputConversion": { "fromFormat": "X12" } }),
    )
    .await;
    assert_eq!(status, 200, "CreateTransformer({name}) failed: {body}");
    (text(&body, "transformerId"), text(&body, "transformerArn"))
}

/// One live resource of every kind this service can tag: `(kind, ARN)`.
async fn given_one_of_every_kind(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    let profile_id = given_profile(router, "tag-write-profile").await;
    let capability_id = given_capability(router, "tag-write-capability").await;
    let partnership_id =
        given_partnership(router, &profile_id, &capability_id, "tag-write-partnership").await;
    let (transformer_id, _) = given_transformer(router, "tag-write-transformer").await;
    vec![
        ("profile", b2bi_arn("profile", &profile_id)),
        ("capability", b2bi_arn("capability", &capability_id)),
        ("partnership", b2bi_arn("partnership", &partnership_id)),
        ("transformer", b2bi_arn("transformer", &transformer_id)),
    ]
}

/// RULE: `TagResource` stores its tags on the resource its `ResourceARN` names —
/// replacing the value of a key the resource already carries, keeping every other tag,
/// and appending the keys that are new — and `ListTagsForResource` then reports exactly
/// that stored set, for every taggable resource type.
///
/// The rows span all four resource kinds a b2bi ARN addresses; each kind is tagged
/// twice, the second call overlapping one key and adding another, so the update half and
/// the keep-the-rest half of the rule are both asserted.
#[tokio::test]
async fn tags_written_by_tag_resource_are_merged_onto_the_resource_and_read_back() {
    let router = b2bi_router();

    for (kind, arn) in given_one_of_every_kind(&router).await {
        let case = format!("{kind} first tagging");
        let (status, body) = tag_resource(&router, &arn, tags_json(&[("env", "test"), ("team", "edi")])).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            tag_pairs(&body),
            vec![
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "edi".to_string())
            ],
            "case {case:?}: expected the two stored tags, got {body}"
        );

        let case = format!("{kind} second tagging");
        let (status, body) = tag_resource(&router, &arn, tags_json(&[("env", "prod"), ("owner", "ops")])).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");

        let (status, body) = list_tags(&router, &arn).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            tag_pairs(&body),
            vec![
                ("env".to_string(), "prod".to_string()),
                ("team".to_string(), "edi".to_string()),
                ("owner".to_string(), "ops".to_string())
            ],
            "case {case:?}: expected env updated, team kept and owner added, got {body}"
        );
    }
}

/// RULE: `TagResource` stores nothing for a `ResourceARN` it cannot resolve to a live
/// resource of this service, and refuses it with the modelled `ValidationException`
/// (HTTP 400) — the ARN is the parameter it validates before it writes. The reporting
/// `ListTagsForResource` answers the SAME refusal for the same ARNs, worded by the step
/// that failed: the rejected value itself when it is not an ARN at all, and the one fixed
/// sentence when it parses and addresses no resource of this account.
///
/// The rows span both ways an ARN fails: strings that fail the ARN grammar (no colons, a
/// truncated ARN, an empty partition / service / resource field, the prefix mis-cased)
/// and well-formed ARNs that resolve to nothing (an id that never existed, a DELETED
/// profile's own ARN, a live id under another resource type, a live resource's ARN with
/// the region or the account rewritten, another service's ARN, a resource segment with
/// no id).
#[tokio::test]
async fn an_arn_tag_resource_cannot_resolve_is_refused_and_stores_nothing() {
    let router = b2bi_router();

    let live = given_profile(&router, "live-profile").await;
    let live_arn = b2bi_arn("profile", &live);
    let doomed = given_profile(&router, "doomed-profile").await;
    let doomed_arn = b2bi_arn("profile", &doomed);
    let (status, body) = call(&router, "DeleteProfile", json!({ "profileId": doomed })).await;
    assert_eq!(status, 200, "DeleteProfile failed: {status} {body}");

    // (case, the ResourceARN, does it satisfy the ARN grammar at all?) — the class is the
    // same either way and the sentence is not, so the flag carries which one to expect.
    let cases: &[(&str, String, bool)] = &[
        ("no colons at all", "this-is-not-an-arn-at-all".to_string(), false),
        ("truncated ARN", format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}"), false),
        (
            "empty partition field",
            format!("arn::b2bi:{SIGNING_REGION}:{ACCOUNT}:profile/{live}"),
            false,
        ),
        (
            "empty service field",
            format!("arn:aws::{SIGNING_REGION}:{ACCOUNT}:profile/{live}"),
            false,
        ),
        (
            "empty resource field",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:"),
            false,
        ),
        ("prefix upper-cased", live_arn.replacen("arn:", "ARN:", 1), false),
        ("an id that never existed", b2bi_arn("profile", "p-00000000000000000"), true),
        ("a deleted profile's own ARN", doomed_arn.clone(), true),
        ("a live id under another resource type", b2bi_arn("transformer", &live), true),
        (
            "a live ARN with the region rewritten",
            live_arn.replacen(SIGNING_REGION, "eu-central-1", 1),
            true,
        ),
        (
            "a live ARN with the account rewritten",
            live_arn.replacen(ACCOUNT, "123456789012", 1),
            true,
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:{SIGNING_REGION}:{ACCOUNT}:bucket/not-a-b2bi-resource"),
            true,
        ),
        (
            "a resource segment with no id",
            format!("arn:aws:b2bi:{SIGNING_REGION}:{ACCOUNT}:profile/"),
            true,
        ),
    ];

    for (case, arn, parses) in cases {
        let expected_message = if *parses {
            INVALID_INPUT_ARN.to_string()
        } else {
            invalid_arn(arn)
        };
        let (status, body) = tag_resource(&router, arn, tags_json(&[("env", "test")])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected TagResource to refuse with 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            text(&body, "Message"),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {body}"
        );

        // The same ARN through the reporting operation: the same refusal, because the same
        // two validation steps decide it.
        let (status, body) = list_tags(&router, arn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected ListTagsForResource to answer 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        assert_eq!(
            text(&body, "Message"),
            expected_message,
            "case {case:?}: expected {expected_message:?}, got {body}"
        );
    }

    // No refused call reached the store: the live profile the stale ARNs resemble still
    // carries no tag at all.
    let (status, body) = list_tags(&router, &live_arn).await;
    assert_eq!(status, 200, "the live profile is still readable: {body}");
    assert!(
        tag_pairs(&body).is_empty(),
        "no refused TagResource stored a tag, got {body}"
    );
}

/// RULE: a tag write that cannot be stored is refused with the modelled
/// `ValidationException` (400) and leaves the stored set exactly as it was: a resource
/// holds at most as many tags as the modelled `TagList` bound allows a response to
/// report (200), and a key under the prefix AWS reserves for its own tags cannot be
/// claimed by a caller.
///
/// The rows walk the bound itself: a full set at the cap, an update of a key already
/// held AT the cap (still allowed, since it adds nothing), one new key past it, and a
/// reserved key both on its own and alongside an acceptable one.
#[tokio::test]
async fn a_tag_write_that_cannot_be_stored_is_refused_and_changes_nothing() {
    let router = b2bi_router();
    let profile = given_profile(&router, "tag-bound-profile").await;
    let arn = b2bi_arn("profile", &profile);

    // The modelled `TagList len 0..200`, which is also `MAX_TAGS_PER_RESOURCE`.
    let keys: Vec<String> = (0..200).map(|index| format!("key-{index}")).collect();
    let full: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "v")).collect();
    let (status, body) = tag_resource(&router, &arn, tags_json(&full)).await;
    assert_eq!(status, 200, "case at the cap: expected 200, got {status} {body}");

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(status, 200, "case at the cap: expected 200, got {status} {body}");
    assert_eq!(
        tag_pairs(&body).len(),
        200,
        "case at the cap: expected 200 stored tags, got {}",
        tag_pairs(&body).len()
    );

    // An update at the cap adds nothing, so it is still allowed.
    let (status, body) = tag_resource(&router, &arn, tags_json(&[("key-0", "updated")])).await;
    assert_eq!(
        status, 200,
        "case update at the cap: expected 200, got {status} {body}"
    );

    // (case, the tags the refused request carries)
    let refused: &[(&str, Value)] = &[
        ("one key past the cap", tags_json(&[("key-200", "v")])),
        (
            "a reserved key on its own",
            tags_json(&[("aws:created-by", "someone")]),
        ),
        (
            "a reserved key beside an acceptable one",
            tags_json(&[("key-0", "v"), ("aws:owner", "someone")]),
        ),
        (
            // The namespace is reserved, not the spelling.
            "a reserved key in another casing",
            tags_json(&[("AWS:Owner", "someone")]),
        ),
    ];
    for (case, tags) in refused {
        let (status, body) = tag_resource(&router, &arn, tags.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
    }

    // Nothing the refusals carried was stored: the set is still the 200 keys, with the
    // one update applied and no reserved key among them.
    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    let stored = tag_pairs(&body);
    assert_eq!(
        stored.len(),
        200,
        "expected the stored set to stay at 200 tags, got {}",
        stored.len()
    );
    assert!(
        stored
            .iter()
            .all(|(key, _)| !key.to_ascii_lowercase().starts_with("aws:")),
        "expected no reserved key to be stored, got {stored:?}"
    );
    assert!(
        stored
            .iter()
            .any(|(key, value)| key == "key-0" && value == "updated"),
        "expected the update at the cap to have been applied, got {stored:?}"
    );
}

/// RULE: a tag key is UTF-8 text, not ASCII (the modelled `TagKey` is `len 1..128` with no
/// pattern), so every key is ANSWERED on its own terms — stored when it is acceptable,
/// refused when it claims the reserved namespace — whatever its encoding, and a key whose
/// bytes straddle the reserved-prefix comparison is not a special case.
///
/// The rows span keys whose fourth byte (the last one the reserved comparison looks at) is
/// interior to a multi-byte character, a key that is entirely multi-byte, a non-Latin key,
/// and a reserved key with a multi-byte tail.
#[tokio::test]
async fn a_multi_byte_tag_key_is_answered_on_its_own_terms() {
    let router = b2bi_router();
    let profile = given_profile(&router, "multi-byte-tag-profile").await;
    let arn = b2bi_arn("profile", &profile);

    let acceptable = tags_json(&[
        ("ab\u{20ac}", "euro-at-byte-2"),
        ("a\u{20ac}bc", "euro-at-byte-1"),
        ("\u{20ac}", "euro-only"),
        ("\u{043a}\u{043b}\u{044e}\u{0447}", "cyrillic"),
    ]);
    let (status, body) = tag_resource(&router, &arn, acceptable).await;
    assert_eq!(
        status, 200,
        "case acceptable multi-byte keys: expected 200, got {status} {body}"
    );

    let (status, body) = list_tags(&router, &arn).await;
    assert_eq!(status, 200, "expected 200, got {status} {body}");
    assert_eq!(
        tag_pairs(&body),
        vec![
            ("ab\u{20ac}".to_string(), "euro-at-byte-2".to_string()),
            ("a\u{20ac}bc".to_string(), "euro-at-byte-1".to_string()),
            ("\u{20ac}".to_string(), "euro-only".to_string()),
            (
                "\u{043a}\u{043b}\u{044e}\u{0447}".to_string(),
                "cyrillic".to_string()
            ),
        ],
        "case acceptable multi-byte keys: expected all four stored, got {body}"
    );

    // The reserved namespace is still the reserved namespace when what follows it is not
    // ASCII.
    let (status, body) = tag_resource(&router, &arn, tags_json(&[("aws:\u{20ac}", "v")])).await;
    assert_eq!(
        status, 400,
        "case a reserved key with a multi-byte tail: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case a reserved key with a multi-byte tail: expected ValidationException, got {body}"
    );
}

/// RULE: a create refused for mixing the deprecated configuration members with the
/// members that replace them persists NOTHING — not even a record of the `clientToken`
/// it carried, so that token still mints a transformer afterwards.
///
/// The rows pair each of the four deprecated members with one of the members that
/// replace it, so the rule is asserted across the deprecated set rather than for one
/// recorded combination.
#[tokio::test]
async fn a_create_refused_for_mixing_member_styles_leaves_its_client_token_free() {
    let router = b2bi_router();

    // (case, the deprecated member and value, the replacement member and value)
    let cases: &[(&str, &str, Value, &str, Value)] = &[
        (
            "fileFormat with inputConversion",
            "fileFormat",
            json!("JSON"),
            "inputConversion",
            json!({ "fromFormat": "X12" }),
        ),
        (
            "mappingTemplate with mapping",
            "mappingTemplate",
            json!("$"),
            "mapping",
            json!({ "templateLanguage": "JSONATA", "template": "$" }),
        ),
        (
            "ediType with outputConversion",
            "ediType",
            json!({ "x12Details": { "transactionSet": "X12_850", "version": "VERSION_4010" } }),
            "outputConversion",
            json!({ "toFormat": "X12" }),
        ),
        (
            "sampleDocument with sampleDocuments",
            "sampleDocument",
            json!("s3://bucket/sample.edi"),
            "sampleDocuments",
            json!({ "bucketName": "samples", "keys": [{ "input": "in/one.json" }] }),
        ),
    ];

    for (index, (case, deprecated, deprecated_value, replacement, replacement_value)) in
        cases.iter().enumerate()
    {
        let token = format!("mixed-token-{index}");
        let mut mixed = json!({ "name": "mixed-styles", "clientToken": token });
        mixed[*deprecated] = deprecated_value.clone();
        mixed[*replacement] = replacement_value.clone();

        let (status, body) = call(&router, "CreateTransformer", mixed).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );

        let mut accepted = json!({ "name": "after-refusal", "clientToken": token });
        accepted[*replacement] = replacement_value.clone();
        let (status, body) = call(&router, "CreateTransformer", accepted).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the token to be free after the refusal, got {status} {body}"
        );
        assert!(
            is_generated_id(&text(&body, "transformerId"), "t"),
            "case {case:?}: expected a generated t- id, got {body}"
        );
    }
}

/// RULE: `clientToken` is an `@idempotencyToken`, so one token names ONE request: however
/// many calls race under one token exactly one transformer is created — a racer either
/// wins the claim, replays what the winner created, or is refused with the modelled
/// `ConflictException` (HTTP 409), never minting a second one — and once the request
/// holding the token has answered, that token and the same parameters answer with that one
/// transformer (a reordered tag list is the same request). A call carrying the token while
/// asking for something DIFFERENT is `ConflictException` too, rather than a second
/// transformer.
///
/// The two conflict arms are the two the corpus records for this operation (trace
/// `b2bi.create-transformer-idempotent-token-concurrent-dedup`: "N concurrent calls
/// sharing one clientToken and identical parameters yield exactly ONE transformer and the
/// racers that lose the in-flight race are refused with ConflictException; only a REPLAY
/// issued after the original request completed returns the original transformerId"), and
/// the conflict rows below span a difference in each kind of member a create carries: the
/// name, a configuration structure, a tag value, and a member the first request did not
/// name at all.
///
/// The racers are SPAWNED on a MULTI-THREADED runtime, so they genuinely contend for the
/// store instead of being polled one after another on one task; the refusal a racer that
/// loses gets is asserted on its own, against a held claim, by
/// `tests/all/idempotent_create_in_flight_window.rs`, since which racer loses is up to the
/// scheduler and must not be up to the clock.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_client_token_yields_one_transformer_however_many_calls_race() {
    use std::sync::Arc;

    let router = Arc::new(b2bi_router());

    let request = json!({
        "name": "idempotent-transformer",
        "clientToken": "shared-token",
        "inputConversion": { "fromFormat": "X12" },
        "tags": tags_json(&[("env", "test"), ("team", "edi")])
    });

    // Five racers carrying one token and identical parameters, in flight together.
    let mut running = Vec::new();
    for _ in 0..5 {
        let router = Arc::clone(&router);
        let request = request.clone();
        running.push(tokio::spawn(async move {
            call(&router, "CreateTransformer", request).await
        }));
    }
    let mut racers = Vec::new();
    for racer in running {
        racers.push(racer.await.expect("the racer task runs to completion"));
    }

    // Every racer that was served reports the ONE transformer the token created; every
    // racer that lost the claim was refused with ConflictException, and none of them
    // created a second transformer.
    let mut served: Vec<String> = Vec::new();
    for (index, (status, body)) in racers.iter().enumerate() {
        match status {
            200 => served.push(text(body, "transformerId")),
            409 => {
                assert_eq!(
                    error_code(body),
                    "ConflictException",
                    "case racer {index}: a racer that lost must be refused with \
                     ConflictException, got {body}"
                );
            }
            other => panic!("case racer {index}: expected 200 or 409, got {other} {body}"),
        }
    }
    assert!(
        !served.is_empty(),
        "case concurrent racers: at least one racer must be served"
    );
    let first = served[0].clone();
    assert!(
        served.iter().all(|id| *id == first),
        "case concurrent racers: exactly one transformer must be created, got {served:?}"
    );
    assert!(
        is_generated_id(&first, "t"),
        "case concurrent racers: expected a generated t- id, got {first:?}"
    );
    let (status, listed) = call(&router, "ListTransformers", json!({})).await;
    assert_eq!(status, 200, "case concurrent racers: ListTransformers failed: {listed}");
    let created: Vec<&Value> = listed["transformers"]
        .as_array()
        .map(|transformers| {
            transformers
                .iter()
                .filter(|transformer| transformer["transformerId"] == json!(first))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        created.len(),
        1,
        "case concurrent racers: the store must hold exactly one transformer under that \
         id, got {listed}"
    );

    // A repeat once that create has answered gets that same transformer, with nothing
    // waited for in between.
    let (status, body) = call(&router, "CreateTransformer", request.clone()).await;
    assert_eq!(status, 200, "case replay: expected 200, got {status} {body}");
    assert_eq!(
        text(&body, "transformerId"),
        first,
        "case replay: expected the original transformerId, got {body}"
    );

    // And exactly one transformer exists: it reads back, tagged once.
    let (status, read) = call(&router, "GetTransformer", json!({ "transformerId": first })).await;
    assert_eq!(
        status, 200,
        "case replay: expected the transformer to be readable, got {read}"
    );
    let (status, tags) = list_tags(&router, &b2bi_arn("transformer", &first)).await;
    assert_eq!(status, 200, "case replay: expected the tags to be readable, got {tags}");
    assert_eq!(
        tag_pairs(&tags),
        vec![
            ("env".to_string(), "test".to_string()),
            ("team".to_string(), "edi".to_string())
        ],
        "case replay: expected the tags the first call attached, once, got {tags}"
    );

    // Tag ORDER is not a difference: the tag list is compared as a set.
    let mut reordered = request.clone();
    reordered["tags"] = tags_json(&[("team", "edi"), ("env", "test")]);
    let (status, body) = call(&router, "CreateTransformer", reordered).await;
    assert_eq!(status, 200, "case reordered tags: expected 200, got {status} {body}");
    assert_eq!(
        text(&body, "transformerId"),
        first,
        "case reordered tags: expected the original transformerId, got {body}"
    );

    // The same token asking for something different is a conflict.
    let conflicting: &[(&str, &str, Value)] = &[
        ("a different name", "name", json!("renamed")),
        (
            "a different conversion",
            "inputConversion",
            json!({
                "fromFormat": "X12",
                "formatOptions": { "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" } }
            }),
        ),
        (
            "a different tag value",
            "tags",
            tags_json(&[("env", "prod"), ("team", "edi")]),
        ),
        (
            "a member the first request did not name",
            "mapping",
            json!({ "templateLanguage": "JSONATA", "template": "$" }),
        ),
    ];
    for (case, member, value) in conflicting {
        let mut altered = request.clone();
        altered[*member] = value.clone();
        let (status, body) = call(&router, "CreateTransformer", altered).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {body}"
        );
    }

    // A fresh token, and no token at all, each mint their own transformer.
    let mut other_token = request.clone();
    other_token["clientToken"] = json!("another-token");
    let (status, body) = call(&router, "CreateTransformer", other_token).await;
    assert_eq!(status, 200, "case another token: expected 200, got {status} {body}");
    assert_ne!(
        text(&body, "transformerId"),
        first,
        "case another token: expected a new transformer, got {body}"
    );

    let mut no_token = request.clone();
    no_token
        .as_object_mut()
        .expect("the request is an object")
        .remove("clientToken");
    let (status, body) = call(&router, "CreateTransformer", no_token).await;
    assert_eq!(status, 200, "case no token: expected 200, got {status} {body}");
    assert_ne!(
        text(&body, "transformerId"),
        first,
        "case no token: expected a new transformer, got {body}"
    );
}

/// RULE: a test-harness operation refuses, with the modelled `ValidationException`
/// (HTTP 400), a document that is not the `fileFormat` it was declared as — and says which
/// of the two refusals it gave: content that is not that format at all is refused for the
/// format it was declared as, while content that IS that format but is not the B2Bi EDI
/// document shape reaches `TestConversion`'s conversion and is refused for the document it
/// is not.
///
/// The rows span both source formats, and for each one several ways a document fails to
/// be it (truncated, empty, not a document of that kind at all, unclosed, more than one
/// XML root) beside documents that are it but carry no interchange to convert. The
/// conversion itself — the interchange a real EDI document is written back as — is
/// asserted by `test_conversion.rs`.
#[tokio::test]
async fn test_conversion_refuses_source_content_that_is_not_its_declared_format() {
    let router = b2bi_router();

    // (case, source fileFormat, fileContent, is the content that format?)
    let cases: &[(&str, &str, &str, bool)] = &[
        ("JSON object", "JSON", r#"{"transactionSets":[]}"#, true),
        ("JSON array", "JSON", "[1, 2, 3]", true),
        ("truncated JSON", "JSON", r#"{"transactionSets":"#, false),
        ("JSON with a trailing comma", "JSON", r#"{"a": 1,}"#, false),
        ("empty JSON content", "JSON", "", false),
        ("EDI text declared as JSON", "JSON", "ISA*00*~GS*PO*~", false),
        ("XML document", "XML", "<transactionSets><set/></transactionSets>", true),
        ("XML with a prolog", "XML", "<?xml version=\"1.0\"?>\n<set/>", true),
        ("mismatched XML tags", "XML", "<set></sets>", false),
        ("unclosed XML tag", "XML", "<set>", false),
        ("two XML roots", "XML", "<one/><two/>", false),
        ("empty XML content", "XML", "", false),
        ("bare text declared as XML", "XML", "this is not xml", false),
    ];

    for (case, file_format, content, is_that_format) in cases {
        let (status, body) = call(
            &router,
            "TestConversion",
            json!({
                "source": {
                    "fileFormat": file_format,
                    "inputFile": { "fileContent": content }
                },
                "target": {
                    "fileFormat": "X12",
                    "formatDetails": { "x12": { "transactionSet": "X12_850", "version": "VERSION_4010" } }
                }
            }),
        )
        .await;

        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );
        // A document that IS its declared format got past the format check and was refused
        // by the conversion; one that is not never reached it. A JSON document that does
        // not parse is refused with the JSON parser's own diagnostic, whose location clause
        // reports the submitted document as redacted (`aws_worded_refusals.rs` asserts that
        // wording over a table of its own); an XML one is refused for the format it was
        // declared as.
        let message = text(&body, "Message");
        let expected = match (*is_that_format, *file_format) {
            (true, _) => "could not be converted to X12",
            (false, "JSON") => "at [Source: REDACTED",
            (false, _) => "is declared as",
        };
        assert!(
            message.contains(expected),
            "case {case:?}: expected a refusal saying {expected:?}, got {message:?}"
        );
    }
}

// `TestMapping` is covered by `tests/all/test_mapping.rs`, which exercises mainline's
// evaluating implementation of that operation (including the invalid-template
// `ValidationException` this issue's cause 4 is about); this suite therefore carries no
// TestMapping rows of its own.
