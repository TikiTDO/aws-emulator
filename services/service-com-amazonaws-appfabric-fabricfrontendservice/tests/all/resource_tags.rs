// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for AppFabric's resource-tagging surface (`TagResource`,
//! `ListTagsForResource`, `UntagResource`), driven over the wire (restJson1)
//! through the same router the emulator boots.
//!
//! Each test states a RULE over a table of inputs rather than one recorded call:
//! a tag set is a map of key-value pairs held on the resource its ARN names,
//! `TagResource` merges into it, `UntagResource` removes exactly the named keys,
//! `ListTagsForResource` reports it without changing it, and an ARN no stored
//! resource carries is refused the same way whichever of the three is called.
//!
//! The harness (the router, `Answer`, `encode`, `arn_of`) is the one
//! `app_bundle_ingestion_lifecycle` already builds; only the tagging calls are
//! new here.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::app_bundle_ingestion_lifecycle::{arn_of, encode, Answer, Emulator};

/// A tag set as a table row writes it: key-value pairs in the order given.
pub(crate) type Tags<'a> = Vec<(&'a str, &'a str)>;

/// One row of the merge table: the case, the tags the resource was created with,
/// the tags the call assigns, and the set that must result.
type MergeCase<'a> = (&'a str, Tags<'a>, Tags<'a>, Tags<'a>);

/// One row of the untag table: the case, the tags held, the keys removed, and
/// what must be left.
type UntagCase<'a> = (&'a str, Tags<'a>, Vec<&'a str>, Tags<'a>);

/// `POST /tags/{resourceArn}` — `tags` is the request body's only member.
pub(crate) async fn tag_resource(emulator: &Emulator, arn: &str, tags: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("/tags/{}", encode(arn)),
            Some(json!({"tags": tags})),
        )
        .await
}

/// `GET /tags/{resourceArn}`.
pub(crate) async fn list_tags(emulator: &Emulator, arn: &str) -> Answer {
    emulator
        .call("GET", &format!("/tags/{}", encode(arn)), None)
        .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `tagKeys` is an `@httpQuery` list,
/// which an AWS SDK renders as the parameter repeated once per key.
pub(crate) async fn untag_resource(emulator: &Emulator, arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect::<Vec<String>>()
        .join("&");
    emulator
        .call(
            "DELETE",
            &format!("/tags/{}?{query}", encode(arn)),
            None,
        )
        .await
}

/// The tag set a `ListTagsForResource` answer reports, as a map: a tag list is a
/// map of key-value pairs, so these tests compare it as one rather than pinning
/// an order the service does not document.
pub(crate) fn reported_tags(answer: &Answer, case: &str) -> BTreeMap<String, String> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} with body {}",
        answer.status, answer.body
    );
    let tags = answer
        .body
        .get("tags")
        .unwrap_or_else(|| panic!("case {case:?}: expected a tags member, got {}", answer.body));
    let listed = tags
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected tags to be a list, got {tags}"));
    let mut mapped = BTreeMap::new();
    for tag in listed {
        let key = tag
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a tag with no key: {tag}"));
        let value = tag
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: a tag with no value: {tag}"));
        let previous = mapped.insert(key.to_string(), value.to_string());
        assert!(
            previous.is_none(),
            "case {case:?}: key {key:?} reported twice in {tags}"
        );
    }
    mapped
}

/// A tag list as the wire carries it, from a map.
pub(crate) fn tag_list(pairs: &[(&str, &str)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"key": key, "value": value}))
            .collect(),
    )
}

