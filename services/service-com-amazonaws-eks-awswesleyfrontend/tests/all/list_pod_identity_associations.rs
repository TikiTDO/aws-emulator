// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListPodIdentityAssociations`: the EKS Pod Identity
//! associations one cluster holds, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (which optional
//! members a create supplied, which filter the listing is given, the page size,
//! the shape of a foreign page token, the shape of a cluster name, the Region the
//! call is signed for), and each assertion names its case and the actual value.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_eks_awswesleyfrontend::state::{is_association_id, MAX_CLUSTER_NAME_LEN};
use smithy_server_runtime::MultiServiceRouter;

// The wire plumbing (one shared engine behind the production router, label
// encoding, the reduced `Answer`) is shared with every other suite in this
// crate — see `tests/all/wire.rs`.
use crate::wire::{call, emulator, encode_label, Answer};

const REGION: &str = "us-west-2";
const OTHER_REGION: &str = "eu-west-1";
const CLUSTER: &str = "listing-cluster";
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/pod-identity";
const TARGET_ROLE_ARN: &str = "arn:aws:iam::111122223333:role/target";

/// The members `PodIdentityAssociationSummary` declares
/// (`smithy_query query="shape" shape="PodIdentityAssociationSummary"`).
const SUMMARY_MEMBERS: &[&str] = &[
    "clusterName",
    "namespace",
    "serviceAccount",
    "associationArn",
    "associationId",
    "ownerArn",
];

/// The members the summary is "simplified by removing ... compared to the full
/// `PodIdentityAssociation`" (that shape's own documentation names the first
/// four; the rest are simply not members of the summary).
const OMITTED_MEMBERS: &[&str] = &[
    "roleArn",
    "createdAt",
    "modifiedAt",
    "tags",
    "disableSessionTags",
    "targetRoleArn",
    "externalId",
    "policy",
];

async fn create_cluster(router: &MultiServiceRouter, region: &str, name: &str) {
    let body = serde_json::json!({
        "name": name,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    });
    let answer = call(router, region, "POST", "/clusters", &body.to_string()).await;
    assert_eq!(
        answer.status, 200,
        "the fixture cluster {name:?} in {region} must be created, got {:?} {:?}",
        answer.status, answer.body
    );
}

