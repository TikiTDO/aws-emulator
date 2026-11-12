// SPDX-License-Identifier: CC0-1.0

//! The RESPONSE CONTRACT of the EMR Studio, notebook-execution, cluster-launch and
//! catalog surfaces, end to end over the wire.
//!
//! Every case drives the real HTTP path — `POST /` with
//! `X-Amz-Target: ElasticMapReduce.<Op>` and an `awsJson1_1` body — through the harness
//! `tests/all/job_flow_lifecycle.rs` owns, so these are the same bytes an SDK client
//! sends, and what each case reads back comes out of the service's own operations.
//!
//! The rules these tables assert, one per suite below:
//!
//! * every refusal the backend behind EMR Studio and EMR Notebooks raises states its
//!   class's own sentence and carries that backend's service detail — its name, the
//!   status and error code of the refusal, and a request id of the response — with no
//!   value of the request in it;
//! * a Studio that authenticates through IAM Identity Center is not created without the
//!   `UserRole` its identities assume;
//! * a cluster launch names ONE Amazon EMR release: not two ways of naming it, and not a
//!   release the catalog does not offer;
//! * the retired job-flow read refuses every call with the service's own deprecation
//!   sentence;
//! * a release reports every application its line bundles, at the version that line
//!   carries.

use serde_json::{json, Value};

use crate::emr_control_plane::studio_request;
use crate::job_flow_lifecycle::{assert_refused, at, minimal_request, Emr};
use crate::refusal_messages::{message, wrapped_request_id};

/// The service the EMR Studio and EMR Notebooks backend names itself as in the refusals
/// it wraps, as the recorded service answers it.
const BACKEND: &str = "AWSEditors";

/// The sentence the notebook-execution operations refuse an execution id they hold no
/// execution for with. The id is deliberately NOT part of it.
const NO_SUCH_EXECUTION: &str = "Execution does not exist";

/// The wrapped form a backend refusal carries `sentence` in, with `request_id` as the
/// request id that response reported.
fn wrapped(sentence: &str, request_id: &str) -> String {
    format!(
        "{sentence} (Service: {BACKEND}; Status Code: 400; Error Code: \
         InvalidRequestException; Request ID: {request_id}; Proxy: null)"
    )
}

/// A `CreateStudio` body with `member` replaced by `value`.
fn studio_with(name: &str, member: &str, value: Value) -> Value {
    let mut body = studio_request(name);
    body[member] = value;
    body
}

/// One row of the backend-wrapper table: the operation refused, the body that earns the
/// refusal, the sentence it must state, and a value of the request the refusal must not
/// echo.
struct WrappedRow {
    case: &'static str,
    operation: &'static str,
    body: Value,
    sentence: &'static str,
    never_echoed: &'static str,
}

/// One row of the Identity Center user-role table: the authentication mode, the `UserRole`
/// the request carries (absent, blank or named) and the status it is answered with.
struct UserRoleRow {
    case: &'static str,
    auth_mode: &'static str,
    user_role: Option<&'static str>,
    expected: u16,
}

/// One row of the one-release table: the two members a launch may name its release by, and
/// the sentence it is refused with when it names them wrongly (`None` = the launch is
/// accepted).
struct ReleaseRow {
    case: &'static str,
    ami_version: Option<&'static str>,
    release_label: Option<&'static str>,
    refusal: Option<String>,
}

/// One row of the primary-group table: the `Instances` configuration, and the sentence the
/// launch is refused with (`None` = accepted).
struct PrimaryGroupRow {
    case: &'static str,
    instances: Value,
    refusal: Option<&'static str>,
}

/// One row of the catalog table: a release label, and applications it must report at a
/// given version.
struct CatalogRow {
    label: &'static str,
    expected: &'static [(&'static str, &'static str)],
}

