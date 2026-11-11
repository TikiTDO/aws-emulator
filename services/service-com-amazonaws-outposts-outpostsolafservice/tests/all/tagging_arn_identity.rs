// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the tagging `ResourceArn` member, driven over the wire through
//! the same front door as [`crate::site_and_outpost_lifecycle`] (whose harness this
//! suite reuses): `MultiServiceRouter::dispatch` with a restJson1 request, exactly as a
//! signed `aws outposts tag-resource` call arrives.
//!
//! The rule under test, stated once and then exercised over a SET of inputs: an ARN is
//! an ABSOLUTE identity — its partition, its Region and its account id are part of the
//! name — so a `ResourceArn` names a resource of the endpoint being called only when
//! all three are that endpoint's own. One that differs in any of them names a resource
//! of some other endpoint, which this endpoint holds no record of however familiar the
//! bare resource id inside it looks: every operation binding the member answers the
//! modelled `NotFoundException` (404) with the tagging API's fixed sentence, and the
//! local resource whose id the ARN carries is left exactly as it was. The endpoint's
//! OWN ARN keeps naming the resource, and keeps being the one its tags are read and
//! written through.
//!
//! The rows differ along everything the rule generalises over: which ARN segment the
//! value differs in (account, Region, partition, partition + Region, all three), the
//! endpoint the call is addressed to and the partition it belongs to (a commercial
//! Region and a Region of another partition, so the check is not "the partition must be
//! `aws`"), the taggable resource kind (site, Outpost), and the operation — the three
//! that bind the member, reading tags, adding them and removing them. The endpoints and
//! the forged identities are the ones [`crate::identifier_arn_identity`] states for the
//! identifier members, since it is one rule about ARNs and not two.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::error_classification::reported_tags;
use crate::identifier_arn_identity::{arn, forgeries, ENDPOINTS};
use crate::site_and_outpost_lifecycle::{query_value, segment, text, Outposts};
use crate::tagging_not_found_wording::RESOURCE_NOT_FOUND;

/// A tag key every resource of this suite is created with, which a forged
/// `UntagResource` asks to have removed and a forged `TagResource` asks to have
/// overwritten.
const STORED_KEY: &str = "Env";
/// The value [`STORED_KEY`] is created with. See [`stored_tags`].
const STORED_VALUE: &str = "dev";

/// A tag key no resource of this suite is created with: what a forged `TagResource`
/// asks to have added, and what a call through the endpoint's own ARN does add.
const NEW_KEY: &str = "owner";

/// The tags every resource of this suite is created with: what a forged call must leave
/// untouched, and what the endpoint's own ARN must keep answering.
fn stored_tags() -> Value {
    json!({ STORED_KEY: STORED_VALUE, "team": "platform" })
}

/// Those tags as the comparable map a reader's answer is compared against.
fn stored_map() -> BTreeMap<String, String> {
    reported_tags(&json!({ "Tags": stored_tags() }))
}

/// `GET`/`POST` `/tags/{ResourceArn}`, with the ARN percent-encoded into the label.
fn tags_path(resource_arn: &str) -> String {
    format!("/tags/{}", segment(resource_arn))
}

/// `DELETE /tags/{ResourceArn}?tagKeys=…`, which is how `UntagResource` names the keys
/// it removes.
fn untag_path(resource_arn: &str, key: &str) -> String {
    format!("/tags/{}?tagKeys={}", segment(resource_arn), query_value(key))
}

/// The three operations that bind `ResourceArn`, each as the request a row sends it:
/// the operation's name, its HTTP method, the path for `resource_arn` and its payload.
///
/// The two that CHANGE tags are sent a change that would show up on a read-back if it
/// landed: one key the resource does not carry and one it does, and the removal of a
/// key it does carry.
fn tagging_calls(resource_arn: &str) -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        ("ListTagsForResource", "GET", tags_path(resource_arn), None),
        (
            "TagResource",
            "POST",
            tags_path(resource_arn),
            Some(json!({ "Tags": { NEW_KEY: "x", STORED_KEY: "overwritten" } })),
        ),
        (
            "UntagResource",
            "DELETE",
            untag_path(resource_arn, STORED_KEY),
            None,
        ),
    ]
}

/// One taggable resource of an endpoint's inventory: how a case names its kind (which
/// is also the resource-type segment that kind's ARN carries), the bare id a forged ARN
/// is built around, the ARN the service itself minted for it, and the route and member
/// a single read reports it under.
struct Taggable {
    kind: &'static str,
    resource_id: String,
    own_arn: String,
    read: String,
    projection: &'static str,
}

/// One resource of every taggable kind this service holds, created through `region`'s
/// endpoint and carrying [`stored_tags`].
async fn taggables(outposts: &Outposts, region: &str) -> Vec<Taggable> {
    let site = outposts
        .create_site_in(
            region,
            &json!({ "Name": format!("{region} tagging identity site"), "Tags": stored_tags() }),
        )
        .await;
    let site_id = text(&site, "SiteId");
    let outpost = outposts
        .create_outpost_in(
            region,
            &json!({
                "Name": format!("{region} tagging identity outpost"),
                "SiteId": site_id,
                "Tags": stored_tags(),
            }),
        )
        .await;
    let outpost_id = text(&outpost, "OutpostId");
    vec![
        Taggable {
            kind: "site",
            read: format!("/sites/{site_id}"),
            resource_id: site_id,
            own_arn: text(&site, "SiteArn"),
            projection: "Site",
        },
        Taggable {
            kind: "outpost",
            read: format!("/outposts/{outpost_id}"),
            resource_id: outpost_id,
            own_arn: text(&outpost, "OutpostArn"),
            projection: "Outpost",
        },
    ]
}