async fn create_association(
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

async fn delete_association(
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

async fn list_associations(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    query: &str,
) -> Answer {
    let path = format!(
        "/clusters/{}/pod-identity-associations",
        encode_label(cluster)
    );
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, region, "GET", &uri, "").await
}

/// The smallest request `CreatePodIdentityAssociation` accepts.
fn minimal_create(namespace: &str, service_account: &str) -> serde_json::Value {
    serde_json::json!({
        "namespace": namespace,
        "serviceAccount": service_account,
        "roleArn": ROLE_ARN,
    })
}

/// The `associationId`s one page reports, in the order they arrived.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["associations"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["associationId"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn summaries(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["associations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: the listing reports one summary per association the cluster's own
// creates persisted, each carrying exactly the members
// `PodIdentityAssociationSummary` declares and none of the ones that shape
// removes compared to the full association.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional members the
/// create supplied (none, tags, a target role, a policy with session tags
/// disabled, all of them) and the shape of the Kubernetes names.
#[tokio::test]
async fn every_created_association_is_listed_once_as_a_summary() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    let cases: Vec<(&str, serde_json::Value)> = vec![
        ("only the required members", minimal_create("default", "sa")),
        (
            "with tags",
            serde_json::json!({
                "namespace": "kube-system",
                "serviceAccount": "tagged-sa",
                "roleArn": ROLE_ARN,
                "tags": { "env": "dev", "team": "platform" },
            }),
        ),
        (
            "with a target role, which earns an externalId",
            serde_json::json!({
                "namespace": "targeted",
                "serviceAccount": "cross-account-sa",
                "roleArn": ROLE_ARN,
                "targetRoleArn": TARGET_ROLE_ARN,
            }),
        ),
        (
            "with a policy and the session tags it requires",
            serde_json::json!({
                "namespace": "restricted",
                "serviceAccount": "least-privilege-sa",
                "roleArn": ROLE_ARN,
                "policy": "{\"Version\":\"2012-10-17\",\"Statement\":[]}",
                "disableSessionTags": true,
            }),
        ),
        (
            "dotted and digit-led Kubernetes names, every optional member",
            serde_json::json!({
                "namespace": "2nd.namespace",
                "serviceAccount": "my.service.account",
                "roleArn": ROLE_ARN,
                "targetRoleArn": TARGET_ROLE_ARN,
                "policy": "{\"Statement\":[]}",
                "disableSessionTags": true,
                "tags": { "only": "one" },
                "clientRequestToken": "token-everything",
            }),
        ),
    ];

    let mut created: Vec<(&str, serde_json::Value)> = Vec::new();
    for (case, request) in &cases {
        let answer = create_association(&router, REGION, CLUSTER, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the fixture create must succeed, got {:?} {:?}",
            answer.status, answer.body
        );
        created.push((case, answer.association().clone()));
    }

    let listed = list_associations(&router, REGION, CLUSTER, "").await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from the listing, got {:?} {:?}",
        listed.status, listed.body
    );
    let ids: BTreeSet<String> = listed_ids(&listed).into_iter().collect();
    assert_eq!(
        ids.len(),
        created.len(),
        "the listing must hold one summary per created association, got {:?}",
        listed.body
    );

    for (case, association) in &created {
        let association_id = association["associationId"].as_str().unwrap_or_default();
        let summary = summaries(&listed)
            .into_iter()
            .find(|summary| summary["associationId"].as_str() == Some(association_id))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: association {association_id:?} is missing from {:?}",
                    listed.body
                )
            });

        assert!(
            is_association_id(association_id),
            "case {case:?}: the listed id must have the documented form, got \
             {association_id:?}"
        );
        // Each declared member is the association's own.
        for member in SUMMARY_MEMBERS {
            assert_eq!(
                summary[member], association[member],
                "case {case:?}: summary {member} must match the association, got {:?}",
                summary[member]
            );
        }
        assert!(
            summary["ownerArn"].is_null(),
            "case {case:?}: only an EKS add-on owns an association, got {:?}",
            summary["ownerArn"]
        );
        // And every member the summary removes is absent — including ones this
        // very association carries (its role, its tags, its policy).
        for member in OMITTED_MEMBERS {
            assert!(
                summary[member].is_null(),
                "case {case:?}: a summary omits {member}, got {:?}",
                summary[member]
            );
        }
    }

    // The listing reports the STORE, so a deleted association leaves it.
    let (_, doomed) = &created[0];
    let doomed_id = doomed["associationId"].as_str().unwrap_or_default();
    let deleted = delete_association(&router, REGION, CLUSTER, doomed_id).await;
    assert_eq!(
        deleted.status, 200,
        "the fixture delete must succeed, got {:?} {:?}",
        deleted.status, deleted.body
    );
    let after = list_associations(&router, REGION, CLUSTER, "").await;
    assert!(
        !listed_ids(&after).iter().any(|id| id == doomed_id),
        "a deleted association must leave the listing, got {:?}",
        after.body
    );
    assert_eq!(
        listed_ids(&after).len(),
        created.len() - 1,
        "exactly the deleted association leaves the listing, got {:?}",
        after.body
    );

    // A cluster nobody created an association in lists nothing and offers no
    // token — an empty page, not a not-found.
    create_cluster(&router, REGION, "empty-cluster").await;
    let empty = list_associations(&router, REGION, "empty-cluster", "").await;
    assert_eq!(
        empty.status, 200,
        "an empty cluster lists 200, got {:?} {:?}",
        empty.status, empty.body
    );
    assert!(
        listed_ids(&empty).is_empty() && empty.body["nextToken"].is_null(),
        "an empty cluster lists nothing and offers no nextToken, got {:?}",
        empty.body
    );
}

// ---------------------------------------------------------------------------
// RULE: a `clusterName` outside its documented form is an invalid parameter
// (400); one that is well formed but names no cluster in the Region the request
// is signed for is not found (404) — whatever else the store holds.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn listing_an_absent_cluster_is_not_found_and_a_malformed_one_is_refused() {
    let router = emulator();
    // The store is not empty: a cluster with an association, so a refusal below
    // is about the NAME rather than about an empty store.
    create_cluster(&router, REGION, CLUSTER).await;
    let seeded = create_association(&router, REGION, CLUSTER, &minimal_create("default", "sa")).await;
    let seeded_id = seeded.association()["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    // And a cluster of another name in another Region.
    create_cluster(&router, OTHER_REGION, "elsewhere").await;

    // WELL FORMED but absent: the rows span name shapes the pattern admits —
    // short, long (at the published bound), underscored, digit-led, the recorded
    // issue's own name, and a name that exists only in another Region.
    let at_the_bound = "c".repeat(MAX_CLUSTER_NAME_LEN);
    for case in [
        "absent",
        "sim2realbehavabsentcluster",
        "elsewhere",
        "Upper_Case-Name1",
        "c",
        at_the_bound.as_str(),
    ] {
        let answer = list_associations(&router, REGION, case, "").await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: an absent cluster is not found, got {:?} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_type
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(format!("No cluster found for name: {case}.").as_str()),
            "case {case:?}: the message names the cluster, got {:?}",
            answer.body["message"]
        );
    }

    // MALFORMED: the rows span the ways a name leaves `CreateCluster$name`'s own
    // pattern and length.
    let past_the_bound = "c".repeat(MAX_CLUSTER_NAME_LEN + 1);
    for case in [
        "-leading-dash",
        "_leading-underscore",
        "has spaces",
        "has!punctuation",
        "has/slash",
        past_the_bound.as_str(),
    ] {
        let answer = list_associations(&router, REGION, case, "").await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a malformed clusterName is a 400, got {:?} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.error_type
        );
        let message = answer.body["message"].as_str().unwrap_or_default();
        assert!(
            message.contains("clusterName")
                && if case.len() > MAX_CLUSTER_NAME_LEN {
                    message.contains(&MAX_CLUSTER_NAME_LEN.to_string())
                } else {
                    // The grammar's own sentence, the one every cluster-name member
                    // of this service is refused with.
                    message.contains("contains invalid characters")
                        && message.contains("regular expression")
                },
            "case {case:?}: expected the constraint it failed, got {message:?}"
        );
    }

    // The listing is a READ: every refusal above left the store as it was.
    let listed = list_associations(&router, REGION, CLUSTER, "").await;
    assert_eq!(
        listed_ids(&listed),
        vec![seeded_id],
        "a refused or not-found listing must not change the store, got {:?}",
        listed.body
    );
}

