// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS Pod Identity association slice:
//! `CreatePodIdentityAssociation`, `DescribePodIdentityAssociation`,
//! `UpdatePodIdentityAssociation` and `DeletePodIdentityAssociation`, driven over
//! the wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (the shape of a
//! malformed identifier, which optional members a create or an update supplies,
//! the partition and path of an IAM role ARN, the Region a call is signed for,
//! how many associations a cluster holds), and each assertion names its case and
//! prints the actual value.

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    is_association_id, ASSOCIATION_ID_BODY_LEN, ASSOCIATION_ID_LEN, ASSOCIATION_ID_PATTERN,
    ASSOCIATION_ID_PREFIX, MAX_CLUSTER_NAME_LEN, MAX_KUBERNETES_NAME_LEN,
};
use smithy_server_runtime::MultiServiceRouter;

// The wire plumbing (one shared engine behind the production router, label
// encoding, the reduced `Answer`) is shared with every other suite in this
// crate — see `tests/all/wire.rs`.
use crate::wire::{call, emulator, encode_label, Answer};

pub(crate) const REGION: &str = "us-west-2";
const OTHER_REGION: &str = "eu-west-1";
pub(crate) const CLUSTER: &str = "pod-identity-cluster";
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/pod-identity";
pub(crate) const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/pod-identity-other";
pub(crate) const TARGET_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/target";

/// A well-formed association id this emulator never minted: the right prefix and
/// exactly [`ASSOCIATION_ID_BODY_LEN`] lowercase alphanumerics.
const UNKNOWN_ASSOCIATION_ID: &str = "a-00000000000000000";

/// How long the update suite waits between the create and the update, so the
/// clock has observably moved and `modifiedAt` can be asserted to be LATER than
/// `createdAt` rather than merely not earlier.
const MODIFICATION_GAP: std::time::Duration = std::time::Duration::from_millis(20);

async fn create_cluster(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    let body = serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    call(router, region, "POST", "/clusters", &body.to_string()).await
}

async fn delete_cluster(router: &MultiServiceRouter, region: &str, name: &str) -> Answer {
    call(
        router,
        region,
        "DELETE",
        &format!("/clusters/{}", encode_label(name)),
        "",
    )
    .await
}

/// A cluster that exists, so an association call reaches the association itself.
pub(crate) async fn cluster_fixture(region: &str, name: &str) -> MultiServiceRouter {
    let router = emulator();
    let created = create_cluster(&router, region, name).await;
    assert_eq!(
        created.status, 200,
        "the fixture cluster must be created, got {:?} {:?}",
        created.status, created.body
    );
    router
}

pub(crate) async fn create_association(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/pod-identity-associations",
            encode_label(cluster)
        ),
        &body.to_string(),
    )
    .await
}

pub(crate) async fn describe_association(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    association_id: &str,
) -> Answer {
    call(
        router,
        region,
        "GET",
        &format!(
            "/clusters/{}/pod-identity-associations/{}",
            encode_label(cluster),
            encode_label(association_id)
        ),
        "",
    )
    .await
}

pub(crate) async fn update_association(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    association_id: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/pod-identity-associations/{}",
            encode_label(cluster),
            encode_label(association_id)
        ),
        &body.to_string(),
    )
    .await
}

pub(crate) async fn delete_association(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    association_id: &str,
) -> Answer {
    call(
        router,
        region,
        "DELETE",
        &format!(
            "/clusters/{}/pod-identity-associations/{}",
            encode_label(cluster),
            encode_label(association_id)
        ),
        "",
    )
    .await
}

/// The smallest request `CreatePodIdentityAssociation` accepts: the three
/// required body members (`clusterName` travels in the URI).
pub(crate) fn minimal_create(namespace: &str, service_account: &str) -> serde_json::Value {
    serde_json::json!({
        "namespace": namespace,
        "serviceAccount": service_account,
        "roleArn": ROLE_ARN,
    })
}

