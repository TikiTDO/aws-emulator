// SPDX-License-Identifier: CC0-1.0

//! How this service WORDS a refusal for a resource that is not there, end to end.
//!
//! Every call goes through the production path: the service is mounted in the real
//! `MultiServiceRouter` by the harness this crate's core suite owns
//! ([`crate::virtual_cluster_job_run_loop::Emr`]) and driven with restJson1 HTTP requests,
//! so routing, deserialization, constraint validation, the handler and serialization all
//! run exactly as they do for an AWS SDK client.
//!
//! The rule under test: EMR on EKS words a not-found refusal of THIS class the same way,
//! from the identifier the request named the resource by — `<Resource type> <identifier>
//! doesn't exist.` — whichever operation issues it and whichever of its modelled client
//! errors carries it. The two operations whose `ValidationException` reports a missing
//! resource as a short fixed sentence instead are the other class, pinned by
//! [`crate::refusal_wording_by_error_class`].
//!
//! The tables span what that rule generalises over: which resource is missing (a virtual
//! cluster, a job run, a managed endpoint, a job template, a security configuration),
//! which operation reports it (a read, a write, a delete, a create that resolves a
//! reference), how the request named it (a bare id, the resource's own ARN), and the form
//! of the identifier (both published length bounds of `ResourceIdString`, an all-digit
//! value, a plausible minted one, and the id of a resource that was deleted).
//!
//! It also pins the rule's BOUNDARY: a resource that exists but cannot take the request is
//! not reported as one that does not exist, and the three tagging operations — which
//! validate the resource REFERENCE their `resourceArn` carries — are the other class
//! altogether ([`crate::tagging_resource_arn_form`]). An operation that resolves NO
//! resource is outside the rule too — the two listings scoped to a virtual cluster by a
//! path label answer an empty page for an id that names none, which
//! [`crate::cluster_scoped_listing_scope`] holds.

use serde_json::{json, Value};
use service_com_amazonaws_emrcontainers_awschicagowebservice::state;

use crate::managed_endpoint_listing::{endpoint_body, wait_for_the_clock};
use crate::resource_tag_removal::encoded;
use crate::virtual_cluster_job_run_loop::{
    runnable_body, spark_submit_driver, text, Answer, Emr, REGION,
};

/// How this service spells each resource type in a refusal. `Job Template` and `Security
/// Configuration` are the spellings the recorded refusals of this service show; the others
/// name the resource the way the model does (shapes `VirtualCluster`, `JobRun` and
/// `Endpoint`).
const VIRTUAL_CLUSTER: &str = "Virtual cluster";
const JOB_RUN: &str = "Job run";
const ENDPOINT: &str = "Endpoint";
const JOB_TEMPLATE: &str = "Job Template";
const SECURITY_CONFIGURATION: &str = "Security Configuration";

/// The client errors this service's operations carry a not-found refusal in: the modelled
/// `ResourceNotFoundException` where the operation declares one, and its
/// `ValidationException` where it does not (smithy_query: digest).
const NOT_FOUND: &str = "ResourceNotFoundException";
const INVALID_REQUEST: &str = "ValidationException";

/// The form under test, written once: the resource type, the identifier the request named
/// the resource by, and nothing else.
fn expected_message(resource_type: &str, identifier: &str) -> String {
    format!("{resource_type} {identifier} doesn't exist.")
}

/// One call that must be refused, and the resource its refusal must report missing.
struct Probe {
    label: String,
    resource_type: &'static str,
    identifier: String,
    method: &'static str,
    path: String,
    body: Option<Value>,
    error_code: &'static str,
}

fn probe(
    label: String,
    resource_type: &'static str,
    identifier: &str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    error_code: &'static str,
) -> Probe {
    Probe {
        label,
        resource_type,
        identifier: identifier.to_string(),
        method,
        path,
        body,
        error_code,
    }
}

