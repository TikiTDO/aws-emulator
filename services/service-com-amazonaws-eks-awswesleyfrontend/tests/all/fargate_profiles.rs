// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS Fargate profile slice: `CreateFargateProfile` and
//! `DescribeFargateProfile`, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! Three rules, each asserted over a TABLE of inputs rather than over one recorded
//! request:
//!
//! 1. a profile read back through `DescribeFargateProfile` is the record the create
//!    wrote — every member the request supplied, plus the server-generated ones —
//!    and only its clock-owed status may have moved;
//! 2. a Fargate profile declares which Pods run on Fargate through its selectors,
//!    so a request whose selectors cannot do that (none at all, more than the
//!    documented five, one without a namespace) is refused 400
//!    `InvalidParameterException` and nothing is written;
//! 3. a refusal decided from the REQUEST ALONE is answered before any gate that
//!    depends on the kind of cluster the request named: the same payload earns the
//!    parameter refusal whether the cluster is one Amazon EKS manages or one
//!    `RegisterCluster` connected, and only a request whose payload is sound is
//!    answered with the connected-cluster `ClientException`.
//!
//! The rows differ along what each rule generalises over — which optional members
//! are supplied, selector count and label shape, subnet-list length, tag map, the
//! region, the cluster, and which member of the payload is wrong — and every
//! assertion names its case and prints the actual value.
//!
//! The wire harness (`Answer`, `emulator`, `call`, `encode_label`) is the shared one
//! in `tests/all/wire.rs` and the cluster helpers are `cluster_lifecycle`'s: no
//! suite grows its own client.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    CONNECTED_CLUSTER_REFUSAL_MESSAGE, FARGATE_PROFILE_ACTIVATION_DELAY,
    MAX_FARGATE_PROFILES_PER_CLUSTER, MAX_SELECTORS_PER_FARGATE_PROFILE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::REGION;
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// A role of the account this emulator serves, so the pass-role authorization
/// (403, pinned by `pass_role_authorization`) is never what answers a row here.
fn pod_execution_role() -> String {
    format!(
        "arn:aws:iam::{}:role/eksFargatePodExecutionRole",
        engine::emulator_account_id()
    )
}

/// The `fargateProfile` document an answer carries.
fn profile(answer: &Answer) -> &serde_json::Value {
    &answer.body["fargateProfile"]
}

/// The same document with `status` dropped, which is the one member a read may
/// legitimately differ on: the clock owes the profile a `CREATING` -> `ACTIVE` move
/// between the create and a later read.
fn without_status(answer: &Answer) -> serde_json::Value {
    let mut document = profile(answer).clone();
    if let Some(object) = document.as_object_mut() {
        object.remove("status");
    }
    document
}

fn cluster_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "roleArn": format!("arn:aws:iam::{}:role/eksClusterRole", engine::emulator_account_id()),
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

async fn create_cluster_in(router: &MultiServiceRouter, region: &str, name: &str) {
    let answer = call(
        router,
        region,
        "POST",
        "/clusters",
        &cluster_request(name).to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup: the cluster {name:?} in {region} must be created, got {answer:?}"
    );
}

async fn register_cluster(router: &MultiServiceRouter, name: &str) {
    let body = serde_json::json!({
        "name": name,
        "connectorConfig": {
            "roleArn": format!(
                "arn:aws:iam::{}:role/AmazonEKSConnectorRole",
                engine::emulator_account_id()
            ),
            "provider": "OTHER",
        },
    });
    let answer = call(
        router,
        REGION,
        "POST",
        "/cluster-registrations",
        &body.to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup: registering {name:?} must succeed, got {answer:?}"
    );
}

async fn create_profile_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!("/clusters/{}/fargate-profiles", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn create_profile(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    create_profile_in(router, REGION, cluster, body).await
}

async fn describe_profile_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    name: &str,
) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!(
            "/clusters/{}/fargate-profiles/{}",
            encode_label(cluster),
            encode_label(name)
        ),
        "",
    )
    .await
}

async fn describe_profile(router: &MultiServiceRouter, cluster: &str, name: &str) -> Answer {
    describe_profile_in(router, REGION, cluster, name).await
}

