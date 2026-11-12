// SPDX-License-Identifier: CC0-1.0

//! `TagResource` (`POST /tags/{resourceArn}`), end to end.
//!
//! Every call goes through the production path: this service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop::Emr`]), so routing, the `@httpLabel` ARN,
//! constraint validation, the handler and serialization all run exactly as they do for an
//! AWS SDK client. Nothing reaches into `crate::state`: a tag is only believed when a
//! later read of the resource reports it back.
//!
//! The rule under test: `TagResource` MERGES the pairs its `tags` names into the ONE tag
//! set held by the record of the resource its `resourceArn` names — a key the resource
//! does not carry is added, a key it already carries takes the new value, and every key
//! the request does not name is left alone — leaving every other resource untouched; a
//! `resourceArn` that does not resolve to a resource this endpoint holds is refused with
//! the modelled `ValidationException`, and so is a merge that would push the resource past
//! `TagMap`'s own maximum entry count, writing nothing.
//!
//! The tables span what that rule generalises over: how many pairs are written, whether a
//! key is new or already held, the key's and value's form (ASCII, Unicode, case, the
//! published length bounds), how many calls the tagging was spread over, how full the
//! resource's tag set already is (empty, partly full, at the maximum, one past it), which
//! resource type the ARN names (a virtual cluster, a job run under one), and which part of
//! the ARN fails to name a stored resource.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::resource_tag_removal::{
    cluster_tags, encoded, job_run_tags, key_of, tag_object, tagged_cluster, tagged_job_run, tags,
    MAX_TAG_KEY_CHARS,
};
use crate::virtual_cluster_job_run_loop::{Answer, Emr, REGION};

/// `StringEmpty256`, the `TagMap` value shape, is `len 0..256` with pattern `\S`
/// (smithy_query: shape `TagMap`), so 256 characters is the longest value a tag holds.
const MAX_TAG_VALUE_CHARS: usize = 256;

/// `TagMap` is `len 0..50` (smithy_query: shape `TagMap`), so 50 pairs is the most one
/// request may carry — and, because that same shape is what a read of a resource's tags
/// answers with, the most one resource can hold.
const MAX_TAG_MAP_ENTRIES: usize = 50;

/// `POST /tags/{resourceArn}` — `tags` is the body, `resourceArn` an `@httpLabel`.
async fn tag(emr: &Emr, resource_arn: &str, pairs: &BTreeMap<String, String>) -> Answer {
    emr.send(
        "POST",
        &format!("/tags/{}", encoded(resource_arn)),
        Some(json!({"tags": tag_object(pairs)})),
    )
    .await
}

/// `MAX_TAG_MAP_ENTRIES` pairs, `k0..` / `v0..`.
fn full_tag_set() -> BTreeMap<String, String> {
    (0..MAX_TAG_MAP_ENTRIES)
        .map(|n| (format!("k{n}"), format!("v{n}")))
        .collect()
}

/// One row of the merge table: the tag set the resource starts from, the successive
/// `TagResource` calls made against it, and the set every later read must report.
struct TagCase {
    case: &'static str,
    /// Also the cluster name, so each row works on its own resource.
    name: &'static str,
    initial: BTreeMap<String, String>,
    calls: Vec<BTreeMap<String, String>>,
    expected: BTreeMap<String, String>,
}

