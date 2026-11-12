// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteAddon`: the removal of an installed add-on, driven
//! over the wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which optional
//! members the installed add-on carries, the status it is deleted from, whether
//! `preserve` is left out, false or true, how many child associations the add-on
//! owns, how many add-ons the cluster holds, which half of the `(cluster, add-on)`
//! pair is missing and which Region the request is signed for), and each assertion
//! names its case and prints the actual value.
//!
//! The request harness (`Answer`, `emulator`, `call`, `encode_label`) is the one
//! every suite in this crate shares, from `tests/all/wire.rs`; the cluster request
//! is `cluster_lifecycle`'s and the add-on install, read-back and association
//! read-back are the helpers `tests/all/addon_lifecycle.rs` already exposes.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ADDON_ACTIVATION_DELAY, ADDON_DELETION_DELAY, CLUSTER_ACTIVATION_DELAY,
    UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_lifecycle::{
    create_addon, describe_addon, describe_association, published_addon_names,
};
use crate::cluster_lifecycle::{create as create_cluster, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// An IAM role ARN of the form the add-on members that take one want.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AddonRole";

/// Another one, so an update can ask for a value the add-on does not already have.
/// A role of THIS account: `serviceAccountRoleArn` is a role the request asks Amazon
/// EKS to pass, so one of another account is refused 403 before anything else about
/// the request is judged ([`crate::addon_request_precedence`]).
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OtherAddonRole";

/// One cluster in [`REGION`], waited into `ACTIVE` so the add-on layer that
/// requires a running control plane is past.
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let answer = create_cluster(router, &minimal_request(name)).await;
    assert_eq!(
        answer.status, 200,
        "setup CreateCluster {name:?}: got {answer:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// `DeleteAddon` over its modelled route, with `preserve` in the query string only
/// when the case supplies it: the member carries a default, so a request may leave
/// it out entirely.
async fn delete_addon_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    addon: &str,
    preserve: Option<bool>,
) -> Answer {
    let mut uri = format!(
        "/clusters/{}/addons/{}",
        encode_label(cluster),
        encode_label(addon)
    );
    if let Some(preserve) = preserve {
        uri.push_str(&format!("?preserve={preserve}"));
    }
    call(router, region, "DELETE", &uri, "").await
}

pub(crate) async fn delete_addon(
    router: &MultiServiceRouter,
    cluster: &str,
    addon: &str,
    preserve: Option<bool>,
) -> Answer {
    delete_addon_in(router, REGION, cluster, addon, preserve).await
}

/// The add-on names one cluster holds, as `ListAddons` reports them.
async fn list_addons(router: &MultiServiceRouter, cluster: &str) -> Vec<String> {
    let answer = call(
        router,
        REGION,
        "GET",
        &format!("/clusters/{}/addons", encode_label(cluster)),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "ListAddons {cluster:?}: got {answer:?}"
    );
    answer.body["addons"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|name| name.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// Install one add-on and wait out the span the clock owes it, so it is `ACTIVE`.
async fn active_addon(router: &MultiServiceRouter, cluster: &str, body: &serde_json::Value) {
    let answer = create_addon(router, cluster, body).await;
    assert_eq!(
        answer.status, 200,
        "setup CreateAddon {body}: got {answer:?}"
    );
    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// The add-on body with the two members a delete is expected to move dropped, so
/// everything else can be compared whole.
fn without_delete_markers(addon: &serde_json::Value) -> serde_json::Value {
    let mut copy = addon.clone();
    if let Some(object) = copy.as_object_mut() {
        object.remove("status");
        object.remove("modifiedAt");
    }
    copy
}

/// A restJson1 timestamp as an epoch value, for the ordering assertions.
fn epoch(value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or_default()
}

/// The association ids an add-on reports owning, taken from the ARNs it reports.
fn owned_association_ids(addon: &serde_json::Value) -> Vec<String> {
    addon["podIdentityAssociations"]
        .as_array()
        .map(|arns| {
            arns.iter()
                .filter_map(|arn| arn.as_str())
                .map(|arn| arn.rsplit('/').next().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: deleting an add-on takes it off its cluster and answers with the record
// that left, unchanged apart from the final status it is reported in and the
// instant it was last modified — so every later read of that add-on is a 404 and
// a second delete of it is one too.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional members the
/// installed add-on carries, and the status it is deleted FROM (one still
/// `CREATING` and the rest settled into `ACTIVE`).
#[tokio::test]
async fn a_deleted_addon_answers_the_record_that_left_and_is_then_gone() {
    let router = emulator();
    active_cluster(&router, "deleted-addons").await;

    // (case, create body, preserve, whether to wait the add-on into ACTIVE)
    let cases: Vec<(&str, serde_json::Value, Option<bool>, bool)> = vec![
        (
            "only the required members, no preserve query",
            serde_json::json!({ "addonName": "vpc-cni" }),
            None,
            true,
        ),
        (
            "tags, preserve=false",
            serde_json::json!({
                "addonName": "coredns",
                "tags": { "team": "platform", "env": "test" },
            }),
            Some(false),
            true,
        ),
        (
            "a service account role and a configuration document, preserve=true",
            serde_json::json!({
                "addonName": "kube-proxy",
                "serviceAccountRoleArn": ROLE_ARN,
                // `priorityClassName` is one of the keys every published add-on's
                // configuration schema declares (source: the common properties
                // `DescribeAddonConfiguration` publishes, read in this crate's
                // `addon_versions` suite).
                "configurationValues": "{\"priorityClassName\":\"system-cluster-critical\"}",
            }),
            Some(true),
            true,
        ),
        (
            "a namespace override and an owned association",
            serde_json::json!({
                "addonName": "amazon-cloudwatch-observability",
                "namespaceConfig": { "namespace": "observability" },
                "podIdentityAssociations": [
                    { "serviceAccount": "cloudwatch-agent", "roleArn": ROLE_ARN },
                ],
            }),
            Some(false),
            true,
        ),
        (
            "deleted while the clock still owes it ACTIVE",
            serde_json::json!({ "addonName": "eks-pod-identity-agent" }),
            None,
            false,
        ),
    ];

    for (case, request, preserve, settle) in &cases {
        let addon_name = request["addonName"].as_str().expect("an addon name");
        let created = create_addon(&router, "deleted-addons", request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: setup CreateAddon must succeed, got {created:?}"
        );
        if *settle {
            tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
        }

        let before = describe_addon(&router, "deleted-addons", addon_name).await;
        assert_eq!(
            before.status, 200,
            "case {case:?}: the add-on must read back before the delete, got {before:?}"
        );

        let deleted = delete_addon(&router, "deleted-addons", addon_name, *preserve).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from DeleteAddon, got {deleted:?}"
        );
        assert_eq!(
            deleted.addon()["status"],
            serde_json::json!("DELETING"),
            "case {case:?}: a deleted add-on is reported in its final status, got {:?}",
            deleted.addon()["status"]
        );
        assert_eq!(
            without_delete_markers(deleted.addon()),
            without_delete_markers(before.addon()),
            "case {case:?}: the delete answers the record that left, got {:?}",
            deleted.addon()
        );
        assert!(
            epoch(&deleted.addon()["modifiedAt"]) >= epoch(&before.addon()["modifiedAt"]),
            "case {case:?}: the delete modified the add-on, so modifiedAt cannot go \
             backwards from {:?}, got {:?}",
            before.addon()["modifiedAt"],
            deleted.addon()["modifiedAt"]
        );
        assert!(
            epoch(&deleted.addon()["modifiedAt"]) >= epoch(&deleted.addon()["createdAt"]),
            "case {case:?}: modifiedAt cannot precede createdAt {:?}, got {:?}",
            deleted.addon()["createdAt"],
            deleted.addon()["modifiedAt"]
        );

        // The removal is ASYNCHRONOUS: until the clock has finished it the add-on
        // is still an add-on of its cluster, reported in the status the delete
        // answered with.
        let deleting = describe_addon(&router, "deleted-addons", addon_name).await;
        assert_eq!(
            deleting.status, 200,
            "case {case:?}: the add-on must still read back while it is deleting, got \
             {deleting:?}"
        );
        assert_eq!(
            deleting.addon()["status"],
            serde_json::json!("DELETING"),
            "case {case:?}: a deleting add-on reports DELETING, got {:?}",
            deleting.addon()["status"]
        );
        let listed_while_deleting = list_addons(&router, "deleted-addons").await;
        assert!(
            listed_while_deleting.iter().any(|name| name == addon_name),
            "case {case:?}: ListAddons lists an add-on that is still deleting, got \
             {listed_while_deleting:?}"
        );
        tokio::time::sleep(ADDON_DELETION_DELAY + Duration::from_millis(250)).await;

        // The add-on has left the store: neither read finds it any more.
        let after = describe_addon(&router, "deleted-addons", addon_name).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: a deleted add-on must not read back, got {after:?}"
        );
        assert_eq!(
            error_code(&after),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {after:?}"
        );
        let listed = list_addons(&router, "deleted-addons").await;
        assert!(
            !listed.iter().any(|name| name == addon_name),
            "case {case:?}: ListAddons must not list {addon_name:?} any more, got {listed:?}"
        );

        // And the delete is not idempotent in the "answer it again" sense: there is
        // no longer an add-on under that name to delete.
        let again = delete_addon(&router, "deleted-addons", addon_name, *preserve).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: a second delete has nothing to delete, got {again:?}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException on the second delete, got \
             {again:?}"
        );
    }

    // Every add-on of the table is gone, so the cluster holds none.
    let listed = list_addons(&router, "deleted-addons").await;
    assert!(
        listed.is_empty(),
        "every add-on of the table was deleted, so the cluster holds none, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: removing an add-on removes what Amazon EKS manages FOR it — the EKS Pod
// Identity associations it owns — unless the request asks for the add-on to be
// preserved, in which case those associations are left in place. The add-on
// itself leaves the cluster either way.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn preserve_decides_whether_the_associations_the_addon_owns_go_with_it() {
    let router = emulator();
    active_cluster(&router, "preserving").await;

    // Rows differ along the `preserve` value a request states (left out, false,
    // true) and along HOW MANY associations the add-on owns, because the rule is
    // about the set the add-on owns rather than about one association.
    let preserve_cases: [(&str, Option<bool>, bool); 3] = [
        ("preserve left out", None, false),
        ("preserve=false", Some(false), false),
        ("preserve=true", Some(true), true),
    ];

    // One add-on name installs at most once per cluster, so each row drives a
    // DIFFERENT add-on of the published catalog.
    let published = published_addon_names(&router).await;
    assert!(
        published.len() >= preserve_cases.len() * 2,
        "setup: the catalog must publish at least {} add-ons, got {}",
        preserve_cases.len() * 2,
        published.len()
    );
    let mut next_addon = 0_usize;

    for (index, (case, preserve, kept)) in preserve_cases.iter().enumerate() {
        for count in [1_usize, 3] {
            let case = format!("{case} with {count} association(s)");
            let addon_name = published[next_addon].clone();
            next_addon += 1;
            let associations: Vec<serde_json::Value> = (0..count)
                .map(|slot| {
                    serde_json::json!({
                        "serviceAccount": format!("sa-{index}-{count}-{slot}"),
                        "roleArn": ROLE_ARN,
                    })
                })
                .collect();
            let created = create_addon(
                &router,
                "preserving",
                &serde_json::json!({
                    "addonName": addon_name,
                    "podIdentityAssociations": associations,
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: setup CreateAddon must succeed, got {created:?}"
            );
            let ids = owned_association_ids(created.addon());
            assert_eq!(
                ids.len(),
                count,
                "case {case:?}: the add-on must own {count} association(s), got {ids:?}"
            );

            let deleted = delete_addon(&router, "preserving", &addon_name, *preserve).await;
            assert_eq!(
                deleted.status, 200,
                "case {case:?}: expected 200 from DeleteAddon, got {deleted:?}"
            );

            // The add-on itself leaves whatever `preserve` says, once the clock has
            // finished the removal it started.
            tokio::time::sleep(ADDON_DELETION_DELAY + Duration::from_millis(250)).await;
            let after = describe_addon(&router, "preserving", &addon_name).await;
            assert_eq!(
                after.status, 404,
                "case {case:?}: the add-on leaves the cluster whatever preserve says, got \
                 {after:?}"
            );

            let expected_status = if *kept { 200 } else { 404 };
            for id in &ids {
                let association = describe_association(&router, "preserving", id).await;
                assert_eq!(
                    association.status, expected_status,
                    "case {case:?}: expected {expected_status} for the association {id:?} \
                     the add-on owned, got {association:?}"
                );
                if *kept {
                    assert_eq!(
                        association.body["association"]["associationId"],
                        serde_json::json!(id),
                        "case {case:?}: a preserved association keeps its identity, got {:?}",
                        association.body["association"]
                    );
                } else {
                    assert_eq!(
                        error_code(&association),
                        "ResourceNotFoundException",
                        "case {case:?}: expected ResourceNotFoundException for the removed \
                         association {id:?}, got {association:?}"
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a delete addresses the `(cluster, add-on)` pair of the Region the request
// is signed for, so a pair this Region's store does not hold is 404
// `ResourceNotFoundException` and nothing is removed anywhere.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pair_this_region_does_not_hold_is_not_found() {
    let router = emulator();
    /// The other Region every row contrasts with: a cluster name identifies a
    /// cluster only within one Region.
    const OTHER_REGION: &str = "eu-west-1";

    active_cluster(&router, "present").await;
    active_addon(
        &router,
        "present",
        &serde_json::json!({ "addonName": "vpc-cni" }),
    )
    .await;

    // The same cluster name, and the same add-on name on it, in ANOTHER Region.
    // Built with the shared wire client rather than the REGION-bound helpers,
    // because stating a second Region is the point of these rows.
    let elsewhere = call(
        &router,
        OTHER_REGION,
        "POST",
        "/clusters",
        &minimal_request("present").to_string(),
    )
    .await;
    assert_eq!(
        elsewhere.status, 200,
        "setup CreateCluster in {OTHER_REGION}: got {elsewhere:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let elsewhere_addon = call(
        &router,
        OTHER_REGION,
        "POST",
        "/clusters/present/addons",
        &serde_json::json!({ "addonName": "coredns" }).to_string(),
    )
    .await;
    assert_eq!(
        elsewhere_addon.status, 200,
        "setup CreateAddon in {OTHER_REGION}: got {elsewhere_addon:?}"
    );

    // (case, the Region the delete is signed for, cluster, add-on, preserve)
    let cases: [(&str, &str, &str, &str, Option<bool>); 5] = [
        (
            "no cluster of that name in this Region",
            REGION,
            "absent",
            "vpc-cni",
            None,
        ),
        (
            "the cluster exists, but holds no add-on of that name",
            REGION,
            "present",
            "kube-proxy",
            Some(false),
        ),
        (
            "the add-on is installed on the same-named cluster in another Region",
            REGION,
            "present",
            "coredns",
            Some(true),
        ),
        (
            "the other Region's cluster holds no add-on of that name",
            OTHER_REGION,
            "present",
            "vpc-cni",
            None,
        ),
        (
            "neither half of the pair exists",
            OTHER_REGION,
            "absent",
            "absent-addon",
            Some(false),
        ),
    ];

    for (case, region, cluster, addon, preserve) in &cases {
        let answer = delete_addon_in(&router, region, cluster, addon, *preserve).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 for a pair this Region does not hold, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
        );
    }

    // Nothing was removed by any of those refusals, in either Region.
    let still_here = describe_addon(&router, "present", "vpc-cni").await;
    assert_eq!(
        still_here.status, 200,
        "a refused delete removes nothing in this Region, got {still_here:?}"
    );
    let still_elsewhere = call(
        &router,
        OTHER_REGION,
        "GET",
        "/clusters/present/addons/coredns",
        "",
    )
    .await;
    assert_eq!(
        still_elsewhere.status, 200,
        "a refused delete removes nothing in {OTHER_REGION}, got {still_elsewhere:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a delete removes the ONE add-on it names, so the other add-ons of the
// cluster are untouched and keep being listed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_one_addon_leaves_the_cluster_its_others() {
    let router = emulator();
    active_cluster(&router, "many-addons").await;

    // The names are installed in a different order from the sorted order
    // `ListAddons` reports, so the assertions below are about the SET the store
    // holds rather than about insertion order.
    let installed = ["kube-proxy", "coredns", "vpc-cni", "eks-pod-identity-agent"];
    for name in &installed {
        active_addon(
            &router,
            "many-addons",
            &serde_json::json!({ "addonName": name }),
        )
        .await;
    }

    // Rows: delete in an order that takes the middle of the list first, then an
    // end, then the rest — the count the cluster holds drops by exactly one each
    // time and the survivors still read back.
    let order = ["vpc-cni", "eks-pod-identity-agent", "coredns", "kube-proxy"];
    let mut remaining: Vec<String> = {
        let mut names: Vec<String> = installed.iter().map(|name| (*name).to_string()).collect();
        names.sort();
        names
    };
    for deleted_name in &order {
        let deleted = delete_addon(&router, "many-addons", deleted_name, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {deleted_name:?}: expected 200 from DeleteAddon, got {deleted:?}"
        );
        remaining.retain(|name| name != deleted_name);
        // The removal is asynchronous, so the deleted add-on is off the cluster
        // once the clock has finished it.
        tokio::time::sleep(ADDON_DELETION_DELAY + Duration::from_millis(250)).await;

        let listed = list_addons(&router, "many-addons").await;
        assert_eq!(
            listed, remaining,
            "case {deleted_name:?}: the cluster must still hold exactly its other \
             add-ons, got {listed:?}"
        );
        for survivor in &remaining {
            let described = describe_addon(&router, "many-addons", survivor).await;
            assert_eq!(
                described.status, 200,
                "case {deleted_name:?}: the surviving add-on {survivor:?} must still read \
                 back, got {described:?}"
            );
            assert_ne!(
                described.addon()["status"],
                serde_json::json!("DELETING"),
                "case {deleted_name:?}: the surviving add-on {survivor:?} is not deleting, \
                 got {:?}",
                described.addon()["status"]
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a change an add-on update is still carrying can no longer be made once
// the add-on it would change is gone, so that update stops being owed its
// completion and never reports success.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_in_flight_when_the_addon_is_deleted_does_not_succeed() {
    let router = emulator();
    active_cluster(&router, "interrupted").await;

    // Rows differ along what the in-flight update asked to change: a member of the
    // add-on itself, and a member plus the set of child associations it owns. Each row
    // installs a DIFFERENT add-on the catalog publishes, and one whose install settles
    // ACTIVE on a cluster with no compute (a per-node workload with no prerequisite
    // add-on), because only a settled add-on takes an update.
    let cases: Vec<(&str, &str, serde_json::Value, Option<bool>)> = vec![
        (
            "a new service account role",
            "vpc-cni",
            serde_json::json!({ "serviceAccountRoleArn": OTHER_ROLE_ARN }),
            None,
        ),
        (
            "a new service account role and a new association set",
            "kube-proxy",
            serde_json::json!({
                "serviceAccountRoleArn": OTHER_ROLE_ARN,
                "podIdentityAssociations": [
                    { "serviceAccount": "updated-sa", "roleArn": OTHER_ROLE_ARN },
                ],
            }),
            Some(true),
        ),
    ];

    for (case, addon, change, preserve) in cases.iter() {
        let addon_name = (*addon).to_string();
        active_addon(
            &router,
            "interrupted",
            &serde_json::json!({
                "addonName": addon_name,
                "serviceAccountRoleArn": ROLE_ARN,
            }),
        )
        .await;

        let update = call(
            &router,
            REGION,
            "POST",
            &format!(
                "/clusters/interrupted/addons/{}/update",
                encode_label(&addon_name)
            ),
            &change.to_string(),
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {case:?}: setup UpdateAddon must start an update, got {update:?}"
        );
        let update_id = update.body["update"]["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            !update_id.is_empty(),
            "case {case:?}: the update must carry an id, got {update:?}"
        );

        let deleted = delete_addon(&router, "interrupted", &addon_name, *preserve).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200 from DeleteAddon, got {deleted:?}"
        );

        // Past the instant the update would have been owed its completion.
        tokio::time::sleep(UPDATE_COMPLETION_DELAY + Duration::from_millis(250)).await;
        let described = call(
            &router,
            REGION,
            "GET",
            &format!(
                "/clusters/interrupted/updates/{}?addonName={}",
                encode_label(&update_id),
                encode_label(&addon_name)
            ),
            "",
        )
        .await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the update of a deleted add-on is still described, got \
             {described:?}"
        );
        assert_ne!(
            described.body["update"]["status"],
            serde_json::json!("Successful"),
            "case {case:?}: an update whose add-on is gone cannot have carried its change \
             out, got {:?}",
            described.body["update"]["status"]
        );
        assert_eq!(
            described.body["update"]["status"],
            serde_json::json!("Failed"),
            "case {case:?}: an update whose change can no longer be made reports Failed, \
             got {:?}",
            described.body["update"]["status"]
        );
    }
}