/// The expected map for the same pairs.
pub(crate) fn expected(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// Every resource kind this service persists, each created on a fresh emulator:
/// the rules below hold whichever kind the ARN names, so every table runs over
/// this list. (Ingestion destinations are not in it because the operation that
/// mints them is still scaffolded, so no such record can exist yet.)
pub(crate) async fn taggable_resources(
    emulator: &Emulator,
    create_tags: Value,
) -> Vec<(&'static str, String)> {
    let bundle = arn_of(
        &emulator
            .create_app_bundle(json!({"tags": create_tags.clone()}))
            .await
            .body,
        "appBundle",
    );
    let ingestion = arn_of(
        &emulator
            .create_ingestion(
                &bundle,
                json!({"app": "SLACK",
                       "tenantId": "tenant-1",
                       "ingestionType": "auditLog",
                       "tags": create_tags}),
            )
            .await
            .body,
        "ingestion",
    );
    vec![("app bundle", bundle), ("ingestion", ingestion)]
}

/// RULE: `TagResource` assigns the given key-value pairs to the resource its ARN
/// names and persists them there, so `ListTagsForResource` reports exactly the
/// resulting map — for every resource kind, and whatever the shape of the tag
/// set.
///
/// The table spans the tag sets the model allows: none, one, several, an empty
/// value, and keys and values at their published maximum lengths (`Tag$key`
/// 1..128 and `Tag$value` 0..256, from
/// `smithy_query(query="shape", shape="Tag")`).
#[tokio::test]
async fn tagging_a_resource_persists_exactly_the_given_tag_set() {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    let cases: Vec<(&str, Tags)> = vec![
        ("no tags", vec![]),
        ("one tag", vec![("env", "prod")]),
        (
            "three tags",
            vec![("env", "prod"), ("team", "data"), ("tier", "1")],
        ),
        ("an empty value", vec![("marker", "")]),
        (
            "a key and a value at their maximum length",
            vec![(longest_key.as_str(), longest_value.as_str())],
        ),
        (
            "values carrying the characters a tag value allows",
            vec![("path", "a/b c:d+e=f_g.h@i")],
        ),
    ];

    for (case, pairs) in cases {
        let emulator = Emulator::new();
        for (kind, arn) in taggable_resources(&emulator, json!([])).await {
            let case = format!("{case} on an {kind}");
            let tagged = tag_resource(&emulator, &arn, tag_list(&pairs)).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: expected HTTP 200 from TagResource, got {} with body {}",
                tagged.status, tagged.body
            );

            let listed = list_tags(&emulator, &arn).await;
            assert_eq!(
                reported_tags(&listed, &case),
                expected(&pairs),
                "case {case:?}: ListTagsForResource must report exactly the tags TagResource wrote, got {}",
                listed.body
            );
        }
    }
}

/// RULE: the tags a create call stored and the tags `TagResource` assigns are ONE
/// tag set on the resource — a later tag naming a key already held replaces that
/// key's value instead of adding a second entry for it, and leaves every other
/// key alone.
///
/// (AWS documents the replacement: "If you specify a tag key that is already
/// associated with the resource, the new tag value that you specify replaces the
/// previous value".)
#[tokio::test]
async fn tagging_an_existing_key_replaces_its_value() {
    // (case, tags the resource was created with, tags the call assigns, result)
    let cases: Vec<MergeCase> = vec![
        (
            "a new key is added beside the created one",
            vec![("env", "prod")],
            vec![("team", "data")],
            vec![("env", "prod"), ("team", "data")],
        ),
        (
            "a held key takes the new value",
            vec![("env", "prod")],
            vec![("env", "staging")],
            vec![("env", "staging")],
        ),
        (
            "one of several keys is replaced",
            vec![("env", "prod"), ("team", "data")],
            vec![("env", "dev")],
            vec![("env", "dev"), ("team", "data")],
        ),
        (
            "a replacement and an addition together",
            vec![("env", "prod"), ("team", "data")],
            vec![("env", "dev"), ("tier", "2")],
            vec![("env", "dev"), ("team", "data"), ("tier", "2")],
        ),
        (
            "a key is replaced by the empty value",
            vec![("env", "prod")],
            vec![("env", "")],
            vec![("env", "")],
        ),
    ];

    for (case, created, assigned, result) in cases {
        let emulator = Emulator::new();
        for (kind, arn) in taggable_resources(&emulator, tag_list(&created)).await {
            let case = format!("{case} on an {kind}");
            // The create call's tags are the starting set, which pins the create
            // path and the tagging path to the same store.
            let listed = list_tags(&emulator, &arn).await;
            assert_eq!(
                reported_tags(&listed, &case),
                expected(&created),
                "case {case:?}: the tags the create call stored must be reported, got {}",
                listed.body
            );

            let tagged = tag_resource(&emulator, &arn, tag_list(&assigned)).await;
            assert_eq!(
                tagged.status, 200,
                "case {case:?}: expected HTTP 200 from TagResource, got {} with body {}",
                tagged.status, tagged.body
            );
            let listed = list_tags(&emulator, &arn).await;
            assert_eq!(
                reported_tags(&listed, &case),
                expected(&result),
                "case {case:?}: expected the merged tag set, got {}",
                listed.body
            );
        }
    }
}