// ---------------------------------------------------------------------------
// RULE: `namespace` and `serviceAccount` narrow the listing to the associations
// whose own member is EXACTLY that value; an absent filter narrows nothing, and
// both filters apply together.
// ---------------------------------------------------------------------------

/// One filter row: its name, the query string it sends, and the
/// `(namespace, serviceAccount)` pairs it must select.
type FilterCase<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);

#[tokio::test]
async fn the_namespace_and_service_account_filters_select_exact_matches() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    // Every combination of two namespaces and two service accounts, so each
    // filter below has both a match and a non-match to tell apart.
    let mut ids: BTreeMap<(&str, &str), String> = BTreeMap::new();
    for namespace in ["ns-a", "ns-b"] {
        for service_account in ["sa-1", "sa-2"] {
            let answer = create_association(
                &router,
                REGION,
                CLUSTER,
                &minimal_create(namespace, service_account),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "fixture {:?}: expected 200, got {:?} {:?}",
                (namespace, service_account),
                answer.status,
                answer.body
            );
            ids.insert(
                (namespace, service_account),
                answer.association()["associationId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            );
        }
    }

    let cases: Vec<FilterCase<'_>> = vec![
        (
            "no filter",
            "",
            vec![
                ("ns-a", "sa-1"),
                ("ns-a", "sa-2"),
                ("ns-b", "sa-1"),
                ("ns-b", "sa-2"),
            ],
        ),
        (
            "namespace only",
            "namespace=ns-a",
            vec![("ns-a", "sa-1"), ("ns-a", "sa-2")],
        ),
        (
            "service account only",
            "serviceAccount=sa-1",
            vec![("ns-a", "sa-1"), ("ns-b", "sa-1")],
        ),
        (
            "both filters",
            "namespace=ns-b&serviceAccount=sa-2",
            vec![("ns-b", "sa-2")],
        ),
        (
            "both filters, the other pairing",
            "namespace=ns-a&serviceAccount=sa-2",
            vec![("ns-a", "sa-2")],
        ),
        ("a namespace nothing is in", "namespace=ns-c", vec![]),
        (
            "a service account nothing uses",
            "serviceAccount=sa-3",
            vec![],
        ),
        (
            "a filter differing from a stored value only in case",
            "namespace=NS-A",
            vec![],
        ),
        (
            "a matching namespace with a non-matching service account",
            "namespace=ns-a&serviceAccount=sa-3",
            vec![],
        ),
    ];

    for (case, query, expected) in cases {
        let answer = list_associations(&router, REGION, CLUSTER, query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {:?} {:?}",
            answer.status, answer.body
        );
        let expected_ids: BTreeSet<String> =
            expected.iter().map(|pair| ids[pair].clone()).collect();
        let selected: BTreeSet<String> = listed_ids(&answer).into_iter().collect();
        assert_eq!(
            selected, expected_ids,
            "case {case:?}: the filter must select exactly its matches, got {:?}",
            answer.body
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: whatever page size the listing is driven at, following `nextToken` to
// exhaustion yields each association exactly once in id order and the last page
// carries no token; a token the listing did not issue is an invalid parameter,
// and so is a `maxResults` outside the modelled range.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_at_any_size_yields_every_association_exactly_once() {
    let router = emulator();
    create_cluster(&router, REGION, CLUSTER).await;

    let mut stored: Vec<String> = Vec::new();
    for index in 0..7 {
        let answer = create_association(
            &router,
            REGION,
            CLUSTER,
            &minimal_create("default", &format!("sa-{index}")),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "fixture {index}: expected 200, got {:?} {:?}",
            answer.status, answer.body
        );
        stored.push(
            answer.association()["associationId"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        );
    }
    stored.sort();

    // `maxResults` is modelled `range 1..100`, so the rows span both published
    // bounds and the sizes between.
    for page_size in [1, 2, 3, 6, 7, 8, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
            };
            let answer = list_associations(&router, REGION, CLUSTER, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected 200, got {:?} {:?}",
                answer.status, answer.body
            );
            let page = listed_ids(&answer);
            assert!(
                page.len() <= page_size as usize,
                "case maxResults={page_size}: a page must not exceed the limit, got {page:?}"
            );
            seen.extend(page.clone());
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case maxResults={page_size}: a token was returned for an empty page"
                    );
                    token = Some(next.to_owned());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, stored,
            "case maxResults={page_size}: paging must yield every association once, in \
             id order; got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: an association was repeated across pages: {seen:?}"
        );
    }

    // One past each published bound is refused by the modelled range, before the
    // handler runs.
    for case in ["maxResults=0", "maxResults=101"] {
        let answer = list_associations(&router, REGION, CLUSTER, case).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: outside the modelled range expected 400, got {:?} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.error_type
        );
    }

    // A token this listing did not issue is a bad parameter, not an empty page.
    // The rows differ along HOW the token is foreign: not this encoding's
    // alphabet, an odd length, a well-formed encoding of bytes that are not an
    // id, and the hardest one — a hex encoding of a REAL association id that
    // carries no provenance tag.
    let untagged: String = stored[0].bytes().map(|byte| format!("{byte:02x}")).collect();
    for case in ["not-hex", "zz", "abc", "ffffffffff", untagged.as_str()] {
        let answer =
            list_associations(&router, REGION, CLUSTER, &format!("nextToken={case}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: a foreign nextToken is a 400, got {:?} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {:?}",
            answer.error_type
        );
    }

    // And a token ANOTHER listing of this same service issued is foreign too: a
    // page token carries the listing it belongs to, so `ListClusters`' token
    // cannot be resumed from here even though it is well formed.
    create_cluster(&router, REGION, "second-cluster").await;
    let clusters = call(&router, REGION, "GET", "/clusters?maxResults=1", "").await;
    let other_listings_token = clusters.body["nextToken"]
        .as_str()
        .expect("ListClusters must offer a token over two clusters")
        .to_owned();
    let answer = list_associations(
        &router,
        REGION,
        CLUSTER,
        &format!("nextToken={other_listings_token}"),
    )
    .await;
    assert_eq!(
        answer.error_type.as_deref(),
        Some("InvalidParameterException"),
        "case {other_listings_token:?}: another listing's token is foreign, got {:?} {:?}",
        answer.error_type,
        answer.body
    );
}