/// Send one probe and hold its answer to the rule: HTTP 400, the error the operation
/// models, and the one not-found wording built from the identifier the request carried.
async fn assert_not_found(emr: &Emr, probe: &Probe) {
    let answer: Answer = emr
        .send(probe.method, &probe.path, probe.body.clone())
        .await;
    answer.refused(&probe.label, 400, probe.error_code);
    let reported = text(&answer.body, "/message", &probe.label);
    let expected = expected_message(probe.resource_type, &probe.identifier);
    assert_eq!(
        reported, expected,
        "case {:?}: expected message {expected:?}, got {reported:?}",
        probe.label
    );
}

/// Identifiers no resource of this service holds, spanning the forms a `ResourceIdString`
/// takes: both published length bounds (len 1..64, `^[0-9a-z]+$` — smithy_query: shape
/// `ResourceIdString`), an all-digit value and a plausible minted one.
fn absent_ids() -> Vec<(&'static str, String)> {
    vec![
        ("the shortest id the model allows", "a".to_string()),
        ("the longest id the model allows", "z".repeat(64)),
        ("an all-digit id", "0123456789".to_string()),
        ("a plausible minted id", "doesnotexist1".to_string()),
    ]
}

/// One acceptable `GetManagedEndpointSessionCredentials` body: the two members the model
/// marks `@required` beside the two `@httpLabel` identifiers (smithy_query: operation
/// `GetManagedEndpointSessionCredentials`).
fn session_body() -> Value {
    json!({
        "executionRoleArn": "arn:aws:iam::000000000000:role/EMRStudioRole",
        "credentialType": state::SESSION_CREDENTIAL_TYPE_TOKEN,
    })
}

/// Every operation of this service that resolves a resource by id, aimed at an `absent`
/// id: the ones that resolve a virtual cluster by the `@httpLabel` of the path, the ones
/// that resolve a job run or a managed endpoint UNDER the `host` cluster that really
/// exists, and the ones that resolve a job template or a security configuration a request
/// references.
///
/// A listing whose path label SCOPES its collection resolves no resource, so it is not one
/// of these: see the module docs.
fn probes_for(absent: &str, label: &str, host: &str, index: usize) -> Vec<Probe> {
    vec![
        probe(
            format!("DescribeVirtualCluster, {label}"),
            VIRTUAL_CLUSTER,
            absent,
            "GET",
            format!("/virtualclusters/{absent}"),
            None,
            NOT_FOUND,
        ),
        probe(
            format!("UpdateVirtualCluster, {label}"),
            VIRTUAL_CLUSTER,
            absent,
            "PATCH",
            format!("/virtualclusters/{absent}"),
            Some(json!({"clientToken": format!("tok-update-{index}")})),
            NOT_FOUND,
        ),
        probe(
            format!("DeleteVirtualCluster, {label}"),
            VIRTUAL_CLUSTER,
            absent,
            "DELETE",
            format!("/virtualclusters/{absent}"),
            None,
            INVALID_REQUEST,
        ),
        probe(
            format!("StartJobRun into an absent cluster, {label}"),
            VIRTUAL_CLUSTER,
            absent,
            "POST",
            format!("/virtualclusters/{absent}/jobruns"),
            Some(runnable_body(
                &format!("tok-start-absent-{index}"),
                spark_submit_driver(),
            )),
            NOT_FOUND,
        ),
        probe(
            format!("CreateManagedEndpoint in an absent cluster, {label}"),
            VIRTUAL_CLUSTER,
            absent,
            "POST",
            format!("/virtualclusters/{absent}/endpoints"),
            Some(endpoint_body(
                "nf-endpoint",
                &format!("tok-endpoint-absent-{index}"),
                "JUPYTER_ENTERPRISE_GATEWAY",
            )),
            NOT_FOUND,
        ),
        probe(
            format!("GetManagedEndpointSessionCredentials on an absent cluster, {label}"),
            VIRTUAL_CLUSTER,
            absent,
            "POST",
            format!("/virtualclusters/{absent}/endpoints/someendpoint/credentials"),
            Some(session_body()),
            NOT_FOUND,
        ),
        probe(
            format!("DescribeJobRun, {label}"),
            JOB_RUN,
            absent,
            "GET",
            format!("/virtualclusters/{host}/jobruns/{absent}"),
            None,
            NOT_FOUND,
        ),
        probe(
            format!("CancelJobRun, {label}"),
            JOB_RUN,
            absent,
            "DELETE",
            format!("/virtualclusters/{host}/jobruns/{absent}"),
            None,
            INVALID_REQUEST,
        ),
        probe(
            format!("DescribeManagedEndpoint, {label}"),
            ENDPOINT,
            absent,
            "GET",
            format!("/virtualclusters/{host}/endpoints/{absent}"),
            None,
            NOT_FOUND,
        ),
        probe(
            format!("DeleteManagedEndpoint, {label}"),
            ENDPOINT,
            absent,
            "DELETE",
            format!("/virtualclusters/{host}/endpoints/{absent}"),
            None,
            INVALID_REQUEST,
        ),
        probe(
            format!("GetManagedEndpointSessionCredentials on an absent endpoint, {label}"),
            ENDPOINT,
            absent,
            "POST",
            format!("/virtualclusters/{host}/endpoints/{absent}/credentials"),
            Some(session_body()),
            NOT_FOUND,
        ),
        probe(
            format!("DescribeJobTemplate, {label}"),
            JOB_TEMPLATE,
            absent,
            "GET",
            format!("/jobtemplates/{absent}"),
            None,
            NOT_FOUND,
        ),
        probe(
            format!("StartJobRun naming an absent job template, {label}"),
            JOB_TEMPLATE,
            absent,
            "POST",
            format!("/virtualclusters/{host}/jobruns"),
            Some(json!({
                "clientToken": format!("tok-start-template-{index}"),
                "jobTemplateId": absent,
            })),
            NOT_FOUND,
        ),
        probe(
            format!("DescribeSecurityConfiguration, {label}"),
            SECURITY_CONFIGURATION,
            absent,
            "GET",
            format!("/securityconfigurations/{absent}"),
            None,
            NOT_FOUND,
        ),
        probe(
            format!("CreateVirtualCluster naming an absent security configuration, {label}"),
            SECURITY_CONFIGURATION,
            absent,
            "POST",
            "/virtualclusters".to_string(),
            Some(json!({
                "name": format!("nf-ref-{index}"),
                "clientToken": format!("tok-nf-ref-{index}"),
                "containerProvider": {"id": format!("eks-nf-{index}"), "type": "EKS"},
                "securityConfigurationId": absent,
            })),
            NOT_FOUND,
        ),
    ]
}

