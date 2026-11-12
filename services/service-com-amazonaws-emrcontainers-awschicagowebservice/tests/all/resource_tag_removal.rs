// SPDX-License-Identifier: CC0-1.0

//! `UntagResource` (`DELETE /tags/{resourceArn}?tagKeys=…`), end to end.
//!
//! Every call goes through the production path: this service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop::Emr`]), so routing, the `@httpLabel` ARN, the
//! `@httpQuery` `tagKeys` list, constraint validation, the handler and serialization all
//! run exactly as they do for an AWS SDK client. Nothing reaches into `crate::state`: a
//! removal is only believed when a later read of the resource reports it back.
//!
//! The rule under test: `UntagResource` removes exactly the tag keys its `tagKeys` names
//! from the ONE tag set held by the record of the resource its `resourceArn` names,
//! leaving every other tag — and every other resource — as it was; a `resourceArn` that
//! does not resolve to a resource this endpoint holds is refused with the modelled
//! `ValidationException`.
//!
//! The tables span what that rule generalises over: how many keys are removed, whether a
//! key is held or absent, the key's form (ASCII, Unicode, case, the published length
//! bound), how many calls the removal was spread over, which resource type the ARN names
//! (a virtual cluster, a job run under one), and which part of the ARN fails to name a
//! stored resource.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::virtual_cluster_job_run_loop::{field, text, Answer, Emr, REGION};

/// `String128`, the `TagMap` key and `TagKeyList` member shape, is `len 1..128` with
/// pattern `\S` (the model, via `OPERATIONS.md`), so 128 characters is the longest key a
/// request may carry and the longest one a resource can hold.
pub(crate) const MAX_TAG_KEY_CHARS: usize = 128;

/// `TagKeyList` is `len 0..50`, so 50 keys is the most one `UntagResource` call removes.
const MAX_TAG_KEYS_PER_CALL: usize = 50;

/// A value that has to travel inside ONE path segment or one query entry, escaped the way
/// an SDK escapes an `@httpLabel` / `@httpQuery` value: every byte outside the unreserved
/// set (`A-Z a-z 0-9 - _ . ~`) becomes `%XX`, so an ARN's own `:` and `/` do not read as
/// path structure and a non-ASCII tag key survives the query string.
///
/// Written here rather than taken from a crate: it is one `match` over the UTF-8 bytes of
/// a test URI, and a dependency edge would be a `Cargo.lock` edit this operation's
/// behaviour has nothing to do with.
///
/// `pub(crate)` because the tagging suites in this one test binary share it rather than
/// keeping a copy each.
pub(crate) fn encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(char::from(byte))
            }
            reserved => format!("%{reserved:02X}"),
        })
        .collect()
}

/// A tag set as the JSON object the wire carries, for comparison against a response
/// member.
pub(crate) fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<Map<String, Value>>(),
    )
}

/// A tag set from borrowed literals, for the tables.
pub(crate) fn tags(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A key of exactly `chars` characters, so a published length bound can be tested AT it.
pub(crate) fn key_of(chars: usize) -> String {
    "k".repeat(chars)
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `tagKeys` is an `@httpQuery` list, so the
/// request carries one entry per key.
pub(crate) async fn untag(emr: &Emr, resource_arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded(key)))
        .collect::<Vec<_>>()
        .join("&");
    emr.send(
        "DELETE",
        &format!("/tags/{}?{query}", encoded(resource_arn)),
        None,
    )
    .await
}

/// Create a virtual cluster carrying `initial` and answer its `(id, arn)`.
pub(crate) async fn tagged_cluster(
    emr: &Emr,
    name: &str,
    initial: &BTreeMap<String, String>,
) -> (String, String) {
    let answer = emr
        .send(
            "POST",
            "/virtualclusters",
            Some(json!({
                "name": name,
                "clientToken": format!("tok-{name}"),
                "containerProvider": {"id": "eks-tagging", "type": "EKS"},
                "tags": tag_object(initial),
            })),
        )
        .await;
    let body = answer.ok(name).clone();
    let id = text(&body, "/id", name);
    let arn = text(&body, "/arn", name);
    // The ARN the create reported is the one this service's ARN builder makes for that
    // id, so the tables below can address a resource that does NOT exist too.
    assert_eq!(
        arn,
        state::virtual_cluster_arn(REGION, &id),
        "case {name:?}: CreateVirtualCluster answered an ARN the ARN builder does not make"
    );
    (id, arn)
}

