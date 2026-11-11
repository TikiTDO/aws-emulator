// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UntagResource`, driven over the wire through the same front
//! door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses):
//! `MultiServiceRouter::dispatch` with a restJson1 request, exactly as a signed
//! `aws outposts untag-resource` call arrives.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `UntagResource` REMOVES from the tags of the resource its `ResourceArn` names
//!    exactly the keys its `TagKeys` list names — dropping a key the resource carries,
//!    ignoring one it does not, and leaving every key the list does not name alone —
//!    and that is what every later reader of the resource answers, whichever taggable
//!    resource kind the ARN names. The call states which keys must be GONE, so sending
//!    it again changes nothing further.
//! 2. A `ResourceArn` that is readable but names no resource this store holds is the
//!    modelled `NotFoundException` (HTTP 404), worded by the tagging API's fixed
//!    not-found sentence, and nothing is created or changed to satisfy it.
//! 3. A removal can only shrink the tag map, so it is never refused for the modelled
//!    `TagMap` maximum: a request at the modelled `TagKeyList` maximum of 50 keys is
//!    accepted, and a resource every one of whose keys is removed is left carrying no
//!    tags at all (an empty map, not a missing resource).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::error_classification::{reported_tags, tag_map};
use crate::site_and_outpost_lifecycle::{query_value, segment, text, Outposts, HOME_REGION};

/// The account id every generated emulator presents, which its ARNs carry.
const ACCOUNT_ID: &str = "000000000000";

/// How many keys one `UntagResource` request may name: `TagKeyList`'s
/// `@length(min: 1, max: 50)`, from
/// `smithy_query(query="shape", shape="TagKeyList")`.
const TAG_KEY_LIST_MAX: usize = 50;

/// The modelled maximum length of a `TagKey` (`@length(min: 1, max: 128)`), from the
/// same query.
const TAG_KEY_MAX: usize = 128;

/// A region other than [`HOME_REGION`], for the regional half of rule 2: a resource
/// belongs to its own region's inventory and to no other's.
const OTHER_REGION: &str = "us-west-2";

/// `DELETE /tags/{ResourceArn}?tagKeys=…`, with one `tagKeys` parameter per key —
/// which is how a list-typed `@httpQuery` member travels. Each key is
/// percent-encoded, because the modelled `TagKey` pattern admits characters a query
/// string reads otherwise (`+` as a space, `/` and `:` as delimiters).
fn untag_path(arn: &str, keys: &[String]) -> String {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    format!("/tags/{}?{}", segment(arn), query.join("&"))
}