/// RULE: a tagging call writes exactly the pairs it names into the resource's own tag set
/// and leaves every other tag in place, whatever the key's or value's form and however
/// many calls the tagging is spread over.
#[tokio::test]
async fn tagging_writes_only_the_named_pairs() {
    let long_key = key_of(MAX_TAG_KEY_CHARS);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS);
    let full = full_tag_set();

    let cases = vec![
        TagCase {
            case: "one pair onto a resource carrying none",
            name: "first-tag",
            initial: BTreeMap::new(),
            calls: vec![tags(&[("owner", "platform")])],
            expected: tags(&[("owner", "platform")]),
        },
        TagCase {
            case: "several new keys merge with the keys already held",
            name: "merge-new-keys",
            initial: tags(&[("a", "1")]),
            calls: vec![tags(&[("b", "2"), ("c", "3")])],
            expected: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
        },
        TagCase {
            case: "a key already held takes the new value",
            name: "revalue-key",
            initial: tags(&[("env", "dev"), ("keep", "me")]),
            calls: vec![tags(&[("env", "prod")])],
            expected: tags(&[("env", "prod"), ("keep", "me")]),
        },
        TagCase {
            case: "an empty tag map leaves the set as it was",
            name: "empty-map",
            initial: tags(&[("a", "1")]),
            calls: vec![BTreeMap::new()],
            expected: tags(&[("a", "1")]),
        },
        TagCase {
            case: "the same call repeated is idempotent",
            name: "repeated-call",
            initial: tags(&[("a", "1")]),
            calls: vec![tags(&[("b", "2")]), tags(&[("b", "2")])],
            expected: tags(&[("a", "1"), ("b", "2")]),
        },
        TagCase {
            case: "successive calls each add their own pair",
            name: "spread-over-calls",
            initial: BTreeMap::new(),
            calls: vec![
                tags(&[("a", "1")]),
                tags(&[("b", "2"), ("c", "3")]),
                tags(&[("a", "9")]),
            ],
            expected: tags(&[("a", "9"), ("b", "2"), ("c", "3")]),
        },
        TagCase {
            case: "a non-ASCII key and value",
            name: "unicode-pair",
            initial: tags(&[("a", "1")]),
            calls: vec![tags(&[("プロジェクト", "Ωmega")])],
            expected: tags(&[("a", "1"), ("プロジェクト", "Ωmega")]),
        },
        TagCase {
            case: "keys differing only in case are different keys",
            name: "case-sensitive",
            initial: tags(&[("Env", "prod")]),
            calls: vec![tags(&[("env", "dev")])],
            expected: tags(&[("Env", "prod"), ("env", "dev")]),
        },
        TagCase {
            case: "a key at String128's maximum length",
            name: "longest-key",
            initial: tags(&[("a", "1")]),
            calls: vec![{
                let mut set = BTreeMap::new();
                set.insert(long_key.clone(), "v".to_string());
                set
            }],
            expected: {
                let mut set = tags(&[("a", "1")]);
                set.insert(long_key.clone(), "v".to_string());
                set
            },
        },
        TagCase {
            case: "a key one character shorter than the longest is a different key",
            name: "near-longest-key",
            initial: {
                let mut set = BTreeMap::new();
                set.insert(long_key.clone(), "held".to_string());
                set
            },
            calls: vec![{
                let mut set = BTreeMap::new();
                set.insert(key_of(MAX_TAG_KEY_CHARS - 1), "added".to_string());
                set
            }],
            expected: {
                let mut set = BTreeMap::new();
                set.insert(long_key.clone(), "held".to_string());
                set.insert(key_of(MAX_TAG_KEY_CHARS - 1), "added".to_string());
                set
            },
        },
        TagCase {
            case: "a value at StringEmpty256's maximum length",
            name: "longest-value",
            initial: BTreeMap::new(),
            calls: vec![{
                let mut set = BTreeMap::new();
                set.insert("wide".to_string(), long_value.clone());
                set
            }],
            expected: {
                let mut set = BTreeMap::new();
                set.insert("wide".to_string(), long_value.clone());
                set
            },
        },
        TagCase {
            case: "TagMap's maximum of 50 pairs in one call",
            name: "fifty-pairs",
            initial: BTreeMap::new(),
            calls: vec![full.clone()],
            expected: full.clone(),
        },
        TagCase {
            case: "a merge that reaches the maximum exactly",
            name: "up-to-the-maximum",
            initial: full
                .iter()
                .take(MAX_TAG_MAP_ENTRIES - 2)
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
            calls: vec![tags(&[("extra-one", "1"), ("extra-two", "2")])],
            expected: {
                let mut set: BTreeMap<String, String> = full
                    .iter()
                    .take(MAX_TAG_MAP_ENTRIES - 2)
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                set.insert("extra-one".to_string(), "1".to_string());
                set.insert("extra-two".to_string(), "2".to_string());
                set
            },
        },
        TagCase {
            case: "re-valuing every key of a resource already at the maximum",
            name: "revalue-at-the-maximum",
            initial: full.clone(),
            calls: vec![full
                .keys()
                .map(|key| (key.clone(), "rewritten".to_string()))
                .collect()],
            expected: full
                .keys()
                .map(|key| (key.clone(), "rewritten".to_string()))
                .collect(),
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.case;
        let (id, arn) = tagged_cluster(&emr, case.name, &case.initial).await;

        for pairs in &case.calls {
            let answer = tag(&emr, &arn, pairs).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: expected TagResource to answer HTTP 200, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code, None,
                "case {label:?}: expected no error code, got {:?}",
                answer.error_code
            );
        }

        let expected = tag_object(&case.expected);
        let reported = cluster_tags(&emr, &id, label).await;
        assert_eq!(
            reported, expected,
            "case {label:?}: expected the cluster to hold {expected}, got {reported}"
        );

        // Only the TAGS changed: the rest of the record is the one the create persisted.
        let answer = emr
            .send("GET", &format!("/virtualclusters/{id}"), None)
            .await;
        let described = answer.ok(label).clone();
        assert_eq!(
            crate::virtual_cluster_job_run_loop::text(&described, "/virtualCluster/name", label),
            case.name,
            "case {label:?}: tagging changed the cluster name"
        );
        assert_eq!(
            crate::virtual_cluster_job_run_loop::text(&described, "/virtualCluster/state", label),
            state::CLUSTER_RUNNING,
            "case {label:?}: tagging changed the cluster state"
        );
    }
}

