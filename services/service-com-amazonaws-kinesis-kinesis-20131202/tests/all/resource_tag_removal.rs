// SPDX-License-Identifier: CC0-1.0

//! A Kinesis resource's tag REMOVAL BY ARN, end to end over the wire: `CreateStream` /
//! `AddTagsToStream` / `TagResource` / `CreateChannel` -> `UntagResource` ->
//! `ListTagsForResource`.
//!
//! RULE: `UntagResource` UNASSIGNS exactly the tag keys its request names from the stored tag
//! set of the Kinesis resource its `ResourceARN` names, inside the region the request was
//! signed for — whichever of the resource kinds that ARN may name (a stream or a channel),
//! since each resource carries ONE stored tag set that every tag write of it writes and every
//! tag read of it reports. A key the resource carries is deleted and cannot be read back, a
//! key it does not carry is ignored, and every tag the request does not name stays as it was,
//! so the set a read reports afterwards is the stored set minus those keys and removing the
//! same keys again changes nothing. An ARN this account and region hold no resource under is
//! `ResourceNotFoundException`, worded for the kind that ARN names. A resource's tag set
//! exists for as long as the resource does, so a removal is served in every lifecycle status
//! the resource has, and because a removal only ever leaves the resource carrying fewer tags
//! it frees room under the per-resource tag ceiling rather than being bounded by it.
//!
//! The recorded flow this was derived from tags ONE stream with ONE pair by ARN and removes
//! that ONE key, which is one row of the table below. The rows differ along what the rule
//! generalises over: how many keys a removal names (one, several, the published maximum),
//! whether those keys are carried, uncarried or a mix of both, the form and length of a key,
//! the order the keys arrive in, how many removals the resource takes, which operation wrote
//! the set being deleted from, and the kind of resource the ARN names.
//!
//! The wire harness, the ARN builders and the tag helpers are [`crate::stream_tags`]',
//! [`crate::stream_tag_removal`]' and [`crate::channel_lifecycle`]', reused rather than
//! copied.

use serde_json::{json, Value};
use service_com_amazonaws_kinesis_kinesis_20131202::state::{
    channel_arn, MAX_STREAM_TAGS, STREAM_ACTIVE, STREAM_CREATING, STREAM_DELETING,
};

use crate::channel_lifecycle::{
    on_demand_stream, past_channel_activation, role_arn, storage_configuration,
    stream_configuration,
};
use crate::stream_tag_removal::{keys, keys_of};
use crate::stream_tags::{
    error_code, error_message, in_key_order, pair, pairs, past_activation, reported_tags,
    stream_arn, tag_map, Emulator, REGION,
};

/// The `@length` maximum of this operation's `TagKeys`, which is how many keys ONE removal
/// may name (`smithy_query(query="operation",operation="UntagResource")` -> `TagKeys` len
/// 1..50).
const MAX_TAG_KEYS_PER_REMOVAL: usize = 50;

/// The `@length` maximum of `Tag$Key`, in characters
/// (`smithy_query(query="shape",shape="Tag")` -> `Key` len 1..128).
const TAG_KEY_MAX_LENGTH: usize = 128;

/// Characters `Tag$Key` documents as valid beyond letters and digits: "Valid characters:
/// Unicode letters, digits, white space, _ . / = + - % @"
/// (`smithy_query(query="shape",shape="Tag")`, member `Key`). The model carries no
/// `@pattern` for the member, so this doc sentence is the catalog.
const TAG_PUNCTUATION_KEY: &str = "_ ./=+-%@";

/// One row of the removal table: the tags the resource is given and the operation that gave
/// them to it, the keys successive `UntagResource` calls name, and the tag set the resource
/// must then report.
struct Row {
    /// The row's label, which is also its stream's name.
    label: &'static str,
    /// Which of the ways into the one stored tag set put `assigned` there: the create itself,
    /// the legacy stream API, or the generic resource API.
    assigned_by: &'static str,
    /// The tags the resource carries before any removal.
    assigned: Vec<(String, String)>,
    /// One `UntagResource` request each, served in order.
    removals: Vec<Vec<String>>,
    /// The tags the resource must report afterwards, given UNSORTED so the key order every
    /// read agrees on is what the assertion checks.
    expected: Vec<(String, String)>,
}

