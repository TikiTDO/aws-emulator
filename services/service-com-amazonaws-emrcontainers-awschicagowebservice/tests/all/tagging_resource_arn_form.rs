// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a tagging call's `resourceArn` earns, end to end.
//!
//! Every call goes through the production path: this service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop::Emr`]), so routing, the `@httpLabel` ARN,
//! constraint validation, the handler and serialization all run exactly as they do for an
//! AWS SDK client.
//!
//! The rule under test: the three `/tags/{resourceArn}` operations validate the resource
//! REFERENCE they are given, so a `resourceArn` that does not resolve to a resource this
//! endpoint holds — outside the `RsiArn` length or pattern the model declares, naming
//! another partition, account or region, naming a resource path none of this service's
//! resource ARNs take, or naming a resource that is not stored — is refused as invalid
//! input: HTTP 400, the modelled `ValidationException`, and ONE fixed sentence with nothing
//! of the request in it. It is never worded as the identifier-bearing refusal an operation
//! that names its resource by id answers with ([`crate::not_found_message`] holds that
//! class), and a reference that does resolve is served.
//!
//! The tables span what the rule generalises over: which of the three operations is called,
//! which part of the reference puts it outside the set (the whole envelope, the partition,
//! the service, the account, the region, the resource path, a path segment's own form, the
//! resource's own absence), which resource type the ARN names (a virtual cluster, a job run
//! under one, a managed endpoint under one), whether the value is refused at the boundary or
//! by the handler, and the LENGTH of the value — both published bounds of `RsiArn`, one
//! character past each of them, and values inside them.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::resource_tag_removal::{
    cluster_tags, encoded, tag_object, tagged_cluster, tagged_job_run, tags,
};
use crate::virtual_cluster_job_run_loop::{text, Answer, Emr, REGION};

/// The class an invalid request is refused in: the `ValidationException` all three tagging
/// operations model (smithy_query: operations `TagResource`, `UntagResource`,
/// `ListTagsForResource`).
const INVALID_REQUEST: &str = "ValidationException";

/// AWS's sentence for a `resourceArn` that is not the ARN of an EMR on EKS resource,
/// written here as the service must emit it: FIXED text, so the expectation is the sentence
/// itself and not a format string.
const INVALID_RESOURCE_ARN: &str = "Invalid input resource arn";

/// `RsiArn` is `len 60..500` (smithy_query: operation `TagResource`, member `resourceArn`),
/// so these are the published bounds a value is held to before any lookup.
const ARN_MIN_CHARS: usize = 60;
const ARN_MAX_CHARS: usize = 500;

/// The shortest envelope `RsiArn`'s pattern admits:
/// `arn:aws:emr-containers:{region}:{12 digits}:/virtualclusters/` with a one-character
/// region is 55 characters before the resource path, so the pattern's own shortest match is
/// 56 characters — inside the pattern and outside the length bound, which is what lets a row
/// sit under the published minimum while still being ARN-shaped.
const SHORTEST_ENVELOPE: &str = "arn:aws:emr-containers:a:123456789012:/virtualclusters/";

/// A twelve-digit account id that is not this endpoint's, the form `RsiArn`'s pattern
/// admits.
const OTHER_ACCOUNT: &str = "111122223333";

/// An ARN-shaped value of exactly `characters` characters: the pattern's shortest envelope
/// with a cluster-id segment sized to the bound under test.
fn shaped_arn_of_length(characters: usize) -> String {
    format!(
        "{SHORTEST_ENVELOPE}{}",
        "a".repeat(characters - SHORTEST_ENVELOPE.len())
    )
}

/// The ARN this endpoint mints for a virtual cluster — the only form a tagging call's
/// `resourceArn` resolves in — with an empty id, i.e. everything before the resource's own
/// segment.
fn minted_prefix() -> String {
    state::virtual_cluster_arn(REGION, "")
}

/// An ARN this endpoint really mints, of exactly `characters` characters, for a cluster id
/// nothing holds: a row AT a published bound rather than near it.
fn minted_arn_of_length(characters: usize) -> String {
    state::virtual_cluster_arn(REGION, &"a".repeat(characters - minted_prefix().len()))
}

/// One row of the table: a `resourceArn`, and nothing else — the same value is driven
/// through all three operations.
struct ArnCase {
    case: String,
    resource_arn: String,
}

fn row(case: &str, resource_arn: String) -> ArnCase {
    ArnCase {
        case: case.to_string(),
        resource_arn,
    }
}

