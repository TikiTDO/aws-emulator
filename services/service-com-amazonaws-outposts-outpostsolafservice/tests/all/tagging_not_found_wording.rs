// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for HOW the tagging API words a `NotFoundException`, driven over
//! the wire through `MultiServiceRouter::dispatch` exactly as a signed `aws outposts`
//! call arrives (reusing the harness of [`crate::site_and_outpost_lifecycle`]).
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an
//! operation that names a resource by the tagging `ResourceArn` member and cannot
//! resolve it to a resource of the inventory the request reached answers the modelled
//! `NotFoundException` (HTTP 404) with the WHOLLY FIXED message `Resource was not
//! found` — a sentence with no variable part, so it renders no part of the ARN the
//! caller supplied and reads the same whichever resource kind, partition, Region or
//! account that ARN named. That is a different wording from the identifier echo the
//! site and Outpost registry routes answer ([`crate::error_message_rules`]'s rule A),
//! which these rows leave untouched.
//!
//! The three operations the rule covers are the ones that bind that member
//! (`smithy_query(query="operation",operation="TagResource,ListTagsForResource,
//! UntagResource")`, each modelling `NotFoundException` with `httpError` 404), so
//! every row is sent to all three — GET, POST and DELETE alike.
//!
//! The rows differ along what the rule generalises over: the resource kind the ARN
//! names (a taggable site, a taggable Outpost, and a kind this service holds no tags
//! for), why it resolves to nothing (never minted, minted in another Region, minted
//! and then deleted), the partition, Region and account the ARN carries, and which
//! operation refuses. Each test ends on a control: while the resource DOES exist the
//! very same ARN is served, so the wording is read off the store rather than returned
//! for the shape of the request.

use serde_json::{json, Value};

use crate::error_classification::reported_tags;
use crate::site_and_outpost_lifecycle::{query_value, segment, text, Outposts, HOME_REGION};

/// The account id every generated emulator presents, which its ARNs carry.
const ACCOUNT_ID: &str = "000000000000";

/// A region other than [`HOME_REGION`]: a resource belongs to its own Region's
/// inventory and to no other's, so its ARN names nothing to another Region's endpoint.
const OTHER_REGION: &str = "us-west-2";

/// AWS's text for a `ResourceArn` the tagging API cannot resolve. Wholly fixed: the
/// ARN is not rendered in it.
pub(crate) const RESOURCE_NOT_FOUND: &str = "Resource was not found";

/// The tag one row sends, and the key one row asks to have removed.
const PROBE_KEY: &str = "sim-probe";
/// See [`PROBE_KEY`].
const PROBE_VALUE: &str = "v";

/// One row: how the case names it, the ARN every tagging operation is called with, and
/// the Region whose endpoint the calls are addressed to.
struct Row {
    case: String,
    arn: String,
    region: &'static str,
}