/// RULE: `UntagResource` removes exactly the keys it names and nothing else; a
/// key the resource does not hold is removed by doing nothing, so the operation
/// is idempotent (`@idempotent` in the model) and repeating it answers the same
/// way.
#[tokio::test]
async fn untagging_removes_exactly_the_named_keys() {
    // (case, tags held, keys removed, what is left)
    let cases: Vec<UntagCase> = vec![
        (
            "one of three keys",
            vec![("env", "prod"), ("team", "data"), ("tier", "1")],
            vec!["team"],
            vec![("env", "prod"), ("tier", "1")],
        ),
        (
            "every key",
            vec![("env", "prod"), ("team", "data")],
            vec!["env", "team"],
            vec![],
        ),
        (
            "a key the resource never held",
            vec![("env", "prod")],
            vec!["absent"],
            vec![("env", "prod")],
        ),
        (
            "a held key and an absent one together",
            vec![("env", "prod"), ("team", "data")],
            vec!["team", "absent"],
            vec![("env", "prod")],
        ),
        (
            "a key whose value is empty",
            vec![("marker", ""), ("env", "prod")],
            vec!["marker"],
            vec![("env", "prod")],
        ),
        (
            "a key differing only in case is not the same key",
            vec![("Env", "prod"), ("env", "dev")],
            vec!["env"],
            vec![("Env", "prod")],
        ),
    ];

    for (case, held, removed, left) in cases {
        let emulator = Emulator::new();
        for (kind, arn) in taggable_resources(&emulator, json!([])).await {
            let case = format!("{case} on an {kind}");
            tag_resource(&emulator, &arn, tag_list(&held)).await;

            // Twice: the second call is the idempotence check — same status, same
            // resulting tag set.
            for attempt in 1..=2 {
                let untagged = untag_resource(&emulator, &arn, &removed).await;
                assert_eq!(
                    untagged.status, 200,
                    "case {case:?} attempt {attempt}: expected HTTP 200 from UntagResource, got {} with body {}",
                    untagged.status, untagged.body
                );
                let listed = list_tags(&emulator, &arn).await;
                assert_eq!(
                    reported_tags(&listed, &case),
                    expected(&left),
                    "case {case:?} attempt {attempt}: expected only the untouched keys to remain, got {}",
                    listed.body
                );
            }
        }
    }
}

