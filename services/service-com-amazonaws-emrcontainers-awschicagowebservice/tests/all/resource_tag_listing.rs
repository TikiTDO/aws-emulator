// SPDX-License-Identifier: CC0-1.0

//! `ListTagsForResource` (`GET /tags/{resourceArn}`), end to end.
//!
//! Every call goes through the production path: this service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop::Emr`]), so routing, the `@httpLabel` ARN,
//! constraint validation, the handler and serialization all run exactly as they do for an
//! AWS SDK client. Nothing reaches into `crate::state`: what the listing answers is only
//! believed when another read of the same resource reports the same thing.
//!
//! The rule under test: `ListTagsForResource` answers the ONE tag set held by the record
//! of the resource its `resourceArn` names — always a map, empty for a resource holding
//! no tags — and changes nothing, so a later read of the same resource reports what it
//! did before. A `resourceArn` that does not resolve to a resource this endpoint holds is
//! refused with the modelled `ValidationException`.
//!
//! The tables span what that rule generalises over: how many tags the resource holds
//! (none, one, several, the published maximum), the form of a key and of a value (ASCII,
//! Unicode, case, the published length bounds), which resource type the ARN names (a
//! virtual cluster, a job run under one, a managed endpoint under one), and which part of
//! the ARN fails to name a stored resource.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::resource_tag_removal::{
    cluster_tags, encoded, job_run_tags, key_of, tag_object, tagged_cluster, tagged_job_run, tags,
    untag, MAX_TAG_KEY_CHARS,
};
use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr, REGION};

/// `TagMap` is `len 0..50` (smithy_query: shape `TagMap`), so 50 pairs is the most a
/// resource can hold and so the most a listing ever reports.
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// `StringEmpty256`, the `TagMap` value shape, is `len 0..256` with pattern `\S`
/// (smithy_query: shape `TagMap`), so 256 characters is the longest value a tag can carry.
const MAX_TAG_VALUE_CHARS: usize = 256;

/// `GET /tags/{resourceArn}` — `resourceArn` is an `@httpLabel`, so the ARN travels inside
/// one path segment, percent-encoded the way an SDK encodes a label.
async fn list_tags(emr: &Emr, resource_arn: &str) -> Answer {
    emr.send("GET", &format!("/tags/{}", encoded(resource_arn)), None)
        .await
}

/// The `tags` member of a successful `ListTagsForResource`.
async fn listed_tags(emr: &Emr, resource_arn: &str, case: &str) -> Value {
    let answer = list_tags(emr, resource_arn).await;
    field(answer.ok(case), "/tags", case).clone()
}

/// Create a managed endpoint carrying `initial` and answer its `(id, arn)`. The body
/// carries the members `CreateManagedEndpoint` marks `@required` beside the
/// `@httpLabel` cluster id (smithy_query: operation `CreateManagedEndpoint`).
async fn tagged_endpoint(
    emr: &Emr,
    cluster: &str,
    token: &str,
    initial: &BTreeMap<String, String>,
) -> (String, String) {
    let answer = emr
        .send(
            "POST",
            &format!("/virtualclusters/{cluster}/endpoints"),
            Some(json!({
                "name": "tagged-endpoint",
                "clientToken": token,
                "type": "JUPYTER_ENTERPRISE_GATEWAY",
                "releaseLabel": "emr-6.10.0-latest",
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
                "tags": tag_object(initial),
            })),
        )
        .await;
    let body = answer.ok(token).clone();
    (text(&body, "/id", token), text(&body, "/arn", token))
}

/// The tag set `DescribeManagedEndpoint` reports for this endpoint.
async fn endpoint_tags(emr: &Emr, cluster: &str, id: &str, case: &str) -> Value {
    let answer = emr
        .send(
            "GET",
            &format!("/virtualclusters/{cluster}/endpoints/{id}"),
            None,
        )
        .await;
    field(answer.ok(case), "/endpoint/tags", case).clone()
}

/// One row of the listing table: the tag set the resource is created with, which a listing
/// of it must report back exactly.
struct ListingCase {
    case: &'static str,
    /// Also the cluster name, so each row works on its own resource.
    name: &'static str,
    tagset: BTreeMap<String, String>,
}

