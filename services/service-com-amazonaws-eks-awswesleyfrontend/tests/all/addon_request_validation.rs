// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REQUEST-VALIDATION layer of the EKS add-on write
//! operations: which `addonName`, which `addonVersion` and which `resolveConflicts`
//! a `CreateAddon` (and, for the two members it shares, an `UpdateAddon`) is allowed
//! to carry, and the sentence each refusal is worded with.
//!
//! Three rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one recorded request:
//!
//! 1. the add-on asked for must be one the catalog publishes a build of FOR THE
//!    CLUSTER'S OWN Kubernetes version — "the name must match one of the names
//!    returned by DescribeAddonVersions" — so the rows differ along how the name
//!    misses that set, and along the cluster the request is sent to, which is where
//!    the Kubernetes version in the refusal comes from;
//! 2. a build the request pins must be one the catalog offers that cluster, so the
//!    rows differ along why the build is not offered (published for other Kubernetes
//!    versions only, published for another add-on, not published at all) and along
//!    the operation that pins it;
//! 3. `resolveConflicts` names one of the values the modelled enum declares,
//!    without regard to case, so the rows differ along how the value spells no
//!    declared option at all and along the operation it was sent to, which the
//!    refusal names — and the accepted rows along the case it is spelled in.
//!
//! Every set the rows are drawn from — the published add-ons, the builds offered for
//! a Kubernetes version, the Kubernetes versions themselves — is read out of the
//! catalog the emulator serves (`DescribeAddonVersions`, `DescribeClusterVersions`),
//! so no row repeats a constant the service also holds.
//!
//! The request harness is the one every suite in this crate shares, from
//! `tests/all/wire.rs`.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ADDON_ACTIVATION_DELAY, CLUSTER_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_lifecycle::{
    catalog_builds, create_addon, default_cluster_version, describe_addon, published_addon_names,
    published_cluster_versions,
};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// The `message` a refusal carries, which is what a client reads to see what it did
/// wrong.
fn message_of(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a message, got {:?}", answer.body))
        .to_string()
}

/// The smallest `CreateCluster` request, at one Kubernetes version when the case
/// names one.
fn cluster_request(name: &str, version: Option<&str>) -> serde_json::Value {
    let mut body = serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    if let Some(version) = version {
        body["version"] = serde_json::json!(version);
    }
    body
}

/// One cluster, waited into `ACTIVE` so the add-on layers that need a running
/// control plane are past.
async fn active_cluster(router: &MultiServiceRouter, name: &str, version: Option<&str>) {
    let answer = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request(name, version).to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup CreateCluster {name:?}: got {answer:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// `UpdateAddon` over its modelled route.
async fn update_addon(
    router: &MultiServiceRouter,
    cluster: &str,
    addon: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/addons/{}/update",
            encode_label(cluster),
            encode_label(addon)
        ),
        &body.to_string(),
    )
    .await
}