/// RULE: a tag set belongs to ONE resource — tagging or untagging the resource an
/// ARN names changes no other resource's tags, not even its parent's or its
/// child's.
#[tokio::test]
async fn a_tag_set_belongs_to_one_resource() {
    let emulator = Emulator::new();
    let resources = taggable_resources(&emulator, json!([])).await;
    // A second bundle, so the table also spans two resources of the SAME kind.
    // It lives in its own Region, because a Region holds one app bundle.
    let other_bundle = emulator.fixture_bundle(1).await;

    let mut arns: Vec<(String, String)> = resources
        .iter()
        .map(|(kind, arn)| ((*kind).to_string(), arn.clone()))
        .collect();
    arns.push(("second app bundle".to_string(), other_bundle));

    // Each resource gets a tag naming itself, so a leak between them is visible.
    for (index, (kind, arn)) in arns.iter().enumerate() {
        let case = format!("tagging the {kind}");
        let tagged = tag_resource(&emulator, arn, tag_list(&[("owner", &format!("r{index}"))])).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected HTTP 200 from TagResource, got {} with body {}",
            tagged.status, tagged.body
        );
    }
    for (index, (kind, arn)) in arns.iter().enumerate() {
        let case = format!("the {kind} keeps only its own tag");
        let listed = list_tags(&emulator, arn).await;
        assert_eq!(
            reported_tags(&listed, &case),
            expected(&[("owner", &format!("r{index}"))]),
            "case {case:?}: expected only this resource's own tag, got {}",
            listed.body
        );
    }

    // Untagging one resource leaves the others as they were.
    let (_, first) = &arns[0];
    untag_resource(&emulator, first, &["owner"]).await;
    for (index, (kind, arn)) in arns.iter().enumerate() {
        let case = format!("the {kind} after untagging the first resource");
        let listed = list_tags(&emulator, arn).await;
        let still = if index == 0 {
            BTreeMap::new()
        } else {
            expected(&[("owner", &format!("r{index}"))])
        };
        assert_eq!(
            reported_tags(&listed, &case),
            still,
            "case {case:?}: untagging one resource must not change another's tags, got {}",
            listed.body
        );
    }
}

/// RULE: `ListTagsForResource` is a REPORT (`@readonly`) — reading a resource's
/// tags, however often, changes nothing a later caller sees.
#[tokio::test]
async fn listing_tags_changes_nothing() {
    let emulator = Emulator::new();
    for (kind, arn) in taggable_resources(&emulator, tag_list(&[("env", "prod")])).await {
        let case = format!("repeated reads of an {kind}'s tags");
        let first = reported_tags(&list_tags(&emulator, &arn).await, &case);
        for read in 2..=4 {
            let again = list_tags(&emulator, &arn).await;
            assert_eq!(
                reported_tags(&again, &case),
                first,
                "case {case:?}: read {read} must report what read 1 did, got {}",
                again.body
            );
        }
    }
}

/// RULE: a `resourceArn` that names no resource this service holds is refused
/// with `ResourceNotFoundException` (HTTP 404) — by every one of the three
/// tagging operations, whatever the ARN looks like. That is what distinguishes a
/// resource holding no tags (HTTP 200, an empty list) from one that does not
/// exist.
#[tokio::test]
async fn an_unknown_resource_arn_is_not_found() {
    let emulator = Emulator::new();
    let live_bundle = emulator.fixture_bundle(0).await;
    // In its own Region, so both bundles can exist at once.
    let deleted_bundle = emulator.fixture_bundle(1).await;
    let deleted_ingestion = arn_of(
        &emulator
            .create_ingestion(
                &deleted_bundle,
                json!({"app": "SLACK", "tenantId": "tenant-1", "ingestionType": "auditLog"}),
            )
            .await
            .body,
        "ingestion",
    );
    // The order AppFabric documents for taking a bundle apart: the ingestion,
    // then the app authorization it depends on, then the bundle itself — which
    // is refused for as long as the bundle holds an authorization.
    let removed_ingestion = emulator
        .delete_ingestion(&deleted_bundle, &deleted_ingestion)
        .await;
    assert_eq!(
        removed_ingestion.status, 204,
        "expected HTTP 204 from DeleteIngestion, got {} with body {}",
        removed_ingestion.status, removed_ingestion.body
    );
    emulator.delete_app_authorizations(&deleted_bundle).await;
    let removed = emulator
        .call(
            "DELETE",
            &format!("/appbundles/{}", encode(&deleted_bundle)),
            None,
        )
        .await;
    assert_eq!(
        removed.status, 204,
        "expected HTTP 204 from DeleteAppBundle, got {} with body {}",
        removed.status, removed.body
    );

    // A UUID-shaped bundle ARN this emulator never minted, built on a live ARN
    // so only the id differs.
    let unminted = live_bundle.replace(
        live_bundle.rsplit('/').next().unwrap_or_default(),
        "ffffffff-ffff-4fff-8fff-ffffffffffff",
    );

    let cases: Vec<(&str, &str)> = vec![
        ("a deleted app bundle", deleted_bundle.as_str()),
        (
            "an ingestion whose bundle was deleted",
            deleted_ingestion.as_str(),
        ),
        ("an app bundle ARN never minted", unminted.as_str()),
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket",
        ),
        (
            "an appfabric ARN of an unknown resource type",
            "arn:aws:appfabric:us-east-1:000000000000:widget/1",
        ),
    ];

    for (case, arn) in cases {
        for (operation, answer) in [
            ("TagResource", tag_resource(&emulator, arn, tag_list(&[("env", "prod")])).await),
            ("ListTagsForResource", list_tags(&emulator, arn).await),
            ("UntagResource", untag_resource(&emulator, arn, &["env"]).await),
        ] {
            let case = format!("{operation} on {case}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected HTTP 404, got {} with body {}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {}",
                answer.body
            );
        }
    }

    // And the live resource still answers, so the refusals above are about the
    // ARN and not about the operation being unavailable.
    let listed = list_tags(&emulator, &live_bundle).await;
    assert_eq!(
        reported_tags(&listed, "a live app bundle with no tags"),
        BTreeMap::new(),
        "a resource holding no tags must report an empty list, got {}",
        listed.body
    );
}

