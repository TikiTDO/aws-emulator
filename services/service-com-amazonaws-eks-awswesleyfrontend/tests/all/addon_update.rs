// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `UpdateAddon`: the asynchronous update of an installed
//! add-on, driven over the wire through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! Every suite here asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which members the
//! request asks to change, the shape of a value, whether a list is absent, empty or
//! populated, which half of the `(cluster, add-on)` pair is missing), and each
//! assertion names its case and prints the actual value.
//!
//! The request harness (`Answer`, `emulator`, `call`, `encode_label`) is the one
//! every suite in this crate shares, from `tests/all/wire.rs`; the add-on install
//! and read-back are the helpers `tests/all/addon_lifecycle.rs` already exposes.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    addon_not_updatable_message, ADDON_ACTIVATION_DELAY, CLUSTER_ACTIVATION_DELAY,
    UPDATE_COMPLETION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::nodegroup_lifecycle::{create_nodegroup_in, minimal_request as nodegroup_request};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The region every request in this suite is signed for — the one the shared
/// add-on helpers use.
const REGION: &str = "us-west-2";

/// The Kubernetes version the clusters here run.
///
/// A mid-calendar version on purpose: the add-on catalog publishes one build per
/// Kubernetes version and a build supports the version it was released on AND the
/// one before it, so a version with a newer version after it in the calendar has
/// MORE than one published build that supports it — which is what gives an add-on
/// installed at its default build somewhere to be upgraded to.
const KUBERNETES_VERSION: &str = "1.33";

const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AddonRole";
/// A SECOND role an add-on's service account can be given, for the rows that change
/// which role the add-on runs under.
///
/// A role of THIS account: `serviceAccountRoleArn` is a role the request asks Amazon
/// EKS to pass, so one of another account is refused 403 before anything else about
/// the request is judged ([`crate::addon_request_precedence`]).
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/OtherAddonRole";
/// A THIRD role of this account, for the association another owner holds.
const CONTESTING_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ContestingRole";

/// A configuration document every published add-on's schema accepts:
/// `priorityClassName` is one of the keys EVERY add-on's configuration schema
/// declares (source: the common properties
/// `DescribeAddonConfiguration` publishes for each add-on, read in this crate's
/// `addon_versions` suite), and it is declared as a string.
const VALID_CONFIGURATION: &str = "{\"priorityClassName\":\"system-cluster-critical\"}";

/// The smallest `CreateCluster` request, at [`KUBERNETES_VERSION`].
fn cluster_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "version": KUBERNETES_VERSION,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

/// One cluster in [`REGION`], waited into `ACTIVE` so the add-on layer that
/// requires a running control plane is past, and carrying one managed node group
/// so the add-ons installed on it have compute to place their replicas onto —
/// which is what lets an install of one settle running rather than degraded.
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let answer = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request(name).to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup CreateCluster {name:?}: got {answer:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let nodes = create_nodegroup_in(router, REGION, name, &nodegroup_request("nodes")).await;
    assert_eq!(
        nodes.status, 200,
        "setup CreateNodegroup on {name:?}: got {nodes:?}"
    );
}