/// The table the removal rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // The recorded shape: one key, carried by the resource, removed on its own.
        Row {
            label: "the-only-tag-of-the-resource",
            assigned_by: "TagResource",
            assigned: vec![pair("Environment", "production")],
            removals: vec![keys(&["Environment"])],
            expected: Vec::new(),
        },
        // One of several, leaving every tag the request did not name alone — on a set the
        // LEGACY stream API wrote, which is the same stored set.
        Row {
            label: "one-key-of-three-written-by-add-tags-to-stream",
            assigned_by: "AddTagsToStream",
            assigned: vec![
                pair("owner", "data-platform"),
                pair("Environment", "staging"),
                pair("costCenter", "4711"),
            ],
            removals: vec![keys(&["Environment"])],
            expected: vec![pair("owner", "data-platform"), pair("costCenter", "4711")],
        },
        // Several keys at once, named in an order that is not key order, on a set the CREATE
        // carried.
        Row {
            label: "three-keys-at-once-out-of-key-order",
            assigned_by: "CreateStream",
            assigned: vec![
                pair("zeta", "3"),
                pair("alpha", "1"),
                pair("mu", "2"),
                pair("kept", "yes"),
            ],
            removals: vec![keys(&["mu", "zeta", "alpha"])],
            expected: vec![pair("kept", "yes")],
        },
        // A key nothing carries is ignored, so a mix of carried and uncarried keys removes
        // the carried ones and is still served.
        Row {
            label: "carried-and-uncarried-keys-mixed",
            assigned_by: "TagResource",
            assigned: vec![pair("owner", "team-a"), pair("tier", "gold")],
            removals: vec![keys(&["tier", "no-such-key", "another-missing-key"])],
            expected: vec![pair("owner", "team-a")],
        },
        // Only keys nothing carries: served, and the stored set is untouched.
        Row {
            label: "only-uncarried-keys-changes-nothing",
            assigned_by: "TagResource",
            assigned: vec![pair("owner", "team-b")],
            removals: vec![keys(&["absent"])],
            expected: vec![pair("owner", "team-b")],
        },
        // Removing the same keys twice: the second call finds them gone and is ignored.
        Row {
            label: "the-same-keys-removed-twice-is-idempotent",
            assigned_by: "AddTagsToStream",
            assigned: vec![pair("owner", "team-c"), pair("tier", "silver")],
            removals: vec![keys(&["tier"]), keys(&["tier"])],
            expected: vec![pair("owner", "team-c")],
        },
        // Successive removals accumulate, one key per call.
        Row {
            label: "three-successive-removals-accumulate",
            assigned_by: "TagResource",
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
            expected: vec![pair("four", "4")],
        },
        // Keys and values outside the Latin script, a key at the modelled `@length` maximum,
        // a key of the documented punctuation and a key at the `@length` minimum of one
        // character — none of which the recorded flow exercises.
        Row {
            label: "keys-outside-the-latin-script-and-at-their-length-bounds",
            assigned_by: "CreateStream",
            assigned: vec![
                pair("Ключ", "значение"),
                pair("プロジェクト", "データ ストリーム"),
                pair(&"k".repeat(TAG_KEY_MAX_LENGTH), "long"),
                pair(TAG_PUNCTUATION_KEY, "punctuated"),
                pair("x", "1"),
            ],
            removals: vec![keys(&[
                "Ключ",
                &"k".repeat(TAG_KEY_MAX_LENGTH),
                TAG_PUNCTUATION_KEY,
            ])],
            expected: vec![pair("プロジェクト", "データ ストリーム"), pair("x", "1")],
        },
        // The most keys ONE removal may name, taking a fully tagged resource to nothing.
        Row {
            label: "the-maximum-number-of-keys-in-one-removal",
            assigned_by: "TagResource",
            assigned: pairs(MAX_TAG_KEYS_PER_REMOVAL),
            removals: vec![keys_of(&pairs(MAX_TAG_KEYS_PER_REMOVAL))],
            expected: Vec::new(),
        },
        // The whole per-resource ceiling emptied across two removals.
        Row {
            label: "the-whole-tag-ceiling-across-two-removals",
            assigned_by: "AddTagsToStream",
            assigned: pairs(MAX_STREAM_TAGS),
            removals: vec![
                keys_of(&pairs(MAX_STREAM_TAGS / 2)),
                keys_of(&pairs(MAX_STREAM_TAGS).split_off(MAX_STREAM_TAGS / 2)),
            ],
            expected: Vec::new(),
        },
    ]
}