/// THE RULE, over every operation that resolves a resource by id and every form an absent
/// id takes: the refusal reports the resource type and the requested identifier in the one
/// not-found wording of this service.
#[tokio::test]
async fn an_absent_resource_is_refused_with_the_services_one_not_found_message() {
    let emr = Emr::new();
    // A cluster that really exists, so a probe aimed at a job run or a managed endpoint
    // under it is refused for the run or the endpoint rather than for its cluster.
    let host = emr.cluster("nf-host", "tok-nf-host").await;

    for (index, (label, absent)) in absent_ids().iter().enumerate() {
        for current in probes_for(absent, label, &host, index) {
            assert_not_found(&emr, &current).await;
        }
    }

    // None of the refusals touched the store: the host cluster still reads back, and its
    // own listing answers for it.
    let described = emr.describe_cluster(&host).await;
    assert_eq!(
        text(described.ok("the host cluster"), "/virtualCluster/id", "the host cluster"),
        host,
        "the refusals must have left the host cluster readable"
    );
    let runs = emr
        .send("GET", &format!("/virtualclusters/{host}/jobruns"), None)
        .await;
    assert_eq!(
        runs.ok("the host cluster's job runs")
            .pointer("/jobRuns")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "a refused StartJobRun must have started nothing, got {}",
        runs.body
    );
}

