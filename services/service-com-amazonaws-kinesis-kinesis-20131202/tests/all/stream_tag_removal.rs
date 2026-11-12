// SPDX-License-Identifier: CC0-1.0

//! A Kinesis data stream's tag REMOVAL, end to end over the wire: `AddTagsToStream` ->
//! `RemoveTagsFromStream` -> `ListTagsForStream`.
//!
//! RULE: `RemoveTagsFromStream` UNASSIGNS exactly the tag keys its request names from the
//! stored tag set of the stream it addresses — a key the stream carries is deleted and
//! cannot be read back, a key it does not carry is ignored, and every tag the request does
//! not name stays as it was — so the set a read reports afterwards is the stored set minus
//! those keys, and removing the same keys again changes nothing. A removal resolves its
//! stream by `StreamName`, by `StreamARN` or by both, inside the region the request was
//! signed for; a request naming a stream this account and region do not hold is
//! `ResourceNotFoundException`, and one naming no stream at all (or two different streams)
//! is `InvalidArgumentException`. A stream's tag set exists for as long as the stream does,
//! so a removal is served in every lifecycle status the stream has, and because a removal
//! only ever leaves the stream carrying fewer tags it frees room under the per-stream tag
//! ceiling.
//!
//! The recorded flow this was derived from tags ONE stream with ONE pair and removes that
//! ONE key (`remove_tags_from_stream(StreamName=…, TagKeys=[…])`), which is one row of the
//! table below. The rows differ along what the rule generalises over: how many keys a
//! removal names, whether those keys are carried, uncarried or a mix of both, the form and
//! length of a key, the order the keys arrive in, how many removals the stream takes, and
//! the identifier the request resolves the stream by.
//!
//! Every case drives the real awsJson1_1 request path (`POST /` with `X-Amz-Target:
//! Kinesis_20131202.<Operation>` through `MultiServiceRouter::dispatch`), so the statuses,
//! bodies and modelled errors asserted here are the ones a client sees.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    stream_not_found_message, MAX_STREAM_TAGS, STREAM_ACTIVE, STREAM_CREATING, STREAM_DELETING,
    stream_identifier_mismatch_message, STREAM_IDENTIFIER_REQUIRED_MESSAGE,
};

use crate::stream_tags::{
    error_code, error_message, in_key_order, pair, pairs, past_activation, reported_tags,
    stream_arn, tag_map, Emulator, REGION,
};

/// The `@length` maximum of `Tag$Key`, in characters
/// (`smithy_query(query="shape",shape="Tag")` -> `Key` len 1..128).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// Characters `Tag$Key` documents as valid beyond letters and digits: "Valid characters:
/// Unicode letters, digits, white space, _ . / = + - % @"
/// (`smithy_query(query="shape",shape="Tag")`, member `Key`). The model carries no
/// `@pattern` for the member, so this doc sentence is the catalog.
const TAG_PUNCTUATION_KEY: &str = "_ ./=+-%@";

/// The `@length` maximum of `TagKeyList`, which is how many keys ONE removal may name
/// (`smithy_query(query="operation",operation="RemoveTagsFromStream")` -> `TagKeys` len
/// 1..50).
const MAX_TAG_KEYS_PER_REMOVAL: usize = 50;

/// How a request addresses the stream it operates on — the identifiers the rule says a
/// removal accepts.
#[derive(Debug, Clone, Copy)]
enum Address {
    /// `StreamName` only.
    Name,
    /// `StreamARN` only, which the docs recommend for this operation.
    Arn,
    /// Both, naming one stream.
    Both,
}

impl Address {
    /// The identifier members this addressing adds to a request body.
    fn members(self, name: &str) -> Value {
        let arn = stream_arn(REGION, name);
        match self {
            Self::Name => json!({ "StreamName": name }),
            Self::Arn => json!({ "StreamARN": arn }),
            Self::Both => json!({ "StreamName": name, "StreamARN": arn }),
        }
    }
}