/// What each of the three `/tags/{resourceArn}` operations answered for one value.
async fn answers(emr: &Emr, resource_arn: &str) -> Vec<(&'static str, Answer)> {
    let path = format!("/tags/{}", encoded(resource_arn));
    vec![
        ("ListTagsForResource", emr.send("GET", &path, None).await),
        (
            "TagResource",
            emr.send("POST", &path, Some(json!({"tags": {"env": "prod"}})))
                .await,
        ),
        (
            "UntagResource",
            emr.send("DELETE", &format!("{path}?tagKeys=env"), None).await,
        ),
    ]
}

/// Hold one value to the rule at all three operations: HTTP 400, the modelled
/// `ValidationException`, the one fixed sentence, and no part of the request inside it.
async fn assert_invalid_arn(emr: &Emr, row: &ArnCase) {
    for (operation, answer) in answers(emr, &row.resource_arn).await {
        let case = format!("{operation}, {}", row.case);
        answer.refused(&case, 400, INVALID_REQUEST);
        let message = text(&answer.body, "/message", &case);
        assert_eq!(
            message, INVALID_RESOURCE_ARN,
            "case {case:?}: expected message {INVALID_RESOURCE_ARN:?}, got {message:?}"
        );
        // The other half of "fixed text": the sentence has no variable part, so the value
        // the request sent may not appear in it.
        assert!(
            !message.contains(&row.resource_arn),
            "case {case:?}: expected a sentence naming nothing of the request, got \
             {message:?} for {:?}",
            row.resource_arn
        );
        // And it is not the wording a request that names its resource by id is refused
        // with, which is what lets a caller tell the two classes apart.
        assert!(
            !message.contains("doesn't exist"),
            "case {case:?}: expected the invalid-reference sentence rather than the \
             not-found wording, got {message:?}"
        );
    }
}

/// RULE: a `resourceArn` that does not resolve to a resource this endpoint holds is refused
/// with one fixed sentence, whichever part of the reference puts it outside that set and
/// whether the modelled constraint or the handler is what refuses it.
#[tokio::test]
async fn a_resource_arn_that_resolves_to_nothing_is_one_fixed_refusal() {
    let emr = Emr::new();
    // A cluster and a job run that really exist, so the rows below are refused for the
    // reference they sent and nothing else — and so the control at the end has something to
    // read.
    let held = tags(&[("env", "dev")]);
    let (cluster, cluster_arn) = tagged_cluster(&emr, "arn-form", &held).await;
    let run_tagset = tags(&[("keep", "me")]);
    let (run, _) = tagged_job_run(&emr, &cluster, "tok-arn-form", &run_tagset).await;

    // `ResourceIdString` ids are `^[0-9a-z]+$` (smithy_query: shape `ResourceIdString`), so
    // this names nothing while still being an id this service could have minted.
    let unknown = "doesnotexist9";

    let rows = vec![
        // Outside the modelled constraint: refused before any handler runs.
        row(
            "a bare virtual cluster id, not an ARN at all",
            cluster.clone(),
        ),
        row(
            "a value long enough for the modelled minimum that is not an ARN",
            "x".repeat(ARN_MIN_CHARS + 20),
        ),
        row(
            "the pattern's shortest match, far under the modelled minimum",
            shaped_arn_of_length(SHORTEST_ENVELOPE.len() + 1),
        ),
        row(
            "one character under the modelled minimum",
            shaped_arn_of_length(ARN_MIN_CHARS - 1),
        ),
        row(
            "one character past the modelled maximum",
            minted_arn_of_length(ARN_MAX_CHARS + 1),
        ),
        row(
            "the ARN envelope with no resource path after /virtualclusters/",
            minted_prefix(),
        ),
        row(
            "an ARN of another AWS service",
            format!(
                "arn:aws:emr:{REGION}:{}:/virtualclusters/{cluster}",
                engine::emulator_account_id()
            ),
        ),
        row(
            "an account id that is not twelve digits",
            format!("arn:aws:emr-containers:{REGION}:12345:/virtualclusters/{cluster}"),
        ),
        row(
            "this service's own job template ARN, not a virtual cluster path",
            state::job_template_arn(REGION, &cluster),
        ),
        // Inside the modelled pattern, and still not a reference this endpoint can resolve:
        // the resource it names belongs to another endpoint.
        row(
            "this cluster's own id in another account",
            format!("arn:aws:emr-containers:{REGION}:{OTHER_ACCOUNT}:/virtualclusters/{cluster}"),
        ),
        row(
            "this cluster's own id in another region",
            state::virtual_cluster_arn("eu-west-1", &cluster),
        ),
        row(
            "this cluster's own id in another partition",
            format!(
                "arn:aws-us-gov:emr-containers:{REGION}:{}:/virtualclusters/{cluster}",
                engine::emulator_account_id()
            ),
        ),
        // This endpoint's own envelope, naming a resource form this service does not model.
        row(
            "a sub-resource type this service does not model",
            format!("{cluster_arn}/widgets/w1"),
        ),
        row(
            "a job run path with no job run id",
            format!("{cluster_arn}/jobruns"),
        ),
        row(
            "a job run path carrying an extra segment",
            format!("{}/extra", state::job_run_arn(REGION, &cluster, &run)),
        ),
        row(
            "a cluster id segment carrying a character no resource ARN admits",
            format!("{cluster_arn}_1"),
        ),
        row(
            "an empty cluster id segment",
            format!("{}/jobruns/{run}", minted_prefix()),
        ),
        // This endpoint's own ARNs, for resources it does not hold: the reference resolves
        // to nothing, which is the same refusal.
        row(
            "a virtual cluster ARN this endpoint mints for a cluster nothing holds",
            state::virtual_cluster_arn(REGION, unknown),
        ),
        row(
            "the shortest virtual cluster ARN this endpoint mints, inside the minimum",
            state::virtual_cluster_arn(REGION, "a"),
        ),
        row(
            "exactly the characters the modelled maximum admits",
            minted_arn_of_length(ARN_MAX_CHARS),
        ),
        row(
            "a job run ARN naming no run under a cluster that is stored",
            state::job_run_arn(REGION, &cluster, unknown),
        ),
        row(
            "a stored job run's id under a cluster nothing holds",
            state::job_run_arn(REGION, unknown, &run),
        ),
        row(
            "a managed endpoint ARN naming no endpoint under a stored cluster",
            state::managed_endpoint_arn(REGION, &cluster, unknown),
        ),
    ];

    // Every row either sits outside the published bounds or inside them, and both halves
    // are represented, so the table spans the constraint as well as the lookup.
    let outside = rows
        .iter()
        .filter(|row| !(ARN_MIN_CHARS..=ARN_MAX_CHARS).contains(&row.resource_arn.chars().count()))
        .count();
    assert!(
        outside > 0 && outside < rows.len(),
        "the table must carry rows on both sides of {ARN_MIN_CHARS}..{ARN_MAX_CHARS}, got \
         {outside} of {} outside",
        rows.len()
    );

    for current in &rows {
        assert_invalid_arn(&emr, current).await;
    }

    // The control: a refused call is wording and nothing else, so the resource that DOES
    // exist still carries exactly the tags it was created with.
    assert_eq!(
        cluster_tags(&emr, &cluster, "after the refusals").await,
        tag_object(&held),
        "a refused tagging call must leave the cluster's tags as they were"
    );
}