/// THE RULE: every refusal the Studio/Notebooks backend raises states the fixed sentence
/// of its class inside that backend's service detail, with a request id of its own and no
/// value taken from the request.
///
/// The rows span the operations and the input members the rule covers: both
/// notebook-execution reads, each network member of a Studio create, and a Studio id the
/// account holds no Studio for.
#[tokio::test]
async fn the_studio_backend_wraps_every_refusal_it_raises() {
    let emr = Emr::new();
    // A Studio and a notebook execution id that resolve to nothing: the refusals below
    // are lookups, and the Studio proves the create path works for a well-formed request.
    let live = emr.ok("CreateStudio", studio_request("live")).await["StudioId"]
        .as_str()
        .expect("a StudioId")
        .to_string();

    let rows = vec![
        WrappedRow {
            case: "a notebook execution id no execution carries",
            operation: "DescribeNotebookExecution",
            body: json!({ "NotebookExecutionId": "ex-IZY8V1S8L2GRPARBQIGH1P5W1H2XX" }),
            sentence: NO_SUCH_EXECUTION,
            never_echoed: "ex-IZY8V1S8L2GRPARBQIGH1P5W1H2XX",
        },
        WrappedRow {
            case: "stopping a notebook execution no execution carries",
            operation: "StopNotebookExecution",
            body: json!({ "NotebookExecutionId": "ex-0000000000000000000000000000X" }),
            sentence: NO_SUCH_EXECUTION,
            never_echoed: "ex-0000000000000000000000000000X",
        },
        WrappedRow {
            case: "an execution id of another form entirely",
            operation: "DescribeNotebookExecution",
            body: json!({ "NotebookExecutionId": "not-an-execution" }),
            sentence: NO_SUCH_EXECUTION,
            never_echoed: "not-an-execution",
        },
        WrappedRow {
            case: "a VpcId that resolves to no VPC",
            operation: "CreateStudio",
            body: studio_with("vpc", "VpcId", json!("vpc-00000000deadbeef")),
            sentence: "VpcId is not valid.",
            never_echoed: "vpc-00000000deadbeef",
        },
        WrappedRow {
            case: "a VpcId outside the form Amazon EC2 gives a VPC",
            operation: "CreateStudio",
            body: studio_with("vpc form", "VpcId", json!("vpc-nope")),
            sentence: "VpcId is not valid.",
            never_echoed: "vpc-nope",
        },
        WrappedRow {
            case: "a SubnetIds entry that resolves to no subnet",
            operation: "CreateStudio",
            body: studio_with("subnet", "SubnetIds", json!(["subnet-zzzzzzzz"])),
            sentence: "SubnetIds is not valid.",
            never_echoed: "subnet-zzzzzzzz",
        },
        WrappedRow {
            case: "one good subnet and one bad",
            operation: "CreateStudio",
            body: studio_with(
                "subnets",
                "SubnetIds",
                json!(["subnet-0a1b2c3d", "subnet-zzzzzzzz"]),
            ),
            sentence: "SubnetIds is not valid.",
            never_echoed: "subnet-zzzzzzzz",
        },
        WrappedRow {
            case: "a WorkspaceSecurityGroupId that names no security group",
            operation: "CreateStudio",
            body: studio_with("workspace sg", "WorkspaceSecurityGroupId", json!("sg-nope")),
            sentence: "WorkspaceSecurityGroupId is not valid.",
            never_echoed: "sg-nope",
        },
        WrappedRow {
            case: "an EngineSecurityGroupId that names no security group",
            operation: "CreateStudio",
            body: studio_with("engine sg", "EngineSecurityGroupId", json!("engine")),
            sentence: "EngineSecurityGroupId is not valid.",
            never_echoed: "engine",
        },
        WrappedRow {
            case: "a StudioId the account holds no Studio for",
            operation: "DescribeStudio",
            body: json!({ "StudioId": "es-ZZZZZZZZZZZZZ" }),
            sentence: "Studio does not exist.",
            never_echoed: "es-ZZZZZZZZZZZZZ",
        },
    ];

    let mut seen_request_ids: Vec<String> = Vec::new();
    for WrappedRow {
        case,
        operation,
        body,
        sentence,
        never_echoed,
    } in rows
    {
        let (status, payload) = emr.call(operation, body).await;
        assert_refused(case, status, &payload, "InvalidRequestException");
        let said = message(case, &payload);

        let request_id = wrapped_request_id(case, &said);
        assert!(
            !seen_request_ids.contains(&request_id),
            "case {case:?}: each refusal carries its own request id, got the \
             already-reported {request_id:?} in {said:?}"
        );
        seen_request_ids.push(request_id.clone());
        assert_eq!(
            said,
            wrapped(sentence, &request_id),
            "case {case:?}: expected {:?}, got {said:?}",
            wrapped(sentence, "<this response's request id>")
        );
        assert!(
            !said.contains(never_echoed),
            "case {case:?}: the refusal must not echo {never_echoed:?}, got {said:?}"
        );
    }

    // Nothing above was stored: the one Studio of this account is still the live one.
    let listed = emr.ok("ListStudios", json!({})).await;
    let studios = at(&listed, &["Studios"])
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        studios.len(),
        1,
        "case {:?}: a refused create stores nothing, got {listed}",
        "after every refusal above"
    );
    assert_eq!(
        studios[0]["StudioId"].as_str(),
        Some(live.as_str()),
        "case {:?}: the live Studio must be the one listed, got {listed}",
        "after every refusal above"
    );
}

