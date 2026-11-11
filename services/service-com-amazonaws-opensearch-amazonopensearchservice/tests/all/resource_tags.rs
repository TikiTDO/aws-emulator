// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the OpenSearch Service resource tags: `AddTags`,
//! `ListTags` and `RemoveTags`, plus the `TagList` a `CreateDomain` carries.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it, only the three
//! tagging paths it drives.
//!
//! The tests assert RULES over sets of inputs, not one recorded call:
//!
//! * a resource's tags are the key-value set the calls on it leave behind, and
//!   `ListTags` reports exactly that set — over a table of tag lists differing in
//!   how many tags they carry, in key and value length (each published bound
//!   included), in character class and in the order they are sent, each read back
//!   through `ListTags` and the read repeated to show it changes nothing;
//! * a tag set holds ONE value per key, so attaching a key the resource already
//!   carries replaces that key's value instead of adding a second tag under it,
//!   whether the earlier value came from the create or from an earlier `AddTags`;
//! * `RemoveTags` removes exactly the keys it names and leaves every other tag in
//!   place, over a table of key subsets — none of them stored, some, all, and a key
//!   named twice;
//! * tags belong to the resource the ARN names: a call on one domain never changes
//!   another domain's set, and a deleted domain's ARN no longer resolves;
//! * an ARN that names no resource of this account is refused by all three with the
//!   modelled `ValidationException` and ONE wording, over a table of ways an ARN
//!   can resolve to nothing, and a value that is no ARN at all is refused as a
//!   malformed request by all three;
//! * a resource carries at most the documented number of tags: attaching more is
//!   refused with the modelled `LimitExceededException` and leaves the stored set
//!   exactly as it was, both through `AddTags` and through a create's `TagList`.

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::TAG_QUOTA;

use super::domain_lifecycle::{
    error_code, string_member, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// `AddTags`: `POST /2021-01-01/tags`.
pub(crate) async fn add_tags(emulator: &Emulator, arn: &str, tag_list: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/tags"),
            Some(json!({ "ARN": arn, "TagList": tag_list })),
        )
        .await
}

/// `ListTags`: `GET /2021-01-01/tags?arn=<ARN>`.
pub(crate) async fn list_tags(emulator: &Emulator, arn: &str) -> Answer {
    emulator
        .call("GET", &format!("{PREFIX}/tags?arn={arn}"), None)
        .await
}

/// `RemoveTags`: `POST /2021-01-01/tags-removal`.
async fn remove_tags(emulator: &Emulator, arn: &str, tag_keys: &[&str]) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/tags-removal"),
            Some(json!({ "ARN": arn, "TagKeys": tag_keys })),
        )
        .await
}

/// Create a domain and hand back the ARN its `DomainStatus` reports, so no test
/// composes an ARN by hand.
pub(crate) async fn domain_arn(emulator: &Emulator, name: &str) -> String {
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain({name}): expected 200, got {} with {:?}",
        created.status, created.body
    );
    string_member(&created.body["DomainStatus"], "ARN", name)
}