pub(crate) fn association_id_of(answer: &Answer) -> String {
    answer.association()["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn number(value: &serde_json::Value) -> f64 {
    value.as_f64().unwrap_or(f64::NAN)
}

// ---------------------------------------------------------------------------
// RULE: an association read back is the record that was written — every member
// the request supplied, plus the server-generated ids, ARN, external id and
// timestamps, unchanged.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional members the
/// create supplies at all, and the one value that switches a derived member
/// (`targetRoleArn`, which is what earns an `externalId`).
#[tokio::test]
async fn a_created_association_reads_back_exactly_as_stored() {
    let router = cluster_fixture(REGION, CLUSTER).await;

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("only the required members", minimal_create("default", "sa")),
        (
            "tags and an idempotency token",
            serde_json::json!({
                "namespace": "kube-system",
                "serviceAccount": "tagged-sa",
                "roleArn": ROLE_ARN,
                "tags": { "env": "dev", "team": "platform" },
                "clientRequestToken": "token-tagged",
            }),
        ),
        (
            "a target role, which earns an externalId",
            serde_json::json!({
                "namespace": "targeted",
                "serviceAccount": "cross-account-sa",
                "roleArn": ROLE_ARN,
                "targetRoleArn": TARGET_ROLE_ARN,
                "disableSessionTags": true,
            }),
        ),
        (
            "a policy, which the docs require disableSessionTags for",
            serde_json::json!({
                "namespace": "restricted",
                "serviceAccount": "least-privilege-sa",
                "roleArn": ROLE_ARN,
                "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "disableSessionTags": true,
            }),
        ),
        (
            "session tags left on, with no policy",
            serde_json::json!({
                "namespace": "plain",
                "serviceAccount": "session-tagged-sa",
                "roleArn": ROLE_ARN,
                "disableSessionTags": false,
            }),
        ),
    ];

    for (case, request) in cases {
        let created = create_association(&router, REGION, CLUSTER, &request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected a created association, got {:?} {:?}",
            created.status, created.body
        );

        let association = created.association().clone();
        let association_id = association_id_of(&created);
        assert!(
            is_association_id(&association_id),
            "case {case:?}: expected a minted id of the documented form, got \
             {association_id:?}"
        );
        assert_eq!(
            association["clusterName"],
            serde_json::json!(CLUSTER),
            "case {case:?}: the association reports the cluster it is in, got {:?}",
            association["clusterName"]
        );
        for member in ["namespace", "serviceAccount", "roleArn"] {
            assert_eq!(
                association[member], request[member],
                "case {case:?}: {member} is stored as supplied, got {:?}",
                association[member]
            );
        }
        for member in ["targetRoleArn", "policy", "disableSessionTags", "tags"] {
            if let Some(expected) = request.get(member) {
                assert_eq!(
                    &association[member], expected,
                    "case {case:?}: {member} is stored as supplied, got {:?}",
                    association[member]
                );
            }
        }
        assert_eq!(
            association["associationArn"],
            serde_json::json!(format!(
                "arn:aws:eks:{REGION}:000000000000:podidentityassociation/{CLUSTER}/\
                 {association_id}"
            )),
            "case {case:?}: the ARN names the Region, cluster and association, got {:?}",
            association["associationArn"]
        );
        // `externalId` exists to be matched in the TARGET role's trust policy, so
        // it is minted exactly for an association that names one.
        assert_eq!(
            association["externalId"].is_string(),
            request.get("targetRoleArn").is_some(),
            "case {case:?}: externalId is present iff targetRoleArn is, got {:?}",
            association["externalId"]
        );
        assert!(
            association["createdAt"].is_number() && association["modifiedAt"].is_number(),
            "case {case:?}: expected epoch-seconds timestamps, got {:?} and {:?}",
            association["createdAt"],
            association["modifiedAt"]
        );
        assert_eq!(
            association["createdAt"], association["modifiedAt"],
            "case {case:?}: a create is also the last modification, got {:?} and {:?}",
            association["createdAt"], association["modifiedAt"]
        );
        // An association created through this API is owned by the caller, not by
        // an EKS add-on.
        assert!(
            association["ownerArn"].is_null(),
            "case {case:?}: expected no ownerArn, got {:?}",
            association["ownerArn"]
        );

        let described = describe_association(&router, REGION, CLUSTER, &association_id).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the created association must describe, got {:?} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.association(),
            &association,
            "case {case:?}: a describe answers the stored record, got {:?}",
            described.association()
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an identifier outside its documented form names no resource anywhere, so
// every operation that takes one refuses it with InvalidParameterException (400)
// BEFORE resolving any resource; a well-formed identifier for a resource that
// does not exist is ResourceNotFoundException (404).
// ---------------------------------------------------------------------------

/// The malformed rows span the ways an id can be outside
/// `a-` + [`ASSOCIATION_ID_BODY_LEN`] lowercase alphanumerics: no prefix, the
/// wrong prefix, a short body, a long body, and a body carrying a character
/// outside the set (uppercase, `-`, `_`, `.`). The rule is asserted on EVERY
/// operation that takes an `associationId`, not only on the describe the issue
/// was filed from, and on both an existing and an absent cluster — the parameter
/// check precedes resolution, so the cluster's existence cannot change the
/// answer.
#[tokio::test]
async fn a_malformed_association_id_is_an_invalid_parameter_on_every_operation() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    let long_body = "a".repeat(ASSOCIATION_ID_BODY_LEN + 1);
    let short_body = "a".repeat(ASSOCIATION_ID_BODY_LEN - 1);
    let body_with = |replacement: char| {
        let mut body = "a".repeat(ASSOCIATION_ID_BODY_LEN);
        body.replace_range(3..4, &replacement.to_string());
        format!("{ASSOCIATION_ID_PREFIX}{body}")
    };

    let malformed: Vec<String> = vec![
        "not-an-id".to_string(),
        // The body alone, without the `a-` prefix.
        "a".repeat(ASSOCIATION_ID_BODY_LEN),
        // The right shape under the wrong prefix.
        format!("b-{}", "a".repeat(ASSOCIATION_ID_BODY_LEN)),
        format!("{ASSOCIATION_ID_PREFIX}{short_body}"),
        format!("{ASSOCIATION_ID_PREFIX}{long_body}"),
        body_with('A'),
        body_with('-'),
        body_with('_'),
        body_with('.'),
    ];

    for cluster in [CLUSTER, "no-such-cluster"] {
        for association_id in &malformed {
            let calls = vec![
                (
                    "describe",
                    describe_association(&router, REGION, cluster, association_id).await,
                ),
                (
                    "update",
                    update_association(
                        &router,
                        REGION,
                        cluster,
                        association_id,
                        &serde_json::json!({ "roleArn": OTHER_ROLE_ARN }),
                    )
                    .await,
                ),
                (
                    "delete",
                    delete_association(&router, REGION, cluster, association_id).await,
                ),
            ];
            for (operation, answer) in calls {
                let case = (cluster, association_id, operation);
                assert_eq!(
                    answer.status, 400,
                    "case {case:?}: a malformed associationId is a 400, got {:?} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_type.as_deref(),
                    Some("InvalidParameterException"),
                    "case {case:?}: expected InvalidParameterException, got {:?}",
                    answer.error_type
                );
                assert!(
                    answer.body["message"]
                        .as_str()
                        .is_some_and(|message| message.contains("associationId")),
                    "case {case:?}: the refusal must name the member, got {:?}",
                    answer.body["message"]
                );
                // The refusal is built from the member and the constraint it
                // failed: the length an association id has when the value is the
                // wrong length, the form when it is the right length.
                let expected_message = if association_id.chars().count() == ASSOCIATION_ID_LEN {
                    format!("The parameter associationId should match the pattern {ASSOCIATION_ID_PATTERN}.")
                } else {
                    format!("The parameter associationId should be {ASSOCIATION_ID_LEN} characters long.")
                };
                assert_eq!(
                    answer.body["message"].as_str(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected the constraint it failed, got {:?}",
                    answer.body["message"]
                );
            }
        }
    }
}

/// The same rule from the other side: a WELL-FORMED id that names nothing is the
/// 404, and so is a well-formed cluster name that names nothing — on every
/// operation that takes them.
#[tokio::test]
async fn a_well_formed_identifier_that_names_nothing_is_not_found() {
    let router = cluster_fixture(REGION, CLUSTER).await;

    let cases: Vec<(&str, &str, &str)> = vec![
        ("an absent association in a cluster that exists", CLUSTER, UNKNOWN_ASSOCIATION_ID),
        ("a cluster that was never created", "absent-cluster", UNKNOWN_ASSOCIATION_ID),
        // A cluster name only identifies a cluster within one Region, so the
        // fixture's own name is absent in another Region.
        ("a cluster that exists in another Region", CLUSTER, UNKNOWN_ASSOCIATION_ID),
    ];

    for (case, cluster, association_id) in cases {
        let region = if case == "a cluster that exists in another Region" {
            OTHER_REGION
        } else {
            REGION
        };
        let calls = vec![
            (
                "describe",
                describe_association(&router, region, cluster, association_id).await,
            ),
            (
                "update",
                update_association(
                    &router,
                    region,
                    cluster,
                    association_id,
                    &serde_json::json!({ "roleArn": OTHER_ROLE_ARN }),
                )
                .await,
            ),
            (
                "delete",
                delete_association(&router, region, cluster, association_id).await,
            ),
            (
                "create",
                create_association(
                    &router,
                    region,
                    cluster,
                    &minimal_create("default", "sa-not-found"),
                )
                .await,
            ),
        ];
        for (operation, answer) in calls {
            // `create` does not take an `associationId`, so it only has the
            // cluster to be not-found about.
            if operation == "create" && cluster == CLUSTER && region == REGION {
                continue;
            }
            let case = (case, operation);
            assert_eq!(
                answer.status, 404,
                "case {case:?}: expected a not-found, got {:?} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_type
            );
        }
    }
}

/// The cluster-name leg of the same rule. The rows span the ways a name is
/// outside `CreateCluster$name`'s own pattern and length: a leading separator,
/// an interior character outside the set, and one character past the published
/// maximum — plus the published maximum itself, which is accepted (and so is a
/// one-character name), to pin the bound rather than an off-by-one.
#[tokio::test]
async fn a_malformed_cluster_name_is_an_invalid_parameter() {
    let router = emulator();
    let at_the_bound = "c".repeat(MAX_CLUSTER_NAME_LEN);
    let past_the_bound = "c".repeat(MAX_CLUSTER_NAME_LEN + 1);

    let malformed: Vec<&str> = vec![
        "-leading-dash",
        "_leading-underscore",
        "has spaces",
        "has!punctuation",
        "has/slash",
        past_the_bound.as_str(),
    ];
    for cluster in malformed {
        let answer = describe_association(&router, REGION, cluster, UNKNOWN_ASSOCIATION_ID).await;
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {cluster:?}: expected InvalidParameterException, got {:?} {:?}",
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.status, 400,
            "case {cluster:?}: a malformed clusterName is a 400, got {:?}",
            answer.status
        );
        // The refusal is built from the member and the constraint it failed.
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("clusterName")
                && if cluster.len() > MAX_CLUSTER_NAME_LEN {
                    message.contains(&MAX_CLUSTER_NAME_LEN.to_string())
                } else {
                    // The grammar's own sentence, the one every cluster-name member
                    // of this service is refused with.
                    message.contains("contains invalid characters")
                        && message.contains("regular expression")
                },
            "case {cluster:?}: expected the constraint it failed, got {message:?}"
        );
    }

    // At and within the bound the name is well formed, so the answer is about the
    // resource instead: these clusters were never created.
    for cluster in ["c", at_the_bound.as_str()] {
        let answer = describe_association(&router, REGION, cluster, UNKNOWN_ASSOCIATION_ID).await;
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {:?}: a well-formed name is resolved, not refused, got {:?} {:?}",
            cluster.len(),
            answer.error_type,
            answer.body
        );
    }
}