/// The rule's BOUNDARY: a reference that DOES resolve — an ARN this endpoint minted for a
/// resource it holds — is served rather than refused, at all three operations, so the check
/// above refuses references and not requests.
#[tokio::test]
async fn a_reference_this_endpoint_resolves_is_served_rather_than_refused() {
    let emr = Emr::new();
    let case = "a stored cluster's own ARN";
    let (cluster, arn) = tagged_cluster(&emr, "arn-form-served", &BTreeMap::new()).await;
    let path = format!("/tags/{}", encoded(&arn));

    emr.send("POST", &path, Some(json!({"tags": {"owner": "platform"}})))
        .await
        .ok(case);
    let listed = emr.send("GET", &path, None).await;
    assert_eq!(
        listed.ok(case).pointer("/tags"),
        Some(&tag_object(&tags(&[("owner", "platform")]))),
        "case {case:?}: the read must report the tag the call wrote, got {}",
        listed.body
    );

    emr.send("DELETE", &format!("{path}?tagKeys=owner"), None)
        .await
        .ok(case);
    assert_eq!(
        cluster_tags(&emr, &cluster, case).await,
        Value::Object(serde_json::Map::new()),
        "case {case:?}: the removal must have dropped the tag it named"
    );

    // And the same for a resource UNDER the cluster, so the served class is not one
    // resource type's alone.
    let case = "a stored job run's own ARN";
    let (run, run_arn) = tagged_job_run(&emr, &cluster, "tok-arn-served", &BTreeMap::new()).await;
    let run_path = format!("/tags/{}", encoded(&run_arn));
    emr.send("POST", &run_path, Some(json!({"tags": {"stage": "beta"}})))
        .await
        .ok(case);
    let listed = emr.send("GET", &run_path, None).await;
    assert_eq!(
        listed.ok(case).pointer("/tags"),
        Some(&tag_object(&tags(&[("stage", "beta")]))),
        "case {case:?}: the read must report the tag the call wrote, got {}",
        listed.body
    );
    assert_eq!(
        run_arn,
        state::job_run_arn(REGION, &cluster, &run),
        "case {case:?}: the job run ARN must be the one this service's ARN builder makes"
    );
}