/// The `TagList` of a `ListTags` answer as (key, value) pairs sorted by key, so a
/// case compares SETS rather than the order one listing happened to use.
pub(crate) fn reported_tags(answer: &Answer, case: &str) -> Vec<(String, String)> {
    let list = answer.body["TagList"].as_array().unwrap_or_else(|| {
        panic!(
            "case {case}: expected a TagList array, got {:?}",
            answer.body
        )
    });
    let mut tags: Vec<(String, String)> = list
        .iter()
        .map(|tag| {
            let key = tag["Key"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string Key, got {tag:?}"));
            let value = tag["Value"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string Value, got {tag:?}"));
            (key.to_owned(), value.to_owned())
        })
        .collect();
    tags.sort();
    tags
}

/// The tags `arn` carries, read through `ListTags`.
pub(crate) async fn stored_tags(
    emulator: &Emulator,
    arn: &str,
    case: &str,
) -> Vec<(String, String)> {
    let read = list_tags(emulator, arn).await;
    assert_eq!(
        read.status, 200,
        "case {case}: expected 200 from ListTags, got {} with {:?}",
        read.status, read.body
    );
    reported_tags(&read, case)
}

/// A tag list as the wire carries it.
pub(crate) fn wire_tags(tags: &[(String, String)]) -> Value {
    Value::Array(
        tags.iter()
            .map(|(key, value)| json!({ "Key": key, "Value": value }))
            .collect(),
    )
}

/// The same pairs sorted by key: what [`reported_tags`] compares against.
pub(crate) fn as_set(tags: &[(String, String)]) -> Vec<(String, String)> {
    let mut set = tags.to_vec();
    set.sort();
    set
}

/// A case label for a tag list whose values may be hundreds of characters long.
fn label(tags: &[(String, String)]) -> String {
    let keys: Vec<String> = tags
        .iter()
        .map(|(key, _)| key.chars().take(12).collect())
        .collect();
    format!("[{}]", keys.join(","))
}

pub(crate) fn tag(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// How the service words the refusal of a tagging `ARN` that resolves to no
/// resource of this account. Fixed text: the ARN named nothing, so nothing from the
/// request fills it in.
const UNRESOLVABLE_ARN: &str = "Invalid ARN. Domain not found.";

/// The `message` an error answer carries, under either casing a wire path may use.
pub(crate) fn error_message(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .or_else(|| answer.body["Message"].as_str())
        .unwrap_or_default()
        .to_string()
}

/// The tag-list table: one row per way one accepted list differs from another — how
/// many tags it carries, the key and value at each published length bound, the
/// character classes the patterns allow, an empty value, and keys sent in
/// descending order so no row depends on the order the request used.
///
/// `TagKey` is `@length(min: 1, max: 128)` and `TagValue` is `@length(min: 0, max:
/// 256)`, both with pattern `.*`, from this service's Smithy model
/// (`smithy_query(query="shape",shape="Tag")`), so every row below is inside the
/// modelled constraints and the service itself is what accepts it.
fn tag_rows() -> Vec<Vec<(String, String)>> {
    vec![
        // A single ordinary tag.
        vec![tag("project", "trinity")],
        // Several tags at once, two of them sharing a value: "Tag values ... don't
        // have to be unique in a tag set" (`Tag$Value`).
        vec![
            tag("project", "trinity"),
            tag("cost-center", "trinity"),
            tag("env", "prod"),
        ],
        // The shortest key the constraint allows (1) and the empty value it allows
        // (0): "Tag values can be null" (`Tag$Value`).
        vec![tag("k", "")],
        // The longest key (128) and the longest value (256).
        vec![tag(&"k".repeat(128), &"v".repeat(256))],
        // Keys and values spanning the character classes the patterns allow, and
        // case-sensitive keys that differ only in case — "Tags are a set of
        // case-sensitive key-value pairs" (`AddTags`).
        vec![
            tag("Owner", "Team Alpha"),
            tag("owner", "team/alpha:1"),
            tag("aws:emulated", "yes_no.maybe"),
            tag("0123", "9876"),
        ],
        // Keys sent in descending order, so the answer cannot be the request's own
        // ordering echoed back.
        vec![tag("zeta", "1"), tag("mu", "2"), tag("alpha", "3")],
        // Exactly as many tags as a resource may carry.
        (0..TAG_QUOTA)
            .map(|index| tag(&format!("key-{index}"), &format!("value-{index}")))
            .collect(),
    ]
}

#[tokio::test]
async fn add_tags_attaches_the_tags_list_tags_then_reports() {
    for (index, tags) in tag_rows().into_iter().enumerate() {
        let case = format!("AddTags({})", label(&tags));
        let emulator = Emulator::new();
        let arn = domain_arn(&emulator, &format!("tagged-domain-{index}")).await;

        // A domain starts out carrying no tags at all, so the listing is empty
        // rather than a failure.
        assert!(
            stored_tags(&emulator, &arn, &case).await.is_empty(),
            "case {case}: expected no tags before the first AddTags"
        );

        let attached = add_tags(&emulator, &arn, wire_tags(&tags)).await;
        assert_eq!(
            attached.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            attached.status, attached.body
        );
        assert_eq!(
            attached.body,
            Value::Null,
            "case {case}: AddTags answers with an empty body, got {:?}",
            attached.body
        );

        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read,
            as_set(&tags),
            "case {case}: expected the attached set back, got {read:?}"
        );

        // The read reported the set and changed nothing: a second one answers the
        // same.
        let again = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            again, read,
            "case {case}: a read must not change what the next caller sees, got {read:?} then \
             {again:?}"
        );
    }
}

#[tokio::test]
async fn a_domain_created_with_a_tag_list_carries_exactly_that_set() {
    for (index, tags) in tag_rows().into_iter().enumerate() {
        let case = format!("CreateDomain(TagList={})", label(&tags));
        let emulator = Emulator::new();
        let name = format!("created-tagged-{index}");

        let created = emulator
            .create(json!({ "DomainName": name, "TagList": wire_tags(&tags) }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_member(&created.body["DomainStatus"], "ARN", &case);

        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read,
            as_set(&tags),
            "case {case}: expected the created set back, got {read:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_set_holds_one_value_per_key_so_attaching_a_key_again_replaces_it() {
    for (index, tags) in tag_rows().into_iter().enumerate() {
        let case = format!("replace({})", label(&tags));
        let emulator = Emulator::new();
        let name = format!("replaced-tags-{index}");

        // Half the row's keys come from the create and all of them from the
        // `AddTags` that follows, so the overlap is written twice with a different
        // value each time. The replacement values are short markers rather than the
        // row's own values extended, because a row already sits at `TagValue`'s
        // published maximum.
        let from_create: Vec<(String, String)> = tags
            .iter()
            .take(tags.len().div_ceil(2))
            .map(|(key, _)| tag(key, "created"))
            .collect();
        let created = emulator
            .create(json!({ "DomainName": name, "TagList": wire_tags(&from_create) }))
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected 200 from the create, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_member(&created.body["DomainStatus"], "ARN", &case);

        let attached = add_tags(&emulator, &arn, wire_tags(&tags)).await;
        assert_eq!(
            attached.status, 200,
            "case {case}: expected 200 from AddTags, got {} with {:?}",
            attached.status, attached.body
        );

        // Every key carries the value the LAST call that named it gave it, and no
        // key appears twice: the stored set has exactly one entry per distinct key.
        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read,
            as_set(&tags),
            "case {case}: expected the re-attached values to replace the created ones, got \
             {read:?}"
        );

        // One more round: attaching the same keys with yet another value replaces
        // them again rather than growing the set.
        let bumped: Vec<(String, String)> = tags
            .iter()
            .enumerate()
            .map(|(slot, (key, _))| tag(key, &format!("bumped-{slot}")))
            .collect();
        let again = add_tags(&emulator, &arn, wire_tags(&bumped)).await;
        assert_eq!(
            again.status, 200,
            "case {case}: expected 200 from the second AddTags, got {} with {:?}",
            again.status, again.body
        );
        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read,
            as_set(&bumped),
            "case {case}: expected the second set of values, got {read:?}"
        );
    }
}

#[tokio::test]
async fn remove_tags_removes_exactly_the_keys_it_names() {
    // The key-subset table: one row per way a removal differs — no key the resource
    // carries, one it does, several, every one of them, and a key named twice (the
    // set holds one value per key, so naming it twice asks for the same removal).
    let subsets: Vec<Vec<usize>> = vec![
        vec![],
        vec![0],
        vec![1],
        vec![0, 2],
        vec![0, 1, 2, 3],
        vec![1, 1],
    ];

    for (index, keys) in subsets.into_iter().enumerate() {
        let emulator = Emulator::new();
        let name = format!("removed-tags-{index}");
        let stored: Vec<(String, String)> = (0..4)
            .map(|slot| tag(&format!("key-{slot}"), &format!("value-{slot}")))
            .collect();
        let created = emulator
            .create(json!({ "DomainName": name, "TagList": wire_tags(&stored) }))
            .await;
        assert_eq!(
            created.status, 200,
            "case RemoveTags({keys:?}): expected 200 from the create, got {} with {:?}",
            created.status, created.body
        );
        let arn = string_member(&created.body["DomainStatus"], "ARN", &name);

        // One key the resource never carried goes in every request: removal names
        // the tags the resource must no longer have, so a key that is already gone
        // is not a failure.
        let mut named: Vec<String> = keys.iter().map(|slot| format!("key-{slot}")).collect();
        named.push("key-never-attached".to_string());
        let named_refs: Vec<&str> = named.iter().map(String::as_str).collect();
        let case = format!("RemoveTags({named:?})");

        let removed = remove_tags(&emulator, &arn, &named_refs).await;
        assert_eq!(
            removed.status, 200,
            "case {case}: expected 200, got {} with {:?}",
            removed.status, removed.body
        );
        assert_eq!(
            removed.body,
            Value::Null,
            "case {case}: RemoveTags answers with an empty body, got {:?}",
            removed.body
        );

        let expected: Vec<(String, String)> = stored
            .iter()
            .filter(|(key, _)| !named.iter().any(|removed| removed == key))
            .cloned()
            .collect();
        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read,
            as_set(&expected),
            "case {case}: expected exactly the unnamed tags to remain, got {read:?}"
        );

        // Removing the same keys again is the same request on a set they are
        // already absent from.
        let repeated = remove_tags(&emulator, &arn, &named_refs).await;
        assert_eq!(
            repeated.status, 200,
            "case {case}: expected 200 from the repeat, got {} with {:?}",
            repeated.status, repeated.body
        );
        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read,
            as_set(&expected),
            "case {case}: expected the repeat to change nothing, got {read:?}"
        );
    }
}