/// The Kubernetes-name and IAM-role-ARN legs of the same rule, on the create.
/// The accepted rows are the forms the catalogue admits that the issue never
/// cited (a role path, a target role in another account and partition — which
/// `targetRoleArn` is documented to allow — a dotted service account name, a
/// namespace at the published length bound); the refused rows are each way a
/// value leaves the form.
///
/// `roleArn` is NOT a row here: it is the role the request asks Amazon EKS to
/// pass, so a value outside this account is a permission failure rather than a
/// parameter one, and `crate::pass_role_authorization` is the suite that pins it.
#[tokio::test]
async fn create_checks_the_form_of_every_identifier_it_carries() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    let long_namespace = "n".repeat(MAX_KUBERNETES_NAME_LEN);
    let too_long_namespace = "n".repeat(MAX_KUBERNETES_NAME_LEN + 1);

    // ACCEPTED: the IAM role ARN forms AWS publishes (partition, path) and the
    // DNS-1123 names Kubernetes admits.
    let accepted: Vec<(&str, serde_json::Value)> = vec![
        (
            "a target role in another partition",
            serde_json::json!({
                "namespace": "cn-namespace",
                "serviceAccount": "sa-1",
                "roleArn": ROLE_ARN,
                "targetRoleArn": "arn:aws-cn:iam::111122223333:role/china",
            }),
        ),
        (
            "a role of this account under a path, and a target role under one too",
            serde_json::json!({
                "namespace": "gov-namespace",
                "serviceAccount": "sa-2",
                "roleArn": "arn:aws:iam::000000000000:role/team/dept/role-name",
                "targetRoleArn": "arn:aws-us-gov:iam::000000000000:role/team/dept/target",
            }),
        ),
        (
            "a dotted service account name and a digit-led namespace",
            serde_json::json!({
                "namespace": "2nd-namespace",
                "serviceAccount": "my.service.account-3",
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a namespace at the published length bound",
            serde_json::json!({
                "namespace": long_namespace,
                "serviceAccount": "sa-4",
                "roleArn": ROLE_ARN,
            }),
        ),
    ];
    for (case, request) in accepted {
        let answer = create_association(&router, REGION, CLUSTER, &request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected the create to be accepted, got {:?} {:?}",
            answer.status, answer.body
        );
    }

    // REFUSED: each way a value leaves its form.
    let refused: Vec<(&str, serde_json::Value)> = vec![
        (
            "a targetRoleArn that is not an ARN at all",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "bad-1",
                "roleArn": ROLE_ARN, "targetRoleArn": "not-an-arn",
            }),
        ),
        (
            "a target ARN for an IAM user rather than a role",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "bad-2",
                "roleArn": ROLE_ARN,
                "targetRoleArn": "arn:aws:iam::111122223333:user/someone",
            }),
        ),
        (
            "a target IAM ARN carrying a Region, which IAM never does",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "bad-3",
                "roleArn": ROLE_ARN,
                "targetRoleArn": "arn:aws:iam:us-west-2:111122223333:role/regional",
            }),
        ),
        (
            "a target ARN of another service",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "bad-4",
                "roleArn": ROLE_ARN,
                "targetRoleArn": "arn:aws:eks:us-west-2:111122223333:cluster/other",
            }),
        ),
        (
            "a target account that is not 12 digits",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "bad-5",
                "roleArn": ROLE_ARN,
                "targetRoleArn": "arn:aws:iam::12345:role/short-account",
            }),
        ),
        (
            "a malformed targetRoleArn beside a well-formed roleArn",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "bad-6",
                "roleArn": ROLE_ARN, "targetRoleArn": "arn:aws:iam::000000000000:group/g",
            }),
        ),
        (
            "an uppercase namespace",
            serde_json::json!({
                "namespace": "Default", "serviceAccount": "bad-7", "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a namespace that ends in a separator",
            serde_json::json!({
                "namespace": "trailing-", "serviceAccount": "bad-8", "roleArn": ROLE_ARN,
            }),
        ),
        (
            "an empty service account",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "", "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a service account with a slash in it",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "sa/one", "roleArn": ROLE_ARN,
            }),
        ),
        (
            "a namespace one character past the published bound",
            serde_json::json!({
                "namespace": too_long_namespace, "serviceAccount": "bad-9",
                "roleArn": ROLE_ARN,
            }),
        ),
        (
            // `policy`'s own documentation: "When using this policy,
            // disableSessionTags must be set to true."
            "a policy without disableSessionTags",
            serde_json::json!({
                "namespace": "default", "serviceAccount": "bad-10", "roleArn": ROLE_ARN,
                "policy": "{}",
            }),
        ),
    ];
    for (case, request) in refused {
        let answer = create_association(&router, REGION, CLUSTER, &request).await;
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?} {:?}",
            answer.error_type,
            answer.body
        );
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected a 400, got {:?}",
            answer.status
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an update overwrites only the members the model lets it change, leaves
// every other member of the stored record alone, and moves `modifiedAt`.
// ---------------------------------------------------------------------------

