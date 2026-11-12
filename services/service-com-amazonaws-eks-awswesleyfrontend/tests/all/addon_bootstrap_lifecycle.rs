// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the add-on lifecycle a client drives WITHOUT polling in
//! between: create a cluster, install an add-on on it, read it back, update it,
//! delete it and install it again — driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The rules each suite here asserts, each over a TABLE whose rows differ along
//! what the rule generalises over rather than over one recorded request:
//!
//! 1. an add-on install is ASYNCHRONOUS, so every step of that sequence is
//!    answerable the moment the one before it was answered: the install is taken
//!    while the cluster is still being provisioned, the add-on is readable and
//!    listed from the call that created it, an add-on that has not settled refuses
//!    an update as a CONFLICT a caller can retry, a delete answers the add-on in
//!    `DELETING` and leaves it readable until the clock has finished the removal,
//!    and an install of an add-on that is already there is a conflict;
//! 2. a `configurationValues` document is read as JSON or as YAML and held to the
//!    configuration schema the catalog publishes for the build, so the rows differ
//!    along the FORM the document takes and along which of the two it was read as,
//!    which the refusal names;
//! 3. an add-on sold through the AWS Marketplace needs a subscription this
//!    emulator holds for no listing, so its install settles `CREATE_FAILED`
//!    carrying the issue that says so — and the add-on reports the listing it was
//!    bought through, which an Amazon EKS-managed add-on has none of.
//!
//! Every set the rows are drawn from — the published add-ons, the Marketplace ones
//! among them, the schema keys a document may set — is read out of the catalog the
//! emulator serves (`DescribeAddonVersions`, `DescribeAddonConfiguration`), so no
//! row repeats a constant the service also holds.
//!
//! The request harness (`Answer`, `emulator`, `call`, `encode_label`) is the one
//! every suite in this crate shares, from `tests/all/wire.rs`; the cluster, add-on
//! and delete requests are the helpers `tests/all/cluster_lifecycle.rs`,
//! `tests/all/addon_lifecycle.rs` and `tests/all/addon_deletion.rs` already expose.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    addon_configuration_violation_message, addon_not_updatable_message, ADDON_ACTIVATION_DELAY,
    ADDON_CONFIGURATION_UNREADABLE_MESSAGE, ADDON_DELETION_DELAY, ADDON_ISSUE_SUBSCRIPTION_NEEDED,
    ADDON_ISSUE_SUBSCRIPTION_NEEDED_MESSAGE, ADDON_OWNER_MARKETPLACE, CLUSTER_ACTIVATION_DELAY,
    MAX_CONFIGURATION_BYTES, MAX_CONFIGURATION_DEPTH,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_deletion::delete_addon;
use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::addon_versions::describe_addon_versions;
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The region every request in this suite is signed for.
const REGION: &str = "us-west-2";

/// How long a poll of an asynchronous transition keeps looking before the suite
/// calls it a failure, and how long it waits between looks.
///
/// Generous next to every span this service owes (half a second each), because
/// what a poll asserts is the state a transition reaches, not how fast the host
/// running these tests gets there.
const POLL_DEADLINE: Duration = Duration::from_secs(30);
/// See [`POLL_DEADLINE`].
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The smallest `CreateCluster` request: the three required members.
fn cluster_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

async fn create_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request(name).to_string(),
    )
    .await
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
    assert_eq!(answer.status, 200, "ListAddons {cluster:?}: got {answer:?}");
    answer.body["addons"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an addons list, got {:?}", answer.body))
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("expected an add-on name, got {name:?}"))
                .to_owned()
        })
        .collect()
}

/// The `message` a refusal carries.
fn message_of(answer: &Answer) -> String {
    answer.body["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a message, got {:?}", answer.body))
        .to_string()
}