/// THE RULE: a Studio that authenticates through IAM Identity Center carries the
/// `UserRole` its identities assume, so a create that authenticates that way and names
/// none is refused as an invalid request — with `ValidationException` and the service's
/// own constant sentence — while an IAM-authenticated Studio needs none.
///
/// The rows differ along the two members the rule reads: the authentication mode, and
/// whether a `UserRole` is absent, blank or named.
#[tokio::test]
async fn an_identity_center_studio_is_not_created_without_a_user_role() {
    let emr = Emr::new();
    let required = "UserRole is required to create a studio with SSO auth-mode.";

    let rows = vec![
        UserRoleRow {
            case: "Identity Center auth, no UserRole",
            auth_mode: "SSO",
            user_role: None,
            expected: 400,
        },
        UserRoleRow {
            case: "Identity Center auth, empty UserRole",
            auth_mode: "SSO",
            user_role: Some(""),
            expected: 400,
        },
        UserRoleRow {
            case: "Identity Center auth, blank UserRole",
            auth_mode: "SSO",
            user_role: Some("   "),
            expected: 400,
        },
        UserRoleRow {
            case: "Identity Center auth, a named UserRole",
            auth_mode: "SSO",
            user_role: Some("EMRStudio_User"),
            expected: 200,
        },
        UserRoleRow {
            case: "Identity Center auth, a UserRole given as an ARN",
            auth_mode: "SSO",
            user_role: Some("arn:aws:iam::000000000000:role/EMRStudio_User"),
            expected: 200,
        },
        UserRoleRow {
            case: "IAM auth, no UserRole",
            auth_mode: "IAM",
            user_role: None,
            expected: 200,
        },
        UserRoleRow {
            case: "IAM auth, a named UserRole",
            auth_mode: "IAM",
            user_role: Some("EMRStudio_User"),
            expected: 200,
        },
    ];

    let mut created = 0usize;
    for UserRoleRow {
        case,
        auth_mode,
        user_role,
        expected,
    } in rows
    {
        let mut body = studio_request(case);
        body["AuthMode"] = json!(auth_mode);
        match user_role {
            Some(role) => body["UserRole"] = json!(role),
            None => {
                body.as_object_mut().expect("an object").remove("UserRole");
            }
        }

        let (status, payload) = emr.call("CreateStudio", body).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} with {payload}"
        );
        if expected == 400 {
            let code = at(&payload, &["__type"]).as_str().unwrap_or_default();
            assert!(
                code.contains("ValidationException"),
                "case {case:?}: expected error code ValidationException, got {code:?} in \
                 {payload}"
            );
            let said = message(case, &payload);
            assert_eq!(
                said, required,
                "case {case:?}: expected {required:?}, got {said:?}"
            );
            assert!(
                payload.get("StudioId").is_none(),
                "case {case:?}: a refused create hands back no studio, got {payload}"
            );
        } else {
            created += 1;
            let studio_id = payload["StudioId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a StudioId, got {payload}"));
            // Round trip: the Studio the create answered is the one a read reports, with
            // the user role it was given.
            let studio = emr
                .ok("DescribeStudio", json!({ "StudioId": studio_id }))
                .await["Studio"]
                .clone();
            assert_eq!(
                studio["UserRole"].as_str(),
                user_role.filter(|role| !role.trim().is_empty()),
                "case {case:?}: the stored UserRole must be the one created, got {studio}"
            );
        }
    }

    let listed = emr.ok("ListStudios", json!({})).await;
    let studios = at(&listed, &["Studios"])
        .as_array()
        .map_or(0, Vec::len);
    assert_eq!(
        studios, created,
        "case {:?}: only the accepted creates may be stored, got {listed}",
        "the whole table"
    );
}