#[tokio::test]
async fn tags_belong_to_the_resource_the_arn_names() {
    let emulator = Emulator::new();
    let first = domain_arn(&emulator, "tag-scope-first").await;
    let second = domain_arn(&emulator, "tag-scope-second").await;

    add_tags(
        &emulator,
        &first,
        wire_tags(&[tag("owner", "first"), tag("env", "prod")]),
    )
    .await;
    add_tags(&emulator, &second, wire_tags(&[tag("owner", "second")])).await;

    assert_eq!(
        stored_tags(&emulator, &first, "first").await,
        as_set(&[tag("env", "prod"), tag("owner", "first")]),
        "case first: a tag written to one domain must not reach the other"
    );
    assert_eq!(
        stored_tags(&emulator, &second, "second").await,
        as_set(&[tag("owner", "second")]),
        "case second: a tag written to one domain must not reach the other"
    );

    // Removing a key from one resource leaves the other resource's tag under that
    // same key in place.
    remove_tags(&emulator, &first, &["owner"]).await;
    assert_eq!(
        stored_tags(&emulator, &first, "first after removal").await,
        as_set(&[tag("env", "prod")]),
        "case first after removal: expected only the key that was not named"
    );
    assert_eq!(
        stored_tags(&emulator, &second, "second after removal").await,
        as_set(&[tag("owner", "second")]),
        "case second after removal: the other resource's tag must be untouched"
    );
}