/// A `ListTagsForResource` read of `arn`, which must be served.
async fn read_tags(emulator: &Emulator, case: &str, arn: &str) -> Value {
    emulator
        .ok(case, "ListTagsForResource", &json!({ "ResourceARN": arn }))
        .await
}

/// Every row's removals are served by ARN, and the resource then reports the stored set minus
/// exactly the keys they named, in key order — whichever operation wrote that set.
#[tokio::test]
async fn a_removal_by_arn_deletes_the_keys_it_names_and_leaves_every_other_tag() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every stream is created first, so the rows share ONE wait for the transition the
    // creates armed rather than one each.
    for row in &rows {
        let mut body = json!({ "StreamName": row.label, "ShardCount": 1 });
        if row.assigned_by == "CreateStream" {
            body["Tags"] = tag_map(&row.assigned);
        }
        emulator.ok(row.label, "CreateStream", &body).await;
    }
    tokio::time::sleep(past_activation()).await;

    for row in &rows {
        let arn = stream_arn(REGION, row.label);
        match row.assigned_by {
            "CreateStream" => {}
            "TagResource" => {
                emulator
                    .ok(
                        row.label,
                        "TagResource",
                        &json!({ "ResourceARN": arn, "Tags": tag_map(&row.assigned) }),
                    )
                    .await;
            }
            operation => {
                emulator
                    .ok(
                        row.label,
                        operation,
                        &json!({ "StreamARN": arn, "Tags": tag_map(&row.assigned) }),
                    )
                    .await;
            }
        }

        for (index, removal) in row.removals.iter().enumerate() {
            let (status, answered) = emulator
                .call(
                    "UntagResource",
                    &json!({ "ResourceARN": arn, "TagKeys": removal }),
                )
                .await;
            assert_eq!(
                (status, &answered),
                (200, &json!({})),
                "case {}: UntagResource #{index} is served and models no output members, \
                 got {status} with body {answered}",
                row.label
            );
        }

        let read = read_tags(&emulator, row.label, &arn).await;
        let reported = reported_tags(row.label, &read);
        assert_eq!(
            reported,
            in_key_order(&row.expected),
            "case {}: expected the stored set minus the removed keys, in key order, got \
             {reported:?}",
            row.label
        );

        // A removed key is deleted and cannot be read back — through the legacy read of the
        // very same stored set either.
        let legacy = emulator
            .ok(
                row.label,
                "ListTagsForStream",
                &json!({ "StreamARN": arn, "Limit": MAX_STREAM_TAGS }),
            )
            .await;
        assert_eq!(
            reported_tags(row.label, &legacy),
            in_key_order(&row.expected),
            "case {}: expected both reads of one stored set to agree, got {legacy}",
            row.label
        );
        let removed: Vec<String> = row
            .removals
            .iter()
            .flatten()
            .filter(|key| !row.expected.iter().any(|(kept, _)| kept == *key))
            .cloned()
            .collect();
        for key in &removed {
            assert!(
                !reported.iter().any(|(reported_key, _)| reported_key == key),
                "case {}: the removed key {key:?} must be gone, got {reported:?}",
                row.label
            );
        }
    }
}