/// THE RULE: a cluster runs ONE Amazon EMR release — named by a release label OR, for the
/// 3.x/2.x generations, by an AMI version, never both — and that release has to be one the
/// catalog offers, so each of those launches is refused with the service's own sentence
/// and no cluster is created.
///
/// The rows span both ways of naming the release, both values of each pair (so the echoed
/// values come from the request rather than from one recorded case), a label the catalog
/// does not offer, and the two accepted shapes.
#[tokio::test]
async fn a_launch_names_one_amazon_emr_release() {
    let emr = Emr::new();

    let rows = vec![
        ReleaseRow {
            case: "an AMI version and a release label together",
            ami_version: Some("3.11.0"),
            release_label: Some("emr-6.15.0"),
            refusal: Some(
                "Only one AMI version and release label may be specified. Provided AMI: \
                 3.11.0, release label: emr-6.15.0."
                    .to_string(),
            ),
        },
        ReleaseRow {
            case: "another AMI version and another release label",
            ami_version: Some("2.4.11"),
            release_label: Some("emr-5.36.0"),
            refusal: Some(
                "Only one AMI version and release label may be specified. Provided AMI: \
                 2.4.11, release label: emr-5.36.0."
                    .to_string(),
            ),
        },
        ReleaseRow {
            case: "a release label the catalog does not offer",
            ami_version: None,
            release_label: Some("emr-0.0.0"),
            refusal: Some("The supplied release label is invalid: emr-0.0.0.".to_string()),
        },
        ReleaseRow {
            case: "a release label of an unpublished patch",
            ami_version: None,
            release_label: Some("emr-6.15.1"),
            refusal: Some("The supplied release label is invalid: emr-6.15.1.".to_string()),
        },
        ReleaseRow {
            case: "a release label the catalog offers",
            ami_version: None,
            release_label: Some("emr-6.15.0"),
            refusal: None,
        },
        ReleaseRow {
            case: "an AMI version alone",
            ami_version: Some("3.11.0"),
            release_label: None,
            refusal: None,
        },
        ReleaseRow {
            case: "neither member",
            ami_version: None,
            release_label: None,
            refusal: None,
        },
    ];

    let mut launched: Vec<String> = Vec::new();
    for ReleaseRow {
        case,
        ami_version,
        release_label,
        refusal,
    } in rows
    {
        let mut body = minimal_request(case);
        if let Some(ami_version) = ami_version {
            body["AmiVersion"] = json!(ami_version);
        }
        if let Some(release_label) = release_label {
            body["ReleaseLabel"] = json!(release_label);
        }

        let (status, payload) = emr.call("RunJobFlow", body).await;
        match refusal {
            Some(expected) => {
                assert_refused(case, status, &payload, "ValidationException");
                let said = message(case, &payload);
                assert_eq!(
                    said, expected,
                    "case {case:?}: expected {expected:?}, got {said:?}"
                );
                assert!(
                    payload.get("JobFlowId").is_none(),
                    "case {case:?}: a refused launch hands back no cluster, got {payload}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {payload}"
                );
                let id = payload["JobFlowId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a JobFlowId, got {payload}"))
                    .to_string();
                // Round trip: the launch that was accepted is readable, on the release it
                // named.
                let cluster = emr.describe_cluster(&id).await;
                assert_eq!(
                    cluster["ReleaseLabel"].as_str(),
                    release_label,
                    "case {case:?}: the cluster must report the release it was launched on, \
                     got {cluster}"
                );
                launched.push(id);
            }
        }
    }

    // Exactly the accepted launches exist: no refused one left a cluster behind.
    let listed = emr.ok("ListClusters", json!({})).await;
    let reported: Vec<String> = at(&listed, &["Clusters"])
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|cluster| cluster["Id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        reported.len(),
        launched.len(),
        "case {:?}: only the accepted launches may be stored, got {listed}",
        "the whole table"
    );
    for id in &launched {
        assert!(
            reported.contains(id),
            "case {id:?}: an accepted launch must be listed, got {reported:?}"
        );
    }
}