/// Start a job run carrying `initial` and answer its `(id, arn)`.
pub(crate) async fn tagged_job_run(
    emr: &Emr,
    cluster: &str,
    token: &str,
    initial: &BTreeMap<String, String>,
) -> (String, String) {
    let answer = emr
        .send(
            "POST",
            &format!("/virtualclusters/{cluster}/jobruns"),
            Some(json!({
                "name": "tagged-run",
                "clientToken": token,
                "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                "releaseLabel": "emr-6.10.0-latest",
                "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/job.py"}},
                "tags": tag_object(initial),
            })),
        )
        .await;
    let body = answer.ok(token).clone();
    (text(&body, "/id", token), text(&body, "/arn", token))
}

/// The tag set `DescribeVirtualCluster` reports for this cluster.
pub(crate) async fn cluster_tags(emr: &Emr, id: &str, case: &str) -> Value {
    let answer = emr
        .send("GET", &format!("/virtualclusters/{id}"), None)
        .await;
    field(answer.ok(case), "/virtualCluster/tags", case).clone()
}

/// The tag set `DescribeJobRun` reports for this job run.
pub(crate) async fn job_run_tags(emr: &Emr, cluster: &str, id: &str, case: &str) -> Value {
    let answer = emr
        .send("GET", &format!("/virtualclusters/{cluster}/jobruns/{id}"), None)
        .await;
    field(answer.ok(case), "/jobRun/tags", case).clone()
}

/// One row of the removal table: the tag set the resource starts from, the successive
/// `UntagResource` calls made against it, and the set every later read must report.
struct RemovalCase {
    case: &'static str,
    /// Also the cluster name, so each row works on its own resource.
    name: &'static str,
    initial: BTreeMap<String, String>,
    calls: Vec<Vec<String>>,
    expected: BTreeMap<String, String>,
}