/// One row of the removal table: the tags the stream is assigned, the keys successive
/// `RemoveTagsFromStream` calls name, and the tag set the stream must then report.
struct RemovalCase {
    label: &'static str,
    /// The tags assigned to the stream before any removal.
    assigned: Vec<(String, String)>,
    /// One `RemoveTagsFromStream` request each, served in order.
    removals: Vec<Vec<String>>,
    /// How those requests, and the read that follows, address the stream.
    address: Address,
    /// The tags the stream must report afterwards, given UNSORTED so the key order every
    /// read agrees on is what the assertion checks.
    expected: Vec<(String, String)>,
}

/// Tag keys, spelled short.
pub(crate) fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|key| (*key).to_string()).collect()
}

/// The keys of `tags`, in the order they are given.
pub(crate) fn keys_of(tags: &[(String, String)]) -> Vec<String> {
    tags.iter().map(|(key, _)| key.clone()).collect()
}

/// The table the removal rule is checked over.
fn removal_cases() -> Vec<RemovalCase> {
    vec![
        // The recorded shape: one key, carried by the stream, removed on its own.
        RemovalCase {
            label: "the-only-tag-of-the-stream",
            assigned: vec![pair("Environment", "production")],
            removals: vec![keys(&["Environment"])],
            address: Address::Name,
            expected: Vec::new(),
        },
        // One of several, leaving every tag the request did not name alone.
        RemovalCase {
            label: "one-key-of-three",
            assigned: vec![
                pair("owner", "data-platform"),
                pair("Environment", "staging"),
                pair("costCenter", "4711"),
            ],
            removals: vec![keys(&["Environment"])],
            address: Address::Arn,
            expected: vec![pair("owner", "data-platform"), pair("costCenter", "4711")],
        },
        // Several keys at once, named in an order that is not key order.
        RemovalCase {
            label: "three-keys-at-once-out-of-key-order",
            assigned: vec![
                pair("zeta", "3"),
                pair("alpha", "1"),
                pair("mu", "2"),
                pair("kept", "yes"),
            ],
            removals: vec![keys(&["mu", "zeta", "alpha"])],
            address: Address::Both,
            expected: vec![pair("kept", "yes")],
        },
        // "If you specify a tag that does not exist, it is ignored": a mix of carried and
        // uncarried keys removes the carried ones and is still served.
        RemovalCase {
            label: "carried-and-uncarried-keys-mixed",
            assigned: vec![pair("owner", "team-a"), pair("tier", "gold")],
            removals: vec![keys(&["tier", "no-such-key", "another-missing-key"])],
            address: Address::Name,
            expected: vec![pair("owner", "team-a")],
        },
        // Only keys nothing carries: served, and the stored set is untouched.
        RemovalCase {
            label: "only-uncarried-keys-changes-nothing",
            assigned: vec![pair("owner", "team-b")],
            removals: vec![keys(&["absent"])],
            address: Address::Arn,
            expected: vec![pair("owner", "team-b")],
        },
        // Removing the same keys twice: the second call finds them gone and is ignored.
        RemovalCase {
            label: "the-same-keys-removed-twice-is-idempotent",
            assigned: vec![pair("owner", "team-c"), pair("tier", "silver")],
            removals: vec![keys(&["tier"]), keys(&["tier"])],
            address: Address::Name,
            expected: vec![pair("owner", "team-c")],
        },
        // Successive removals accumulate, one key per call.
        RemovalCase {
            label: "three-successive-removals-accumulate",
            assigned: vec![
                pair("one", "1"),
                pair("two", "2"),
                pair("three", "3"),
                pair("four", "4"),
            ],
            removals: vec![
                keys(&["three"]),
                keys(&["one"]),
                keys(&["two", "still-absent"]),
            ],
            address: Address::Both,
            expected: vec![pair("four", "4")],
        },
        // A key at the modelled `@length` maximum, a key of the documented punctuation, and
        // a key at the `@length` minimum of one character — none of which the recorded flow
        // exercises.
        RemovalCase {
            label: "keys-at-their-length-bounds-and-documented-punctuation",
            assigned: vec![
                pair(&"k".repeat(TAG_KEY_MAX_LENGTH), "long"),
                pair(TAG_PUNCTUATION_KEY, "punctuated"),
                pair("x", "1"),
            ],
            removals: vec![keys(&[
                &"k".repeat(TAG_KEY_MAX_LENGTH),
                TAG_PUNCTUATION_KEY,
            ])],
            address: Address::Arn,
            expected: vec![pair("x", "1")],
        },
        // The most keys ONE removal may name, taking a fully tagged stream to nothing.
        RemovalCase {
            label: "the-maximum-number-of-keys-in-one-removal",
            assigned: pairs(MAX_TAG_KEYS_PER_REMOVAL),
            removals: vec![keys_of(&pairs(MAX_TAG_KEYS_PER_REMOVAL))],
            address: Address::Name,
            expected: Vec::new(),
        },
        // The whole per-stream ceiling emptied across two removals.
        RemovalCase {
            label: "the-whole-tag-ceiling-across-two-removals",
            assigned: pairs(MAX_STREAM_TAGS),
            removals: vec![
                keys_of(&pairs(MAX_STREAM_TAGS / 2)),
                keys_of(&pairs(MAX_STREAM_TAGS).split_off(MAX_STREAM_TAGS / 2)),
            ],
            address: Address::Both,
            expected: Vec::new(),
        },
    ]
}