/// RULE, across resource TYPES: the ARN selects which record's tag set is written, so
/// tagging a job run leaves its virtual cluster's tags alone and the other way round.
#[tokio::test]
async fn tagging_edits_only_the_resource_the_arn_names() {
    let emr = Emr::new();
    let cluster_tagset = tags(&[("owner", "platform")]);
    let (cluster, cluster_arn) = tagged_cluster(&emr, "arn-selects", &cluster_tagset).await;
    let run_tagset = tags(&[("owner", "analytics")]);
    let (run, run_arn) = tagged_job_run(&emr, &cluster, "tok-arn-selects", &run_tagset).await;
    assert_eq!(
        run_arn,
        state::job_run_arn(REGION, &cluster, &run),
        "the job run ARN is the one this service's ARN builder makes"
    );

    // A second job run under the same cluster, so a tagging has a sibling to leave alone.
    let sibling_tagset = tags(&[("owner", "analytics")]);
    let (sibling, _) = tagged_job_run(&emr, &cluster, "tok-sibling", &sibling_tagset).await;

    let case = "tagging the job run";
    tag(&emr, &run_arn, &tags(&[("owner", "ml"), ("stage", "beta")]))
        .await
        .ok(case);
    assert_eq!(
        job_run_tags(&emr, &cluster, &run, case).await,
        tag_object(&tags(&[("owner", "ml"), ("stage", "beta")])),
        "case {case:?}: the job run must carry exactly the merged set"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &sibling, case).await,
        tag_object(&sibling_tagset),
        "case {case:?}: the sibling job run's tags must be untouched"
    );
    assert_eq!(
        cluster_tags(&emr, &cluster, case).await,
        tag_object(&cluster_tagset),
        "case {case:?}: the parent cluster's tags must be untouched"
    );

    let case = "tagging the virtual cluster";
    tag(&emr, &cluster_arn, &tags(&[("stage", "ga")]))
        .await
        .ok(case);
    assert_eq!(
        cluster_tags(&emr, &cluster, case).await,
        tag_object(&tags(&[("owner", "platform"), ("stage", "ga")])),
        "case {case:?}: the cluster must carry exactly the merged set"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &run, case).await,
        tag_object(&tags(&[("owner", "ml"), ("stage", "beta")])),
        "case {case:?}: the job run's tags must be untouched"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &sibling, case).await,
        tag_object(&sibling_tagset),
        "case {case:?}: the sibling job run's tags must be untouched"
    );
}

/// RULE: a tag written through `TagResource` is the tag the REMOVAL path sees, because
/// both edit the one tag set the resource's record holds.
#[tokio::test]
async fn a_tag_written_here_is_the_tag_untag_removes() {
    let emr = Emr::new();
    let case = "tag then untag";
    let (id, arn) = tagged_cluster(&emr, "tag-then-untag", &tags(&[("a", "1")])).await;
    tag(&emr, &arn, &tags(&[("b", "2"), ("c", "3")])).await.ok(case);
    emr.send(
        "DELETE",
        &format!("/tags/{}?tagKeys=b&tagKeys=a", encoded(&arn)),
        None,
    )
    .await
    .ok(case);
    assert_eq!(
        cluster_tags(&emr, &id, case).await,
        tag_object(&tags(&[("c", "3")])),
        "case {case:?}: the removal must see the tags the tagging wrote"
    );
}

/// One row of the refusal table: an `RsiArn` the boundary accepts, and what `TagResource`
/// must answer for it.
struct ArnCase {
    case: &'static str,
    resource_arn: String,
    code: &'static str,
}