/// What the resource must still carry, read through the ARN the service minted for it
/// and through the resource's own projection — the two readers a change written through
/// a forged ARN would show up on.
async fn assert_tags_unchanged(outposts: &Outposts, region: &str, taggable: &Taggable, case: &str) {
    let expected = stored_map();

    let listed = outposts
        .ok_in(region, "GET", &tags_path(&taggable.own_arn), None)
        .await;
    assert_eq!(
        reported_tags(&listed),
        expected,
        "case {case:?}: the {} must still carry the tags it was created with, got {listed:?}",
        taggable.kind
    );

    let read = outposts.ok_in(region, "GET", &taggable.read, None).await;
    let projected = read[taggable.projection].clone();
    assert_eq!(
        reported_tags(&projected),
        expected,
        "case {case:?}: the {} projection must still report those tags, got {projected:?}",
        taggable.projection
    );
}

/// The rule, over every forged identity, every taggable kind and all three operations
/// that bind `ResourceArn`, at each endpoint.
///
/// Each case starts from the control that the endpoint's OWN ARN names the resource, so
/// a refusal is about the identity the ARN states and not about the ARN form itself, and
/// ends on a read-back of the local resource: a forged call that had written or removed
/// a tag shows up there.
#[tokio::test]
async fn a_tagging_arn_of_another_endpoint_names_nothing_and_changes_nothing() {
    for endpoint in ENDPOINTS {
        let outposts = Outposts::new();

        for taggable in taggables(&outposts, endpoint.region).await {
            assert_tags_unchanged(
                &outposts,
                endpoint.region,
                &taggable,
                &format!("the endpoint's own ARN names the {}", taggable.kind),
            )
            .await;

            for (forgery, partition, region, account) in forgeries(endpoint) {
                let forged = arn(
                    &partition,
                    &region,
                    &account,
                    taggable.kind,
                    &taggable.resource_id,
                );
                for (operation, method, path, body) in tagging_calls(&forged) {
                    let case = format!(
                        "{operation} on a {} ARN with {forgery}, at the {} endpoint: {forged}",
                        taggable.kind, endpoint.region
                    );
                    let reply = outposts
                        .call_in(endpoint.region, method, &path, body.as_ref())
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
                    let message = text(&reply.body, "Message");
                    assert_eq!(
                        message, RESOURCE_NOT_FOUND,
                        "case {case:?}: expected $.Message {RESOURCE_NOT_FOUND:?}, got \
                         {message:?}"
                    );
                }

                assert_tags_unchanged(
                    &outposts,
                    endpoint.region,
                    &taggable,
                    &format!(
                        "after a tagging call with {forgery} on the {}",
                        taggable.kind
                    ),
                )
                .await;
            }
        }
    }
}

/// The positive half of the rule, with two endpoints in ONE store: each endpoint's own
/// ARN is the one its tags are written and read through, and that same ARN names
/// nothing at the other endpoint — so the identity an ARN states is what decides, not a
/// fixed partition, Region or account the code could be matching on.
#[tokio::test]
async fn each_endpoints_own_tagging_arn_is_served_only_at_that_endpoint() {
    let outposts = Outposts::new();

    let mut owned: Vec<(&'static str, String)> = Vec::new();
    for endpoint in ENDPOINTS {
        let site = outposts
            .create_site_in(
                endpoint.region,
                &json!({
                    "Name": format!("{} tagging identity owner", endpoint.region),
                    "Tags": stored_tags(),
                }),
            )
            .await;
        owned.push((endpoint.region, text(&site, "SiteArn")));
    }

    for (region, own_arn) in &owned {
        // At its own endpoint the ARN is served: a tag written through it is the tag
        // read back through it.
        let case = format!("the ARN of a {region} site, at the {region} endpoint");
        outposts
            .no_content_in(
                region,
                "POST",
                &tags_path(own_arn),
                Some(&json!({ "Tags": { NEW_KEY: "written here" } })),
            )
            .await;
        let mut expected = stored_map();
        expected.insert(NEW_KEY.to_string(), "written here".to_string());
        let listed = outposts.ok_in(region, "GET", &tags_path(own_arn), None).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: the written tag must be read back beside the stored ones, got \
             {listed:?}"
        );

        // At every OTHER endpoint the very same ARN names nothing, and neither of the
        // changing operations reaches the resource.
        for (other, _) in owned.iter().filter(|(other, _)| other != region) {
            for (operation, method, path, body) in tagging_calls(own_arn) {
                let case =
                    format!("{operation} on the ARN of a {region} site, at the {other} endpoint");
                let reply = outposts.call_in(other, method, &path, body.as_ref()).await;
                assert_eq!(
                    (reply.status, reply.error_code.as_str()),
                    (404, "NotFoundException"),
                    "case {case:?}: expected 404 NotFoundException, got {} {:?}",
                    reply.status,
                    reply.body
                );
            }
        }

        let listed = outposts.ok_in(region, "GET", &tags_path(own_arn), None).await;
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {case:?}: a refused call at another endpoint changes nothing here, got \
             {listed:?}"
        );
    }
}