/// The table spans the members an update may carry, one per row and then all of
/// them together, because the rule is about WHICH members a request changes.
#[tokio::test]
async fn an_update_changes_only_the_members_it_carries() {
    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("the IAM role alone", serde_json::json!({ "roleArn": OTHER_ROLE_ARN })),
        (
            "the target role alone, which mints the externalId",
            serde_json::json!({ "targetRoleArn": TARGET_ROLE_ARN }),
        ),
        (
            "the session tags alone",
            serde_json::json!({ "disableSessionTags": true }),
        ),
        (
            "a policy, with the session tags it requires",
            serde_json::json!({ "policy": "{\"Statement\":[]}", "disableSessionTags": true }),
        ),
        (
            "every updatable member at once",
            serde_json::json!({
                "roleArn": OTHER_ROLE_ARN,
                "targetRoleArn": TARGET_ROLE_ARN,
                "disableSessionTags": true,
                "policy": "{\"Statement\":[]}",
            }),
        ),
    ];

    for (case, update) in cases {
        // A fresh store per row, so each row updates the same starting record.
        let router = cluster_fixture(REGION, CLUSTER).await;
        let created = create_association(
            &router,
            REGION,
            CLUSTER,
            &serde_json::json!({
                "namespace": "default",
                "serviceAccount": "updatable-sa",
                "roleArn": ROLE_ARN,
                "tags": { "env": "dev" },
            }),
        )
        .await;
        let before = created.association().clone();
        let association_id = association_id_of(&created);

        // The clock has to have moved for `modifiedAt` to be observably later
        // than `createdAt`, so this waits out one tick of the wire format's
        // resolution rather than asserting a bound that cannot fail.
        tokio::time::sleep(MODIFICATION_GAP).await;
        let updated =
            update_association(&router, REGION, CLUSTER, &association_id, &update).await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: expected the update to apply, got {:?} {:?}",
            updated.status, updated.body
        );
        let after = updated.association().clone();

        // What the request carried is what changed.
        for (member, expected) in update.as_object().expect("an update body") {
            assert_eq!(
                &after[member], expected,
                "case {case:?}: {member} must be overwritten, got {:?}",
                after[member]
            );
        }
        // "An association can't be moved between clusters, namespaces, or service
        // accounts", and the server-generated identity never changes either.
        for member in [
            "clusterName",
            "namespace",
            "serviceAccount",
            "associationId",
            "associationArn",
            "createdAt",
            "tags",
        ] {
            assert_eq!(
                after[member], before[member],
                "case {case:?}: {member} must survive the update, got {:?}",
                after[member]
            );
        }
        // An absent member leaves the stored value alone.
        if update.get("roleArn").is_none() {
            assert_eq!(
                after["roleArn"], before["roleArn"],
                "case {case:?}: an absent roleArn leaves the stored one, got {:?}",
                after["roleArn"]
            );
        }
        assert!(
            number(&after["modifiedAt"]) > number(&before["modifiedAt"]),
            "case {case:?}: an applied update moves modifiedAt forward, got {:?} after {:?}",
            after["modifiedAt"],
            before["modifiedAt"]
        );
        assert_eq!(
            after["externalId"].is_string(),
            after["targetRoleArn"].is_string(),
            "case {case:?}: externalId is present iff a target role is, got {:?}",
            after["externalId"]
        );

        // The update is in the store, not only in its own answer.
        let described = describe_association(&router, REGION, CLUSTER, &association_id).await;
        assert_eq!(
            described.association(),
            &after,
            "case {case:?}: a describe answers the updated record, got {:?}",
            described.association()
        );
    }
}