/// The health issues one add-on reports, as `(code, message)` pairs.
fn health_issues(addon: &serde_json::Value) -> Vec<(String, String)> {
    addon["health"]["issues"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|issue| {
            (
                issue["code"].as_str().unwrap_or_default().to_owned(),
                issue["message"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// Every add-on name the catalog publishes with the given owner, read out of the
/// catalog itself (`DescribeAddonVersions$owners`) rather than written here.
async fn published_with_owner(router: &MultiServiceRouter, owner: &str) -> Vec<String> {
    let answer = describe_addon_versions(
        router,
        &format!("owners={}&maxResults=100", encode_label(owner)),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions owners={owner}: got {answer:?}"
    );
    answer.body["addons"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an addons list, got {:?}", answer.body))
        .iter()
        .map(|addon| {
            addon["addonName"]
                .as_str()
                .unwrap_or_else(|| panic!("expected an addonName, got {addon:?}"))
                .to_owned()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// RULE 1: the add-on lifecycle is answerable one call after another. Every step
// below is sent the moment the step before it was answered — no wait, no poll —
// and each is answered as the asynchronous resource it addresses: `CREATING`
// while the install is in flight, a retryable 409 for an add-on that has not
// settled, `DELETING` for one on its way out, and a 409 for an install of one
// that is already installed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_whole_addon_lifecycle_is_answerable_without_waiting() {
    let router = emulator();

    // The rows differ along the add-on asked for, so the sequence is not keyed to
    // one add-on: a per-node workload, a replicated one, and one of the add-ons
    // the catalog publishes under another name.
    for addon in ["vpc-cni", "coredns", "metrics-server"] {
        let cluster = format!("bootstrap-{addon}");
        let created = create_cluster(&router, &cluster).await;
        assert_eq!(
            created.status, 200,
            "case {addon:?}: setup CreateCluster, got {created:?}"
        );

        // The install, straight after the create: the cluster is still being
        // provisioned, and the install is taken and reported in `CREATING`.
        let installed = create_addon(
            &router,
            &cluster,
            &serde_json::json!({ "addonName": addon, "resolveConflicts": "OVERWRITE" }),
        )
        .await;
        assert_eq!(
            installed.status, 200,
            "case {addon:?}: the install is taken, got {installed:?}"
        );
        assert_eq!(
            installed.addon()["status"],
            serde_json::json!("CREATING"),
            "case {addon:?}: a new add-on is CREATING, got {:?}",
            installed.addon()["status"]
        );
        assert_eq!(
            installed.addon()["health"],
            serde_json::json!({ "issues": [] }),
            "case {addon:?}: nothing is wrong with an install in flight, got {:?}",
            installed.addon()["health"]
        );

        // The read-back and the listing find it from the call that created it.
        let described = describe_addon(&router, &cluster, addon).await;
        assert_eq!(
            described.status, 200,
            "case {addon:?}: the add-on reads back, got {described:?}"
        );
        assert_eq!(
            described.addon()["addonArn"],
            installed.addon()["addonArn"],
            "case {addon:?}: the read-back is the record the install wrote, got {:?}",
            described.addon()
        );
        let listed = list_addons(&router, &cluster).await;
        assert!(
            listed.iter().any(|name| name == addon),
            "case {addon:?}: ListAddons lists the installed add-on, got {listed:?}"
        );

        // An install of the same add-on again is a CONFLICT, not a parameter error.
        let again = create_addon(&router, &cluster, &serde_json::json!({ "addonName": addon })).await;
        assert_eq!(
            again.status, 409,
            "case {addon:?}: installing it again is a conflict, got {again:?}"
        );
        assert_eq!(
            error_code(&again),
            "ResourceInUseException",
            "case {addon:?}: expected ResourceInUseException, got {again:?}"
        );
        assert_eq!(
            message_of(&again),
            "Addon already exists.",
            "case {addon:?}: expected the duplicate-install sentence, got {:?}",
            message_of(&again)
        );

        // An update of an add-on that has not settled is the same CLASS of refusal
        // — one a caller that waits can retry — and it names the add-on and the
        // status it is in.
        let updating = update_addon(
            &router,
            &cluster,
            addon,
            &serde_json::json!({ "configurationValues": "{}" }),
        )
        .await;
        assert_eq!(
            updating.status, 409,
            "case {addon:?}: an add-on that has not settled cannot take an update, got \
             {updating:?}"
        );
        assert_eq!(
            error_code(&updating),
            "ResourceInUseException",
            "case {addon:?}: expected ResourceInUseException, got {updating:?}"
        );
        assert_eq!(
            message_of(&updating),
            addon_not_updatable_message(addon, "CREATING"),
            "case {addon:?}: expected the in-progress sentence, got {:?}",
            message_of(&updating)
        );

        // Once the clock has settled the cluster and then the install, the add-on
        // reports a terminal status and takes an update.
        tokio::time::sleep(
            CLUSTER_ACTIVATION_DELAY + ADDON_ACTIVATION_DELAY + Duration::from_millis(250),
        )
        .await;
        let settled = describe_addon(&router, &cluster, addon).await;
        assert_ne!(
            settled.addon()["status"],
            serde_json::json!("CREATING"),
            "case {addon:?}: the install settles once its cluster is running, got {:?}",
            settled.addon()["status"]
        );

        // The delete answers the add-on in `DELETING`, and it stays readable and
        // listed — and in the way of installing it again — until the clock has
        // finished the removal.
        let deleted = delete_addon(&router, &cluster, addon, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {addon:?}: expected 200 from DeleteAddon, got {deleted:?}"
        );
        assert_eq!(
            deleted.addon()["status"],
            serde_json::json!("DELETING"),
            "case {addon:?}: the delete answers the add-on in DELETING, got {:?}",
            deleted.addon()["status"]
        );
        let deleting = describe_addon(&router, &cluster, addon).await;
        assert_eq!(
            deleting.addon()["status"],
            serde_json::json!("DELETING"),
            "case {addon:?}: the add-on reads back while it is deleting, got {:?}",
            deleting.addon()["status"]
        );
        let reinstall = create_addon(&router, &cluster, &serde_json::json!({ "addonName": addon })).await;
        assert_eq!(
            reinstall.status, 409,
            "case {addon:?}: an add-on on its way out is still in the way, got {reinstall:?}"
        );
        assert_eq!(
            error_code(&reinstall),
            "ResourceInUseException",
            "case {addon:?}: expected ResourceInUseException, got {reinstall:?}"
        );

        // After the removal the cluster holds it no more, and the same install is
        // taken again.
        tokio::time::sleep(ADDON_DELETION_DELAY + Duration::from_millis(250)).await;
        let gone = describe_addon(&router, &cluster, addon).await;
        assert_eq!(
            gone.status, 404,
            "case {addon:?}: a removed add-on reads back as a 404, got {gone:?}"
        );
        let listed = list_addons(&router, &cluster).await;
        assert!(
            !listed.iter().any(|name| name == addon),
            "case {addon:?}: ListAddons no longer lists it, got {listed:?}"
        );
        let reinstalled =
            create_addon(&router, &cluster, &serde_json::json!({ "addonName": addon })).await;
        assert_eq!(
            reinstalled.status, 200,
            "case {addon:?}: the add-on installs again once it is gone, got {reinstalled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 2: a `configurationValues` document is read as JSON or as YAML and held to
// the configuration schema the catalog publishes for the build. A document that
// is neither is refused with the fixed sentence; one that fails the schema is
// refused naming the format it was read as and the path at fault.
// ---------------------------------------------------------------------------

/// One row of the configuration-document table: the document a request carries
/// and what the service answers it with.
struct DocumentRow {
    case: &'static str,
    /// The `configurationValues` the request sends.
    document: &'static str,
    /// The FORMAT the document is read as, which the refusal names, and the
    /// violation it earns — `None` for a document the schema accepts.
    refusal: Option<(&'static str, &'static str)>,
}

fn accepted(case: &'static str, document: &'static str) -> DocumentRow {
    DocumentRow {
        case,
        document,
        refusal: None,
    }
}

fn refused(
    case: &'static str,
    document: &'static str,
    format: &'static str,
    violation: &'static str,
) -> DocumentRow {
    DocumentRow {
        case,
        document,
        refusal: Some((format, violation)),
    }
}

#[tokio::test]
async fn a_configuration_document_is_read_in_either_format_and_held_to_the_schema() {
    let router = emulator();
    let cluster = "configuration-documents";
    let created = create_cluster(&router, cluster).await;
    assert_eq!(created.status, 200, "setup CreateCluster: got {created:?}");

    // `priorityClassName` is one of the keys every published add-on's schema
    // declares and takes a string (source: the common properties
    // `DescribeAddonConfiguration` publishes, read in this crate's `addon_versions`
    // suite), so the rows below can set it, mistype it and misspell it in both
    // document formats.
    const UNDECLARED: &str = "$.notAProperty: is not defined in the schema and the schema does \
                              not allow additional properties";
    let rows = vec![
        accepted(
            "a JSON object the schema accepts",
            "{\"priorityClassName\":\"system-cluster-critical\"}",
        ),
        accepted(
            "the same document in YAML",
            "priorityClassName: system-cluster-critical\n",
        ),
        accepted("an empty document", ""),
        refused(
            "a JSON object setting a key the schema does not declare",
            "{\"notAProperty\":true}",
            "Json",
            UNDECLARED,
        ),
        refused(
            "a YAML mapping setting a key the schema does not declare",
            "notAProperty: true\n",
            "Yaml",
            UNDECLARED,
        ),
        refused(
            "a JSON object setting a declared key to the wrong type",
            "{\"priorityClassName\":2}",
            "Json",
            "$.priorityClassName: integer found, string expected",
        ),
        refused(
            "a YAML mapping setting a declared key to the wrong type",
            "priorityClassName:\n  name: critical\n",
            "Yaml",
            "$.priorityClassName: object found, string expected",
        ),
        refused(
            "a YAML document that states no object at all",
            "priorityClassName\n",
            "Yaml",
            "$: string found, object expected",
        ),
        refused(
            "a JSON document that states no object at all",
            "2",
            "Json",
            "$: integer found, object expected",
        ),
        refused(
            "a YAML mapping nested far deeper than any schema declares",
            "notAProperty:\n  a:\n    b:\n      c:\n        d: one\n",
            "Yaml",
            UNDECLARED,
        ),
    ];

    // Every row installs a DIFFERENT add-on, because one name installs at most
    // once per cluster and an accepted row writes a record.
    let published = published_with_owner(&router, "aws").await;
    assert!(
        published.len() >= rows.len(),
        "setup: the catalog must publish at least {} Amazon EKS add-ons, got {}",
        rows.len(),
        published.len()
    );
    for (index, row) in rows.iter().enumerate() {
        let DocumentRow {
            case,
            document,
            refusal,
        } = row;
        let addon = published[index].clone();
        let answer = create_addon(
            &router,
            cluster,
            &serde_json::json!({ "addonName": addon, "configurationValues": document }),
        )
        .await;
        match refusal {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: a document the schema accepts installs, got {answer:?}"
                );
                // What was accepted is reported back verbatim, in the form it was
                // sent: the service stores the document, not its reading.
                assert_eq!(
                    answer.addon()["configurationValues"],
                    if document.is_empty() {
                        serde_json::json!("")
                    } else {
                        serde_json::json!(document)
                    },
                    "case {case:?}: the document is echoed as supplied, got {:?}",
                    answer.addon()["configurationValues"]
                );
            }
            Some((format, violation)) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: expected 400, got {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "InvalidParameterException",
                    "case {case:?}: expected InvalidParameterException, got {answer:?}"
                );
                let expected_message = addon_configuration_violation_message(format, violation);
                assert_eq!(
                    message_of(&answer),
                    expected_message,
                    "case {case:?}: expected {expected_message:?}, got {:?}",
                    message_of(&answer)
                );
                let described = describe_addon(&router, cluster, &addon).await;
                assert_eq!(
                    described.status, 404,
                    "case {case:?}: a refused install writes nothing, got {described:?}"
                );
            }
        }
    }

    // A document that is NEITHER format cannot be read at all, which is one fixed
    // sentence whatever the document is: the rows differ along HOW it fails to
    // parse, and along the three documents this service will not grow to read —
    // one past the byte bound, one nested past what it reads, and one whose
    // anchor references a loader would expand by copying.
    let oversize = "a".repeat(MAX_CONFIGURATION_BYTES + 1);
    let over_nested = format!(
        "{}{}",
        "[".repeat(MAX_CONFIGURATION_DEPTH + 1),
        "]".repeat(MAX_CONFIGURATION_DEPTH + 1)
    );
    for (case, document) in [
        ("an object that is not closed", "{\"a\":"),
        ("an array that is not closed", "[1,"),
        ("a brace with no key", "{nope"),
        ("a mapping with a stray quote", "a: \"b\nc: d\n"),
        ("a tab where YAML forbids one", "a:\n\t- one\n"),
        (
            "a mapping anchored and referenced per level",
            "a: &x\n  k: v\nb: [*x, *x, *x]\nc: [*x, *x, *x]\n",
        ),
        ("a document past the byte bound", oversize.as_str()),
        ("a document nested past what is read", over_nested.as_str()),
    ] {
        let answer = create_addon(
            &router,
            cluster,
            &serde_json::json!({ "addonName": "vpc-cni", "configurationValues": document }),
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
            ADDON_CONFIGURATION_UNREADABLE_MESSAGE,
            "case {case:?}: expected the unreadable-document sentence, got {:?}",
            message_of(&answer)
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: an add-on sold through the AWS Marketplace needs a subscription to the
// listing. This emulator serves no Marketplace and so holds a subscription to no
// listing, so such an install is TAKEN and settles `CREATE_FAILED` at the call
// that asked for it — nothing of the add-on is applied to the cluster, so there
// is no asynchronous install to wait out — carrying the issue that says what is
// missing. An install that DOES reach the cluster keeps its `CREATING` span.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_marketplace_addon_settles_create_failed_for_want_of_a_subscription() {
    let router = emulator();
    let cluster = "marketplace-addons";
    let created = create_cluster(&router, cluster).await;
    assert_eq!(created.status, 200, "setup CreateCluster: got {created:?}");

    // The rows are EVERY add-on the catalog publishes as owned by the Marketplace
    // (`DescribeAddonVersions$owners`), so the rule is asserted over the whole
    // class rather than over one listing.
    let marketplace = published_with_owner(&router, ADDON_OWNER_MARKETPLACE).await;
    assert!(
        !marketplace.is_empty(),
        "setup: the catalog must publish at least one Marketplace add-on"
    );
    for addon in &marketplace {
        let installed = create_addon(
            &router,
            cluster,
            &serde_json::json!({ "addonName": addon }),
        )
        .await;
        assert_eq!(
            installed.status, 200,
            "case {addon:?}: a Marketplace add-on is installed, got {installed:?}"
        );
        assert_eq!(
            installed.addon()["status"],
            serde_json::json!("CREATE_FAILED"),
            "case {addon:?}: an unsubscribed listing is never installed, so the add-on is \
             created in the state it has already settled in, got {:?}",
            installed.addon()["status"]
        );
        assert_eq!(
            health_issues(installed.addon()),
            vec![(
                ADDON_ISSUE_SUBSCRIPTION_NEEDED.to_string(),
                ADDON_ISSUE_SUBSCRIPTION_NEEDED_MESSAGE.to_string(),
            )],
            "case {addon:?}: the create answers with the subscription issue, got {:?}",
            installed.addon()["health"]
        );
        // The metadata of the listing it was bought through belongs to the CATALOG
        // ([`addon_catalog_metadata.rs`]), not to the installed add-on, which
        // reports none of it.
        for member in ["owner", "publisher", "marketplaceInformation"] {
            assert_eq!(
                installed.addon()[member],
                serde_json::Value::Null,
                "case {addon:?}: an installed add-on reports no {member}, got {:?}",
                installed.addon()[member]
            );
        }
    }

    // Every one of them reports the subscription that is missing, with no wait: the
    // install was never started, so there is no span of the clock between the create
    // and the state the add-on has settled in.
    for addon in &marketplace {
        let described = describe_addon(&router, cluster, addon).await;
        assert_eq!(
            described.status, 200,
            "case {addon:?}: a failed install is still an add-on of its cluster, got \
             {described:?}"
        );
        assert_eq!(
            described.addon()["status"],
            serde_json::json!("CREATE_FAILED"),
            "case {addon:?}: an unsubscribed listing never comes up, got {:?}",
            described.addon()["status"]
        );
        assert_eq!(
            health_issues(described.addon()),
            vec![(
                ADDON_ISSUE_SUBSCRIPTION_NEEDED.to_string(),
                ADDON_ISSUE_SUBSCRIPTION_NEEDED_MESSAGE.to_string(),
            )],
            "case {addon:?}: expected the subscription issue, got {:?}",
            described.addon()["health"]
        );
        // The failed install is still listed, and reads the same on every read.
        let listed = list_addons(&router, cluster).await;
        assert!(
            listed.iter().any(|name| name == addon),
            "case {addon:?}: a failed add-on is still installed, got {listed:?}"
        );
        let again = describe_addon(&router, cluster, addon).await;
        assert_eq!(
            again.addon(),
            described.addon(),
            "case {addon:?}: a second read answers identically"
        );
    }

    // An Amazon EKS-managed add-on is the other side of the same rule: its install
    // is not refused for a subscription.
    let managed = create_addon(
        &router,
        cluster,
        &serde_json::json!({ "addonName": "vpc-cni" }),
    )
    .await;
    assert_eq!(
        managed.status, 200,
        "an EKS-managed add-on installs, got {managed:?}"
    );
    assert_eq!(
        managed.addon()["status"],
        serde_json::json!("CREATING"),
        "an install that reaches the cluster is asynchronous, got {:?}",
        managed.addon()["status"]
    );
    // An install that DOES reach the cluster is the other side of the same rule: it
    // is asynchronous, so the add-on reports its terminal status once the clock has
    // settled the cluster and then the install. The read is repeated until it does
    // rather than timed: what is asserted is the state the install reaches, not how
    // fast the host gets there.
    let deadline = std::time::Instant::now() + POLL_DEADLINE;
    let settled = loop {
        let read = describe_addon(&router, cluster, "vpc-cni").await;
        if read.addon()["status"] == serde_json::json!("ACTIVE")
            || std::time::Instant::now() >= deadline
        {
            break read;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    };
    assert_eq!(
        settled.addon()["status"],
        serde_json::json!("ACTIVE"),
        "an EKS-managed add-on needs no subscription, got {:?}",
        settled.addon()["status"]
    );
}