// ---------------------------------------------------------------------------
// RULE: an association belongs to one cluster in one Region, so a listing
// reports only the associations of the cluster of that name in the Region the
// request is signed for.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_region_and_cluster_lists_only_its_own_associations() {
    let router = emulator();

    // The rows differ along the pair of Regions, so the rule is not tested only
    // against the suite's default one.
    for (left, right) in [(REGION, OTHER_REGION), ("us-east-1", "ap-south-1")] {
        let cluster = format!("shared-{left}");
        let sibling = format!("sibling-{left}");
        create_cluster(&router, left, &cluster).await;
        create_cluster(&router, right, &cluster).await;
        create_cluster(&router, left, &sibling).await;

        let in_left =
            create_association(&router, left, &cluster, &minimal_create("default", "sa-left")).await;
        assert_eq!(
            in_left.status, 200,
            "case {left}: expected 200, got {:?} {:?}",
            in_left.status, in_left.body
        );
        let left_id = in_left.association()["associationId"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        assert!(
            in_left.association()["associationArn"]
                .as_str()
                .is_some_and(|arn| arn.contains(&format!(":{left}:"))),
            "case {left}: the ARN carries the request's Region, got {:?}",
            in_left.association()["associationArn"]
        );

        // The same cluster name in the other Region holds none of it, and neither
        // does another cluster in the same Region.
        for (case, region, name) in [
            ("the same name in another Region", right, cluster.as_str()),
            ("another cluster in the same Region", left, sibling.as_str()),
        ] {
            let listed = list_associations(&router, region, name, "").await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected 200, got {:?} {:?}",
                listed.status, listed.body
            );
            assert!(
                listed_ids(&listed).is_empty(),
                "case {case:?}: another cluster's associations must not leak, got {:?}",
                listed.body
            );
        }

        let listed_left = list_associations(&router, left, &cluster, "").await;
        assert_eq!(
            listed_ids(&listed_left),
            vec![left_id],
            "case {left}: expected its own association, got {:?}",
            listed_left.body
        );
    }
}