#[tokio::test]
async fn an_arn_that_names_no_resource_is_refused_by_every_tagging_operation() {
    let emulator = Emulator::new();

    // A domain that existed and was deleted: its ARN names nothing once the record
    // is released.
    let deleted = domain_arn(&emulator, "deleted-tag-target").await;
    add_tags(&emulator, &deleted, wire_tags(&[tag("env", "prod")])).await;
    let dropped = emulator
        .call(
            "DELETE",
            &format!("{PREFIX}/opensearch/domain/deleted-tag-target"),
            None,
        )
        .await;
    assert_eq!(
        dropped.status, 200,
        "case DeleteDomain: expected 200, got {} with {:?}",
        dropped.status, dropped.body
    );

    // One existing domain, so every row below differs from an ARN that DOES
    // resolve only in the resource it names.
    let live = domain_arn(&emulator, "live-tag-target").await;
    let other_region = live.replace(":us-east-1:", ":eu-west-1:");
    let never_created = live.replace("live-tag-target", "never-created-domain");
    let application = live.replace("domain/live-tag-target", "application/some-app");
    let data_source = live.replace(
        "domain/live-tag-target",
        "datasource/live-tag-target/some-source",
    );

    let unresolvable = [
        ("a deleted domain", deleted.as_str()),
        ("a domain that was never created", never_created.as_str()),
        ("the same name in another Region", other_region.as_str()),
        ("an application", application.as_str()),
        ("a data source", data_source.as_str()),
    ];

    for (what, arn) in unresolvable {
        let case = format!("{what}: {arn}");

        // `ValidationException` is the only not-found refusal all three tagging
        // operations model — the two writers declare no `ResourceNotFoundException`
        // at all — so the reader and the writers of one tag set answer a miss on it
        // identically, down to the wording.
        for (operation, answer) in [
            (
                "AddTags",
                add_tags(&emulator, arn, wire_tags(&[tag("env", "prod")])).await,
            ),
            ("RemoveTags", remove_tags(&emulator, arn, &["env"]).await),
            ("ListTags", list_tags(&emulator, arn).await),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {operation}({case}): expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {operation}({case}): expected ValidationException, got {:?}",
                answer.body
            );
            assert_eq!(
                error_message(&answer),
                UNRESOLVABLE_ARN,
                "case {operation}({case}): expected the unresolvable-ARN wording, got {:?}",
                answer.body
            );
        }
    }

    // The domain that does exist is untouched by every refusal above.
    assert!(
        stored_tags(&emulator, &live, "live").await.is_empty(),
        "case live: a refused call must not write to a resource it did not name"
    );
}