/// An update that changes nothing is refused: "You must change at least one of
/// these in an update." So is a policy left without the session-tag setting it
/// requires, whichever call supplies which half.
#[tokio::test]
async fn an_update_that_changes_nothing_or_breaks_a_documented_pairing_is_refused() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    let created =
        create_association(&router, REGION, CLUSTER, &minimal_create("default", "sa")).await;
    let association_id = association_id_of(&created);

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("an empty update", serde_json::json!({})),
        (
            "an update carrying only an idempotency token",
            serde_json::json!({ "clientRequestToken": "token-nothing" }),
        ),
        (
            "a policy on an association whose session tags are still on",
            serde_json::json!({ "policy": "{}" }),
        ),
        (
            "a policy with session tags explicitly left on",
            serde_json::json!({ "policy": "{}", "disableSessionTags": false }),
        ),
    ];
    for (case, update) in cases {
        let answer = update_association(&router, REGION, CLUSTER, &association_id, &update).await;
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?} {:?}",
            answer.error_type,
            answer.body
        );
    }

    // And the record is untouched by every refusal above.
    let described = describe_association(&router, REGION, CLUSTER, &association_id).await;
    assert_eq!(
        described.association(),
        created.association(),
        "a refused update must leave the record as it was, got {:?}",
        described.association()
    );
}