/// THE SAME RULE for an identifier that DID resolve once: a resource that has been deleted
/// is absent, and is reported exactly as one that never existed.
#[tokio::test]
async fn a_deleted_resource_is_reported_as_one_that_does_not_exist() {
    let emr = Emr::new();

    let template = text(
        emr.send(
            "POST",
            "/jobtemplates",
            Some(json!({
                "name": "nf-template",
                "clientToken": "tok-nf-template",
                "jobTemplateData": {
                    "executionRoleArn": "arn:aws:iam::000000000000:role/EMRJobRole",
                    "releaseLabel": "emr-6.15.0-latest",
                    "jobDriver": {"sparkSubmitJobDriver": {"entryPoint": "s3://bucket/a.py"}},
                },
            })),
        )
        .await
        .ok("a job template to delete"),
        "/id",
        "a job template to delete",
    );
    emr.send("DELETE", &format!("/jobtemplates/{template}"), None)
        .await
        .ok("deleting the job template");

    let configuration = text(
        emr.send(
            "POST",
            "/securityconfigurations",
            Some(json!({
                "name": "nf-configuration",
                "clientToken": "tok-nf-configuration",
                "securityConfigurationData": {},
            })),
        )
        .await
        .ok("a security configuration to delete"),
        "/id",
        "a security configuration to delete",
    );
    emr.send(
        "DELETE",
        &format!("/securityconfigurations/{configuration}"),
        None,
    )
    .await
    .ok("deleting the security configuration");

    let probes = vec![
        probe(
            "DescribeJobTemplate of a deleted template".to_string(),
            JOB_TEMPLATE,
            &template,
            "GET",
            format!("/jobtemplates/{template}"),
            None,
            NOT_FOUND,
        ),
        probe(
            "DescribeSecurityConfiguration of a deleted configuration".to_string(),
            SECURITY_CONFIGURATION,
            &configuration,
            "GET",
            format!("/securityconfigurations/{configuration}"),
            None,
            NOT_FOUND,
        ),
    ];
    for current in &probes {
        assert_not_found(&emr, current).await;
    }
}

/// The rule's OTHER boundary, where the request names the resource by its own ARN rather
/// than by an id: the three `/tags/{resourceArn}` operations validate the REFERENCE, so one
/// that resolves to no resource of this endpoint is refused as an invalid reference — one
/// fixed sentence naming nothing of the request ([`crate::tagging_resource_arn_form`] pins
/// that class) — and never with the identifier-bearing wording this file holds.
///
/// The rows span the three resource ARNs an `RsiArn` admits — a virtual cluster, a job run
/// under one, a managed endpoint under one — across all three tagging operations.
#[tokio::test]
async fn an_arn_naming_no_stored_resource_is_not_refused_in_this_wording() {
    let emr = Emr::new();
    let host = emr.cluster("nf-arn-host", "tok-nf-arn-host").await;
    let absent = "doesnotexist2";

    // The ARNs this service's own ARN builders make for ids nothing holds, so each row
    // addresses a resource that really is not there.
    let cluster_arn = state::virtual_cluster_arn(REGION, absent);
    let job_run_arn = state::job_run_arn(REGION, &host, absent);
    let endpoint_arn = state::managed_endpoint_arn(REGION, &host, absent);
    let targets = [
        ("a virtual cluster ARN", VIRTUAL_CLUSTER, cluster_arn),
        ("a job run ARN", JOB_RUN, job_run_arn),
        ("a managed endpoint ARN", ENDPOINT, endpoint_arn),
    ];

    for (label, resource_type, arn) in &targets {
        let path = format!("/tags/{}", encoded(arn));
        let calls = [
            (
                format!("ListTagsForResource for {label}"),
                "GET",
                path.clone(),
                None,
            ),
            (
                format!("TagResource for {label}"),
                "POST",
                path.clone(),
                Some(json!({"tags": {"team": "data"}})),
            ),
            (
                format!("UntagResource for {label}"),
                "DELETE",
                format!("{path}?tagKeys=team"),
                None,
            ),
        ];
        for (case, method, path, body) in calls {
            let answer: Answer = emr.send(method, &path, body).await;
            answer.refused(&case, 400, INVALID_REQUEST);
            let reported = text(&answer.body, "/message", &case);
            for identifier in [absent, arn.as_str()] {
                assert_ne!(
                    reported,
                    expected_message(resource_type, identifier),
                    "case {case:?}: a tagging call must not report an unresolvable \
                     reference in the identifier-bearing wording"
                );
                assert!(
                    !reported.contains(identifier),
                    "case {case:?}: expected a sentence naming nothing of the request, got \
                     {reported:?}"
                );
            }
        }
    }

    // The control: the very same calls against the ARN of a resource that IS stored are
    // answered, so the rows above were refused for the resource being absent and nothing
    // else.
    let host_arn = state::virtual_cluster_arn(REGION, &host);
    let host_path = format!("/tags/{}", encoded(&host_arn));
    emr.send("POST", &host_path, Some(json!({"tags": {"team": "data"}})))
        .await
        .ok("tagging the stored cluster");
    let listed = emr.send("GET", &host_path, None).await;
    assert_eq!(
        text(
            listed.ok("the stored cluster's tags"),
            "/tags/team",
            "the stored cluster's tags"
        ),
        "data",
        "the stored cluster must report the tag the call wrote"
    );
}