#[tokio::test]
async fn a_value_that_is_not_an_arn_is_refused_as_a_malformed_request() {
    let emulator = Emulator::new();
    let arn = domain_arn(&emulator, "malformed-arn-target").await;
    add_tags(&emulator, &arn, wire_tags(&[tag("env", "prod")])).await;

    // `ARN` is `@length(min: 20, max: 2048)` with pattern `.*`, so each row is long
    // enough to pass the modelled constraint and the service itself is what refuses
    // the syntax.
    let malformed = [
        ("no fields at all", "x".repeat(20)),
        ("too few fields", "arn:aws:es:us-east-1:1".to_string()),
        (
            "not the arn literal",
            "urn:aws:es:us-east-1:123456789012:domain/d".to_string(),
        ),
        (
            "an empty partition",
            "arn::es:us-east-1:123456789012:domain/d".to_string(),
        ),
        (
            "an empty service",
            "arn:aws::us-east-1:123456789012:domain/d".to_string(),
        ),
        (
            "an empty resource",
            "arn:aws:es:us-east-1:123456789012:".to_string(),
        ),
    ];

    for (what, value) in malformed {
        let case = format!("{what}: {value}");

        for (operation, answer) in [
            (
                "AddTags",
                add_tags(&emulator, &value, wire_tags(&[tag("env", "dev")])).await,
            ),
            ("RemoveTags", remove_tags(&emulator, &value, &["env"]).await),
            ("ListTags", list_tags(&emulator, &value).await),
        ] {
            assert_eq!(
                answer.status, 400,
                "case {operation}({case}): expected 400, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {operation}({case}): expected ValidationException, got {:?}",
                answer.body
            );
        }
    }

    // A tag key or value outside its published length bound is refused at the
    // request boundary, and nothing is written.
    let past_bounds = [
        ("a key one past the published maximum", "k".repeat(129), "v".to_string()),
        ("an empty key", String::new(), "v".to_string()),
        (
            "a value one past the published maximum",
            "k".to_string(),
            "v".repeat(257),
        ),
    ];
    for (what, key, value) in past_bounds {
        let case = format!("{what}: key {} chars, value {} chars", key.len(), value.len());
        let answer = add_tags(&emulator, &arn, wire_tags(&[tag(&key, &value)])).await;
        assert_eq!(
            answer.status, 400,
            "case AddTags({case}): expected 400 outside the constraint, got {} with {:?}",
            answer.status, answer.body
        );
    }

    // Every refusal above left the one tag this domain carries exactly as it was.
    let read = stored_tags(&emulator, &arn, "malformed-arn-target").await;
    assert_eq!(
        read,
        as_set(&[tag("env", "prod")]),
        "case malformed-arn-target: a refused call must write nothing, got {read:?}"
    );
}