/// THE RULE: a cluster comes up on its PRIMARY node, so a launch stating its capacity as
/// instance groups names exactly one `MASTER` group — among the groups it carries or
/// through the uniform master instance type that implies one — and a launch naming none,
/// or naming several, is refused with the service's own constant sentence for that side of
/// the requirement, leaving no cluster behind.
///
/// The rows differ along what the rule generalises over: which roles the group list
/// carries, how many groups it carries, how many of them are primary, an empty list, the
/// uniform form, and the instance fleets form, which states its primary in its own
/// vocabulary.
#[tokio::test]
async fn a_launch_states_its_capacity_with_a_primary_instance_group() {
    let emr = Emr::new();
    let no_primary = "Zero master instance groups supplied, you must specify exactly one \
                      master instance group.";
    let several_primaries = "Multiple master instance groups supplied, you must specify \
                             exactly one master instance group.";
    let group = |role: &str| {
        json!({ "InstanceRole": role, "InstanceType": "m5.xlarge", "InstanceCount": 1 })
    };

    let rows = vec![
        PrimaryGroupRow {
            case: "a core group alone",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [group("CORE")],
            }),
            refusal: Some(no_primary),
        },
        PrimaryGroupRow {
            case: "a task group alone",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [group("TASK")],
            }),
            refusal: Some(no_primary),
        },
        PrimaryGroupRow {
            case: "a core and a task group, no primary",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [group("CORE"), group("TASK")],
            }),
            refusal: Some(no_primary),
        },
        PrimaryGroupRow {
            case: "an empty group list and no master instance type",
            instances: json!({ "KeepJobFlowAliveWhenNoSteps": true, "InstanceGroups": [] }),
            refusal: Some(no_primary),
        },
        PrimaryGroupRow {
            case: "no instances configuration at all beyond the flag",
            instances: json!({ "KeepJobFlowAliveWhenNoSteps": true }),
            refusal: Some(no_primary),
        },
        PrimaryGroupRow {
            // A highly available cluster states its several primary NODES as the one
            // primary group's InstanceCount, so two primary groups are one too many.
            case: "two primary groups",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [group("MASTER"), group("MASTER")],
            }),
            refusal: Some(several_primaries),
        },
        PrimaryGroupRow {
            case: "two primary groups and a core group",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [group("MASTER"), group("CORE"), group("MASTER")],
            }),
            refusal: Some(several_primaries),
        },
        PrimaryGroupRow {
            case: "a primary group alone",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [group("MASTER")],
            }),
            refusal: None,
        },
        PrimaryGroupRow {
            case: "one primary group of three nodes, which is how a cluster is made available",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [{
                    "InstanceRole": "MASTER",
                    "InstanceType": "m5.xlarge",
                    "InstanceCount": 3,
                }],
            }),
            refusal: None,
        },
        PrimaryGroupRow {
            case: "a primary and a core group",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceGroups": [group("MASTER"), group("CORE")],
            }),
            refusal: None,
        },
        PrimaryGroupRow {
            case: "the uniform form, which implies a primary group",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceCount": 2,
                "MasterInstanceType": "m5.xlarge",
                "SlaveInstanceType": "m5.xlarge",
            }),
            refusal: None,
        },
        PrimaryGroupRow {
            case: "the instance fleets form, whose primary is a fleet",
            instances: json!({
                "KeepJobFlowAliveWhenNoSteps": true,
                "InstanceFleets": [{
                    "InstanceFleetType": "MASTER",
                    "TargetOnDemandCapacity": 1,
                    "InstanceTypeConfigs": [{ "InstanceType": "m5.xlarge" }],
                }],
            }),
            refusal: None,
        },
    ];

    let mut launched = 0usize;
    for PrimaryGroupRow {
        case,
        instances,
        refusal,
    } in rows
    {
        let (status, payload) = emr
            .call(
                "RunJobFlow",
                json!({
                    "Name": case,
                    "JobFlowRole": crate::job_flow_lifecycle::INSTANCE_PROFILE,
                    "ServiceRole": crate::job_flow_lifecycle::SERVICE_ROLE,
                    "Instances": instances,
                }),
            )
            .await;
        match refusal {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {payload}"
                );
                launched += 1;
                // Round trip: the cluster is readable and carries a primary node.
                let id = payload["JobFlowId"].as_str().unwrap_or_default().to_string();
                let cluster = emr.describe_cluster(&id).await;
                assert_eq!(
                    cluster["Id"].as_str(),
                    Some(id.as_str()),
                    "case {case:?}: the launched cluster must read back, got {cluster}"
                );
            }
            Some(expected) => {
                assert_refused(case, status, &payload, "ValidationException");
                let said = message(case, &payload);
                assert_eq!(
                    said, expected,
                    "case {case:?}: expected {expected:?}, got {said:?}"
                );
                assert!(
                    payload.get("JobFlowId").is_none(),
                    "case {case:?}: a refused launch hands back no cluster, got {payload}"
                );
            }
        }
    }

    let listed = emr.ok("ListClusters", json!({})).await;
    assert_eq!(
        at(&listed, &["Clusters"]).as_array().map_or(0, Vec::len),
        launched,
        "case {:?}: only the accepted launches may be stored, got {listed}",
        "the whole table"
    );
}