/// An idempotency token absorbs a retry of the call that SENT it, and of no
/// other call: a create's token reused on an update must not swallow the
/// update's change, and an update's token absorbs only that update's retry.
#[tokio::test]
async fn an_idempotency_token_absorbs_only_a_retry_of_its_own_call() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    let created = create_association(
        &router,
        REGION,
        CLUSTER,
        &serde_json::json!({
            "namespace": "default",
            "serviceAccount": "token-scoped",
            "roleArn": ROLE_ARN,
            "clientRequestToken": "token-from-the-create",
        }),
    )
    .await;
    let association_id = association_id_of(&created);

    // The CREATE's token on an update that carries a change: the change applies.
    let updated = update_association(
        &router,
        REGION,
        CLUSTER,
        &association_id,
        &serde_json::json!({
            "roleArn": OTHER_ROLE_ARN,
            "clientRequestToken": "token-from-the-create",
        }),
    )
    .await;
    assert_eq!(
        updated.association()["roleArn"],
        serde_json::json!(OTHER_ROLE_ARN),
        "a create's token must not absorb an update that carries a change, got {:?}",
        updated.association()["roleArn"]
    );

    // The UPDATE's own token, resent: absorbed, and the record stands as it is.
    let update = serde_json::json!({
        "roleArn": ROLE_ARN,
        "clientRequestToken": "token-from-the-update",
    });
    let first = update_association(&router, REGION, CLUSTER, &association_id, &update).await;
    let retry = update_association(
        &router,
        REGION,
        CLUSTER,
        &association_id,
        &serde_json::json!({
            // A retry resends the whole call, token included; a DIFFERENT body
            // under the same token is still read as that call's retry.
            "roleArn": OTHER_ROLE_ARN,
            "clientRequestToken": "token-from-the-update",
        }),
    )
    .await;
    assert_eq!(
        retry.association(),
        first.association(),
        "an absorbed retry returns the record the first call left, got {:?}",
        retry.association()
    );

    // A fresh token applies the next change.
    let next = update_association(
        &router,
        REGION,
        CLUSTER,
        &association_id,
        &serde_json::json!({
            "roleArn": OTHER_ROLE_ARN,
            "clientRequestToken": "token-from-a-later-update",
        }),
    )
    .await;
    assert_eq!(
        next.association()["roleArn"],
        serde_json::json!(OTHER_ROLE_ARN),
        "a new token applies the change, got {:?}",
        next.association()["roleArn"]
    );
}