#[tokio::test]
async fn a_resource_carries_at_most_the_documented_number_of_tags() {
    let emulator = Emulator::new();
    let arn = domain_arn(&emulator, "quota-tag-target").await;

    // One tag at a time up to the quota: each is accepted and the set grows by one.
    for index in 0..TAG_QUOTA {
        let case = format!("AddTags(key-{index})");
        let answer = add_tags(
            &emulator,
            &arn,
            wire_tags(&[tag(&format!("key-{index}"), "v")]),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200 inside the quota, got {} with {:?}",
            answer.status, answer.body
        );
        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read.len(),
            index + 1,
            "case {case}: expected {} tags, got {read:?}",
            index + 1
        );
    }

    let full = stored_tags(&emulator, &arn, "full").await;

    // One past the quota, named by a single call and by a call carrying several
    // tags at once: both are refused and neither writes.
    for (what, tags) in [
        (
            "one tag past the quota",
            vec![tag("one-too-many", "v")],
        ),
        (
            "several tags, one of them past the quota",
            vec![tag("key-0", "replaced"), tag("also-too-many", "v")],
        ),
    ] {
        let case = format!("AddTags({what})");
        let answer = add_tags(&emulator, &arn, wire_tags(&tags)).await;
        assert_eq!(
            answer.status, 409,
            "case {case}: expected 409 past the quota, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "LimitExceededException",
            "case {case}: expected LimitExceededException, got {:?}",
            answer.body
        );
        let read = stored_tags(&emulator, &arn, &case).await;
        assert_eq!(
            read, full,
            "case {case}: a refused call must leave the stored set as it was, got {read:?}"
        );
    }

    // Replacing the value of a key the resource already carries is inside the
    // quota: the set does not grow.
    let replaced = add_tags(&emulator, &arn, wire_tags(&[tag("key-0", "replaced")])).await;
    assert_eq!(
        replaced.status, 200,
        "case AddTags(key-0 again): expected 200, got {} with {:?}",
        replaced.status, replaced.body
    );
    let read = stored_tags(&emulator, &arn, "key-0 again").await;
    assert_eq!(
        read.len(),
        TAG_QUOTA,
        "case AddTags(key-0 again): expected the set to stay at the quota, got {read:?}"
    );

    // Freeing a slot lets the next tag in.
    remove_tags(&emulator, &arn, &["key-0"]).await;
    let accepted = add_tags(&emulator, &arn, wire_tags(&[tag("one-too-many", "v")])).await;
    assert_eq!(
        accepted.status, 200,
        "case AddTags(after a removal): expected 200, got {} with {:?}",
        accepted.status, accepted.body
    );

    // The same bound holds on the `TagList` a create carries: exactly the quota is
    // accepted, one past it is refused and no domain is created.
    let at_quota: Vec<(String, String)> = (0..TAG_QUOTA)
        .map(|index| tag(&format!("key-{index}"), "v"))
        .collect();
    let created = emulator
        .create(json!({ "DomainName": "quota-create-ok", "TagList": wire_tags(&at_quota) }))
        .await;
    assert_eq!(
        created.status, 200,
        "case CreateDomain(at the quota): expected 200, got {} with {:?}",
        created.status, created.body
    );

    let mut past_quota = at_quota.clone();
    past_quota.push(tag("one-too-many", "v"));
    let refused = emulator
        .create(json!({ "DomainName": "quota-create-refused", "TagList": wire_tags(&past_quota) }))
        .await;
    assert_eq!(
        refused.status, 409,
        "case CreateDomain(past the quota): expected 409, got {} with {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        error_code(&refused),
        "LimitExceededException",
        "case CreateDomain(past the quota): expected LimitExceededException, got {:?}",
        refused.body
    );
    let described = emulator.describe("quota-create-refused").await;
    assert_eq!(
        described.status, RECORDED_NOT_FOUND_STATUS,
        "case CreateDomain(past the quota): the refused create must persist no domain, got {} \
         with {:?}",
        described.status, described.body
    );
}