/// The tags a resource is left carrying after the named keys are removed: what rule 1
/// requires of every reader.
fn without(stored: &BTreeMap<String, String>, removed: &[String]) -> BTreeMap<String, String> {
    stored
        .iter()
        .filter(|(key, _)| !removed.contains(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// The tag set every rule-1 row starts from, as a request map.
///
/// It spans the forms a `TagKey` can take: ordinary keys, every character class the
/// modelled pattern `^(?!aws:)[a-zA-Z+-=._:/]+$` admits, a one-character key, and a
/// key at the modelled maximum length — so a removal that mangled a key on the way in
/// (a query string read as form data, say) fails here rather than silently removing
/// nothing.
fn fixture_tags() -> Value {
    let mut tags = serde_json::Map::new();
    for (key, value) in [
        ("Env", "prod"),
        ("team", "platform"),
        ("cost.center", "4711"),
        ("a+b", "plus"),
        ("a-b", "dash"),
        ("a.b", "dot"),
        ("a_b", "underscore"),
        ("a:b", "colon"),
        ("a/b", "slash"),
        ("A1", "alnum"),
        ("k", "v"),
    ] {
        tags.insert(key.to_string(), json!(value));
    }
    tags.insert("K".repeat(TAG_KEY_MAX), json!("at the maximum"));
    tags.into()
}

/// One taggable resource to exercise the rules against: how a case names it, the ARN
/// the tagging routes are called with, and the route a single read of it goes to,
/// with the member that read answers the resource under.
struct Taggable {
    case: &'static str,
    arn: String,
    read: String,
    projection: &'static str,
}

/// One resource of every taggable kind this service holds.
async fn taggables(outposts: &Outposts) -> Vec<Taggable> {
    let site = outposts
        .create_site(&json!({ "Name": "untagging site" }))
        .await;
    let site_id = text(&site, "SiteId");
    let outpost = outposts
        .create_outpost(&json!({ "Name": "untagging outpost", "SiteId": site_id }))
        .await;

    vec![
        Taggable {
            case: "site",
            arn: text(&site, "SiteArn"),
            read: format!("/sites/{site_id}"),
            projection: "Site",
        },
        Taggable {
            case: "outpost",
            arn: text(&outpost, "OutpostArn"),
            read: format!("/outposts/{}", text(&outpost, "OutpostId")),
            projection: "Outpost",
        },
    ]
}

/// Put the resource back at `tags` (`TagResource` merges, so a key an earlier row
/// removed comes back), and answer the map it now carries.
async fn restore(outposts: &Outposts, arn: &str, tags: &Value) -> BTreeMap<String, String> {
    outposts
        .no_content(
            "POST",
            &format!("/tags/{}", segment(arn)),
            Some(&json!({ "Tags": tags })),
        )
        .await;
    tag_map(tags)
}

/// Rule 1, over every taggable resource kind and the ways one request's key list can
/// relate to the tags the resource carries.
///
/// The rows differ along the NUMBER of keys named (one, several, every key the
/// resource has), whether each named key is carried or not (carried, absent, and a
/// mixture), and the FORM of the keys (the character classes the modelled pattern
/// admits, a one-character key, a key at the modelled maximum length). Each row starts
/// from the same restored tag set, and is then sent a SECOND time: the call states
/// which keys must be gone, so repeating it must change nothing further.
#[tokio::test]
async fn untag_resource_removes_exactly_the_named_keys_for_every_reader() {
    let outposts = Outposts::new();
    let fixture = fixture_tags();
    let every_key: Vec<String> = tag_map(&fixture).keys().cloned().collect();

    for taggable in taggables(&outposts).await {
        let rows: Vec<(&str, Vec<String>)> = vec![
            ("one key the resource carries", vec!["Env".to_string()]),
            (
                "several keys the resource carries, at once",
                vec!["team".to_string(), "cost.center".to_string()],
            ),
            (
                "a key the resource does not carry",
                vec!["never-applied".to_string()],
            ),
            (
                "one carried key and one absent key, in one request",
                vec!["Env".to_string(), "never-applied".to_string()],
            ),
            (
                "every character class the modelled key pattern admits",
                vec![
                    "a+b".to_string(),
                    "a-b".to_string(),
                    "a.b".to_string(),
                    "a_b".to_string(),
                    "a:b".to_string(),
                    "a/b".to_string(),
                    "A1".to_string(),
                ],
            ),
            ("a one-character key", vec!["k".to_string()]),
            (
                "a key at the modelled maximum length",
                vec!["K".repeat(TAG_KEY_MAX)],
            ),
            ("every key the resource carries", every_key.clone()),
        ];

        for (row, keys) in &rows {
            let case = format!("{}: {row}", taggable.case);
            let stored = restore(&outposts, &taggable.arn, &fixture).await;
            let expected = without(&stored, keys);

            // Sending the same removal twice: the first call makes the change, the
            // second finds every named key already gone and must leave the rest alone.
            for attempt in 1..=2 {
                // The removal reports nothing back about the resource, so it answers
                // the no-content status with no body at all.
                outposts
                    .no_content("DELETE", &untag_path(&taggable.arn, keys), None)
                    .await;

                // Every reader of the resource answers the tags as they now stand: the
                // tagging API's own read, and the resource's own projection.
                let listed = outposts
                    .ok("GET", &format!("/tags/{}", segment(&taggable.arn)), None)
                    .await;
                assert_eq!(
                    reported_tags(&listed),
                    expected,
                    "case {case:?} (attempt {attempt}): ListTagsForResource must answer the \
                     remaining tags, got {listed:?}"
                );

                let read = outposts.ok("GET", &taggable.read, None).await;
                let projected = read[taggable.projection].clone();
                assert_eq!(
                    reported_tags(&projected),
                    expected,
                    "case {case:?} (attempt {attempt}): the {} projection must report the \
                     remaining tags, got {projected:?}",
                    taggable.projection
                );
            }
        }
    }
}

/// Rule 2, over the ways a readable Outposts resource ARN can name nothing: an absent
/// site, an absent Outpost, a resource kind this service does not hold at all, an ARN
/// of another partition, region and account, and — the regional half of the rule — the
/// ARN of a resource that DOES exist, on a request addressed to another region.
///
/// Each refusal is the modelled 404 with the tagging API's fixed not-found sentence
/// (`crate::tagging_not_found_wording`), nothing is created to satisfy it, and the
/// resource that does exist still carries every tag it had.
#[tokio::test]
async fn untagging_an_arn_that_names_no_resource_is_not_found_and_changes_nothing() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "untag refusal site", "Tags": { "Env": "prod" } }))
        .await;
    let live_arn = text(&site, "SiteArn");

    let absent: Vec<(&str, String, &str)> = vec![
        (
            "an absent site",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:site/os-00000000000000000"),
            HOME_REGION,
        ),
        (
            "an absent Outpost",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-00000000000000000"),
            HOME_REGION,
        ),
        (
            "a resource kind this service does not hold",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:order/oo-00000000000000000"),
            HOME_REGION,
        ),
        (
            "an absent site in another partition, region and account",
            "arn:aws-us-gov:outposts:us-gov-west-1:123456789012:site/os-11111111111111111"
                .to_string(),
            HOME_REGION,
        ),
        (
            "a stored site, from another region's endpoint",
            live_arn.clone(),
            OTHER_REGION,
        ),
    ];

    for (case, arn, region) in &absent {
        let refused = outposts
            .call_in(region, "DELETE", &untag_path(arn, &["Env".to_string()]), None)
            .await;
        assert_eq!(
            (
                refused.status,
                refused.error_code.as_str(),
                refused.errortype_header.as_str()
            ),
            (404, "NotFoundException", "NotFoundException"),
            "case {case:?}: an ARN naming no resource of this region is not found, got {:?}",
            refused.body
        );
        assert_eq!(
            text(&refused.body, "Message"),
            crate::tagging_not_found_wording::RESOURCE_NOT_FOUND,
            "case {case:?}: the refusal states the tagging API's fixed sentence, got {:?}",
            refused.body
        );

        // Nothing was created: the same ARN is still not found to the tagging read in
        // the region the refused call was addressed to.
        let listed = outposts
            .call_in(region, "GET", &format!("/tags/{}", segment(arn)), None)
            .await;
        assert_eq!(
            (listed.status, listed.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a refused UntagResource must not have created the resource, \
             got {:?}",
            listed.body
        );
    }

    // The resource that does exist kept every tag it had: the refusals above removed
    // nothing from it.
    let listed = outposts
        .ok("GET", &format!("/tags/{}", segment(&live_arn)), None)
        .await;
    assert_eq!(
        reported_tags(&listed),
        tag_map(&json!({ "Env": "prod" })),
        "case {:?}: a refused removal must leave the stored resource's tags alone, got \
         {listed:?}",
        "the stored site after every refusal"
    );
}

