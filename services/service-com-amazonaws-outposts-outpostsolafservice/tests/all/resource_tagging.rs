// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `TagResource`, driven over the wire through the same front
//! door as [`crate::site_and_outpost_lifecycle`] (whose harness this suite reuses):
//! `MultiServiceRouter::dispatch` with a restJson1 request, exactly as a signed
//! `aws outposts tag-resource` call arrives.
//!
//! The rules under test, each stated once and then exercised over a SET of inputs:
//!
//! 1. `TagResource` MERGES the tag map its request carries into the tags of the
//!    resource its `ResourceArn` names — adding a key the resource does not carry,
//!    replacing the value of one it does, and leaving every other key untouched —
//!    and that is what every later reader of the resource answers, whichever
//!    taggable resource kind the ARN names.
//! 2. A `ResourceArn` that is readable but names no resource this store holds is
//!    the modelled `NotFoundException` (HTTP 404), worded by the tagging API's fixed
//!    not-found sentence, and nothing is created to satisfy it.
//! 3. A resource's tag map is reported as the modelled `TagMap`
//!    (`@length(min: 1, max: 50)`), so a change that would take the resource past
//!    that maximum is the modelled `ValidationException` (HTTP 400) and writes
//!    nothing; a change that lands ON the maximum, or that only replaces values, is
//!    accepted.
//! 4. A successful tag reports nothing back about the resource, so it is answered
//!    with [`crate::site_and_outpost_lifecycle::TAG_MUTATION_SUCCESS_STATUS`] and an
//!    empty body — asserted on every accepted row below, through the one helper that
//!    issues the call.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::error_classification::{reported_tags, tag_map};
use crate::site_and_outpost_lifecycle::{segment, text, Outposts};

/// The account id every generated emulator presents, which its ARNs carry.
const ACCOUNT_ID: &str = "000000000000";

/// How many entries the modelled `TagMap` holds: `@length(min: 1, max: 50)`, from
/// `smithy_query(query="shape", shape="TagMap")`. It bounds a RESOURCE's tag map as
/// well as one request's, because the tags a resource carries are reported as that
/// same `TagMap` (`ListTagsForResource$Tags`, `Site$Tags`, `Outpost$Tags`).
const TAG_MAP_MAX: usize = 50;

/// The modelled maximum length of a `TagKey` (`@length(min: 1, max: 128)`) and of a
/// `TagValue` (`@length(min: 0, max: 256)`), from the same query. A `TagValue` also
/// carries `@pattern ^[\S \n]+$`, so its shortest usable value is one character.
const TAG_KEY_MAX: usize = 128;
/// See [`TAG_KEY_MAX`].
const TAG_VALUE_MAX: usize = 256;

/// `POST /tags/{ResourceArn}` with `tags` as the request's `Tags` map, which answers
/// the no-content status every tag mutation reports success with.
async fn tag_resource(outposts: &Outposts, arn: &str, tags: &Value) {
    outposts
        .no_content(
            "POST",
            &format!("/tags/{}", segment(arn)),
            Some(&json!({ "Tags": tags })),
        )
        .await;
}

/// What `TagResource` must leave behind: the tags the resource had, with every entry
/// the request carried written over them.
fn merged(stored: &BTreeMap<String, String>, request: &Value) -> BTreeMap<String, String> {
    let mut merged = stored.clone();
    merged.extend(tag_map(request));
    merged
}