/// THE RULE: the retired job-flow read serves no job flows at all — every call, whatever
/// it asks for, is refused with `ValidationException` and the service's own CONSTANT
/// deprecation sentence, which carries the API reference URL and nothing of the request.
#[tokio::test]
async fn the_deprecated_job_flow_read_states_the_services_own_sentence() {
    let emr = Emr::new();
    let deprecated = "DescribeJobFlows API is deprecated. Please refer to \
                      http://docs.aws.amazon.com/ElasticMapReduce/latest/API/API_DescribeJobFlows.html \
                      for further information.";

    // A live cluster, so the refusal is the operation's and not an empty-store answer.
    let live = emr.run_job_flow(minimal_request("live")).await;

    // The rows span every member the request may carry: none, ids, states, and a
    // creation-date window.
    for (case, body) in [
        ("no members at all", json!({})),
        ("a job flow id it could serve", json!({ "JobFlowIds": [live.clone()] })),
        ("a job flow id it holds none for", json!({ "JobFlowIds": ["j-ZZZZZZZZZZZZZ"] })),
        ("a state filter", json!({ "JobFlowStates": ["RUNNING", "WAITING"] })),
        ("a creation window", json!({ "CreatedAfter": 0, "CreatedBefore": 32503680000i64 })),
    ] {
        let (status, payload) = emr.call("DescribeJobFlows", body).await;
        assert_refused(case, status, &payload, "ValidationException");
        let said = message(case, &payload);
        assert_eq!(
            said, deprecated,
            "case {case:?}: expected {deprecated:?}, got {said:?}"
        );
        assert!(
            !said.contains(&live),
            "case {case:?}: the refusal states nothing of the request, got {said:?}"
        );
    }

    // And the cluster it would not serve is still readable through the operations that
    // replace it.
    let cluster = emr.describe_cluster(&live).await;
    assert_eq!(
        cluster["Id"].as_str(),
        Some(live.as_str()),
        "the live cluster must still read back, got {cluster}"
    );
}