/// RULE: a listing reports exactly the tag set the resource's own record holds, whatever
/// the number of tags and whatever the form of their keys and values — and the empty set
/// is reported as an empty map, not as an absent member.
#[tokio::test]
async fn listing_reports_the_tag_set_the_resource_holds() {
    let long_key = key_of(MAX_TAG_KEY_CHARS);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    let full_set: BTreeMap<String, String> = (0..MAX_TAGS_PER_RESOURCE)
        .map(|n| (format!("k{n}"), format!("v{n}")))
        .collect();

    let cases = vec![
        ListingCase {
            case: "a resource holding no tags",
            name: "no-tags",
            tagset: BTreeMap::new(),
        },
        ListingCase {
            case: "a single tag",
            name: "one-tag",
            tagset: tags(&[("env", "prod")]),
        },
        ListingCase {
            case: "several tags",
            name: "several-tags",
            tagset: tags(&[("env", "dev"), ("owner", "platform"), ("cost-centre", "42")]),
        },
        ListingCase {
            case: "keys differing only in case are different tags",
            name: "case-sensitive",
            tagset: tags(&[("Env", "prod"), ("env", "dev")]),
        },
        ListingCase {
            case: "a non-ASCII key and value",
            name: "unicode",
            tagset: tags(&[("プロジェクト", "Ωmega"), ("a", "1")]),
        },
        ListingCase {
            case: "a key at String128's maximum length",
            name: "longest-key",
            tagset: {
                let mut set = tags(&[("a", "1")]);
                set.insert(long_key.clone(), "v".to_string());
                set
            },
        },
        ListingCase {
            case: "a value at StringEmpty256's maximum length",
            name: "longest-value",
            tagset: {
                let mut set = tags(&[("a", "1")]);
                set.insert("long".to_string(), long_value.clone());
                set
            },
        },
        ListingCase {
            case: "TagMap's maximum of 50 tags",
            name: "fifty-tags",
            tagset: full_set.clone(),
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.case;
        let (id, arn) = tagged_cluster(&emr, case.name, &case.tagset).await;

        let expected = tag_object(&case.tagset);
        let listed = listed_tags(&emr, &arn, label).await;
        assert_eq!(
            listed, expected,
            "case {label:?}: expected the listing to report {expected}, got {listed}"
        );

        // The SAME tag set the resource's own reader reports: one copy, two readers.
        let described = cluster_tags(&emr, &id, label).await;
        assert_eq!(
            listed, described,
            "case {label:?}: the listing reported {listed} where DescribeVirtualCluster \
             reports {described}"
        );
    }
}

/// RULE, across resource TYPES: the ARN selects whose tag set is reported, so a job run's
/// listing is its own and not its virtual cluster's, nor a sibling run's, nor an
/// endpoint's.
#[tokio::test]
async fn listing_reports_the_resource_the_arn_names() {
    let emr = Emr::new();
    let cluster_tagset = tags(&[("owner", "platform"), ("env", "dev")]);
    let (cluster, cluster_arn) = tagged_cluster(&emr, "arn-selects-read", &cluster_tagset).await;
    let run_tagset = tags(&[("owner", "analytics"), ("cost-centre", "42")]);
    let (run, run_arn) = tagged_job_run(&emr, &cluster, "tok-list-run", &run_tagset).await;
    let sibling_tagset = tags(&[("owner", "research")]);
    let (sibling, sibling_arn) =
        tagged_job_run(&emr, &cluster, "tok-list-sibling", &sibling_tagset).await;
    let untagged_tagset = BTreeMap::new();
    let (_, untagged_arn) =
        tagged_job_run(&emr, &cluster, "tok-list-untagged", &untagged_tagset).await;
    let endpoint_tagset = tags(&[("owner", "studio"), ("env", "test")]);
    let (endpoint, endpoint_arn) =
        tagged_endpoint(&emr, &cluster, "tok-list-endpoint", &endpoint_tagset).await;

    let rows: Vec<(&str, &str, &BTreeMap<String, String>)> = vec![
        ("the virtual cluster", cluster_arn.as_str(), &cluster_tagset),
        ("the job run", run_arn.as_str(), &run_tagset),
        ("the sibling job run", sibling_arn.as_str(), &sibling_tagset),
        (
            "a job run carrying no tags",
            untagged_arn.as_str(),
            &untagged_tagset,
        ),
        (
            "the managed endpoint under the same cluster",
            endpoint_arn.as_str(),
            &endpoint_tagset,
        ),
    ];

    for (label, arn, expected) in rows {
        let expected = tag_object(expected);
        let listed = listed_tags(&emr, arn, label).await;
        assert_eq!(
            listed, expected,
            "case {label:?}: expected the listing to report {expected}, got {listed}"
        );
    }

    // And the job run's own reader agrees with the listing of it.
    let case = "the job run's own reader";
    let described = job_run_tags(&emr, &cluster, &run, case).await;
    assert_eq!(
        listed_tags(&emr, &run_arn, case).await,
        described,
        "case {case:?}: the listing must report what DescribeJobRun reports"
    );
    let case = "the sibling's own reader";
    assert_eq!(
        listed_tags(&emr, &sibling_arn, case).await,
        job_run_tags(&emr, &cluster, &sibling, case).await,
        "case {case:?}: the listing must report what DescribeJobRun reports"
    );
    let case = "the managed endpoint's own reader";
    assert_eq!(
        listed_tags(&emr, &endpoint_arn, case).await,
        endpoint_tags(&emr, &cluster, &endpoint, case).await,
        "case {case:?}: the listing must report what DescribeManagedEndpoint reports"
    );
}

/// RULE: the listing is a report, so it changes nothing — and it is computed from state,
/// so an edit made between two listings is the only thing that moves it.
#[tokio::test]
async fn listing_reports_state_and_changes_none_of_it() {
    let emr = Emr::new();
    let initial = tags(&[("a", "1"), ("b", "2"), ("c", "3")]);
    let (id, arn) = tagged_cluster(&emr, "reads-are-reads", &initial).await;

    let case = "repeated listings";
    let first = listed_tags(&emr, &arn, case).await;
    let second = listed_tags(&emr, &arn, case).await;
    assert_eq!(
        first,
        tag_object(&initial),
        "case {case:?}: the first listing must report the created tag set, got {first}"
    );
    assert_eq!(
        second, first,
        "case {case:?}: a second listing must report {first}, got {second}"
    );
    assert_eq!(
        cluster_tags(&emr, &id, case).await,
        first,
        "case {case:?}: listing must not change what DescribeVirtualCluster reports"
    );

    let case = "after a removal";
    untag(&emr, &arn, &["b"]).await.ok(case);
    let remaining = tags(&[("a", "1"), ("c", "3")]);
    let listed = listed_tags(&emr, &arn, case).await;
    assert_eq!(
        listed,
        tag_object(&remaining),
        "case {case:?}: the listing must report the set left after the removal, got {listed}"
    );

    let case = "after every tag is removed";
    untag(&emr, &arn, &["a", "c"]).await.ok(case);
    let listed = listed_tags(&emr, &arn, case).await;
    assert_eq!(
        listed,
        tag_object(&BTreeMap::new()),
        "case {case:?}: the listing must report an empty map, got {listed}"
    );
}

/// One row of the refusal table: an `RsiArn` the boundary accepts, and what
/// `ListTagsForResource` must answer for it.
struct ArnCase {
    case: &'static str,
    resource_arn: String,
    code: &'static str,
}

/// RULE: a `resourceArn` that does not resolve to a resource this endpoint holds — outside
/// the modelled `RsiArn` constraint, naming another partition, account or region, naming a
/// resource path none of the forms this service's resource ARNs declare, or naming a
/// resource that is not stored — is an invalid resource reference: HTTP 400 with the
/// modelled `ValidationException`, and never an empty tag set
/// ([`crate::tagging_resource_arn_form`] pins its wording).
#[tokio::test]
async fn listing_an_arn_that_names_no_resource_is_refused() {
    let emr = Emr::new();
    let cluster_tagset = tags(&[("env", "dev")]);
    let (cluster, _) = tagged_cluster(&emr, "read-bystander", &cluster_tagset).await;
    let run_tagset = tags(&[("keep", "me")]);
    let (run, _) = tagged_job_run(&emr, &cluster, "tok-read-bystander", &run_tagset).await;

    // `ResourceIdString` ids are `^[0-9a-z]+$`, so these name nothing while still being
    // ARNs this service could have minted.
    let unknown = "0000000000000000000000000";

    let cases = vec![
        ArnCase {
            case: "a virtual cluster ARN naming no cluster",
            resource_arn: state::virtual_cluster_arn(REGION, unknown),
            code: "ValidationException",
        },
        ArnCase {
            case: "a job run ARN naming no run under a real cluster",
            resource_arn: state::job_run_arn(REGION, &cluster, unknown),
            code: "ValidationException",
        },
        ArnCase {
            case: "a real job run id under a cluster that does not exist",
            resource_arn: state::job_run_arn(REGION, unknown, &run),
            code: "ValidationException",
        },
        ArnCase {
            case: "a managed endpoint ARN naming no endpoint under a real cluster",
            resource_arn: state::managed_endpoint_arn(REGION, &cluster, unknown),
            code: "ValidationException",
        },
        ArnCase {
            case: "the cluster's own id in another region",
            resource_arn: state::virtual_cluster_arn("eu-west-1", &cluster),
            code: "ValidationException",
        },
        ArnCase {
            case: "the cluster's own id in another account",
            resource_arn: format!(
                "arn:aws:emr-containers:{REGION}:111122223333:/virtualclusters/{cluster}"
            ),
            code: "ValidationException",
        },
        ArnCase {
            case: "the cluster's own id in another partition",
            resource_arn: format!(
                "arn:aws-us-gov:emr-containers:{REGION}:{}:/virtualclusters/{cluster}",
                engine::emulator_account_id()
            ),
            code: "ValidationException",
        },
        ArnCase {
            case: "a sub-resource type this service does not model",
            resource_arn: format!(
                "arn:aws:emr-containers:{REGION}:{}:/virtualclusters/{cluster}/widgets/w1",
                engine::emulator_account_id()
            ),
            code: "ValidationException",
        },
        ArnCase {
            case: "a job run path with no job run id",
            resource_arn: format!(
                "arn:aws:emr-containers:{REGION}:{}:/virtualclusters/{cluster}/jobruns",
                engine::emulator_account_id()
            ),
            code: "ValidationException",
        },
        ArnCase {
            case: "a job run path carrying an extra segment",
            resource_arn: format!("{}/extra", state::job_run_arn(REGION, &cluster, &run)),
            code: "ValidationException",
        },
        ArnCase {
            case: "a cluster id segment carrying a character no resource ARN admits",
            resource_arn: format!(
                "arn:aws:emr-containers:{REGION}:{}:/virtualclusters/{cluster}_1",
                engine::emulator_account_id()
            ),
            code: "ValidationException",
        },
        ArnCase {
            case: "a cluster id segment that is empty",
            resource_arn: format!(
                "arn:aws:emr-containers:{REGION}:{}:/virtualclusters//jobruns/{run}",
                engine::emulator_account_id()
            ),
            code: "ValidationException",
        },
    ];

    for case in &cases {
        let label = case.case;
        let answer = list_tags(&emr, &case.resource_arn).await;
        answer.refused(label, 400, case.code);
        assert!(
            answer.body.pointer("/tags").is_none(),
            "case {label:?}: a refusal must carry no tag set, got {}",
            answer.body
        );
    }

    // The refusals left both resources as they were.
    let case = "after the refusals";
    assert_eq!(
        cluster_tags(&emr, &cluster, case).await,
        tag_object(&cluster_tagset),
        "case {case:?}: a refused listing must leave the cluster's tags as they were"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &run, case).await,
        tag_object(&run_tagset),
        "case {case:?}: a refused listing must leave the job run's tags as they were"
    );
}