#[tokio::test]
async fn a_removal_deletes_the_keys_it_names_and_leaves_every_other_tag() {
    let emulator = Emulator::new();
    let cases = removal_cases();

    // Every stream is created first, so the cases share ONE wait for the transition the
    // creates armed rather than one each.
    for case in &cases {
        emulator
            .ok(
                case.label,
                "CreateStream",
                &json!({ "StreamName": case.label, "ShardCount": 1 }),
            )
            .await;
    }
    tokio::time::sleep(past_activation()).await;

    for case in &cases {
        let mut assign = json!({ "StreamName": case.label });
        assign["Tags"] = tag_map(&case.assigned);
        emulator.ok(case.label, "AddTagsToStream", &assign).await;

        for (index, removal) in case.removals.iter().enumerate() {
            let mut body = case.address.members(case.label);
            body["TagKeys"] = json!(removal);
            let answered = emulator
                .ok(case.label, "RemoveTagsFromStream", &body)
                .await;
            assert_eq!(
                answered,
                json!({}),
                "case {}: RemoveTagsFromStream #{index} models no output members, got \
                 {answered}",
                case.label
            );
        }

        let response = emulator
            .ok(
                case.label,
                "ListTagsForStream",
                &case.address.members(case.label),
            )
            .await;
        let reported = reported_tags(case.label, &response);
        assert_eq!(
            reported,
            in_key_order(&case.expected),
            "case {}: the stream must report every tag the removals left, in key order, \
             got {reported:?}",
            case.label
        );

        // A removed key cannot be read back, however it is spelled in a later read.
        let removed: Vec<String> = case
            .removals
            .iter()
            .flatten()
            .filter(|key| !case.expected.iter().any(|(kept, _)| kept == *key))
            .cloned()
            .collect();
        for key in &removed {
            assert!(
                !reported.iter().any(|(reported_key, _)| reported_key == key),
                "case {}: the removed key must be gone, got {reported:?}",
                case.label
            );
        }
    }
}

/// One row of the lifecycle table: the status a stream is put into before its tags are
/// removed, and how it is put there.
struct LifecycleCase {
    /// The case's label, which is also its stream's name.
    label: &'static str,
    /// Whether the case waits past the instant the create armed the activation for.
    wait_for_activation: bool,
    /// Whether the case accepts a `DeleteStream` for the stream first, which leaves it in
    /// `DELETING` until the removal that delete owes falls due.
    delete_first: bool,
    /// The `StreamStatus` the stream is in when the removal lands, which the case asserts
    /// through `DescribeStreamSummary` rather than assuming.
    expected_status: &'static str,
}