/// THE RULE's identifier, where an operation accepts either an id or the resource's own
/// ARN: the refusal reports the identifier AS THE REQUEST SPELLED IT, so a caller sees the
/// value it sent.
#[tokio::test]
async fn an_identifier_is_reported_as_the_request_spelled_it() {
    let emr = Emr::new();
    let host = emr.cluster("nf-ident-host", "tok-nf-ident-host").await;
    let absent = "doesnotexist3";
    let cluster_arn = state::virtual_cluster_arn(REGION, absent);
    let endpoint_arn = state::managed_endpoint_arn(REGION, &host, absent);

    let probes = vec![
        probe(
            "a session for a cluster named by an absent cluster's ARN".to_string(),
            VIRTUAL_CLUSTER,
            &cluster_arn,
            "POST",
            format!(
                "/virtualclusters/{}/endpoints/someendpoint/credentials",
                encoded(&cluster_arn)
            ),
            Some(session_body()),
            NOT_FOUND,
        ),
        probe(
            "a session for an endpoint named by an absent endpoint's ARN".to_string(),
            ENDPOINT,
            &endpoint_arn,
            "POST",
            format!(
                "/virtualclusters/{host}/endpoints/{}/credentials",
                encoded(&endpoint_arn)
            ),
            Some(session_body()),
            NOT_FOUND,
        ),
    ];
    for current in &probes {
        assert_not_found(&emr, current).await;
    }
}

/// The rule's BOUNDARY: a resource that EXISTS but cannot take the request is not reported
/// as one that does not exist. A cluster whose teardown has started, and one the clock has
/// finished tearing down, are both still describable — so the refusal of work for them says
/// what it is, not that they are absent.
#[tokio::test]
async fn a_cluster_that_exists_is_never_reported_as_absent() {
    let emr = Emr::new();
    let cluster = emr.cluster("nf-boundary", "tok-nf-boundary").await;
    emr.delete_cluster(&cluster)
        .await
        .ok("starting the teardown");

    for phase in ["TERMINATING", "TERMINATED"] {
        if phase == "TERMINATED" {
            // The teardown reaches its end on the clock; the cluster stays describable.
            wait_for_the_clock();
        }
        let described = emr.describe_cluster(&cluster).await;
        let reported_state = text(described.ok(phase), "/virtualCluster/state", phase);
        assert_eq!(
            reported_state, phase,
            "case {phase:?}: the cluster must still be describable in this state, got \
             {reported_state:?}"
        );

        let refused = emr
            .send(
                "POST",
                &format!("/virtualclusters/{cluster}/jobruns"),
                Some(runnable_body(
                    &format!("tok-nf-boundary-{phase}"),
                    spark_submit_driver(),
                )),
            )
            .await;
        refused.refused(phase, 400, NOT_FOUND);
        let reported = text(&refused.body, "/message", phase);
        assert_ne!(
            reported,
            expected_message(VIRTUAL_CLUSTER, &cluster),
            "case {phase:?}: a cluster that exists must not be reported as absent"
        );
        assert!(
            reported.contains(&cluster),
            "case {phase:?}: the refusal must name the cluster it is about, got {reported:?}"
        );
    }
}