/// `GET`/`POST` `/tags/{ResourceArn}`, with the ARN percent-encoded into the label.
fn tags_path(arn: &str) -> String {
    format!("/tags/{}", segment(arn))
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…`, which is how `UntagResource` names the keys
/// it removes.
fn untag_path(arn: &str, key: &str) -> String {
    format!("/tags/{}?tagKeys={}", segment(arn), query_value(key))
}

/// The three operations that bind `ResourceArn`, each as the request one row sends it:
/// the operation's name, its HTTP method, the path for `arn` and its payload.
fn tagging_calls(arn: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        ("ListTagsForResource", "GET", tags_path(arn), None),
        (
            "TagResource",
            "POST",
            tags_path(arn),
            Some(json!({ "Tags": { PROBE_KEY: PROBE_VALUE } })),
        ),
        (
            "UntagResource",
            "DELETE",
            untag_path(arn, PROBE_KEY),
            None,
        ),
    ]
}

/// Every row's ARN, with the stored resources a row needs created first.
async fn rows(outposts: &Outposts) -> Vec<Row> {
    // A site and an Outpost of this endpoint's own Region, for the rows about a
    // resource that exists somewhere else than where it is looked for.
    let site = outposts
        .create_site(&json!({ "Name": "tagging wording site" }))
        .await;
    let outpost = outposts
        .create_outpost(&json!({
            "Name": "tagging wording outpost",
            "SiteId": text(&site, "SiteId"),
        }))
        .await;
    let stored_site_arn = text(&site, "SiteArn");
    let stored_outpost_arn = text(&outpost, "OutpostArn");

    // A site that EXISTED and was then deleted: its ARN stays well formed and stops
    // naming anything, which is the same refusal as one never minted.
    let deleted = outposts
        .create_site(&json!({ "Name": "tagging wording, deleted" }))
        .await;
    let deleted_arn = text(&deleted, "SiteArn");
    let _ = outposts
        .ok("DELETE", &format!("/sites/{}", text(&deleted, "SiteId")), None)
        .await;

    vec![
        Row {
            case: "a site id this emulator never minted".to_string(),
            arn: format!("arn:aws:outposts:{HOME_REGION}:{ACCOUNT_ID}:site/os-00000000000000000"),
            region: HOME_REGION,
        },
        Row {
            case: "an Outpost id this emulator never minted".to_string(),
            arn: format!(
                "arn:aws:outposts:{HOME_REGION}:{ACCOUNT_ID}:outpost/op-00000000000000000"
            ),
            region: HOME_REGION,
        },
        Row {
            case: "a resource kind this service holds no tags for".to_string(),
            arn: format!("arn:aws:outposts:{HOME_REGION}:{ACCOUNT_ID}:order/oo-00000000000000000"),
            region: HOME_REGION,
        },
        Row {
            case: "another partition, Region and account".to_string(),
            arn: "arn:aws-us-gov:outposts:us-gov-west-1:123456789012:site/os-11111111111111111"
                .to_string(),
            region: HOME_REGION,
        },
        Row {
            case: "a stored site, from another Region's endpoint".to_string(),
            arn: stored_site_arn,
            region: OTHER_REGION,
        },
        Row {
            case: "a stored Outpost, from another Region's endpoint".to_string(),
            arn: stored_outpost_arn,
            region: OTHER_REGION,
        },
        Row {
            case: "a site that was created and then deleted".to_string(),
            arn: deleted_arn,
            region: HOME_REGION,
        },
    ]
}

/// The rule, over every class of unresolvable `ResourceArn` and all three operations
/// that bind the member.
#[tokio::test]
async fn every_tagging_operation_states_the_fixed_resource_sentence() {
    let outposts = Outposts::new();

    for row in rows(&outposts).await {
        for (operation, method, path, body) in tagging_calls(&row.arn) {
            let case = format!("{operation} on {}", row.case);
            let reply = outposts
                .call_in(row.region, method, &path, body.as_ref())
                .await;
            assert_eq!(
                (
                    reply.status,
                    reply.error_code.as_str(),
                    reply.errortype_header.as_str()
                ),
                (404, "NotFoundException", "NotFoundException"),
                "case {case:?}: expected 404 NotFoundException, got {} {:?}",
                reply.status,
                reply.body
            );
            let actual = text(&reply.body, "Message");
            assert_eq!(
                actual, RESOURCE_NOT_FOUND,
                "case {case:?}: expected $.Message {RESOURCE_NOT_FOUND:?}, got {actual:?}"
            );
            // The sentence has no variable part, so none of what the caller supplied
            // is in it — neither the ARN nor the name of the member it arrived in.
            for leaked in [row.arn.as_str(), "resourceArn", "arn:"] {
                assert!(
                    !actual.contains(leaked),
                    "case {case:?}: the fixed sentence must render no part of the request, \
                     got {actual:?} carrying {leaked:?}"
                );
            }
        }
    }
}

/// The control, and the state half of the rule: while the resource exists the SAME ARN
/// is served by all three operations — the tag a `TagResource` wrote is what
/// `ListTagsForResource` answers and what `UntagResource` removes — and deleting the
/// resource is what turns that ARN into one every tagging operation refuses with the
/// fixed sentence.
#[tokio::test]
async fn a_stored_resource_is_served_through_the_arn_the_refusals_name() {
    let outposts = Outposts::new();
    let site = outposts
        .create_site(&json!({ "Name": "tagging wording control" }))
        .await;
    let site_id = text(&site, "SiteId");
    let arn = text(&site, "SiteArn");

    outposts
        .no_content(
            "POST",
            &tags_path(&arn),
            Some(&json!({ "Tags": { PROBE_KEY: PROBE_VALUE } })),
        )
        .await;

    let listed = outposts.ok("GET", &tags_path(&arn), None).await;
    assert_eq!(
        reported_tags(&listed).get(PROBE_KEY).map(String::as_str),
        Some(PROBE_VALUE),
        "case \"ListTagsForResource on the stored site\": expected the written tag, got {listed:?}"
    );

    outposts
        .no_content("DELETE", &untag_path(&arn, PROBE_KEY), None)
        .await;
    let listed = outposts.ok("GET", &tags_path(&arn), None).await;
    assert!(
        !reported_tags(&listed).contains_key(PROBE_KEY),
        "case \"UntagResource on the stored site\": expected the tag gone, got {listed:?}"
    );

    // Deleting the resource is what makes the very same ARN unresolvable, and then
    // every tagging operation states the fixed sentence for it.
    let _ = outposts
        .ok("DELETE", &format!("/sites/{site_id}"), None)
        .await;
    for (operation, method, path, body) in tagging_calls(&arn) {
        let case = format!("{operation} on the deleted site");
        let reply = outposts.call(method, &path, body.as_ref()).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "NotFoundException"),
            "case {case:?}: expected 404 NotFoundException, got {} {:?}",
            reply.status,
            reply.body
        );
        let actual = text(&reply.body, "Message");
        assert_eq!(
            actual, RESOURCE_NOT_FOUND,
            "case {case:?}: expected $.Message {RESOURCE_NOT_FOUND:?}, got {actual:?}"
        );
    }
}