/// The smallest request `CreateFargateProfile` accepts: the three required members
/// plus the one selector a profile cannot work without.
fn minimal_request(name: &str) -> serde_json::Value {
    serde_json::json!({
        "fargateProfileName": name,
        "podExecutionRoleArn": pod_execution_role(),
        "selectors": [{ "namespace": "default" }],
    })
}

// ---------------------------------------------------------------------------
// RULE 1: a Fargate profile read back through `DescribeFargateProfile` is the
// record `CreateFargateProfile` wrote — the members the request supplied plus the
// server-generated ARN, `createdAt`, health and status.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: how many selectors the profile
/// carries (both ends of the documented 1..5 range), whether a selector carries
/// labels, how long the subnet list is (including absent), and whether tags and an
/// idempotency token are supplied at all.
fn roundtrip_rows() -> Vec<(&'static str, serde_json::Value)> {
    let role = pod_execution_role();
    let five_selectors: Vec<serde_json::Value> = (0..MAX_SELECTORS_PER_FARGATE_PROFILE)
        .map(|index| serde_json::json!({ "namespace": format!("team-{index}") }))
        .collect();
    vec![
        (
            "only the required members and one selector",
            serde_json::json!({
                "fargateProfileName": "fp-minimal",
                "podExecutionRoleArn": role,
                "selectors": [{ "namespace": "default" }],
            }),
        ),
        (
            "a selector with labels",
            serde_json::json!({
                "fargateProfileName": "fp-labels",
                "podExecutionRoleArn": role,
                "selectors": [{
                    "namespace": "payments",
                    "labels": { "tier": "web", "env": "prod" },
                }],
            }),
        ),
        (
            "the documented ceiling of five selectors",
            serde_json::json!({
                "fargateProfileName": "fp-five-selectors",
                "podExecutionRoleArn": role,
                "selectors": five_selectors,
            }),
        ),
        (
            "one subnet",
            serde_json::json!({
                "fargateProfileName": "fp-one-subnet",
                "podExecutionRoleArn": role,
                "selectors": [{ "namespace": "default" }],
                "subnets": ["subnet-aaaa1111"],
            }),
        ),
        (
            "three subnets, tags and an idempotency token",
            serde_json::json!({
                "fargateProfileName": "fp-everything",
                "podExecutionRoleArn": role,
                "selectors": [
                    { "namespace": "default", "labels": { "app": "api" } },
                    { "namespace": "kube-system" },
                ],
                "subnets": ["subnet-aaaa1111", "subnet-bbbb2222", "subnet-cccc3333"],
                "tags": { "owner": "platform", "cost-centre": "42" },
                "clientRequestToken": "token-everything",
            }),
        ),
    ]
}