/// Install one add-on and wait out the span the clock owes it, so it is `ACTIVE`
/// and can take an update.
async fn active_addon(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> serde_json::Value {
    let answer = create_addon(router, cluster, body).await;
    assert_eq!(
        answer.status, 200,
        "setup CreateAddon {body}: got {answer:?}"
    );
    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let described = describe_addon(
        router,
        cluster,
        body["addonName"].as_str().expect("an addon name"),
    )
    .await;
    assert_eq!(
        described.status, 200,
        "setup DescribeAddon {body}: got {described:?}"
    );
    described.addon().clone()
}

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

/// `DescribeUpdate` for an update of a resource inside the cluster: `addonName`
/// is what narrows the lookup to one add-on's updates.
async fn describe_update(
    router: &MultiServiceRouter,
    cluster: &str,
    update_id: &str,
    query: &str,
) -> Answer {
    let path = format!(
        "/clusters/{}/updates/{}",
        encode_label(cluster),
        encode_label(update_id)
    );
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

async fn list_updates(router: &MultiServiceRouter, cluster: &str, query: &str) -> Answer {
    let path = format!("/clusters/{}/updates", encode_label(cluster));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

async fn list_associations(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/pod-identity-associations",
            encode_label(cluster)
        ),
        "",
    )
    .await
}

/// Wait out the span the clock owes an in-flight update before it is `Successful`.
async fn wait_for_update() {
    tokio::time::sleep(UPDATE_COMPLETION_DELAY + Duration::from_millis(250)).await;
}

fn update_of(answer: &Answer) -> &serde_json::Value {
    &answer.body["update"]
}

fn update_id(answer: &Answer) -> String {
    update_of(answer)["id"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The `UpdateParamType` keys an update reports, in the order it reports them.
fn param_types(answer: &Answer) -> Vec<String> {
    update_of(answer)["params"]
        .as_array()
        .map(|params| {
            params
                .iter()
                .filter_map(|param| param["type"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// The value an update reports for one `UpdateParamType` key.
fn param_value(answer: &Answer, key: &str) -> Option<String> {
    update_of(answer)["params"]
        .as_array()?
        .iter()
        .find(|param| param["type"].as_str() == Some(key))?
        .get("value")?
        .as_str()
        .map(str::to_owned)
}

/// Every build the catalog publishes for one add-on on [`KUBERNETES_VERSION`],
/// read through `DescribeAddonVersions` — so this suite upgrades an add-on to a
/// build the catalog a caller reads actually offers, rather than to a constant.
async fn published_versions(router: &MultiServiceRouter, addon: &str) -> Vec<String> {
    let answer = call(
        router,
        REGION,
        "GET",
        &format!(
            "/addons/supported-versions?addonName={}&kubernetesVersion={}",
            encode_label(addon),
            encode_label(KUBERNETES_VERSION)
        ),
        "",
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions {addon:?}: got {answer:?}"
    );
    answer.body["addons"]
        .as_array()
        .and_then(|addons| addons.first())
        .and_then(|entry| entry.get("addonVersions"))
        .and_then(serde_json::Value::as_array)
        .map(|builds| {
            builds
                .iter()
                .filter_map(|build| build["addonVersion"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// A published build of `addon` that is not the one `installed` names — the build
/// an upgrade moves the add-on to.
async fn upgrade_target(router: &MultiServiceRouter, addon: &str, installed: &str) -> String {
    let published = published_versions(router, addon).await;
    published
        .iter()
        .find(|version| version.as_str() != installed)
        .cloned()
        .unwrap_or_else(|| {
            panic!(
                "the catalog must publish a build of {addon:?} for Kubernetes \
                 {KUBERNETES_VERSION} other than the installed {installed:?}, got {published:?}"
            )
        })
}

// ---------------------------------------------------------------------------
// RULE: `UpdateAddon` starts an ASYNCHRONOUS update of one installed add-on. It
// answers an `Update` that is `InProgress` and of type `AddonUpdate`, reporting
// one parameter per member the request asked to change; the add-on itself reports
// `UPDATING` and its OLD values until the clock completes that update, and only
// then reports exactly the requested change — the members the request did not
// name keep what they had.
// ---------------------------------------------------------------------------

/// One row of the table: which add-on it installs, with what, what it then asks to
/// change, and what the add-on must report once the update completes.
struct Case {
    case: &'static str,
    install: serde_json::Value,
    request: serde_json::Value,
    /// The `UpdateParamType` keys the answered update must carry, in order.
    params: Vec<&'static str>,
    /// `(member, value)` pairs the add-on must report after the update completes;
    /// `serde_json::Value::Null` means the member must be absent.
    expected: Vec<(&'static str, serde_json::Value)>,
}

#[tokio::test]
async fn an_addon_update_applies_exactly_what_the_request_asked_to_change() {
    let router = emulator();
    let cluster = "addon-update-cluster";
    active_cluster(&router, cluster).await;

    // Each row drives its own add-on of the published catalog, so no row's update
    // is the next row's conflict, and the add-on's own version line is the one its
    // upgrade target is taken from.
    let cni_installed = active_addon(&router, cluster, &serde_json::json!({ "addonName": "vpc-cni" }))
        .await;
    let cni_version = cni_installed["addonVersion"]
        .as_str()
        .expect("the installed version")
        .to_owned();
    let cni_target = upgrade_target(&router, "vpc-cni", &cni_version).await;

    let observability_installed = active_addon(
        &router,
        cluster,
        &serde_json::json!({
            "addonName": "amazon-cloudwatch-observability",
            "serviceAccountRoleArn": ROLE_ARN,
        }),
    )
    .await;
    let observability_version = observability_installed["addonVersion"]
        .as_str()
        .expect("the installed version")
        .to_owned();
    let observability_target =
        upgrade_target(&router, "amazon-cloudwatch-observability", &observability_version).await;

    let cases = vec![
        Case {
            case: "a new addonVersion alone",
            install: serde_json::json!({ "addonName": "vpc-cni" }),
            request: serde_json::json!({ "addonVersion": cni_target }),
            params: vec!["AddonVersion"],
            expected: vec![
                ("addonVersion", serde_json::json!(cni_target)),
                // Not named by the request, so it keeps what the install gave it:
                // nothing at all.
                ("serviceAccountRoleArn", serde_json::Value::Null),
            ],
        },
        Case {
            case: "a new serviceAccountRoleArn alone",
            install: serde_json::json!({ "addonName": "coredns", "serviceAccountRoleArn": ROLE_ARN }),
            request: serde_json::json!({ "serviceAccountRoleArn": OTHER_ROLE_ARN }),
            params: vec!["ServiceAccountRoleArn"],
            expected: vec![("serviceAccountRoleArn", serde_json::json!(OTHER_ROLE_ARN))],
        },
        Case {
            case: "a configuration document alone",
            install: serde_json::json!({ "addonName": "kube-proxy" }),
            request: serde_json::json!({ "configurationValues": VALID_CONFIGURATION }),
            params: vec!["ConfigurationValues"],
            expected: vec![("configurationValues", serde_json::json!(VALID_CONFIGURATION))],
        },
        Case {
            case: "pod identity associations where the add-on owned none",
            install: serde_json::json!({ "addonName": "aws-ebs-csi-driver" }),
            request: serde_json::json!({
                "podIdentityAssociations": [
                    { "serviceAccount": "ebs-csi-controller-sa", "roleArn": ROLE_ARN },
                ],
            }),
            params: vec!["PodIdentityAssociations"],
            expected: vec![],
        },
        Case {
            case: "an empty association list, which deletes the ones the add-on owns",
            install: serde_json::json!({
                "addonName": "external-dns",
                "podIdentityAssociations": [
                    { "serviceAccount": "external-dns", "roleArn": ROLE_ARN },
                ],
            }),
            request: serde_json::json!({ "podIdentityAssociations": [] }),
            params: vec!["PodIdentityAssociations"],
            expected: vec![("podIdentityAssociations", serde_json::Value::Null)],
        },
        Case {
            case: "every member at once",
            install: serde_json::json!({
                "addonName": "amazon-cloudwatch-observability",
                "serviceAccountRoleArn": ROLE_ARN,
            }),
            request: serde_json::json!({
                "addonVersion": observability_target,
                "serviceAccountRoleArn": OTHER_ROLE_ARN,
                "configurationValues": VALID_CONFIGURATION,
                "podIdentityAssociations": [
                    { "serviceAccount": "cloudwatch-agent", "roleArn": ROLE_ARN },
                ],
                "resolveConflicts": "OVERWRITE",
            }),
            params: vec![
                "AddonVersion",
                "ServiceAccountRoleArn",
                "ConfigurationValues",
                "PodIdentityAssociations",
                "ResolveConflicts",
            ],
            expected: vec![
                ("addonVersion", serde_json::json!(observability_target)),
                ("serviceAccountRoleArn", serde_json::json!(OTHER_ROLE_ARN)),
                ("configurationValues", serde_json::json!(VALID_CONFIGURATION)),
            ],
        },
        Case {
            case: "a request that asks for no change at all",
            install: serde_json::json!({ "addonName": "aws-efs-csi-driver" }),
            request: serde_json::json!({}),
            params: vec![],
            expected: vec![],
        },
    ];

    for case in &cases {
        let addon = case.install["addonName"].as_str().expect("an addon name");
        // The first two add-ons are already installed above, which is what their
        // upgrade target was read from; the rest are installed here.
        let before = if addon == "vpc-cni" {
            cni_installed.clone()
        } else if addon == "amazon-cloudwatch-observability" && case.case == "every member at once" {
            observability_installed.clone()
        } else {
            active_addon(&router, cluster, &case.install).await
        };

        let started = update_addon(&router, cluster, addon, &case.request).await;
        assert_eq!(
            started.status, 200,
            "case {:?}: expected 200, got {started:?}",
            case.case
        );
        assert_eq!(
            update_of(&started)["status"],
            serde_json::json!("InProgress"),
            "case {:?}: a started update is InProgress, got {:?}",
            case.case,
            update_of(&started)["status"]
        );
        assert_eq!(
            update_of(&started)["type"],
            serde_json::json!("AddonUpdate"),
            "case {:?}: an add-on's update is an AddonUpdate, got {:?}",
            case.case,
            update_of(&started)["type"]
        );
        assert_eq!(
            param_types(&started),
            case.params,
            "case {:?}: expected one param per member asked to change, got {:?}",
            case.case,
            update_of(&started)["params"]
        );
        let id = update_id(&started);
        assert!(
            !id.is_empty(),
            "case {:?}: an update must carry an id, got {:?}",
            case.case,
            update_of(&started)
        );
        // Each parameter's VALUE is what the request asked for: a scalar member
        // verbatim, a structured member as the JSON document it arrived as.
        for key in &case.params {
            let reported = param_value(&started, key).unwrap_or_else(|| {
                panic!(
                    "case {:?}: the update must report a value for {key}, got {:?}",
                    case.case,
                    update_of(&started)["params"]
                )
            });
            let member = match *key {
                "AddonVersion" => "addonVersion",
                "ServiceAccountRoleArn" => "serviceAccountRoleArn",
                "ConfigurationValues" => "configurationValues",
                "PodIdentityAssociations" => "podIdentityAssociations",
                "ResolveConflicts" => "resolveConflicts",
                other => panic!("case {:?}: unexpected update param {other}", case.case),
            };
            let asked = &case.request[member];
            let reported_value = match asked {
                serde_json::Value::String(_) => serde_json::Value::String(reported.clone()),
                _ => serde_json::from_str(&reported).unwrap_or(serde_json::Value::Null),
            };
            assert_eq!(
                &reported_value, asked,
                "case {:?}: expected {key} to report the {member} the request asked for, got \
                 {reported:?}",
                case.case
            );
        }

        // The update is ASYNCHRONOUS: until the clock completes it the add-on is
        // `UPDATING` and still reports every value it had.
        let during = describe_addon(&router, cluster, addon).await;
        assert_eq!(
            during.addon()["status"],
            serde_json::json!("UPDATING"),
            "case {:?}: the add-on is UPDATING while its update runs, got {:?}",
            case.case,
            during.addon()["status"]
        );
        for member in [
            "addonVersion",
            "serviceAccountRoleArn",
            "configurationValues",
            "podIdentityAssociations",
        ] {
            assert_eq!(
                during.addon()[member],
                before[member],
                "case {:?}: {member} must not change before the update completes, got {:?}",
                case.case,
                during.addon()[member]
            );
        }

        wait_for_update().await;

        let completed = describe_update(&router, cluster, &id, &format!("addonName={addon}")).await;
        assert_eq!(
            completed.status, 200,
            "case {:?}: the add-on's update is reported for that add-on, got {completed:?}",
            case.case
        );
        assert_eq!(
            update_of(&completed)["status"],
            serde_json::json!("Successful"),
            "case {:?}: the clock owes the update Successful, got {:?}",
            case.case,
            update_of(&completed)["status"]
        );

        let after = describe_addon(&router, cluster, addon).await;
        assert_eq!(
            after.addon()["status"],
            serde_json::json!("ACTIVE"),
            "case {:?}: a completed update hands the add-on back to ACTIVE, got {:?}",
            case.case,
            after.addon()["status"]
        );
        for (member, value) in &case.expected {
            assert_eq!(
                after.addon()[member],
                *value,
                "case {:?}: expected {member} {value:?}, got {:?}",
                case.case,
                after.addon()[member]
            );
        }
        // Whatever the request did not name is still what it was.
        let unchanged: Vec<&str> = [
            "addonName",
            "clusterName",
            "addonArn",
            "createdAt",
            "owner",
            "publisher",
            "namespaceConfig",
        ]
        .into_iter()
        .collect();
        for member in unchanged {
            assert_eq!(
                after.addon()[member],
                before[member],
                "case {:?}: {member} is not something an update changes, got {:?}",
                case.case,
                after.addon()[member]
            );
        }
    }

    // The associations the two association rows asked for are the set those
    // add-ons own: the one row that named an entry has it, and the row that named
    // an empty list has none.
    let listed = list_associations(&router, cluster).await;
    assert_eq!(
        listed.status, 200,
        "ListPodIdentityAssociations must answer, got {listed:?}"
    );
    let summaries = listed.body["associations"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let service_accounts: Vec<&str> = summaries
        .iter()
        .filter_map(|summary| summary["serviceAccount"].as_str())
        .collect();
    assert!(
        service_accounts.contains(&"ebs-csi-controller-sa"),
        "an update that named an association creates it, got {service_accounts:?}"
    );
    assert!(
        service_accounts.contains(&"cloudwatch-agent"),
        "an update that named an association creates it, got {service_accounts:?}"
    );
    assert!(
        !service_accounts.contains(&"adot-collector"),
        "an update that named an EMPTY list deletes the associations the add-on owned, got \
         {service_accounts:?}"
    );
    // Each association an add-on owns reports that add-on as its owner.
    for summary in &summaries {
        assert!(
            summary["ownerArn"].as_str().is_some_and(|arn| !arn.is_empty()),
            "an association an add-on owns reports its ownerArn, got {summary:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an update of an add-on belongs to THAT add-on — it is what
// `DescribeUpdate` and `ListUpdates` narrow to with `addonName`, and never
// reported for the cluster's own control plane or for another add-on — and
// starting one leaves the cluster itself alone.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_addon_update_is_reported_for_that_addon_alone() {
    let router = emulator();
    let cluster = "addon-update-scope";
    active_cluster(&router, cluster).await;
    active_addon(&router, cluster, &serde_json::json!({ "addonName": "vpc-cni" })).await;
    active_addon(&router, cluster, &serde_json::json!({ "addonName": "coredns" })).await;

    let started = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({ "serviceAccountRoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(started.status, 200, "the update must start, got {started:?}");
    let id = update_id(&started);

    // The scope the lookup is given decides whether the update is found: only the
    // add-on it is an update OF reports it.
    let lookups: Vec<(&str, &str, u16)> = vec![
        ("the add-on the update belongs to", "addonName=vpc-cni", 200),
        ("another add-on of the same cluster", "addonName=coredns", 404),
        ("the cluster's own control plane", "", 404),
        ("a node group scope", "nodegroupName=vpc-cni", 404),
        ("a capability scope", "capabilityName=vpc-cni", 404),
    ];
    for (case, query, expected) in &lookups {
        let answer = describe_update(&router, cluster, &id, query).await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected {expected}, got {answer:?}"
        );
        if *expected == 404 {
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {answer:?}"
            );
        }
    }

    // Naming two resources names no single one, which is a bad parameter rather
    // than a resource that is absent.
    let ambiguous = describe_update(
        &router,
        cluster,
        &id,
        "addonName=vpc-cni&nodegroupName=nodes",
    )
    .await;
    assert_eq!(
        ambiguous.status, 400,
        "naming two scoping members is refused, got {ambiguous:?}"
    );
    assert_eq!(
        error_code(&ambiguous),
        "InvalidParameterException",
        "naming two scoping members is an invalid parameter, got {ambiguous:?}"
    );

    // The same selection in the listing: the add-on's updates are listed for the
    // add-on, and not among the cluster's own.
    let listings: Vec<(&str, &str, bool)> = vec![
        ("the add-on the update belongs to", "addonName=vpc-cni", true),
        ("another add-on of the same cluster", "addonName=coredns", false),
        ("the cluster's own control plane", "", false),
    ];
    for (case, query, holds) in &listings {
        let answer = list_updates(&router, cluster, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        let ids: Vec<&str> = answer.body["updateIds"]
            .as_array()
            .map(|items| items.iter().filter_map(serde_json::Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            ids.contains(&id.as_str()),
            *holds,
            "case {case:?}: expected the listing to hold the update: {holds}, got {ids:?}"
        );
    }

    // An add-on's update is not a change to the control plane, so the cluster is
    // never taken out of `ACTIVE` for it.
    for when in ["while the add-on update runs", "after it completes"] {
        let cluster_answer = call(
            &router,
            REGION,
            "GET",
            &format!("/clusters/{}", encode_label(cluster)),
            "",
        )
        .await;
        assert_eq!(
            cluster_answer.cluster()["status"],
            serde_json::json!("ACTIVE"),
            "case {when:?}: the cluster stays ACTIVE, got {:?}",
            cluster_answer.cluster()["status"]
        );
        wait_for_update().await;
    }
}

// ---------------------------------------------------------------------------
// RULE: one add-on takes one update at a time — a second request while its
// update runs is a 409 `ResourceInUseException` — unless it replays the
// `clientRequestToken` of the call that started that update, which is answered
// with THAT update rather than starting another.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn one_addon_takes_one_update_at_a_time() {
    let router = emulator();
    let cluster = "addon-update-conflict";
    active_cluster(&router, cluster).await;
    active_addon(&router, cluster, &serde_json::json!({ "addonName": "vpc-cni" })).await;
    active_addon(&router, cluster, &serde_json::json!({ "addonName": "coredns" })).await;

    let token = "a4d2d3fa-0b6c-4b95-9f0c-1f0ad4e4fcb1";
    let started = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({
            "serviceAccountRoleArn": ROLE_ARN,
            "clientRequestToken": token,
        }),
    )
    .await;
    assert_eq!(started.status, 200, "the update must start, got {started:?}");
    let id = update_id(&started);

    // The rows differ along what the second call carries: the same token (a retry),
    // a different token, and no token at all.
    let retried = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({
            "serviceAccountRoleArn": ROLE_ARN,
            "clientRequestToken": token,
        }),
    )
    .await;
    assert_eq!(
        retried.status, 200,
        "case \"a retry of the same call\": expected 200, got {retried:?}"
    );
    assert_eq!(
        update_id(&retried),
        id,
        "case \"a retry of the same call\": expected the update it already started, got {:?}",
        update_of(&retried)
    );

    for (case, body) in [
        (
            "a second call with another token",
            serde_json::json!({
                "serviceAccountRoleArn": OTHER_ROLE_ARN,
                "clientRequestToken": "0e0b2b1e-6d7a-4f02-9c5e-6a3c2a0f5d11",
            }),
        ),
        (
            "a second call with no token",
            serde_json::json!({ "serviceAccountRoleArn": OTHER_ROLE_ARN }),
        ),
        (
            "a second call that asks for nothing",
            serde_json::json!({}),
        ),
    ] {
        let answer = update_addon(&router, cluster, "vpc-cni", &body).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceInUseException",
            "case {case:?}: expected ResourceInUseException, got {answer:?}"
        );
        assert!(
            answer.body["message"]
                .as_str()
                .is_some_and(|message| message.contains(&id)),
            "case {case:?}: the refusal names the update already in progress, got {answer:?}"
        );
    }

    // ANOTHER add-on of the same cluster is not in use: its own update starts.
    let other = update_addon(
        &router,
        cluster,
        "coredns",
        &serde_json::json!({ "serviceAccountRoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        other.status, 200,
        "another add-on's update is not blocked by this one, got {other:?}"
    );
    assert_ne!(
        update_id(&other),
        id,
        "each add-on's update is its own record, got {:?}",
        update_of(&other)
    );

    // Once the update has completed the add-on takes another one.
    wait_for_update().await;
    let again = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({ "serviceAccountRoleArn": OTHER_ROLE_ARN }),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "a completed update releases the add-on, got {again:?}"
    );
    assert_ne!(
        update_id(&again),
        id,
        "a new call mints a new update, got {:?}",
        update_of(&again)
    );
}

// ---------------------------------------------------------------------------
// RULE: a request that cannot be carried out is refused with the modelled error
// for what was wrong with it — the missing resource, the state of the cluster or
// the add-on, or the member of the body at fault — and writes nothing.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unanswerable_update_is_refused_and_writes_nothing() {
    let router = emulator();
    let cluster = "addon-update-refusals";
    active_cluster(&router, cluster).await;
    let before = active_addon(
        &router,
        cluster,
        &serde_json::json!({ "addonName": "vpc-cni", "serviceAccountRoleArn": ROLE_ARN }),
    )
    .await;
    // One association of another owner, so a request that asks for the same
    // `(namespace, service account)` pair double-books it.
    let held = call(
        &router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/pod-identity-associations",
            encode_label(cluster)
        ),
        &serde_json::json!({
            "namespace": "kube-system",
            "serviceAccount": "taken-by-the-caller",
            "roleArn": ROLE_ARN,
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        held.status, 200,
        "setup CreatePodIdentityAssociation: got {held:?}"
    );

    // The rows span every layer that can refuse: the pair that could not be
    // resolved, and each member of the body a rule holds.
    let cases: Vec<(&str, &str, &str, serde_json::Value, u16, &str)> = vec![
        (
            "a cluster this region does not hold",
            "no-such-cluster",
            "vpc-cni",
            serde_json::json!({}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an add-on that is not installed",
            cluster,
            "not-installed",
            serde_json::json!({}),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a serviceAccountRoleArn this caller may not pass",
            cluster,
            "vpc-cni",
            serde_json::json!({ "serviceAccountRoleArn": "AddonRole" }),
            403,
            "AccessDeniedException",
        ),
        (
            "a serviceAccountRoleArn carrying a region (IAM is global)",
            cluster,
            "vpc-cni",
            serde_json::json!({
                "serviceAccountRoleArn": "arn:aws:iam:us-west-2:000000000000:role/AddonRole",
            }),
            403,
            "AccessDeniedException",
        ),
        (
            "configurationValues that open a JSON object and do not close it",
            cluster,
            "vpc-cni",
            serde_json::json!({ "configurationValues": "{\"a\":" }),
            400,
            "InvalidParameterException",
        ),
        (
            "configurationValues setting a key the add-on's schema does not declare",
            cluster,
            "vpc-cni",
            serde_json::json!({ "configurationValues": "{\"notAProperty\":true}" }),
            400,
            "InvalidParameterException",
        ),
        (
            "an association roleArn that is not a role ARN",
            cluster,
            "vpc-cni",
            serde_json::json!({
                "podIdentityAssociations": [
                    { "serviceAccount": "sa", "roleArn": "arn:aws:iam::000000000000:role/" },
                ],
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "a serviceAccount no Kubernetes object can be named",
            cluster,
            "vpc-cni",
            serde_json::json!({
                "podIdentityAssociations": [
                    { "serviceAccount": "Not A Name", "roleArn": ROLE_ARN },
                ],
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "the same serviceAccount mapped twice",
            cluster,
            "vpc-cni",
            serde_json::json!({
                "podIdentityAssociations": [
                    { "serviceAccount": "sa", "roleArn": ROLE_ARN },
                    { "serviceAccount": "sa", "roleArn": ROLE_ARN },
                ],
            }),
            400,
            "InvalidParameterException",
        ),
        (
            "an association for a pair another owner already holds",
            cluster,
            "vpc-cni",
            serde_json::json!({
                "podIdentityAssociations": [
                    { "serviceAccount": "taken-by-the-caller", "roleArn": ROLE_ARN },
                ],
            }),
            409,
            "ResourceInUseException",
        ),
    ];

    for (case, target_cluster, addon, request, status, code) in &cases {
        let answer = update_addon(&router, target_cluster, addon, request).await;
        assert_eq!(
            answer.status, *status,
            "case {case:?}: expected {status}, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            *code,
            "case {case:?}: expected {code}, got {answer:?}"
        );
        // A refused request starts no update and changes nothing.
        let listed = list_updates(&router, cluster, "addonName=vpc-cni").await;
        let count = listed.body["updateIds"]
            .as_array()
            .map(Vec::len)
            .unwrap_or_default();
        assert_eq!(
            count, 0,
            "case {case:?}: a refused request must start no update, got {listed:?}"
        );
        let described = describe_addon(&router, cluster, "vpc-cni").await;
        assert_eq!(
            described.addon(),
            &before,
            "case {case:?}: a refused request must leave the add-on as it was, got {:?}",
            described.addon()
        );
    }

    // The same operation on a cluster `RegisterCluster` connected: it has no
    // Amazon EKS-managed plane for an add-on to live in, which is the permanent
    // client refusal rather than a state error.
    let connected = call(
        &router,
        REGION,
        "POST",
        "/cluster-registrations",
        &serde_json::json!({
            "name": "connected-cluster",
            "connectorConfig": {
                "roleArn": "arn:aws:iam::000000000000:role/eksConnectorRole",
                "provider": "OTHER",
            },
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        connected.status, 200,
        "setup RegisterCluster: got {connected:?}"
    );
    let refused = update_addon(
        &router,
        "connected-cluster",
        "vpc-cni",
        &serde_json::json!({}),
    )
    .await;
    assert_eq!(
        refused.status, 400,
        "a connected cluster has no add-on to update, got {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ClientException",
        "a connected cluster is refused permanently, got {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an add-on update's change is made when the clock completes it, so a
// change that can no longer be made then FAILS the update: the add-on keeps
// every value it had, reports `UPDATE_FAILED` with the issue that says why, and
// the update reports `Failed` instead of the `Successful` the clock owed it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_whose_change_can_no_longer_be_made_fails() {
    let router = emulator();
    let cluster = "addon-update-conflicting";
    active_cluster(&router, cluster).await;
    let before = active_addon(&router, cluster, &serde_json::json!({ "addonName": "vpc-cni" })).await;

    let started = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({
            "podIdentityAssociations": [
                { "serviceAccount": "contested-sa", "roleArn": ROLE_ARN },
            ],
        }),
    )
    .await;
    assert_eq!(started.status, 200, "the update must start, got {started:?}");
    let id = update_id(&started);

    // Another owner takes the very pair the in-flight update asked for, BEFORE the
    // clock completes it — the one window in which the pair is free when the
    // update is accepted and taken when it is applied.
    let taken = call(
        &router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/pod-identity-associations",
            encode_label(cluster)
        ),
        &serde_json::json!({
            "namespace": "kube-system",
            "serviceAccount": "contested-sa",
            "roleArn": CONTESTING_ROLE_ARN,
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        taken.status, 200,
        "the competing association must be created, got {taken:?}"
    );

    wait_for_update().await;

    let completed = describe_update(&router, cluster, &id, "addonName=vpc-cni").await;
    assert_eq!(
        completed.status, 200,
        "the update is still reported, got {completed:?}"
    );
    assert_eq!(
        update_of(&completed)["status"],
        serde_json::json!("Failed"),
        "a change that could not be made fails the update, got {:?}",
        update_of(&completed)["status"]
    );

    let after = describe_addon(&router, cluster, "vpc-cni").await;
    assert_eq!(
        after.addon()["status"],
        serde_json::json!("UPDATE_FAILED"),
        "the add-on reports the failure, got {:?}",
        after.addon()["status"]
    );
    assert_eq!(
        after.addon()["health"]["issues"][0]["code"],
        serde_json::json!("ConfigurationConflict"),
        "the add-on carries the issue that says why, got {:?}",
        after.addon()["health"]
    );
    assert_eq!(
        after.addon()["health"]["issues"][0]["resourceIds"],
        serde_json::json!(["contested-sa"]),
        "the issue names the service account at fault, got {:?}",
        after.addon()["health"]
    );
    // The conflict names the IAM role the service account is ALREADY bound to —
    // the role of the association another owner holds, not the one this update
    // asked for — which is what a reconciliation loop reads it for.
    assert_eq!(
        after.addon()["health"]["issues"][0]["message"],
        serde_json::json!(format!(
            "The service account is already associated with a different IAM role: \
             {CONTESTING_ROLE_ARN}"
        )),
        "the issue names the role already bound, got {:?}",
        after.addon()["health"]
    );
    // Nothing of the add-on changed: a failed update writes none of what it asked
    // for, and the association the other owner holds is still theirs alone.
    for member in [
        "addonVersion",
        "serviceAccountRoleArn",
        "configurationValues",
        "podIdentityAssociations",
    ] {
        assert_eq!(
            after.addon()[member], before[member],
            "a failed update must change no member of the add-on, got {member} = {:?}",
            after.addon()[member]
        );
    }
    let listed = list_associations(&router, cluster).await;
    let owners: Vec<&serde_json::Value> = listed.body["associations"]
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    assert_eq!(
        owners.len(),
        1,
        "the cluster holds the one association its own caller created, got {listed:?}"
    );
    assert_eq!(
        owners[0]["ownerArn"],
        serde_json::Value::Null,
        "that association is the caller's, owned by no add-on, got {:?}",
        owners[0]
    );

    // An add-on whose update failed has SETTLED, so another update is how it is
    // remediated: it is accepted, and completing it clears the issue the failure
    // left behind.
    let remediation = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({ "serviceAccountRoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        remediation.status, 200,
        "an add-on whose update failed takes another one, got {remediation:?}"
    );
    wait_for_update().await;
    let remediated = describe_addon(&router, cluster, "vpc-cni").await;
    assert_eq!(
        remediated.addon()["status"],
        serde_json::json!("ACTIVE"),
        "a successful update brings the add-on back, got {:?}",
        remediated.addon()["status"]
    );
    assert_eq!(
        remediated.addon()["health"]["issues"],
        serde_json::json!([]),
        "a successful update clears the issue the failure left, got {:?}",
        remediated.addon()["health"]
    );
    assert_eq!(
        remediated.addon()["serviceAccountRoleArn"],
        serde_json::json!(ROLE_ARN),
        "the remediating update's own change is applied, got {:?}",
        remediated.addon()["serviceAccountRoleArn"]
    );
}

// ---------------------------------------------------------------------------
// RULE: an add-on that has not SETTLED cannot take an update: one still being
// created is in use by its own install, which is the modelled 409
// `ResourceInUseException` — a refusal a caller that waits can retry — worded
// with the add-on the request named and the status it is currently in, and
// carrying both the add-on and its cluster.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_addon_that_is_not_active_cannot_take_an_update() {
    let router = emulator();
    let cluster = "addon-update-state";
    active_cluster(&router, cluster).await;

    // No wait after the install, so the add-on is still `CREATING`.
    let created = create_addon(&router, cluster, &serde_json::json!({ "addonName": "vpc-cni" })).await;
    assert_eq!(created.status, 200, "setup CreateAddon: got {created:?}");
    assert_eq!(
        created.addon()["status"],
        serde_json::json!("CREATING"),
        "setup: a new add-on is CREATING, got {:?}",
        created.addon()["status"]
    );

    let refused = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({ "serviceAccountRoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        refused.status, 409,
        "an add-on that has not settled cannot take an update, got {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceInUseException",
        "an add-on its own install still holds is in use, got {refused:?}"
    );
    assert_eq!(
        refused.body["message"].as_str(),
        Some(addon_not_updatable_message("vpc-cni", "CREATING").as_str()),
        "the refusal names the add-on and the status it is in, got {:?}",
        refused.body["message"]
    );
    for (member, expected) in [("addonName", "vpc-cni"), ("clusterName", cluster)] {
        assert_eq!(
            refused.body[member].as_str(),
            Some(expected),
            "case {member:?}: the refusal reports the add-on it could not update, got {:?}",
            refused.body[member]
        );
    }

    // Once the clock has made it `ACTIVE` the same request is taken.
    tokio::time::sleep(ADDON_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let accepted = update_addon(
        &router,
        cluster,
        "vpc-cni",
        &serde_json::json!({ "serviceAccountRoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "an ACTIVE add-on takes the same request, got {accepted:?}"
    );
}