/// Rule 3, at the bound of what one request may name and at the bottom of what a
/// resource may carry: a key list of exactly the modelled maximum — most of whose keys
/// the resource does not carry — is accepted and removes exactly the carried ones, and
/// a resource stripped of every key reports an empty tag map rather than disappearing.
#[tokio::test]
async fn a_maximum_sized_key_list_is_accepted_and_can_empty_the_tag_map() {
    let outposts = Outposts::new();
    // Ten generated keys beside three named ones, so the maximum-sized key list below
    // names both keys the resource carries and keys it does not.
    let carried: Value = (0..10)
        .map(|index| (format!("bulk-{index}"), json!(format!("value-{index}"))))
        .chain([
            ("keep".to_string(), json!("kept")),
            ("Env".to_string(), json!("prod")),
            ("team".to_string(), json!("platform")),
        ])
        .collect::<serde_json::Map<String, Value>>()
        .into();

    let site = outposts
        .create_site(&json!({ "Name": "untag bound site", "Tags": carried }))
        .await;
    let arn = text(&site, "SiteArn");
    let path = format!("/tags/{}", segment(&arn));
    let stored = tag_map(&carried);

    // Exactly the modelled maximum: the ten `bulk-*` keys the resource carries, `Env`
    // and `team`, and generated keys it never had, padding the list to 50.
    let mut keys: Vec<String> = (0..10).map(|index| format!("bulk-{index}")).collect();
    keys.push("Env".to_string());
    keys.push("team".to_string());
    while keys.len() < TAG_KEY_LIST_MAX {
        keys.push(format!("absent-{}", keys.len()));
    }
    assert_eq!(
        keys.len(),
        TAG_KEY_LIST_MAX,
        "case {:?}: the fixture must sit exactly on the modelled maximum, got {}",
        "a key list at the modelled maximum",
        keys.len()
    );

    outposts
        .no_content("DELETE", &untag_path(&arn, &keys), None)
        .await;
    let expected = without(&stored, &keys);
    let listed = outposts.ok("GET", &path, None).await;
    assert_eq!(
        reported_tags(&listed),
        expected,
        "case {:?}: a maximum-sized key list removes exactly the keys the resource \
         carried, got {listed:?}",
        "a key list at the modelled maximum"
    );
    assert_eq!(
        expected,
        tag_map(&json!({ "keep": "kept" })),
        "case {:?}: the key the list did not name must survive, got {expected:?}",
        "a key list at the modelled maximum"
    );

    // The last key removed: the resource is still there, now carrying no tags.
    outposts
        .no_content("DELETE", &untag_path(&arn, &["keep".to_string()]), None)
        .await;
    let listed = outposts.ok("GET", &path, None).await;
    assert!(
        reported_tags(&listed).is_empty(),
        "case {:?}: a resource stripped of every key reports no tags, got {listed:?}",
        "every key removed"
    );
    let read = outposts.ok("GET", &format!("/sites/{}", text(&site, "SiteId")), None).await;
    assert!(
        reported_tags(&read["Site"]).is_empty(),
        "case {:?}: the resource's own projection reports no tags either, got {read:?}",
        "every key removed"
    );
}