#[tokio::test]
async fn a_created_fargate_profile_reads_back_as_it_was_written() {
    let router = emulator();
    let cluster = "fp-roundtrip";
    create_cluster_in(&router, REGION, cluster).await;

    for (case, body) in roundtrip_rows() {
        let name = body["fargateProfileName"].as_str().expect("a name").to_owned();
        let created = create_profile(&router, cluster, &body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the profile must be created, got {created:?} ({})",
            created.message()
        );

        let document = profile(&created);
        assert_eq!(
            document["fargateProfileName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the answer must name the profile, got {document}"
        );
        assert_eq!(
            document["clusterName"].as_str(),
            Some(cluster),
            "case {case:?}: the answer must name the cluster, got {document}"
        );
        assert_eq!(
            document["selectors"], body["selectors"],
            "case {case:?}: the selectors must be the ones the request gave, got {}",
            document["selectors"]
        );
        assert_eq!(
            document["podExecutionRoleArn"].as_str(),
            Some(pod_execution_role().as_str()),
            "case {case:?}: the Pod execution role must be the one the request gave, got {document}"
        );
        assert_eq!(
            document["subnets"],
            body.get("subnets")
                .cloned()
                .unwrap_or_else(|| serde_json::json!([])),
            "case {case:?}: the subnets must be the ones the request gave, got {}",
            document["subnets"]
        );
        assert_eq!(
            document["tags"],
            body.get("tags")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
            "case {case:?}: the tags must be the ones the request gave, got {}",
            document["tags"]
        );
        assert_eq!(
            document["status"].as_str(),
            Some("CREATING"),
            "case {case:?}: a new profile is CREATING, got {document}"
        );
        assert_eq!(
            document["health"],
            serde_json::json!({ "issues": [] }),
            "case {case:?}: a healthy profile reports an empty issue list, got {}",
            document["health"]
        );
        assert!(
            document["createdAt"].is_number(),
            "case {case:?}: the profile must report when it was created, got {document}"
        );
        let arn = document["fargateProfileArn"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let expected_prefix = format!(
            "arn:aws:eks:{REGION}:{}:fargateprofile/{cluster}/{name}/",
            engine::emulator_account_id()
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an ARN under {expected_prefix:?}, got {arn:?}"
        );
        assert!(
            arn.len() > expected_prefix.len(),
            "case {case:?}: the ARN must carry the profile's own id, got {arn:?}"
        );

        // The read: the same document, status aside.
        let read = describe_profile(&router, cluster, &name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the stored profile must read back, got {read:?}"
        );
        assert_eq!(
            without_status(&read),
            without_status(&created),
            "case {case:?}: the read must be the record the create wrote, got {:?}",
            profile(&read)
        );
    }
}

/// The clock owes a new profile the `CREATING` -> `ACTIVE` move, and nothing else
/// about the record changes with it.
#[tokio::test]
async fn a_fargate_profile_becomes_active_once_the_clock_owes_it() {
    let router = emulator();
    let cluster = "fp-activation";
    create_cluster_in(&router, REGION, cluster).await;

    let created = create_profile(&router, cluster, &minimal_request("fp-clock")).await;
    assert_eq!(
        created.status, 200,
        "the profile must be created, got {created:?}"
    );
    // The span the clock owes is never zero, so the create itself answers the
    // transient status. It is asserted on the create's OWN answer rather than on a
    // second wire call, which a loaded machine could issue after the span has already
    // passed.
    assert_eq!(
        profile(&created)["status"].as_str(),
        Some("CREATING"),
        "a caller sees the transient status on the create itself, got {created:?}"
    );

    tokio::time::sleep(FARGATE_PROFILE_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let later = describe_profile(&router, cluster, "fp-clock").await;
    assert_eq!(
        later.body["fargateProfile"]["status"].as_str(),
        Some("ACTIVE"),
        "once the span the clock owed has passed the profile is ACTIVE, got {later:?}"
    );
    assert_eq!(
        without_status(&later),
        without_status(&created),
        "the move must change nothing but the status, got {:?}",
        profile(&later)
    );
    // And it stays there: a second read does not move it again.
    let again = describe_profile(&router, cluster, "fp-clock").await;
    assert_eq!(
        profile(&again),
        profile(&later),
        "a repeated read must answer identically, got {again:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the selectors are what a Fargate profile declares, so a request that
// declares nothing usable is refused as the bad parameter it is, and nothing is
// written.
// ---------------------------------------------------------------------------

/// The rows differ along WHICH way the selectors are unusable and along their
/// count: absent, present but empty, one past the documented ceiling of five, and a
/// selector carrying no namespace at all or a blank one.
fn invalid_selector_rows() -> Vec<(&'static str, serde_json::Value)> {
    let role = pod_execution_role();
    let too_many: Vec<serde_json::Value> = (0..=MAX_SELECTORS_PER_FARGATE_PROFILE)
        .map(|index| serde_json::json!({ "namespace": format!("team-{index}") }))
        .collect();
    vec![
        (
            "no selectors member at all",
            serde_json::json!({
                "fargateProfileName": "fp-no-selectors",
                "podExecutionRoleArn": role,
            }),
        ),
        (
            "an empty selectors list",
            serde_json::json!({
                "fargateProfileName": "fp-empty-selectors",
                "podExecutionRoleArn": role,
                "selectors": [],
            }),
        ),
        (
            "one selector past the documented ceiling",
            serde_json::json!({
                "fargateProfileName": "fp-too-many-selectors",
                "podExecutionRoleArn": role,
                "selectors": too_many,
            }),
        ),
        (
            "a selector with no namespace",
            serde_json::json!({
                "fargateProfileName": "fp-no-namespace",
                "podExecutionRoleArn": role,
                "selectors": [{ "labels": { "app": "api" } }],
            }),
        ),
        (
            "a selector whose namespace is blank",
            serde_json::json!({
                "fargateProfileName": "fp-blank-namespace",
                "podExecutionRoleArn": role,
                "selectors": [{ "namespace": "   " }],
            }),
        ),
        (
            "one sound selector beside one without a namespace",
            serde_json::json!({
                "fargateProfileName": "fp-mixed-selectors",
                "podExecutionRoleArn": role,
                "selectors": [{ "namespace": "default" }, { "labels": { "app": "api" } }],
            }),
        ),
    ]
}

#[tokio::test]
async fn selectors_a_profile_cannot_declare_with_are_refused_and_nothing_is_written() {
    let router = emulator();
    let cluster = "fp-selectors";
    create_cluster_in(&router, REGION, cluster).await;

    for (case, body) in invalid_selector_rows() {
        let name = body["fargateProfileName"].as_str().expect("a name").to_owned();
        let answer = create_profile(&router, cluster, &body).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected the parameter refusal, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "InvalidParameterException",
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        assert_eq!(
            answer.body["clusterName"].as_str(),
            Some(cluster),
            "case {case:?}: the payload must name the cluster, got {answer:?}"
        );
        assert_eq!(
            answer.body["fargateProfileName"].as_str(),
            Some(name.as_str()),
            "case {case:?}: the payload must name the profile at fault, got {answer:?}"
        );
        assert!(
            !answer.message().is_empty(),
            "case {case:?}: the refusal must say what is wrong, got {answer:?}"
        );

        let read = describe_profile(&router, cluster, &name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: a refused create must have written nothing, got {read:?}"
        );
    }

    // The CONTROL: the counts either side of the refusals are accepted, so the rule
    // is a bound and not a blanket refusal — one selector and the documented five.
    for count in [1, MAX_SELECTORS_PER_FARGATE_PROFILE] {
        let selectors: Vec<serde_json::Value> = (0..count)
            .map(|index| serde_json::json!({ "namespace": format!("ns-{index}") }))
            .collect();
        let name = format!("fp-accepted-{count}");
        let body = serde_json::json!({
            "fargateProfileName": name,
            "podExecutionRoleArn": pod_execution_role(),
            "selectors": selectors,
        });
        let answer = create_profile(&router, cluster, &body).await;
        assert_eq!(
            answer.status, 200,
            "case {count} selectors: within the documented range the create is served, got \
             {answer:?} ({})",
            answer.message()
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: a refusal decided from the request alone precedes every gate that
// depends on the cluster the request named — and only a sound payload reaches
// those gates.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_payload_is_judged_before_the_kind_of_cluster_it_names() {
    let router = emulator();
    let managed = "fp-order-managed";
    let connected = "fp-order-connected";
    create_cluster_in(&router, REGION, managed).await;
    register_cluster(&router, connected).await;

    for (case, body) in invalid_selector_rows() {
        // The SAME body against both kinds of cluster: the answer is the parameter
        // refusal either way, because nothing about the cluster can make these
        // selectors usable.
        for cluster in [managed, connected] {
            let answer = create_profile(&router, cluster, &body).await;
            assert_eq!(
                (answer.status, error_code(&answer).as_str()),
                (400, "InvalidParameterException"),
                "case {case:?} on {cluster}: the payload is judged before the cluster's own \
                 facts, got {answer:?}"
            );
            assert_ne!(
                answer.message(),
                CONNECTED_CLUSTER_REFUSAL_MESSAGE,
                "case {case:?} on {cluster}: a bad payload must not be reported as the \
                 connected-cluster refusal, got {answer:?}"
            );
        }
    }

    // A SOUND payload reaches the gate, and that is what the connected cluster
    // answers — so the ordering above did not cost the gate.
    let refused = create_profile(&router, connected, &minimal_request("fp-order-sound")).await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (400, "ClientException"),
        "a sound payload on a connected cluster earns the Amazon EKS-managed-plane refusal, \
         got {refused:?}"
    );
    assert_eq!(
        refused.message(),
        CONNECTED_CLUSTER_REFUSAL_MESSAGE,
        "expected the connected-cluster refusal sentence, got {:?}",
        refused.message()
    );
    // And the same payload on the cluster Amazon EKS manages is created.
    let created = create_profile(&router, managed, &minimal_request("fp-order-sound")).await;
    assert_eq!(
        created.status, 200,
        "the same payload on a managed cluster is created, got {created:?}"
    );

    // A cluster this region does not hold is still the 404 that names only the
    // cluster, whatever the payload says.
    for (case, body) in invalid_selector_rows()
        .into_iter()
        .chain(std::iter::once(("a sound payload", minimal_request("fp-ghost"))))
    {
        let answer = create_profile(&router, "fp-no-such-cluster", &body).await;
        assert_eq!(
            (answer.status, error_code(&answer).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: an unresolvable cluster is reported before the body, got {answer:?}"
        );
        assert_eq!(
            answer.body["clusterName"].as_str(),
            Some("fp-no-such-cluster"),
            "case {case:?}: the payload must name the cluster that was not found, got {answer:?}"
        );
        assert_eq!(
            answer.body["fargateProfileName"],
            serde_json::Value::Null,
            "case {case:?}: the not-found payload must not echo the profile name, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A profile name is unique WITHIN its cluster, and an idempotent retry of the
// create that made it is absorbed rather than conflicting.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_profile_name_is_unique_within_its_cluster() {
    let router = emulator();
    let other_region = "eu-west-1";
    let first = "fp-unique-first";
    let second = "fp-unique-second";
    create_cluster_in(&router, REGION, first).await;
    create_cluster_in(&router, REGION, second).await;
    create_cluster_in(&router, other_region, first).await;

    let name = "fp-shared-name";
    let created = create_profile(&router, first, &minimal_request(name)).await;
    assert_eq!(
        created.status, 200,
        "the first create must be served, got {created:?}"
    );

    // The same name in the same cluster: refused, and the stored profile is untouched.
    let conflict = create_profile(&router, first, &minimal_request(name)).await;
    assert_eq!(
        (conflict.status, error_code(&conflict).as_str()),
        (400, "InvalidRequestException"),
        "a second create under a name the cluster holds is refused, got {conflict:?}"
    );
    assert!(
        conflict.message().contains(name) && conflict.message().contains(first),
        "the refusal must name the profile and the cluster, got {:?}",
        conflict.message()
    );

    // The SAME name is a different profile in another cluster, and in another region.
    let sibling = create_profile(&router, second, &minimal_request(name)).await;
    assert_eq!(
        sibling.status, 200,
        "the same name in another cluster is a different profile, got {sibling:?}"
    );
    let across_region = create_profile_in(&router, other_region, first, &minimal_request(name)).await;
    assert_eq!(
        across_region.status, 200,
        "the same name in another region is a different profile, got {across_region:?}"
    );
    assert_ne!(
        profile(&across_region)["fargateProfileArn"],
        profile(&created)["fargateProfileArn"],
        "two profiles of the same name in different regions are different resources, got {:?}",
        profile(&across_region)
    );
    // Each region reads back its own.
    let read_here = describe_profile(&router, first, name).await;
    let read_there = describe_profile_in(&router, other_region, first, name).await;
    assert_eq!(
        (read_here.status, read_there.status),
        (200, 200),
        "both profiles must read back, got {read_here:?} and {read_there:?}"
    );
    assert_eq!(
        profile(&read_here)["fargateProfileArn"],
        profile(&created)["fargateProfileArn"],
        "the read in {REGION} must answer the profile created there, got {read_here:?}"
    );

    // An idempotent retry — the same `clientRequestToken` — is absorbed and answered
    // with the profile that call created.
    let body = serde_json::json!({
        "fargateProfileName": "fp-idempotent",
        "podExecutionRoleArn": pod_execution_role(),
        "selectors": [{ "namespace": "default" }],
        "clientRequestToken": "retry-token-1",
    });
    let once = create_profile(&router, first, &body).await;
    assert_eq!(once.status, 200, "the create must be served, got {once:?}");
    let retried = create_profile(&router, first, &body).await;
    assert_eq!(
        retried.status, 200,
        "a retry carrying the same idempotency token is absorbed, got {retried:?}"
    );
    assert_eq!(
        without_status(&retried),
        without_status(&once),
        "the retry must answer the profile the first call created, got {:?}",
        profile(&retried)
    );

    // A DIFFERENT token under the same name is a new request, so it conflicts.
    let mut other_token = body.clone();
    other_token["clientRequestToken"] = serde_json::json!("retry-token-2");
    let conflict = create_profile(&router, first, &other_token).await;
    assert_eq!(
        (conflict.status, error_code(&conflict).as_str()),
        (400, "InvalidRequestException"),
        "a different token under a name the cluster holds still conflicts, got {conflict:?}"
    );
}

// ---------------------------------------------------------------------------
// A Fargate profile is a child of its cluster: the quota is per cluster, the
// cluster cannot be deleted while a profile is attached to it, and once they are
// gone a cluster created again under the same name inherits none of them.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn profiles_are_bounded_per_cluster_and_go_with_the_cluster() {
    let router = emulator();
    let cluster = "fp-quota";
    create_cluster_in(&router, REGION, cluster).await;

    for index in 0..MAX_FARGATE_PROFILES_PER_CLUSTER {
        let answer = create_profile(&router, cluster, &minimal_request(&format!("fp-{index}"))).await;
        assert_eq!(
            answer.status, 200,
            "profile {index} is within the quota, got {answer:?} ({})",
            answer.message()
        );
    }
    let past_quota = create_profile(&router, cluster, &minimal_request("fp-past-quota")).await;
    assert_eq!(
        (past_quota.status, error_code(&past_quota).as_str()),
        (400, "ResourceLimitExceededException"),
        "the {MAX_FARGATE_PROFILES_PER_CLUSTER}-profile quota must be reported as a limit, got \
         {past_quota:?}"
    );

    // Another cluster has its own quota, so the bound is per cluster.
    let neighbour = "fp-quota-neighbour";
    create_cluster_in(&router, REGION, neighbour).await;
    let answer = create_profile(&router, neighbour, &minimal_request("fp-0")).await;
    assert_eq!(
        answer.status, 200,
        "the quota is per cluster, so a sibling cluster still accepts a profile, got {answer:?}"
    );

    let delete_cluster = || async {
        call(
            &router,
            REGION,
            "DELETE",
            &format!("/clusters/{}", encode_label(cluster)),
            "",
        )
        .await
    };

    // The cluster keeps every profile attached to it, and those profiles keep the
    // cluster: "If you have managed node groups or Fargate profiles attached to the
    // cluster, you must delete them first".
    let refused = delete_cluster().await;
    assert_eq!(
        (refused.status, error_code(&refused).as_str()),
        (409, "ResourceInUseException"),
        "a cluster with profiles attached must not delete, got {refused:?} ({})",
        refused.message()
    );
    let still_there = describe_profile(&router, cluster, "fp-0").await;
    assert_eq!(
        still_there.status, 200,
        "a refused delete leaves the cluster and its profiles alone, got {still_there:?}"
    );

    // Deleted first, the profiles no longer hold the cluster — and a cluster created
    // again under the same name inherits none of them.
    for index in 0..MAX_FARGATE_PROFILES_PER_CLUSTER {
        let removed = call(
            &router,
            REGION,
            "DELETE",
            &format!(
                "/clusters/{}/fargate-profiles/{}",
                encode_label(cluster),
                encode_label(&format!("fp-{index}"))
            ),
            "",
        )
        .await;
        assert_eq!(
            removed.status, 200,
            "setup: profile fp-{index} must delete, got {removed:?} ({})",
            removed.message()
        );
    }
    let deleted = delete_cluster().await;
    assert_eq!(
        deleted.status, 200,
        "setup: the cluster must delete once nothing is attached, got {deleted:?} ({})",
        deleted.message()
    );
    create_cluster_in(&router, REGION, cluster).await;
    let read = describe_profile(&router, cluster, "fp-0").await;
    assert_eq!(
        read.status, 404,
        "a cluster created again under the same name inherits no profile, got {read:?}"
    );
    let recreated = create_profile(&router, cluster, &minimal_request("fp-0")).await;
    assert_eq!(
        recreated.status, 200,
        "and the name is free again, got {recreated:?} ({})",
        recreated.message()
    );
}