/// RULE: a resource holds at most as many tags as the modelled `TagList` can
/// report (`@length(min: 0, max: 50)` on `TagList`, which is also
/// `ListTagsForResource`'s output member), so a merge that would take it past
/// that bound is refused with `ValidationException` (HTTP 400) and changes
/// nothing.
///
/// The table runs the published bound and one past it.
#[tokio::test]
async fn a_merge_past_the_modelled_tag_bound_is_refused() {
    use service_com_amazonaws_appfabric_fabricfrontendservice::state::MAX_TAGS_PER_RESOURCE;

    let emulator = Emulator::new();
    for (kind, arn) in taggable_resources(&emulator, json!([])).await {
        let case = format!("an {kind} at the tag bound");
        // Exactly at the bound: accepted, and all of them reported.
        let keys: Vec<String> = (0..MAX_TAGS_PER_RESOURCE)
            .map(|index| format!("key-{index:02}"))
            .collect();
        let at_bound: Vec<(&str, &str)> = keys.iter().map(|key| (key.as_str(), "held")).collect();
        let tagged = tag_resource(&emulator, &arn, tag_list(&at_bound)).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: a tag set exactly at the bound must be accepted, got {} with body {}",
            tagged.status, tagged.body
        );
        let full = reported_tags(&list_tags(&emulator, &arn).await, &case);
        assert_eq!(
            full.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: expected {MAX_TAGS_PER_RESOURCE} tags held, got {}",
            full.len()
        );

        // One past it: refused, with the resource's tags untouched.
        let case = format!("an {kind} one tag past the bound");
        let refused = tag_resource(&emulator, &arn, tag_list(&[("one-too-many", "x")])).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            refused.status, refused.body
        );
        assert!(
            refused.error_code().ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {}",
            refused.body
        );
        let after = reported_tags(&list_tags(&emulator, &arn).await, &case);
        assert_eq!(
            after, full,
            "case {case:?}: a refused merge must leave every tag as it was, got {after:?}"
        );

        // Replacing a value at the bound is still accepted: the merged set is no
        // larger, so the bound is about the resulting size and not the request.
        let case = format!("an {kind} replacing a value at the bound");
        let replaced = tag_resource(&emulator, &arn, tag_list(&[(keys[0].as_str(), "new")])).await;
        assert_eq!(
            replaced.status, 200,
            "case {case:?}: expected HTTP 200, got {} with body {}",
            replaced.status, replaced.body
        );
        let after = reported_tags(&list_tags(&emulator, &arn).await, &case);
        assert_eq!(
            after.get(&keys[0]).map(String::as_str),
            Some("new"),
            "case {case:?}: expected the replaced value, got {after:?}"
        );
        assert_eq!(
            after.len(),
            MAX_TAGS_PER_RESOURCE,
            "case {case:?}: a replacement must not grow the tag set, got {}",
            after.len()
        );
    }
}