/// RULE: a removal drops exactly the keys it names from the resource's own tag set and
/// leaves every other tag in place, whatever the key's form and however many calls the
/// removal is spread over.
#[tokio::test]
async fn untagging_removes_only_the_named_keys() {
    let long_key = key_of(MAX_TAG_KEY_CHARS);
    let full_set: Vec<(String, String)> = (0..MAX_TAG_KEYS_PER_CALL)
        .map(|n| (format!("k{n}"), format!("v{n}")))
        .collect();

    let cases = vec![
        RemovalCase {
            case: "one key of several",
            name: "one-of-several",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            calls: vec![vec!["b".to_string()]],
            expected: tags(&[("a", "1"), ("c", "3")]),
        },
        RemovalCase {
            case: "several keys in one call",
            name: "several-at-once",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3")]),
            calls: vec![vec!["a".to_string(), "c".to_string()]],
            expected: tags(&[("b", "2")]),
        },
        RemovalCase {
            case: "a key the resource does not hold removes nothing",
            name: "absent-key",
            initial: tags(&[("a", "1")]),
            calls: vec![vec!["never-set".to_string()]],
            expected: tags(&[("a", "1")]),
        },
        RemovalCase {
            case: "the same removal repeated is idempotent",
            name: "repeated-call",
            initial: tags(&[("a", "1"), ("b", "2")]),
            calls: vec![vec!["a".to_string()], vec!["a".to_string()]],
            expected: tags(&[("b", "2")]),
        },
        RemovalCase {
            case: "successive calls each drop their own key",
            name: "spread-over-calls",
            initial: tags(&[("a", "1"), ("b", "2"), ("c", "3"), ("d", "4")]),
            calls: vec![
                vec!["a".to_string()],
                vec!["c".to_string(), "d".to_string()],
            ],
            expected: tags(&[("b", "2")]),
        },
        RemovalCase {
            case: "every key, leaving an empty set",
            name: "everything",
            initial: tags(&[("a", "1"), ("b", "2")]),
            calls: vec![vec!["a".to_string(), "b".to_string()]],
            expected: BTreeMap::new(),
        },
        RemovalCase {
            case: "a resource that never carried tags stays empty",
            name: "never-tagged",
            initial: BTreeMap::new(),
            calls: vec![vec!["a".to_string()]],
            expected: BTreeMap::new(),
        },
        RemovalCase {
            case: "a non-ASCII key",
            name: "unicode-key",
            initial: tags(&[("プロジェクト", "Ωmega"), ("a", "1")]),
            calls: vec![vec!["プロジェクト".to_string()]],
            expected: tags(&[("a", "1")]),
        },
        RemovalCase {
            case: "keys differing only in case are different keys",
            name: "case-sensitive",
            initial: tags(&[("Env", "prod"), ("env", "dev")]),
            calls: vec![vec!["env".to_string()]],
            expected: tags(&[("Env", "prod")]),
        },
        RemovalCase {
            case: "a key at String128's maximum length",
            name: "longest-key",
            initial: {
                let mut set = tags(&[("a", "1")]);
                set.insert(long_key.clone(), "v".to_string());
                set
            },
            calls: vec![vec![long_key.clone()]],
            expected: tags(&[("a", "1")]),
        },
        RemovalCase {
            case: "a key one character shorter than the longest is a different key",
            name: "near-longest-key",
            initial: {
                let mut set = BTreeMap::new();
                set.insert(long_key.clone(), "v".to_string());
                set.insert(key_of(MAX_TAG_KEY_CHARS - 1), "w".to_string());
                set
            },
            calls: vec![vec![key_of(MAX_TAG_KEY_CHARS - 1)]],
            expected: {
                let mut set = BTreeMap::new();
                set.insert(long_key.clone(), "v".to_string());
                set
            },
        },
        RemovalCase {
            case: "TagKeyList's maximum of 50 keys in one call",
            name: "fifty-keys",
            initial: full_set.iter().cloned().collect(),
            calls: vec![full_set.iter().map(|(key, _)| key.clone()).collect()],
            expected: BTreeMap::new(),
        },
        RemovalCase {
            case: "a single key removed from a resource holding the maximum tag count",
            name: "fifty-minus-one",
            initial: full_set.iter().cloned().collect(),
            calls: vec![vec!["k7".to_string()]],
            expected: full_set
                .iter()
                .filter(|(key, _)| key != "k7")
                .cloned()
                .collect(),
        },
    ];

    let emr = Emr::new();
    for case in &cases {
        let label = case.case;
        let (id, arn) = tagged_cluster(&emr, case.name, &case.initial).await;

        for keys in &case.calls {
            let borrowed: Vec<&str> = keys.iter().map(String::as_str).collect();
            let answer = untag(&emr, &arn, &borrowed).await;
            assert_eq!(
                answer.status, 200,
                "case {label:?}: expected UntagResource to answer HTTP 200, got {} with body {}",
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
            text(&described, "/virtualCluster/name", label),
            case.name,
            "case {label:?}: untagging changed the cluster name"
        );
        assert_eq!(
            text(&described, "/virtualCluster/state", label),
            state::CLUSTER_RUNNING,
            "case {label:?}: untagging changed the cluster state"
        );
    }
}

/// RULE, across resource TYPES: the ARN selects which record's tag set is edited, so
/// untagging a job run leaves its virtual cluster's tags alone and the other way round.
#[tokio::test]
async fn untagging_edits_only_the_resource_the_arn_names() {
    let emr = Emr::new();
    let cluster_tagset = tags(&[("owner", "platform"), ("env", "dev")]);
    let (cluster, cluster_arn) = tagged_cluster(&emr, "arn-selects", &cluster_tagset).await;
    let run_tagset = tags(&[("owner", "analytics"), ("cost-centre", "42")]);
    let (run, run_arn) = tagged_job_run(&emr, &cluster, "tok-arn-selects", &run_tagset).await;
    assert_eq!(
        run_arn,
        state::job_run_arn(REGION, &cluster, &run),
        "the job run ARN is the one this service's ARN builder makes"
    );

    // A second job run under the same cluster, so a removal has a sibling to leave alone.
    let sibling_tagset = tags(&[("owner", "analytics"), ("keep", "me")]);
    let (sibling, _) = tagged_job_run(&emr, &cluster, "tok-sibling", &sibling_tagset).await;

    let case = "untagging the job run";
    untag(&emr, &run_arn, &["owner"]).await.ok(case);
    assert_eq!(
        job_run_tags(&emr, &cluster, &run, case).await,
        tag_object(&tags(&[("cost-centre", "42")])),
        "case {case:?}: the job run must have lost exactly the key named"
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

    let case = "untagging the virtual cluster";
    untag(&emr, &cluster_arn, &["owner", "absent"]).await.ok(case);
    assert_eq!(
        cluster_tags(&emr, &cluster, case).await,
        tag_object(&tags(&[("env", "dev")])),
        "case {case:?}: the cluster must have lost exactly the key named"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &run, case).await,
        tag_object(&tags(&[("cost-centre", "42")])),
        "case {case:?}: the job run's tags must be untouched"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &sibling, case).await,
        tag_object(&sibling_tagset),
        "case {case:?}: the sibling job run's tags must be untouched"
    );
}

/// One row of the refusal table: an `RsiArn` the boundary accepts, and what
/// `UntagResource` must answer for it.
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
async fn an_untag_call_on_an_arn_that_names_no_resource_is_refused() {
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
        untag(&emr, &case.resource_arn, &["env", "keep"])
            .await
            .refused(label, 400, case.code);
    }

    // A refused call writes nothing: both resources still carry what they were given.
    assert_eq!(
        cluster_tags(&emr, &cluster, "after the refusals").await,
        tag_object(&bystander_tagset),
        "a refused untag must leave the cluster's tags as they were"
    );
    assert_eq!(
        job_run_tags(&emr, &cluster, &run, "after the refusals").await,
        tag_object(&run_tagset),
        "a refused untag must leave the job run's tags as they were"
    );
}