/// A stream's tag set is UNASSIGNED FROM in every lifecycle status the stream has.
///
/// The rows span the statuses a stream reaches here: one still inside the transition its
/// create armed (`CREATING`), one past it (`ACTIVE`), and one a delete has been accepted
/// for (`DELETING`).
#[tokio::test]
async fn a_removal_is_served_in_every_lifecycle_status() {
    let cases = [
        LifecycleCase {
            label: "removal-creating",
            wait_for_activation: false,
            delete_first: false,
            expected_status: STREAM_CREATING,
        },
        LifecycleCase {
            label: "removal-active",
            wait_for_activation: true,
            delete_first: false,
            expected_status: STREAM_ACTIVE,
        },
        LifecycleCase {
            label: "removal-deleting",
            wait_for_activation: true,
            delete_first: true,
            expected_status: STREAM_DELETING,
        },
    ];

    for case in &cases {
        let emulator = Emulator::new();
        emulator
            .ok(
                case.label,
                "CreateStream",
                &json!({
                    "StreamName": case.label,
                    "ShardCount": 1,
                    "Tags": { "owner": "team-a", "tier": "gold" }
                }),
            )
            .await;
        if case.wait_for_activation {
            tokio::time::sleep(past_activation()).await;
        }
        if case.delete_first {
            emulator
                .ok(
                    case.label,
                    "DeleteStream",
                    &json!({ "StreamName": case.label }),
                )
                .await;
        }

        let summary = emulator
            .ok(
                case.label,
                "DescribeStreamSummary",
                &json!({ "StreamName": case.label }),
            )
            .await;
        let reported_status = summary
            .get("StreamDescriptionSummary")
            .and_then(|summary| summary.get("StreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or("<no StreamStatus>");
        assert_eq!(
            reported_status, case.expected_status,
            "case {}: the stream is in {} when the removal lands, got {summary}",
            case.label, case.expected_status
        );

        let (status, response) = emulator
            .call(
                "RemoveTagsFromStream",
                &json!({ "StreamName": case.label, "TagKeys": ["tier"] }),
            )
            .await;
        assert_eq!(
            (status, &response),
            (200, &json!({})),
            "case {}: a removal from a stream in {} is served, got {status} with body \
             {response}",
            case.label,
            case.expected_status
        );

        let after = emulator
            .ok(
                case.label,
                "ListTagsForStream",
                &json!({ "StreamName": case.label }),
            )
            .await;
        assert_eq!(
            reported_tags(case.label, &after),
            vec![pair("owner", "team-a")],
            "case {}: the removal served in {} left exactly the untouched tag, got {after}",
            case.label,
            case.expected_status
        );
    }
}

/// One row of the refusal table: a request that names no stream of this account and region,
/// and the modelled error it must answer with.
struct RefusalCase {
    label: &'static str,
    /// The identifier members the request carries.
    identifiers: Value,
    expected_code: &'static str,
    expected_message: String,
}

#[tokio::test]
async fn a_removal_naming_no_stream_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    let name = "removal-refusals";
    let other = "removal-refusals-other";
    for stream in [name, other] {
        emulator
            .ok(
                stream,
                "CreateStream",
                &json!({
                    "StreamName": stream,
                    "ShardCount": 1,
                    "Tags": { "owner": "team-a" }
                }),
            )
            .await;
    }
    // The same name in another region is another stream, and only the region that created
    // one resolves it.
    let elsewhere = "removal-refusals-elsewhere";
    emulator
        .ok_in(
            elsewhere,
            "eu-west-1",
            "CreateStream",
            &json!({
                "StreamName": elsewhere,
                "ShardCount": 1,
                "Tags": { "owner": "team-a" }
            }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let cases = vec![
        RefusalCase {
            label: "no-identifier-at-all",
            identifiers: json!({}),
            expected_code: "InvalidArgumentException",
            expected_message: STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        RefusalCase {
            label: "only-the-reserved-stream-id",
            // `StreamId` is documented "Not Implemented. Reserved for future use", so it
            // names no stream.
            identifiers: json!({ "StreamId": "abcdefghijklmnopqrst-001" }),
            expected_code: "InvalidArgumentException",
            expected_message: STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string(),
        },
        RefusalCase {
            label: "name-and-arn-naming-different-streams",
            identifiers: json!({ "StreamName": name, "StreamARN": stream_arn(REGION, other) }),
            expected_code: "InvalidArgumentException",
            expected_message: stream_identifier_mismatch_message(
                name,
                &stream_arn(REGION, other),
            ),
        },
        RefusalCase {
            label: "a-name-no-stream-holds",
            identifiers: json!({ "StreamName": "no-such-stream" }),
            expected_code: "ResourceNotFoundException",
            expected_message: stream_not_found_message("no-such-stream"),
        },
        RefusalCase {
            label: "an-arn-no-stream-holds",
            identifiers: json!({ "StreamARN": stream_arn(REGION, "no-such-stream") }),
            expected_code: "ResourceNotFoundException",
            expected_message: stream_not_found_message(&stream_arn(REGION, "no-such-stream")),
        },
        RefusalCase {
            label: "a-stream-another-region-holds",
            identifiers: json!({ "StreamName": elsewhere }),
            expected_code: "ResourceNotFoundException",
            expected_message: stream_not_found_message(elsewhere),
        },
    ];

    for case in &cases {
        let mut body = case.identifiers.clone();
        body["TagKeys"] = json!(["owner"]);
        let (status, response) = emulator.call("RemoveTagsFromStream", &body).await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, case.expected_code),
            "case {}: expected the modelled {}, got {status} with body {response}",
            case.label,
            case.expected_code
        );
        assert_eq!(
            error_message(&response),
            case.expected_message,
            "case {}: the refusal names what was at fault, got {response}",
            case.label
        );
    }

    // Nothing above removed a tag anywhere.
    for (region, stream) in [(REGION, name), (REGION, other), ("eu-west-1", elsewhere)] {
        let response = emulator
            .ok_in(
                "refused-removals-changed-nothing",
                region,
                "ListTagsForStream",
                &json!({ "StreamName": stream }),
            )
            .await;
        assert_eq!(
            reported_tags("refused-removals-changed-nothing", &response),
            vec![pair("owner", "team-a")],
            "case refused-removals-changed-nothing: {stream} in {region} must still carry \
             its tag, got {response}"
        );
    }
}

#[tokio::test]
async fn a_tag_keys_list_outside_its_modelled_bounds_is_refused() {
    let emulator = Emulator::new();
    let name = "removal-tag-keys-bounds";
    let stored = pairs(MAX_STREAM_TAGS);
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    emulator
        .ok(
            name,
            "AddTagsToStream",
            &json!({ "StreamName": name, "Tags": tag_map(&stored) }),
        )
        .await;

    // `TagKeys` is modelled `len 1..50`: a list at its published minimum is served...
    let first_key = keys_of(&stored)[0].clone();
    emulator
        .ok(
            "tag-keys-1",
            "RemoveTagsFromStream",
            &json!({ "StreamName": name, "TagKeys": [first_key.clone()] }),
        )
        .await;

    // ...and a list one past either published bound is refused, with the stream still
    // carrying every key that served removal left it, so the read-back below compares a
    // NON-empty set.
    for refused in [Vec::new(), keys_of(&pairs(MAX_TAG_KEYS_PER_REMOVAL + 1))] {
        let count = refused.len();
        let (status, response) = emulator
            .call(
                "RemoveTagsFromStream",
                &json!({ "StreamName": name, "TagKeys": refused }),
            )
            .await;
        assert_eq!(
            (status, error_code(&response).as_str()),
            (400, "ValidationException"),
            "case tag-keys-{count}: a TagKeys list outside the modelled bounds is refused as \
             a ValidationException, got {status} with body {response}"
        );
        // AWS's clause for a COLLECTION that broke its `@length` names the member at fault
        // in its wire casing and the bound it broke, and quotes no value: a list has no
        // scalar rendering to echo.
        let bound = if count == 0 {
            "Member must have length greater than or equal to 1".to_string()
        } else {
            format!("Member must have length less than or equal to {MAX_TAG_KEYS_PER_REMOVAL}")
        };
        let message = error_message(&response);
        assert!(
            message.contains("tagKeys") && message.contains(&bound),
            "case tag-keys-{count}: the refusal names the member and the bound at fault, got \
             {response}"
        );
    }

    // The refused requests removed nothing: the stream still carries every key except the
    // one the served removal above took.
    let response = emulator
        .ok(
            "tag-keys-bounds-read-back",
            "ListTagsForStream",
            &json!({ "StreamName": name }),
        )
        .await;
    let expected: Vec<(String, String)> = stored
        .iter()
        .filter(|(key, _)| key != &first_key)
        .cloned()
        .collect();
    assert_eq!(
        expected.len(),
        MAX_STREAM_TAGS - 1,
        "case tag-keys-bounds-read-back: the read-back must compare a non-empty set, got \
         {expected:?}"
    );
    assert_eq!(
        reported_tags("tag-keys-bounds-read-back", &response),
        in_key_order(&expected),
        "case tag-keys-bounds-read-back: the refused removals changed nothing, got {response}"
    );

    // A list at the published MAXIMUM is served too, and it empties the rest of the stream
    // (the key the first removal already took is among the 50 it names, and is ignored).
    emulator
        .ok(
            &format!("tag-keys-{MAX_TAG_KEYS_PER_REMOVAL}"),
            "RemoveTagsFromStream",
            &json!({ "StreamName": name, "TagKeys": keys_of(&stored) }),
        )
        .await;
    let emptied = emulator
        .ok(
            "tag-keys-maximum-read-back",
            "ListTagsForStream",
            &json!({ "StreamName": name }),
        )
        .await;
    assert!(
        reported_tags("tag-keys-maximum-read-back", &emptied).is_empty(),
        "case tag-keys-maximum-read-back: a removal naming every key leaves no tag, got \
         {emptied}"
    );
}

#[tokio::test]
async fn a_removal_frees_room_under_the_per_stream_tag_ceiling() {
    let emulator = Emulator::new();
    let name = "removal-frees-ceiling-room";
    let stored = pairs(MAX_STREAM_TAGS);
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    emulator
        .ok(
            name,
            "AddTagsToStream",
            &json!({ "StreamName": name, "Tags": tag_map(&stored) }),
        )
        .await;

    // A fully tagged stream takes no further key...
    let (status, response) = emulator
        .call(
            "AddTagsToStream",
            &json!({ "StreamName": name, "Tags": { "one-too-many": "x" } }),
        )
        .await;
    assert_eq!(
        (status, error_code(&response).as_str()),
        (400, "LimitExceededException"),
        "case at-the-ceiling: an assignment past the tag ceiling is refused, got {status} \
         with body {response}"
    );

    // ...until a removal frees room under the ceiling.
    emulator
        .ok(
            "below-the-ceiling",
            "RemoveTagsFromStream",
            &json!({ "StreamName": name, "TagKeys": ["key-00"] }),
        )
        .await;
    emulator
        .ok(
            "below-the-ceiling",
            "AddTagsToStream",
            &json!({ "StreamName": name, "Tags": { "one-too-many": "x" } }),
        )
        .await;

    let response = emulator
        .ok(
            "ceiling-read-back",
            "ListTagsForStream",
            &json!({ "StreamName": name }),
        )
        .await;
    let mut expected: Vec<(String, String)> = stored
        .iter()
        .filter(|(key, _)| key != "key-00")
        .cloned()
        .collect();
    expected.push(pair("one-too-many", "x"));
    assert_eq!(
        reported_tags("ceiling-read-back", &response),
        in_key_order(&expected),
        "case ceiling-read-back: the removed key is gone and the newly assigned one is \
         there, got {response}"
    );
}
