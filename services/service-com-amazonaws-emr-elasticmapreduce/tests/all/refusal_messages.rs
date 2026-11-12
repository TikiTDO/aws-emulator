// SPDX-License-Identifier: CC0-1.0

//! What an Amazon EMR refusal SAYS, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the
//! router's `dispatch`, using the harness `tests/all/job_flow_lifecycle.rs` owns, so
//! these are the same bytes an SDK client sends.
//!
//! THE RULE every table below asserts: a refusal's MESSAGE is the service's own text for
//! that refusal — the operation's wording, echoing the request's id only where the
//! service echoes it — rather than one sentence the emulator writes for a whole family of
//! operations. The status and the error code are asserted beside it, because a client
//! reads all three.

use serde_json::{json, Value};

use crate::job_flow_lifecycle::{assert_refused, at, kept_alive_request, Emr};

/// Cluster ids no cluster carries: the shape a real id has, a shorter and a longer body,
/// a prefix with nothing after it, the empty string (`ClusterId` is `len 0..256`, so the
/// boundary validator passes it to the handler) and a value of another form entirely.
const UNKNOWN_CLUSTER_IDS: &[&str] = &[
    "j-ZZZZZZZZZZZZZ",
    "j-1",
    "j-000000000000000000000000",
    "j-",
    "",
    "not-a-cluster-id",
];

/// Studio ids no Studio carries, spanning the same classes.
///
/// `pub(crate)` so a sibling suite refuses the same ids through one list rather than its
/// own copy.
pub(crate) const UNKNOWN_STUDIO_IDS: &[&str] = &["es-NOPE", "es-ZZZZZZZZZZZZZ", "es-", "", "studio"];

/// The message of a refusal: the modelled shapes carry it as `Message`, the framework's
/// `ValidationException` channel as `message`, and a client reads whichever its service
/// sends. A refusal that carries neither fails the case rather than comparing against an
/// empty string.
///
/// `pub(crate)` so a sibling suite reads a refusal's message through one helper rather
/// than its own copy.
pub(crate) fn message(case: &str, payload: &Value) -> String {
    let said = crate::input_refusals::error_message(payload);
    assert!(
        !said.is_empty(),
        "case {case:?}: a refusal must carry a message, got {payload}"
    );
    said
}

/// The members `CreateStudio` requires, with every network id in the form Amazon EC2
/// gives that resource.
///
/// `pub(crate)` so a sibling suite creates a Studio through one request shape rather than
/// its own copy.
pub(crate) fn studio_request(name: &str) -> Value {
    json!({
        "Name": name,
        "AuthMode": "IAM",
        "VpcId": "vpc-0a1b2c3d",
        "SubnetIds": ["subnet-0a1b2c3d"],
        "ServiceRole": "EMR_Studio_ServiceRole",
        "WorkspaceSecurityGroupId": "sg-0a1b2c3d",
        "EngineSecurityGroupId": "sg-1a2b3c4d",
        "DefaultS3Location": "s3://emr-studio/backup/",
    })
}

/// A request body for each cluster-scoped operation, paired with the message Amazon EMR
/// answers an unresolvable `ClusterId` with: the cluster-id sentence, which quotes the id
/// the caller sent, except where the operation words it otherwise — the legacy job-flow
/// sentence for `AddInstanceFleet`, and the same statement with no id for
/// `GetClusterSessionCredentials`.
fn unknown_cluster_cases(cluster_id: &str) -> Vec<(&'static str, Value, String)> {
    let quoted = format!("Cluster id '{cluster_id}' is not valid.");
    vec![
        (
            "ModifyCluster",
            json!({ "ClusterId": cluster_id, "StepConcurrencyLevel": 2 }),
            quoted.clone(),
        ),
        (
            "CancelSteps",
            json!({ "ClusterId": cluster_id, "StepIds": ["s-ZZZZZZZZZZZZZ"] }),
            quoted.clone(),
        ),
        (
            "DescribeCluster",
            json!({ "ClusterId": cluster_id }),
            quoted.clone(),
        ),
        (
            "ListSteps",
            json!({ "ClusterId": cluster_id }),
            quoted.clone(),
        ),
        (
            "ListInstanceGroups",
            json!({ "ClusterId": cluster_id }),
            quoted.clone(),
        ),
        (
            "DescribeStep",
            json!({ "ClusterId": cluster_id, "StepId": "s-ZZZZZZZZZZZZZ" }),
            quoted.clone(),
        ),
        (
            "GetSession",
            json!({ "ClusterId": cluster_id, "SessionId": "is-0000000000000" }),
            quoted,
        ),
        (
            "AddInstanceFleet",
            json!({
                "ClusterId": cluster_id,
                "InstanceFleet": {
                    "InstanceFleetType": "TASK",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                },
            }),
            "Specified job flow ID not valid.".to_string(),
        ),
        (
            "GetClusterSessionCredentials",
            json!({
                "ClusterId": cluster_id,
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime",
            }),
            "Cluster id is not valid.".to_string(),
        ),
    ]
}