/// One tag as a request map, for a key or value a JSON literal cannot spell (one at
/// its modelled maximum length, say).
fn one_tag(key: &str, value: &str) -> Value {
    std::iter::once((key.to_string(), json!(value)))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

/// `count` distinct tags, keyed `<prefix>-<n>`: how a case builds a tag map of a
/// given SIZE without naming fifty literals.
fn tags_of(count: usize, prefix: &str) -> Value {
    (0..count)
        .map(|index| (format!("{prefix}-{index}"), json!(format!("value-{index}"))))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

/// One taggable resource to exercise rule 1 against: how the case names it, the ARN
/// `TagResource` is called with, the route a single read of it goes to, and the tags
/// it was created with.
struct Taggable {
    case: String,
    arn: String,
    read: String,
    /// The member a single read answers the resource under (`Site` / `Outpost`).
    projection: &'static str,
    stored: BTreeMap<String, String>,
}

/// One resource of every taggable kind this service holds, each at a different
/// starting tag set — none, one and several — since the merge is computed from what
/// the resource already carries.
async fn taggables(outposts: &Outposts) -> Vec<Taggable> {
    let starting: [(&str, Value); 3] = [
        ("no tags at create", json!(null)),
        ("one tag at create", json!({ "Env": "dev" })),
        (
            "several tags at create",
            json!({ "Env": "prod", "team": "platform", "cost.center": "4711" }),
        ),
    ];

    let mut taggables = Vec::new();
    for (start, tags) in &starting {
        let mut site_request = json!({ "Name": format!("tagging site: {start}") });
        let mut outpost_request = json!({ "Name": format!("tagging outpost: {start}") });
        if tags.is_object() {
            site_request["Tags"] = tags.clone();
            outpost_request["Tags"] = tags.clone();
        }

        let site = outposts.create_site(&site_request).await;
        outpost_request["SiteId"] = json!(text(&site, "SiteId"));
        let outpost = outposts.create_outpost(&outpost_request).await;

        taggables.push(Taggable {
            case: format!("site with {start}"),
            arn: text(&site, "SiteArn"),
            read: format!("/sites/{}", text(&site, "SiteId")),
            projection: "Site",
            stored: tag_map(tags),
        });
        taggables.push(Taggable {
            case: format!("outpost with {start}"),
            arn: text(&outpost, "OutpostArn"),
            read: format!("/outposts/{}", text(&outpost, "OutpostId")),
            projection: "Outpost",
            stored: tag_map(tags),
        });
    }
    taggables
}

/// Rule 1, over every taggable resource kind, three starting tag sets and the ways
/// one request's map can relate to the stored one.
///
/// The rows differ along the size of the request's map (one entry, several, and the
/// modelled maximum), whether each key is new or already stored (add, replace,
/// re-send the same pair, and a mixture), and the form of the keys and values (the
/// character classes the modelled `TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$`
/// admits, a one-character key and value, and a key and a value at their modelled
/// maximum lengths). They are applied in sequence to the SAME resource, so a row
/// that dropped a previous row's write, or wrote a key it never named, shows up at
/// once.
#[tokio::test]
async fn tag_resource_merges_its_map_into_the_stored_tags_for_every_reader() {
    let outposts = Outposts::new();

    for mut taggable in taggables(&outposts).await {
        let rows: Vec<(&str, Value)> = vec![
            ("one key the resource does not carry", json!({ "owner": "sre" })),
            (
                "several new keys at once",
                json!({ "tier": "gold", "window": "sat 02:00", "contact": "oncall" }),
            ),
            ("a stored key, new value", json!({ "owner": "platform" })),
            ("the same key and value again", json!({ "owner": "platform" })),
            (
                "one new key and one replaced, in one request",
                json!({ "owner": "network", "zone": "cage-4" }),
            ),
            (
                "every character class the modelled key pattern admits",
                json!({
                    "a+b": "plus",
                    "a-b": "dash",
                    "a.b": "dot",
                    "a_b": "underscore",
                    "a:b": "colon",
                    "a/b": "slash",
                    "A1": "alnum",
                }),
            ),
            (
                "a one-character key and a one-character value",
                json!({ "k": "v" }),
            ),
            (
                "a key and a value at their modelled maximum lengths",
                one_tag(&"K".repeat(TAG_KEY_MAX), &"v".repeat(TAG_VALUE_MAX)),
            ),
        ];

        for (row, request) in &rows {
            let case = format!("{}: {row}", taggable.case);
            let expected = merged(&taggable.stored, request);

            tag_resource(&outposts, &taggable.arn, request).await;

            // Every reader of the resource answers the tags as they now stand: the
            // tagging API's own read, and the resource's own projection.
            let listed = outposts
                .ok("GET", &format!("/tags/{}", segment(&taggable.arn)), None)
                .await;
            assert_eq!(
                reported_tags(&listed),
                expected,
                "case {case:?}: ListTagsForResource must answer the merged tags, got {listed:?}"
            );

            let read = outposts.ok("GET", &taggable.read, None).await;
            let projected = read[taggable.projection].clone();
            assert_eq!(
                reported_tags(&projected),
                expected,
                "case {case:?}: the {} projection must report the merged tags, got {projected:?}",
                taggable.projection
            );

            taggable.stored = expected;
        }

        // The last row of the sequence: one request big enough to fill the resource
        // to the modelled maximum, so the size dimension is exercised at its bound.
        let case = format!("{}: a request filling the resource to the maximum", taggable.case);
        let request = tags_of(TAG_MAP_MAX - taggable.stored.len(), "bulk");
        tag_resource(&outposts, &taggable.arn, &request).await;
        let listed = outposts
            .ok("GET", &format!("/tags/{}", segment(&taggable.arn)), None)
            .await;
        assert_eq!(
            reported_tags(&listed),
            merged(&taggable.stored, &request),
            "case {case:?}: every entry of a maximum-sized request must land, got {listed:?}"
        );
    }
}

/// Rule 2, over the ways a readable Outposts resource ARN can name nothing: an
/// absent site, an absent Outpost, and a resource kind this service does not hold at
/// all.
///
/// Each refusal is the modelled 404 and its body is the tagging API's fixed sentence
/// (`crate::tagging_not_found_wording`), and the ARN still names nothing afterwards
/// — a refused tagging call that had created a record would show up on the read-back.
#[tokio::test]
async fn tagging_an_arn_that_names_no_resource_is_not_found_and_writes_nothing() {
    let outposts = Outposts::new();
    let absent: [(&str, String); 4] = [
        (
            "an absent site",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:site/os-00000000000000000"),
        ),
        (
            "an absent Outpost",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:outpost/op-00000000000000000"),
        ),
        (
            "a resource kind this service does not hold",
            format!("arn:aws:outposts:us-east-1:{ACCOUNT_ID}:order/oo-00000000000000000"),
        ),
        (
            "an absent site in another partition, region and account",
            "arn:aws-us-gov:outposts:us-gov-west-1:123456789012:site/os-11111111111111111"
                .to_string(),
        ),
    ];

    for (case, arn) in &absent {
        let refused = outposts
            .call(
                "POST",
                &format!("/tags/{}", segment(arn)),
                Some(&json!({ "Tags": { "Env": "dev" } })),
            )
            .await;
        assert_eq!(
            (
                refused.status,
                refused.error_code.as_str(),
                refused.errortype_header.as_str()
            ),
            (404, "NotFoundException", "NotFoundException"),
            "case {case:?}: an ARN naming nothing is not found, got {:?}",
            refused.body
        );
        assert_eq!(
            text(&refused.body, "Message"),
            crate::tagging_not_found_wording::RESOURCE_NOT_FOUND,
            "case {case:?}: the refusal states the tagging API's fixed sentence, got {:?}",
            refused.body
        );

        // Nothing was created: the same ARN is still not found to the tagging read.
        let listed = outposts
            .call("GET", &format!("/tags/{}", segment(arn)), None)
            .await;
        assert_eq!(
            (listed.status, listed.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: a refused TagResource must not have created the resource, got {:?}",
            listed.body
        );
    }
}

/// Rule 3, on each side of the modelled `TagMap` maximum: a change that lands exactly
/// ON it, one that would go a single entry past it, one that would go well past it,
/// and one that only replaces values of a resource already at the maximum.
///
/// Every refused row is followed by a read, so a refusal that had written part of its
/// map — or all of it — fails here.
#[tokio::test]
async fn a_change_past_the_modelled_tag_map_maximum_is_refused_and_writes_nothing() {
    let outposts = Outposts::new();
    // Twenty tags at create, so the request below lands the resource exactly on the
    // modelled maximum.
    let at_create = tags_of(20, "start");
    let site = outposts
        .create_site(&json!({ "Name": "tag limit site", "Tags": at_create }))
        .await;
    let arn = text(&site, "SiteArn");
    let path = format!("/tags/{}", segment(&arn));

    // ON the bound: 20 stored + 30 new = the modelled maximum, accepted.
    let fill = tags_of(TAG_MAP_MAX - 20, "fill");
    tag_resource(&outposts, &arn, &fill).await;
    let mut expected = merged(&tag_map(&at_create), &fill);
    assert_eq!(
        expected.len(),
        TAG_MAP_MAX,
        "case {:?}: the fixture must sit exactly on the modelled maximum, got {}",
        "exactly at the maximum",
        expected.len()
    );
    let listed = outposts.ok("GET", &path, None).await;
    assert_eq!(
        reported_tags(&listed),
        expected,
        "case {:?}: a change landing on the modelled maximum is accepted, got {listed:?}",
        "exactly at the maximum"
    );

    // PAST the bound: one entry over, and far over. The report names the member and
    // the modelled constraint and renders none of the tags.
    let refused: [(&str, Value); 2] = [
        ("one entry past the maximum", json!({ "one-too-many": "x" })),
        ("well past the maximum", tags_of(10, "excess")),
    ];
    for (case, request) in &refused {
        let reply = outposts
            .call("POST", &path, Some(&json!({ "Tags": request })))
            .await;
        assert_eq!(
            (
                reply.status,
                reply.error_code.as_str(),
                reply.errortype_header.as_str()
            ),
            (400, "ValidationException", "ValidationException"),
            "case {case:?}: a change past the modelled maximum is a bad parameter, got {:?}",
            reply.body
        );
        assert_eq!(
            text(&reply.body, "Message"),
            format!(
                "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
                 Member must have length less than or equal to {TAG_MAP_MAX}"
            ),
            "case {case:?}: the refusal states the modelled constraint without rendering a \
             value, got {:?}",
            reply.body
        );

        let listed = outposts.ok("GET", &path, None).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: a refused change writes nothing, got {listed:?}"
        );
    }

    // A resource already at the maximum still takes a change that only REPLACES
    // values: it does not grow the map.
    let replacement: Value = expected
        .keys()
        .take(5)
        .map(|key| (key.clone(), json!("replaced")))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    tag_resource(&outposts, &arn, &replacement).await;
    expected = merged(&expected, &replacement);
    let listed = outposts.ok("GET", &path, None).await;
    assert_eq!(
        reported_tags(&listed),
        expected,
        "case {:?}: replacing values of a resource at the maximum is accepted, got {listed:?}",
        "replace only, at the maximum"
    );
    assert_eq!(
        reported_tags(&listed).len(),
        TAG_MAP_MAX,
        "case {:?}: a replacement must not grow the map, got {listed:?}",
        "replace only, at the maximum"
    );
}