/// A `ResourceARN` names a resource of either kind this service holds
/// (`^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$`), so a CHANNEL's stored tag set is
/// deleted from by its ARN by the same rule — and a key the channel does not carry is ignored
/// there too.
#[tokio::test]
async fn a_channels_stored_tag_set_is_deleted_from_by_its_arn() {
    let emulator = Emulator::new();
    let source = on_demand_stream(&emulator, "channel-untag-source").await;
    let assigned = vec![
        pair("Owner", "analytics"),
        pair("Environment", "production"),
        pair("tier", "gold"),
    ];
    let name = "untagged-by-arn-channel";
    emulator
        .ok(
            name,
            "CreateChannel",
            &json!({
                "ChannelName": name,
                "ServiceExecutionRoleARN": role_arn("channel-delivery"),
                "StreamConfigurationList": [stream_configuration(&source, "JSON", None)],
                "S3DestinationConfiguration": {
                    "StorageConfiguration": storage_configuration("channel-untag-bucket"),
                },
                "Tags": tag_map(&assigned),
            }),
        )
        .await;

    // A channel's tag set is metadata of the channel, so the removal is served while the
    // channel is still provisioning...
    let arn = channel_arn(REGION, name);
    let (status, answered) = emulator
        .call(
            "UntagResource",
            &json!({ "ResourceARN": arn, "TagKeys": ["tier", "no-such-key"] }),
        )
        .await;
    assert_eq!(
        (status, &answered),
        (200, &json!({})),
        "case {name}: a removal from a provisioning channel is served, got {status} with \
         body {answered}"
    );
    let creating = read_tags(&emulator, name, &arn).await;
    assert_eq!(
        reported_tags(name, &creating),
        in_key_order(&assigned[..2]),
        "case {name}: expected the carried key gone and the uncarried one ignored, got \
         {creating}"
    );

    // ...and once it is ready, over the very same stored set.
    tokio::time::sleep(past_channel_activation()).await;
    emulator
        .ok(
            name,
            "UntagResource",
            &json!({ "ResourceARN": arn, "TagKeys": ["Owner"] }),
        )
        .await;
    let active = read_tags(&emulator, name, &arn).await;
    assert_eq!(
        reported_tags(name, &active),
        vec![pair("Environment", "production")],
        "case {name}: expected exactly the tag no removal named, got {active}"
    );
}

/// One row of the lifecycle table: the status a stream is put into before its tags are
/// removed, and how it is put there.
struct LifecycleRow {
    /// The row's label, which is also its stream's name.
    label: &'static str,
    /// Whether the row waits past the instant the create armed the activation for.
    wait_for_activation: bool,
    /// Whether the row accepts a `DeleteStream` for the stream first, which leaves it in
    /// `DELETING` until the removal that delete owes falls due.
    delete_first: bool,
    /// The `StreamStatus` the stream is in when the removal lands, which the row asserts
    /// through `DescribeStreamSummary` rather than assuming.
    expected_status: &'static str,
}