/// THE RULE for the cluster-scoped family: each operation answers its OWN wording, and
/// the id is quoted only by the operations that quote it — the same refusal while another
/// cluster exists, so it is a lookup rather than a blanket refusal.
#[tokio::test]
async fn each_cluster_scoped_operation_words_its_own_unknown_cluster_refusal() {
    let emr = Emr::new();
    let live = emr.run_job_flow(kept_alive_request("live")).await;

    for unknown in UNKNOWN_CLUSTER_IDS {
        for (operation, body, expected) in unknown_cluster_cases(unknown) {
            let case = format!("{operation} with cluster id {unknown:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(&case, status, &payload, "InvalidRequestException");
            assert_eq!(
                message(&case, &payload),
                expected,
                "case {case:?}: expected {expected:?}, got {payload}"
            );
        }
    }

    // The live cluster is untouched, and resolves — so the refusals above were lookups.
    let cluster = emr.describe_cluster(&live).await;
    assert_eq!(
        cluster["Id"].as_str(),
        Some(live.as_str()),
        "the live cluster must still read back, got {cluster}"
    );
}

/// THE RULE: `GetClusterSessionCredentials` on a cluster that EXISTS but whose security
/// configuration enables neither runtime roles nor IAM Identity Center is refused with
/// the sentence naming that missing feature — not with the unknown-cluster wording, and
/// not with an emulator-authored sentence about endpoints.
#[tokio::test]
async fn session_credentials_name_the_missing_security_configuration_feature() {
    let emr = Emr::new();
    // Rows differ along what the cluster carries and whether the caller names a runtime
    // role, which is what the rule generalises over: the refusal is about the CLUSTER.
    let plain = emr.run_job_flow(kept_alive_request("no security configuration")).await;
    let configured = emr
        .run_job_flow(json!({
            "Name": "with a security configuration",
            "ReleaseLabel": "emr-7.1.0",
            "SecurityConfiguration": "named-but-not-runtime-roles",
            "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE,
            "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
            "Instances": { "InstanceCount": 1, "MasterInstanceType": "m5.xlarge",
                           "KeepJobFlowAliveWhenNoSteps": true },
        }))
        .await;

    let expected = "You haven\u{2019}t enabled runtime roles or IAM Identity Center for your \
                    cluster in its security configuration.";
    for (case, body) in [
        ("no execution role", json!({ "ClusterId": &plain })),
        (
            "an execution role",
            json!({
                "ClusterId": &plain,
                "ExecutionRoleArn": "arn:aws:iam::000000000000:role/EMRRuntime",
            }),
        ),
        (
            "a cluster naming a security configuration",
            json!({ "ClusterId": &configured }),
        ),
    ] {
        let (status, payload) = emr.call("GetClusterSessionCredentials", body).await;
        assert_refused(case, status, &payload, "InvalidRequestException");
        assert_eq!(
            message(case, &payload),
            expected,
            "case {case:?}: expected {expected:?}, got {payload}"
        );
    }
}

/// THE RULE: every call of the deprecated `DescribeJobFlows` is refused with
/// `ValidationException` whose message states that the API is deprecated — whatever
/// arguments the call carries.
#[tokio::test]
async fn describe_job_flows_states_that_the_api_is_deprecated() {
    let emr = Emr::new();
    // A cluster exists, so the refusal is not "there is nothing to report".
    emr.run_job_flow(kept_alive_request("described")).await;

    // Rows span the argument shapes the operation accepts.
    for (case, body) in [
        ("no arguments at all", json!({})),
        ("one job flow id", json!({ "JobFlowIds": ["j-ZZZZZZZZZZZZZ"] })),
        ("an empty id list", json!({ "JobFlowIds": [] })),
        ("a state filter", json!({ "JobFlowStates": ["WAITING"] })),
        (
            "a creation window",
            json!({ "CreatedAfter": 0, "CreatedBefore": 4_000_000_000i64 }),
        ),
    ] {
        let (status, payload) = emr.call("DescribeJobFlows", body).await;
        assert_refused(case, status, &payload, "ValidationException");
        let said = message(case, &payload);
        assert!(
            said.to_lowercase().contains("deprecated"),
            "case {case:?}: the refusal must state the API is deprecated, got {said:?}"
        );
        assert!(
            payload.get("JobFlows").is_none(),
            "case {case:?}: the refusal serves no job flows, got {payload}"
        );
    }
}

/// THE RULE: an unknown `StudioId` is refused with the EMR Studio backend's wrapped form
/// — the backend's service name and the wire fields of the error it raised, a fresh
/// request id per call, and no Studio id anywhere in it.
#[tokio::test]
async fn an_unknown_studio_is_refused_with_the_studio_backends_wrapped_message() {
    let emr = Emr::new();
    // A Studio exists, so each refusal below is a lookup.
    let live = emr.ok("CreateStudio", studio_request("live")).await["StudioId"]
        .as_str()
        .expect("a StudioId")
        .to_string();

    let mut seen_request_ids: Vec<String> = Vec::new();
    for unknown in UNKNOWN_STUDIO_IDS {
        for (operation, body) in studio_id_requests(unknown) {
            let case = format!("{operation} with studio id {unknown:?}");
            let (status, payload) = emr.call(operation, body).await;
            assert_refused(&case, status, &payload, "InvalidRequestException");
            let said = message(&case, &payload);

            let request_id = wrapped_request_id(&case, &said);
            assert!(
                !seen_request_ids.contains(&request_id),
                "case {case:?}: each refusal carries its own request id, got the \
                 already-reported {request_id:?}"
            );
            seen_request_ids.push(request_id.clone());
            assert_eq!(
                said,
                format!(
                    "Studio does not exist. (Service: AWSEditors; Status Code: 400; \
                     Error Code: InvalidRequestException; Request ID: {request_id}; Proxy: null)"
                ),
                "case {case:?}: unexpected message {said:?}"
            );
            assert!(
                unknown.is_empty() || !said.contains(unknown),
                "case {case:?}: the refusal must not echo the Studio id, got {said:?}"
            );
        }
    }

    // The live Studio still resolves.
    let studio = emr.ok("DescribeStudio", json!({ "StudioId": &live })).await;
    assert_eq!(
        at(&studio, &["Studio", "StudioId"]).as_str(),
        Some(live.as_str()),
        "the live studio must still read back, got {studio}"
    );
}

/// Every operation that resolves a `StudioId`, with a body naming `studio_id`.
fn studio_id_requests(studio_id: &str) -> Vec<(&'static str, Value)> {
    let identity = json!({
        "StudioId": studio_id,
        "IdentityName": "analyst",
        "IdentityType": "USER",
    });
    let mut mapped = identity.clone();
    mapped["SessionPolicyArn"] = json!("arn:aws:iam::aws:policy/EMRStudioBasic");
    vec![
        ("DescribeStudio", json!({ "StudioId": studio_id })),
        (
            "UpdateStudio",
            json!({ "StudioId": studio_id, "Name": "renamed" }),
        ),
        ("DeleteStudio", json!({ "StudioId": studio_id })),
        ("CreateStudioSessionMapping", mapped.clone()),
        ("GetStudioSessionMapping", identity.clone()),
        ("UpdateStudioSessionMapping", mapped),
        ("DeleteStudioSessionMapping", identity),
    ]
}

/// The request id a wrapped Studio refusal reports, asserted to be the 8-4-4-4-12
/// lowercase-hex form an AWS request id carries.
///
/// `pub(crate)` so a sibling suite reads the wrapped form through one helper rather than
/// its own copy.
pub(crate) fn wrapped_request_id(case: &str, said: &str) -> String {
    let after = said
        .split_once("Request ID: ")
        .unwrap_or_else(|| panic!("case {case:?}: the wrapped form names a request id, got {said:?}"))
        .1;
    let request_id = after
        .split_once(';')
        .unwrap_or_else(|| panic!("case {case:?}: the request id ends the field, got {said:?}"))
        .0
        .to_string();
    let groups: Vec<&str> = request_id.split('-').collect();
    assert_eq!(
        groups.iter().map(|group| group.len()).collect::<Vec<_>>(),
        vec![8usize, 4, 4, 4, 12],
        "case {case:?}: expected an 8-4-4-4-12 request id, got {request_id:?}"
    );
    assert!(
        groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()
                && !byte.is_ascii_uppercase())),
        "case {case:?}: expected lowercase hex in the request id, got {request_id:?}"
    );
    request_id
}