/// RULE: a `resourceArn` that does not resolve to a resource this endpoint holds — outside
/// the modelled `RsiArn` constraint, naming another partition, account or region, naming a
/// resource path none of the forms this service's resource ARNs declare, or naming a
/// resource that is not stored — is an invalid resource reference: HTTP 400 with the
/// modelled `ValidationException`, writing nothing
/// ([`crate::tagging_resource_arn_form`] pins its wording).
#[tokio::test]
async fn a_tag_call_on_an_arn_that_names_no_resource_is_refused() {
    let emr = Emr::new();
    let bystander_tagset = tags(&[("env", "dev")]);
    let (cluster, _) = tagged_cluster(&emr, "bystander", &bystander_tagset).await;
    let run_tagset = tags(&[("keep", "me")]);
    let (run, _) = tagged_job_run(&emr, &cluster, "tok-bystander", &run_tagset).await;

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
            case: "a managed endpoint ARN, a resource form nothing stores yet",
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
        tag(&emr, &case.resource_arn, &tags(&[("env", "prod")]))
            .await
            .refused(label, 400, case.code);
    }

    // A refused call writes nothing: both resources still carry what they were given.
    assert_eq!(
        cluster_tags(&emr, &cluster, "after the refusals").await,
        tag_object(&bystander_tagset),
        "a refused tagging must leave the cluster's tags as they were"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &run, "after the refusals").await,
        tag_object(&run_tagset),
        "a refused tagging must leave the job run's tags as they were"
    );
}

/// RULE, at the published bound: `TagMap` holds at most [`MAX_TAG_MAP_ENTRIES`] entries,
/// and that is the shape a read of a resource's tags answers with, so a merge that would
/// push a resource past it is refused and writes nothing — while one that only re-values
/// keys the resource already holds is accepted at the bound.
#[tokio::test]
async fn a_merge_past_the_maximum_tag_count_is_refused_and_writes_nothing() {
    assert_eq!(
        state::MAX_RESOURCE_TAGS, MAX_TAG_MAP_ENTRIES,
        "the cap the service enforces must be TagMap's own maximum, got {}",
        state::MAX_RESOURCE_TAGS
    );

    let emr = Emr::new();
    let full = full_tag_set();
    let case = "a resource already at the maximum";
    let (id, arn) = tagged_cluster(&emr, "at-the-maximum", &full).await;

    // One past the bound: a single new key on a full resource.
    let mut one_more = BTreeMap::new();
    one_more.insert("one-too-many".to_string(), "x".to_string());
    tag(&emr, &arn, &one_more)
        .await
        .refused(case, 400, "ValidationException");
    assert_eq!(
        cluster_tags(&emr, &id, case).await,
        tag_object(&full),
        "case {case:?}: a refused merge must leave every tag as it was"
    );

    // AT the bound: re-valuing a key it already holds does not grow the set.
    let case = "re-valuing a held key at the maximum";
    let mut rewrite = BTreeMap::new();
    rewrite.insert("k7".to_string(), "rewritten".to_string());
    tag(&emr, &arn, &rewrite).await.ok(case);
    let mut expected = full.clone();
    expected.insert("k7".to_string(), "rewritten".to_string());
    assert_eq!(
        cluster_tags(&emr, &id, case).await,
        tag_object(&expected),
        "case {case:?}: the held key must have taken the new value"
    );

    // A partly full resource: a merge whose new keys would overflow is refused whole, so
    // not even the pairs that would have fitted are written.
    let case = "a merge whose new keys overflow is refused whole";
    let nearly: BTreeMap<String, String> = full
        .iter()
        .take(MAX_TAG_MAP_ENTRIES - 1)
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let (partial_id, partial_arn) = tagged_cluster(&emr, "nearly-full", &nearly).await;
    let mut overflowing = BTreeMap::new();
    overflowing.insert("fits".to_string(), "1".to_string());
    overflowing.insert("overflows".to_string(), "2".to_string());
    tag(&emr, &partial_arn, &overflowing)
        .await
        .refused(case, 400, "ValidationException");
    assert_eq!(
        cluster_tags(&emr, &partial_id, case).await,
        tag_object(&nearly),
        "case {case:?}: a refused merge must write none of its pairs"
    );

    // And a single request carrying more than TagMap admits is refused too.
    let case = "one request carrying more pairs than TagMap admits";
    let too_many: BTreeMap<String, String> = (0..=MAX_TAG_MAP_ENTRIES)
        .map(|n| (format!("t{n}"), format!("v{n}")))
        .collect();
    let (empty_id, empty_arn) = tagged_cluster(&emr, "over-wide-request", &BTreeMap::new()).await;
    tag(&emr, &empty_arn, &too_many)
        .await
        .refused(case, 400, "ValidationException");
    assert_eq!(
        cluster_tags(&emr, &empty_id, case).await,
        Value::Object(serde_json::Map::new()),
        "case {case:?}: a refused request must write nothing"
    );
}