// ---------------------------------------------------------------------------
// RULE: one (namespace, serviceAccount) pair holds at most one association per
// cluster; a retry carrying the token of the call that created it is absorbed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pair_holds_one_association_and_a_retry_token_is_absorbed() {
    let router = cluster_fixture(REGION, CLUSTER).await;
    let request = serde_json::json!({
        "namespace": "default",
        "serviceAccount": "only-one",
        "roleArn": ROLE_ARN,
        "clientRequestToken": "token-only-one",
    });

    let first = create_association(&router, REGION, CLUSTER, &request).await;
    assert_eq!(
        first.status, 200,
        "the first create must succeed, got {:?} {:?}",
        first.status, first.body
    );

    // Same token: a retry of the call that already applied.
    let retry = create_association(&router, REGION, CLUSTER, &request).await;
    assert_eq!(
        retry.status, 200,
        "an absorbed retry answers 200, got {:?} {:?}",
        retry.status, retry.body
    );
    assert_eq!(
        retry.association(),
        first.association(),
        "an absorbed retry returns the original association, got {:?}",
        retry.association()
    );

    // A different (or absent) token on the same pair is a new call, and the pair
    // is taken.
    let conflicting: Vec<(&str, serde_json::Value)> = vec![
        ("a different token", serde_json::json!({
            "namespace": "default", "serviceAccount": "only-one", "roleArn": ROLE_ARN,
            "clientRequestToken": "token-other",
        })),
        ("no token at all", minimal_create("default", "only-one")),
        ("a blank token, which identifies nothing", serde_json::json!({
            "namespace": "default", "serviceAccount": "only-one", "roleArn": ROLE_ARN,
            "clientRequestToken": "   ",
        })),
        ("a different role for the same pair", serde_json::json!({
            "namespace": "default", "serviceAccount": "only-one", "roleArn": OTHER_ROLE_ARN,
        })),
    ];
    for (case, request) in conflicting {
        let answer = create_association(&router, REGION, CLUSTER, &request).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: a taken pair is a 409, got {:?} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {:?}",
            answer.error_type
        );
    }

    // Differing in either half of the pair is a different association.
    for (namespace, service_account) in [("default", "another-sa"), ("other-ns", "only-one")] {
        let answer =
            create_association(&router, REGION, CLUSTER, &minimal_create(namespace, service_account))
                .await;
        assert_eq!(
            answer.status, 200,
            "case {:?}: a different pair is a different association, got {:?} {:?}",
            (namespace, service_account),
            answer.status,
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an association id is unique — within its cluster, across clusters and
// Regions, and across a delete and re-create of the same pair — and always of
// the documented form.
// ---------------------------------------------------------------------------

/// The rows walk the dimensions the uniqueness invariant generalises over
/// (namespace, service account, cluster, Region, and time through a
/// delete/re-create), rather than one recorded pair.
#[tokio::test]
async fn minted_association_ids_are_well_formed_and_never_repeat() {
    let router = emulator();
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut minted = 0usize;

    for (region, cluster) in [
        (REGION, "cluster-one"),
        (REGION, "cluster-two"),
        (OTHER_REGION, "cluster-one"),
    ] {
        let created_cluster = create_cluster(&router, region, cluster).await;
        assert_eq!(
            created_cluster.status, 200,
            "case {:?}: the fixture cluster must be created, got {:?}",
            (region, cluster),
            created_cluster.body
        );
        for namespace in ["default", "kube-system"] {
            for index in 0..3 {
                let service_account = format!("sa-{index}");
                let created = create_association(
                    &router,
                    region,
                    cluster,
                    &minimal_create(namespace, &service_account),
                )
                .await;
                let case = (region, cluster, namespace, service_account.clone());
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: expected a created association, got {:?} {:?}",
                    created.status, created.body
                );
                let id = association_id_of(&created);
                assert!(
                    is_association_id(&id),
                    "case {case:?}: minted id must satisfy the documented form, got {id:?}"
                );
                assert!(
                    ids.insert(id.clone()),
                    "case {case:?}: minted id {id:?} was already handed out"
                );
                minted += 1;
            }
        }
    }
    assert_eq!(
        ids.len(),
        minted,
        "every minted id must be distinct, got {} for {minted} creates",
        ids.len()
    );

    // Time is the last dimension: the same pair, created again after a delete,
    // gets a NEW id rather than resurrecting the old one.
    let first = create_association(&router, REGION, "cluster-one", &minimal_create("recycled", "sa"))
        .await;
    let first_id = association_id_of(&first);
    let deleted = delete_association(&router, REGION, "cluster-one", &first_id).await;
    assert_eq!(
        deleted.status, 200,
        "the delete must succeed, got {:?} {:?}",
        deleted.status, deleted.body
    );
    let second =
        create_association(&router, REGION, "cluster-one", &minimal_create("recycled", "sa")).await;
    let second_id = association_id_of(&second);
    assert!(
        is_association_id(&second_id) && second_id != first_id,
        "a re-created pair must mint a new well-formed id, got {second_id:?} after {first_id:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a delete removes the association from the store — and deleting the
// cluster removes the associations that live in it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_removes_the_association_and_the_cluster_takes_its_children() {
    let router = cluster_fixture(REGION, CLUSTER).await;

    let created =
        create_association(&router, REGION, CLUSTER, &minimal_create("default", "doomed")).await;
    let association_id = association_id_of(&created);

    let deleted = delete_association(&router, REGION, CLUSTER, &association_id).await;
    assert_eq!(
        deleted.status, 200,
        "expected the association to be deleted, got {:?} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.association(),
        created.association(),
        "a delete answers the record it removed, got {:?}",
        deleted.association()
    );

    // Gone from the store: both a second delete and a describe are not-founds.
    for (operation, answer) in [
        (
            "describe",
            describe_association(&router, REGION, CLUSTER, &association_id).await,
        ),
        (
            "delete again",
            delete_association(&router, REGION, CLUSTER, &association_id).await,
        ),
        (
            "update",
            update_association(
                &router,
                REGION,
                CLUSTER,
                &association_id,
                &serde_json::json!({ "roleArn": OTHER_ROLE_ARN }),
            )
            .await,
        ),
    ] {
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {operation:?}: a deleted association is not found, got {:?} {:?}",
            answer.error_type,
            answer.body
        );
    }

    // An association is a child of its cluster: a cluster created again with the
    // same name in the same Region does not inherit the old one's associations.
    let survivor =
        create_association(&router, REGION, CLUSTER, &minimal_create("default", "survivor")).await;
    let survivor_id = association_id_of(&survivor);
    let cluster_deleted = delete_cluster(&router, REGION, CLUSTER).await;
    assert_eq!(
        cluster_deleted.status, 200,
        "the cluster must delete, got {:?} {:?}",
        cluster_deleted.status, cluster_deleted.body
    );
    let recreated = create_cluster(&router, REGION, CLUSTER).await;
    assert_eq!(
        recreated.status, 200,
        "the cluster must be creatable again, got {:?} {:?}",
        recreated.status, recreated.body
    );
    let orphan = describe_association(&router, REGION, CLUSTER, &survivor_id).await;
    assert_eq!(
        orphan.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "a re-created cluster must not inherit associations, got {:?} {:?}",
        orphan.error_type,
        orphan.body
    );
}

// ---------------------------------------------------------------------------
// RULE: an association belongs to one cluster in one Region, so the same cluster
// name in two Regions holds two independent sets.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_cluster_name_in_two_regions_holds_independent_associations() {
    let router = emulator();
    for region in [REGION, OTHER_REGION] {
        let created = create_cluster(&router, region, CLUSTER).await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the fixture cluster must be created, got {:?}",
            created.body
        );
    }

    let mut ids = Vec::new();
    for region in [REGION, OTHER_REGION] {
        let created =
            create_association(&router, region, CLUSTER, &minimal_create("default", "shared-sa"))
                .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: the same pair is free in each Region, got {:?} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.association()["associationArn"]
                .as_str()
                .map(|arn| arn.contains(region)),
            Some(true),
            "case {region:?}: the ARN names the Region the request was signed for, got {:?}",
            created.association()["associationArn"]
        );
        ids.push(association_id_of(&created));
    }

    // Each Region answers only for its own association.
    for (index, region) in [REGION, OTHER_REGION].into_iter().enumerate() {
        let own = describe_association(&router, region, CLUSTER, &ids[index]).await;
        assert_eq!(
            own.status, 200,
            "case {region:?}: a Region describes its own association, got {:?} {:?}",
            own.status, own.body
        );
        let other = describe_association(&router, region, CLUSTER, &ids[1 - index]).await;
        assert_eq!(
            other.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {region:?}: the other Region's id is not found here, got {:?} {:?}",
            other.error_type,
            other.body
        );
    }
}