/// A resource's tag set exists for as long as the resource does, so a removal by ARN is
/// served whatever lifecycle status the resource is in.
#[tokio::test]
async fn a_removal_by_arn_is_served_in_every_lifecycle_status() {
    let rows = [
        LifecycleRow {
            label: "untag-resource-creating",
            wait_for_activation: false,
            delete_first: false,
            expected_status: STREAM_CREATING,
        },
        LifecycleRow {
            label: "untag-resource-active",
            wait_for_activation: true,
            delete_first: false,
            expected_status: STREAM_ACTIVE,
        },
        LifecycleRow {
            label: "untag-resource-deleting",
            wait_for_activation: true,
            delete_first: true,
            expected_status: STREAM_DELETING,
        },
    ];

    for row in &rows {
        let emulator = Emulator::new();
        emulator
            .ok(
                row.label,
                "CreateStream",
                &json!({
                    "StreamName": row.label,
                    "ShardCount": 1,
                    "Tags": { "owner": "team-a", "tier": "gold" }
                }),
            )
            .await;
        if row.wait_for_activation {
            tokio::time::sleep(past_activation()).await;
        }
        if row.delete_first {
            emulator
                .ok(row.label, "DeleteStream", &json!({ "StreamName": row.label }))
                .await;
        }

        let summary = emulator
            .ok(
                row.label,
                "DescribeStreamSummary",
                &json!({ "StreamName": row.label }),
            )
            .await;
        let reported_status = summary
            .get("StreamDescriptionSummary")
            .and_then(|summary| summary.get("StreamStatus"))
            .and_then(Value::as_str)
            .unwrap_or("<no StreamStatus>");
        assert_eq!(
            reported_status, row.expected_status,
            "case {}: the resource is in {} when the removal lands, got {summary}",
            row.label, row.expected_status
        );

        let arn = stream_arn(REGION, row.label);
        let (status, response) = emulator
            .call(
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": ["tier"] }),
            )
            .await;
        assert_eq!(
            (status, &response),
            (200, &json!({})),
            "case {}: a removal from a resource in {} is served, got {status} with body \
             {response}",
            row.label,
            row.expected_status
        );

        let after = read_tags(&emulator, row.label, &arn).await;
        assert_eq!(
            reported_tags(row.label, &after),
            vec![pair("owner", "team-a")],
            "case {}: the removal served in {} left exactly the untouched tag, got {after}",
            row.label,
            row.expected_status
        );
    }
}

/// An ARN this account and region hold no resource under names nothing to remove tags from,
/// whatever form it takes: a stream nothing created, a stream of another region, a channel
/// nothing created, and a channel of another region. The refusal names the resource kind the
/// ARN names.
#[tokio::test]
async fn an_arn_naming_no_resource_of_this_account_and_region_is_refused() {
    let emulator = Emulator::new();
    let held = "untag-resource-refusals";
    emulator
        .ok(
            held,
            "CreateStream",
            &json!({
                "StreamName": held,
                "ShardCount": 1,
                "Tags": { "owner": "team-a" }
            }),
        )
        .await;
    let elsewhere = "eu-west-1";
    emulator
        .ok_in(
            held,
            elsewhere,
            "CreateStream",
            &json!({
                "StreamName": held,
                "ShardCount": 1,
                "Tags": { "owner": "team-a" }
            }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;

    let account = engine::EMULATOR_ACCOUNT_ID;
    // (case, the ARN the removal names, what the message must say — spelled out here with
    // only the resource the request named and the account interpolated, so a drift away from
    // the wording fails rather than comparing the handler against the helper it builds the
    // message with. Either resource kind is named the way a client addresses it — by its
    // NAME, which its ARN carries as the resource.)
    let rows: Vec<(&str, String, String)> = vec![
        (
            "a stream nothing created",
            stream_arn(REGION, "never-created"),
            format!("Stream never-created under account {account} not found."),
        ),
        (
            "a stream of another region",
            stream_arn(elsewhere, held),
            format!("Stream {held} under account {account} not found."),
        ),
        (
            "a channel nothing created",
            channel_arn(REGION, "never-created"),
            format!("Channel never-created under account {account} not found."),
        ),
        (
            "a channel of another region",
            channel_arn(elsewhere, "never-created"),
            format!("Channel never-created under account {account} not found."),
        ),
    ];

    for (case, arn, says) in &rows {
        let (status, response) = emulator
            .call(
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": ["owner"] }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected the modelled refusal, got {status} with body {response}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {response}"
        );
        assert_eq!(
            &error_message(&response),
            says,
            "case {case}: expected the not-found sentence this service states for that \
             resource kind, got {response}"
        );
    }

    // Nothing above removed a tag anywhere.
    for region in [REGION, elsewhere] {
        let response = emulator
            .ok_in(
                "refused-removals-changed-nothing",
                region,
                "ListTagsForResource",
                &json!({ "ResourceARN": stream_arn(region, held) }),
            )
            .await;
        assert_eq!(
            reported_tags("refused-removals-changed-nothing", &response),
            vec![pair("owner", "team-a")],
            "case refused-removals-changed-nothing: the stream {region} holds must still \
             carry its tag, got {response}"
        );
    }
}

/// `TagKeys` is modelled `len 1..50`: a list at either published bound is served, and one
/// past either bound is refused before anything is deleted.
#[tokio::test]
async fn a_tag_keys_list_outside_its_modelled_bounds_is_refused() {
    let emulator = Emulator::new();
    let name = "untag-resource-tag-keys-bounds";
    let stored = pairs(MAX_STREAM_TAGS);
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let arn = stream_arn(REGION, name);
    emulator
        .ok(
            name,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&stored) }),
        )
        .await;

    // A list at the published MINIMUM of one key is served...
    let first_key = keys_of(&stored)[0].clone();
    emulator
        .ok(
            "tag-keys-1",
            "UntagResource",
            &json!({ "ResourceARN": arn, "TagKeys": [first_key.clone()] }),
        )
        .await;

    // ...and a list one past either published bound is refused, leaving the stream carrying
    // every key the served removal did not take, so the read-back below compares a NON-empty
    // set.
    for refused in [Vec::new(), keys_of(&pairs(MAX_TAG_KEYS_PER_REMOVAL + 1))] {
        let count = refused.len();
        let (status, response) = emulator
            .call(
                "UntagResource",
                &json!({ "ResourceARN": arn, "TagKeys": refused }),
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

    let read = read_tags(&emulator, "tag-keys-bounds-read-back", &arn).await;
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
        reported_tags("tag-keys-bounds-read-back", &read),
        in_key_order(&expected),
        "case tag-keys-bounds-read-back: the refused removals deleted nothing, got {read}"
    );

    // A list at the published MAXIMUM is served too, and it empties the rest of the stream
    // (the key the first removal already took is among the keys it names, and is ignored).
    emulator
        .ok(
            &format!("tag-keys-{MAX_TAG_KEYS_PER_REMOVAL}"),
            "UntagResource",
            &json!({ "ResourceARN": arn, "TagKeys": keys_of(&stored) }),
        )
        .await;
    let emptied = read_tags(&emulator, "tag-keys-maximum-read-back", &arn).await;
    assert!(
        reported_tags("tag-keys-maximum-read-back", &emptied).is_empty(),
        "case tag-keys-maximum-read-back: a removal naming every key leaves no tag, got \
         {emptied}"
    );
}