/// THE RULE: `DescribeReleaseLabel` reports the COMPLETE application set of the release
/// line asked for, each application at the version that line carries, and the Amazon Linux
/// releases of the generation the release is built on.
///
/// The rows are release lines from each major generation the catalog publishes, asserted
/// on the applications whose version CHANGES between them — including applications no
/// case the issue cited mentions (`Sqoop`, `HCatalog`) — with the versions written out by
/// hand from the Amazon EMR Release Guide's "Application versions" tables, the same source
/// `state::APPLICATION_CATALOG` records.
#[tokio::test]
async fn a_release_reports_the_whole_application_set_of_its_line() {
    let emr = Emr::new();

    let rows = [
        CatalogRow {
            label: "emr-6.15.0",
            expected: &[
                ("HCatalog", "3.1.3"),
                ("Hive", "3.1.3"),
                ("JupyterHub", "1.5.0"),
                ("MXNet", "1.9.1"),
                ("Phoenix", "5.1.3"),
                ("Sqoop", "1.4.7"),
                ("TensorFlow", "2.11.0"),
            ],
        },
        CatalogRow {
            label: "emr-6.0.0",
            expected: &[
                ("HCatalog", "3.1.2"),
                ("Hive", "3.1.2"),
                ("JupyterHub", "1.1.0"),
                ("MXNet", "1.5.1"),
                ("Phoenix", "5.0.0"),
                ("Sqoop", "1.4.7"),
                ("TensorFlow", "1.14.0"),
            ],
        },
        CatalogRow {
            label: "emr-5.36.2",
            expected: &[
                ("HCatalog", "2.3.9"),
                ("Hive", "2.3.9"),
                ("JupyterHub", "1.4.1"),
                ("MXNet", "1.8.0"),
                ("Phoenix", "4.14.3"),
                ("Sqoop", "1.4.7"),
                ("TensorFlow", "2.4.1"),
            ],
        },
    ];

    for CatalogRow { label, expected } in rows {
        let payload = emr
            .ok("DescribeReleaseLabel", json!({ "ReleaseLabel": label }))
            .await;
        let reported: Vec<(String, String)> = at(&payload, &["Applications"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: expected Applications, got {payload}"))
            .iter()
            .map(|application| {
                (
                    at(application, &["Name"]).as_str().unwrap_or_default().to_string(),
                    at(application, &["Version"]).as_str().unwrap_or_default().to_string(),
                )
            })
            .collect();

        for (name, version) in expected {
            let found = reported
                .iter()
                .find(|(reported_name, _)| reported_name == name)
                .map(|(_, reported_version)| reported_version.as_str());
            assert_eq!(
                found,
                Some(*version),
                "case {label:?}: expected {name} {version}, got {found:?} in {reported:?}"
            );
        }

        // The Amazon Linux releases of the generation that release is built on, in the
        // documented dotted form, each reported once.
        let os_releases: Vec<String> = at(&payload, &["AvailableOSReleases"])
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: expected AvailableOSReleases, got {payload}"))
            .iter()
            .map(|release| at(release, &["Label"]).as_str().unwrap_or_default().to_string())
            .collect();
        assert!(
            !os_releases.is_empty(),
            "case {label:?}: a release offers Amazon Linux releases, got {payload}"
        );
        let mut unique = os_releases.clone();
        unique.dedup();
        assert_eq!(
            unique, os_releases,
            "case {label:?}: an Amazon Linux release is offered once, got {os_releases:?}"
        );

        // And a cluster launched on the release runs the newest of them when it names
        // none itself, which is what ties the published list to the launch path.
        let cluster_id = emr
            .run_job_flow({
                let mut body = minimal_request(label);
                body["ReleaseLabel"] = json!(label);
                body
            })
            .await;
        let cluster = emr.describe_cluster(&cluster_id).await;
        assert_eq!(
            cluster["OSReleaseLabel"].as_str(),
            os_releases.last().map(String::as_str),
            "case {label:?}: the default Amazon Linux release is the newest the release \
             offers, got {cluster}"
        );
    }
}