/// Install one add-on and wait out the span the clock owes it, so it has settled and
/// can take an update.
async fn active_addon(router: &MultiServiceRouter, cluster: &str, addon: &str) {
    let created = create_addon(
        router,
        cluster,
        &serde_json::json!({ "addonName": addon }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup CreateAddon {addon:?} on {cluster:?}: got {created:?}"
    );
    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

/// Every build the catalog publishes of one add-on, whatever Kubernetes version it
/// is for — the listing narrowed by `addonName` alone.
async fn every_build_of(router: &MultiServiceRouter, addon_name: &str) -> Vec<String> {
    let answer = call(
        router,
        REGION,
        "GET",
        &format!(
            "/addons/supported-versions?addonName={}&maxResults=100",
            encode_label(addon_name)
        ),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions {addon_name:?}: got {answer:?}"
    );
    answer.body["addons"]
        .as_array()
        .and_then(|addons| addons.first())
        .and_then(|addon| addon.get("addonVersions"))
        .and_then(serde_json::Value::as_array)
        .map(|builds| {
            builds
                .iter()
                .filter_map(|build| build["addonVersion"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The builds of one add-on the catalog publishes but does NOT offer a cluster of
/// `kubernetes_version`: a real catalog version whose `compatibilities` leave that
/// version out.
async fn builds_not_offered_for(
    router: &MultiServiceRouter,
    addon_name: &str,
    kubernetes_version: &str,
) -> Vec<String> {
    let offered: Vec<String> = catalog_builds(router, addon_name, kubernetes_version)
        .await
        .iter()
        .filter_map(|build| build["addonVersion"].as_str().map(str::to_owned))
        .collect();
    every_build_of(router, addon_name)
        .await
        .into_iter()
        .filter(|build| !offered.contains(build))
        .collect()
}

// ---------------------------------------------------------------------------
// RULE 1: `CreateAddon` installs only an add-on the catalog publishes a build of
// for the cluster's own Kubernetes version. A name outside that set is refused
// 400 `InvalidParameterException` with `Addon <addonName> specified is not
// supported in <the cluster's Kubernetes version> kubernetes version`, and
// nothing is written.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_addon_name_the_catalog_does_not_publish_for_the_cluster_is_refused() {
    let router = emulator();
    let published = published_addon_names(&router).await;
    let calendar = published_cluster_versions(&router).await;
    let newest = calendar
        .first()
        .expect("the calendar publishes a version")
        .clone();
    let default_version = default_cluster_version(&router).await;
    // A published add-on the catalog publishes no build of for every one of the
    // Kubernetes versions below: Amazon EKS began offering it on a later version, so a
    // cluster on one of those is offered it under no name at all. (A Kubernetes
    // version the calendar does not publish cannot stand in for one here:
    // `CreateCluster` refuses a version Amazon EKS does not offer.)
    let mut uncovered: Vec<(String, Vec<String>)> = Vec::new();
    for addon_name in &published {
        let mut versions: Vec<String> = Vec::new();
        for version in &calendar {
            if catalog_builds(&router, addon_name, version).await.is_empty() {
                versions.push(version.clone());
            }
        }
        if versions.len() >= 2 {
            uncovered.push((addon_name.clone(), versions));
        }
    }
    let (introduced_later, uncovered_versions) = uncovered.first().cloned().unwrap_or_else(|| {
        panic!(
            "setup: the catalog must publish an add-on it offers no build of for two of the \
             published Kubernetes versions {calendar:?}"
        )
    });

    active_cluster(&router, "newest", Some(&newest)).await;
    active_cluster(&router, "defaulted", None).await;
    active_cluster(&router, "uncovered-first", Some(&uncovered_versions[0])).await;
    active_cluster(&router, "uncovered-second", Some(&uncovered_versions[1])).await;

    // (case, cluster, that cluster's Kubernetes version, the add-on name asked for).
    // The rows differ along HOW the name misses the set the cluster is offered: a
    // name nothing publishes, a near miss of a published name (a separator swapped,
    // the case changed, a suffix added), a name of the Marketplace form, and a name
    // the catalog DOES publish — on a cluster whose Kubernetes version it publishes
    // no build of that add-on for, which is the same miss from the other side.
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a name nothing publishes",
            "newest",
            &newest,
            "sim2real-no-such-addon".to_string(),
        ),
        (
            "a published name with its separator swapped",
            "newest",
            &newest,
            "vpc_cni".to_string(),
        ),
        (
            "a published name in another case",
            "defaulted",
            &default_version,
            "CoreDNS".to_string(),
        ),
        (
            "a published name with a suffix",
            "defaulted",
            &default_version,
            "coredns-extra".to_string(),
        ),
        (
            "a name of the Marketplace form that nothing publishes",
            "defaulted",
            &default_version,
            "vendor_product.v2".to_string(),
        ),
        (
            "a published name on a cluster of a Kubernetes version the catalog publishes no \
             build of it for",
            "uncovered-first",
            &uncovered_versions[0],
            introduced_later.clone(),
        ),
        (
            "that same published name on a cluster of another such version",
            "uncovered-second",
            &uncovered_versions[1],
            introduced_later.clone(),
        ),
    ];

    for (case, cluster, kubernetes_version, addon_name) in &rows {
        let answer = create_addon(
            &router,
            cluster,
            &serde_json::json!({ "addonName": addon_name }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        let expected = format!(
            "Addon {addon_name} specified is not supported in {kubernetes_version} kubernetes \
             version"
        );
        assert_eq!(
            message_of(&answer),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            message_of(&answer)
        );
        // A refused create writes nothing: the add-on it named cannot be read back.
        let described = describe_addon(&router, cluster, addon_name).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must store nothing, got {described:?}"
        );
    }

    // The accepted side of the same rule, on a row the issue did not cite: a name
    // the catalog publishes for the cluster's Kubernetes version installs, and the
    // version it lands on is one the catalog offers that cluster.
    for addon_name in ["aws-mountpoint-s3-csi-driver", "kube-state-metrics"] {
        let answer = create_addon(
            &router,
            "newest",
            &serde_json::json!({ "addonName": addon_name }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {addon_name:?}: a published add-on installs, got {answer:?}"
        );
        let offered: Vec<String> = catalog_builds(&router, addon_name, &newest)
            .await
            .iter()
            .filter_map(|build| build["addonVersion"].as_str().map(str::to_owned))
            .collect();
        let installed = answer.addon()["addonVersion"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            offered.contains(&installed),
            "case {addon_name:?}: the installed build must be one the catalog offers \
             {newest}, got {installed:?} of {offered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a build an add-on request PINS must be one the catalog offers the
// cluster's Kubernetes version. A build outside that set is refused 400
// `InvalidParameterException` with the fixed sentence `Addon version specified
// is not supported`, by the install and by the update alike, and nothing is
// written.
// ---------------------------------------------------------------------------

/// The sentence that refusal carries. Fixed text, no variable part: the build asked
/// for is already in the caller's own request.
const VERSION_REFUSAL: &str = "Addon version specified is not supported";

#[tokio::test]
async fn an_addon_version_the_catalog_does_not_offer_the_cluster_is_refused() {
    let router = emulator();
    let version = default_cluster_version(&router).await;
    active_cluster(&router, "pinned", None).await;

    // A REAL catalog build of this add-on that the cluster's Kubernetes version is
    // not offered: published for other Kubernetes versions only.
    let stale = builds_not_offered_for(&router, "vpc-cni", &version).await;
    let stale_build = stale
        .first()
        .unwrap_or_else(|| {
            panic!("setup: the catalog must publish a vpc-cni build not offered for {version}")
        })
        .clone();
    let oldest_stale = stale
        .last()
        .expect("setup: at least one build not offered")
        .clone();
    // A build of ANOTHER add-on, which this add-on's own version line never carries.
    let other_addons_build = catalog_builds(&router, "coredns", &version)
        .await
        .first()
        .map(|build| {
            build["addonVersion"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .expect("setup: the catalog offers a coredns build");

    // (case, the build the request pins). The rows differ along WHY the build is not
    // one the cluster is offered.
    let rows: Vec<(&str, String)> = vec![
        (
            "a real build of this add-on published for other Kubernetes versions",
            stale_build.clone(),
        ),
        (
            "the oldest build of this add-on, also published for other versions",
            oldest_stale,
        ),
        (
            "a real build of ANOTHER add-on",
            other_addons_build.clone(),
        ),
        (
            "a build in the published form that nothing publishes",
            "v9.99.0-eksbuild.1".to_string(),
        ),
        ("a build that is not a version at all", "latest".to_string()),
    ];

    for (case, build) in &rows {
        let answer = create_addon(
            &router,
            "pinned",
            &serde_json::json!({ "addonName": "vpc-cni", "addonVersion": build }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        assert_eq!(
            message_of(&answer),
            VERSION_REFUSAL,
            "case {case:?}: expected {VERSION_REFUSAL:?}, got {:?}",
            message_of(&answer)
        );
        let described = describe_addon(&router, "pinned", "vpc-cni").await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must store nothing, got {described:?}"
        );
    }

    // Every build the catalog DOES offer that cluster is accepted and is the build
    // the add-on then reports. One add-on per build, since one name installs once.
    let offered: Vec<String> = catalog_builds(&router, "vpc-cni", &version)
        .await
        .iter()
        .filter_map(|build| build["addonVersion"].as_str().map(str::to_owned))
        .collect();
    assert!(
        !offered.is_empty(),
        "setup: the catalog must offer a vpc-cni build for {version}"
    );
    for (index, build) in offered.iter().enumerate() {
        let cluster = format!("offered-{index}");
        active_cluster(&router, &cluster, None).await;
        let answer = create_addon(
            &router,
            &cluster,
            &serde_json::json!({ "addonName": "vpc-cni", "addonVersion": build }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {build:?}: a build the catalog offers is accepted, got {answer:?}"
        );
        let described = describe_addon(&router, &cluster, "vpc-cni").await;
        assert_eq!(
            described.addon()["addonVersion"],
            serde_json::json!(build),
            "case {build:?}: the pinned build must read back, got {:?}",
            described.addon()["addonVersion"]
        );
    }

    // The same rule holds for the operation that moves an installed add-on to
    // another build: an update to a build the cluster is not offered is refused with
    // the same sentence, and the add-on keeps the build it was running.
    active_addon(&router, "pinned", "vpc-cni").await;
    let running = describe_addon(&router, "pinned", "vpc-cni")
        .await
        .addon()["addonVersion"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    for (case, build) in &rows {
        let answer = update_addon(
            &router,
            "pinned",
            "vpc-cni",
            &serde_json::json!({ "addonVersion": build }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} (UpdateAddon): expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?} (UpdateAddon): expected InvalidParameterException, got {answer:?}"
        );
        assert_eq!(
            message_of(&answer),
            VERSION_REFUSAL,
            "case {case:?} (UpdateAddon): expected {VERSION_REFUSAL:?}, got {:?}",
            message_of(&answer)
        );
        let described = describe_addon(&router, "pinned", "vpc-cni").await;
        assert_eq!(
            described.addon()["addonVersion"],
            serde_json::json!(running),
            "case {case:?} (UpdateAddon): a refused update must change nothing, got {:?}",
            described.addon()["addonVersion"]
        );
    }

    // An update to a build the catalog DOES offer starts an update instead. The
    // cluster's Kubernetes version is offered more than one build of this add-on — the
    // build published for it and the one published for the version after it — so a
    // target other than the one it already runs always exists.
    let target = offered
        .iter()
        .find(|build| *build != &running)
        .unwrap_or_else(|| {
            panic!(
                "setup: the catalog must offer a vpc-cni build for {version} other than the \
                 installed {running:?}, got {offered:?}"
            )
        });
    let answer = update_addon(
        &router,
        "pinned",
        "vpc-cni",
        &serde_json::json!({ "addonVersion": target }),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {target:?} (UpdateAddon): a build the catalog offers is accepted, got {answer:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 3: `resolveConflicts` names one of the values the modelled enum
// declares, WITHOUT regard to case — a value that spells a declared option in
// another case names that option. A value that spells none of them is refused
// 400 `InvalidParameterException` with `Resolve conflicts option <value> is not
// supported for operation <the operation it was sent to>`, and nothing is
// written.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_resolve_conflicts_value_outside_the_enum_is_refused_in_this_services_wording() {
    let router = emulator();
    active_cluster(&router, "conflicts", None).await;

    // Source for the accepted rows: the `ResolveConflicts` enum values the model
    // declares (`smithy_query query="shape" shape="ResolveConflicts"` reports
    // OVERWRITE | NONE | PRESERVE). The refused rows differ along HOW the value
    // spells no declared option in any case: unknown, a declared value with
    // padding, a comma-joined pair, and the empty string.
    let refused = ["BANANA", " NONE", "NONE,OVERWRITE", ""];

    for (index, value) in refused.iter().enumerate() {
        let addon_name = format!("refused-{index}");
        let answer = create_addon(
            &router,
            "conflicts",
            // The add-on name is one nothing publishes, which the catalog layer would
            // also refuse: this value is refused BEFORE any of that, so the sentence
            // is about `resolveConflicts` whatever else the body says.
            &serde_json::json!({ "addonName": addon_name, "resolveConflicts": value }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {value:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {value:?}: expected InvalidParameterException, got {answer:?}"
        );
        let expected =
            format!("Resolve conflicts option {value} is not supported for operation CreateAddon");
        assert_eq!(
            message_of(&answer),
            expected,
            "case {value:?}: expected {expected:?}, got {:?}",
            message_of(&answer)
        );
        let described = describe_addon(&router, "conflicts", &addon_name).await;
        assert_eq!(
            described.status, 404,
            "case {value:?}: a refused create must store nothing, got {described:?}"
        );
    }

    // The sentence rewords a refusal; it does not outrank the framework's own
    // validating walk. A body that ALSO breaks a modelled constraint — here
    // `serviceAccountRoleArn`, whose `RoleArn` target is `@length(1..255)`, given an
    // empty value — keeps the whole violation report, which one sentence about one
    // member cannot carry.
    let both_wrong = create_addon(
        &router,
        "conflicts",
        &serde_json::json!({
            "addonName": "vpc-cni",
            "serviceAccountRoleArn": "",
            "resolveConflicts": "BANANA",
        }),
    )
    .await;
    assert_eq!(
        both_wrong.status, 400,
        "case a modelled constraint broken too: expected 400, got {both_wrong:?}"
    );
    assert!(
        message_of(&both_wrong).starts_with("2 validation errors detected: ")
            && message_of(&both_wrong).contains("serviceAccountRoleArn"),
        "case a modelled constraint broken too: the framework's violation report must \
         stand, got {:?}",
        message_of(&both_wrong)
    );

    // The same member on the UPDATE operation is refused the same way, naming THAT
    // operation: the operation is the sentence's second variable part.
    active_addon(&router, "conflicts", "vpc-cni").await;
    for value in &refused {
        let answer = update_addon(
            &router,
            "conflicts",
            "vpc-cni",
            &serde_json::json!({ "resolveConflicts": value }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {value:?} (UpdateAddon): expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {value:?} (UpdateAddon): expected InvalidParameterException, got {answer:?}"
        );
        let expected =
            format!("Resolve conflicts option {value} is not supported for operation UpdateAddon");
        assert_eq!(
            message_of(&answer),
            expected,
            "case {value:?} (UpdateAddon): expected {expected:?}, got {:?}",
            message_of(&answer)
        );
    }

    // Every value the enum declares is accepted, on both operations, and so is a
    // value that spells one of them in another case: the member is read WITHOUT
    // regard to case, which is how the recorded service reads it. One add-on per
    // row, since one name installs at most once per cluster, and each name is one
    // the catalog offers this cluster's Kubernetes version — read out of the
    // catalog rather than written here.
    let accepted = [
        "NONE",
        "OVERWRITE",
        "PRESERVE",
        "overwrite",
        "Preserve",
        "none",
    ];
    let version = default_cluster_version(&router).await;
    let mut installable: Vec<String> = Vec::new();
    for name in published_addon_names(&router).await {
        if !catalog_builds(&router, &name, &version).await.is_empty() {
            installable.push(name);
        }
    }
    assert!(
        installable.len() >= accepted.len(),
        "setup: the catalog must offer at least {} add-ons on {version}, got {}",
        accepted.len(),
        installable.len()
    );
    for (index, value) in accepted.iter().enumerate() {
        let addon_name = installable[index].clone();
        let answer = create_addon(
            &router,
            "conflicts",
            &serde_json::json!({ "addonName": addon_name, "resolveConflicts": value }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {value:?}: a value that names a declared option is accepted, got {answer:?}"
        );
        let update = update_addon(
            &router,
            "conflicts",
            "vpc-cni",
            &serde_json::json!({ "resolveConflicts": value }),
        )
        .await;
        assert_eq!(
            update.status, 200,
            "case {value:?} (UpdateAddon): a value that names a declared option is \
             accepted, got {update:?}"
        );
        // One add-on takes one update at a time, so the update just started is waited
        // out before the next row starts another.
        tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    }
}