/// A removal only ever leaves the resource carrying fewer tags, so it frees room under the
/// per-resource tag ceiling an assignment is bounded by.
#[tokio::test]
async fn a_removal_by_arn_frees_room_under_the_tag_ceiling() {
    let emulator = Emulator::new();
    let name = "untag-resource-frees-ceiling-room";
    let stored = pairs(MAX_STREAM_TAGS);
    emulator
        .ok(
            name,
            "CreateStream",
            &json!({ "StreamName": name, "ShardCount": 1 }),
        )
        .await;
    tokio::time::sleep(past_activation()).await;
    let arn = stream_arn(REGION, name);
    emulator
        .ok(
            name,
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": tag_map(&stored) }),
        )
        .await;

    // A fully tagged resource takes no further key...
    let (status, response) = emulator
        .call(
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": { "one-too-many": "x" } }),
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
            "UntagResource",
            &json!({ "ResourceARN": arn, "TagKeys": ["key-00"] }),
        )
        .await;
    emulator
        .ok(
            "below-the-ceiling",
            "TagResource",
            &json!({ "ResourceARN": arn, "Tags": { "one-too-many": "x" } }),
        )
        .await;

    let read = read_tags(&emulator, "ceiling-read-back", &arn).await;
    let mut expected: Vec<(String, String)> = stored
        .iter()
        .filter(|(key, _)| key != "key-00")
        .cloned()
        .collect();
    expected.push(pair("one-too-many", "x"));
    assert_eq!(
        reported_tags("ceiling-read-back", &read),
        in_key_order(&expected),
        "case ceiling-read-back: the removed key is gone and the newly assigned one is \
         there, got {read}"
    );
}
