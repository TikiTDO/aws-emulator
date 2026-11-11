// SPDX-License-Identifier: CC0-1.0

//! The sentences CodeDeploy error classes whose text is FIXED carry.
//!
//! RULE under test: a refusal — and the `errorInformation` a failed deployment
//! reports — carries the sentence that belongs to its error class, with only the
//! variable parts that class's own text has. So a class whose text is fixed
//! answers exactly ONE sentence to every request it refuses: neither the value
//! the request was refused for, nor the state that made it conflict, appears in
//! it.
//!
//! Four classes here have no variable part at all:
//!
//! * `InvalidEC2TagCombinationException` and
//!   `InvalidOnPremisesTagCombinationException` — both forms of one tag selector
//!   in a single call;
//! * `IamUserArnAlreadyRegisteredException` and
//!   `IamSessionArnAlreadyRegisteredException` — an IAM ARN a registration
//!   already holds;
//! * `InvalidLifecycleEventHookExecutionIdException` — an id that names no hook
//!   execution the deployment can take a result for;
//! * the `NO_INSTANCES` `errorInformation` of a deployment whose group resolved
//!   no target.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over:
//! the operation that read the member, which of the two forms the selector was
//! given in and how many entries each carried, the type and form of the IAM ARN
//! and which instance already held it, the form of the hook-execution id and why
//! it names no hook execution, and the compute platform and targeting of a group
//! that selects nothing. Every row of one class must answer the SAME sentence,
//! which is what "fixed" means.
//!
//! The sentences are spelled out here rather than read from the service, so a
//! later edit to the service's wording fails this suite instead of passing it.
//! The harness (`Emulator`, `error_code`) is the lifecycle suite's, so every case
//! goes through the same front door a client uses.

use serde_json::{json, Value};

use super::codedeploy_lifecycle::{
    error_code, s3_revision, seeded_group, terminal_deployment, Emulator,
};

/// The service role every deployment group here is created with.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// `InvalidEC2TagCombinationException`'s whole sentence.
const EC2_TAG_COMBINATION: &str = "The request specified both Ec2TagFilters and Ec2TagSet, but \
     only one of these data types can be used in a single call. Remove one tag specification \
     type, and then try again.";

/// `InvalidOnPremisesTagCombinationException`'s whole sentence.
const ON_PREMISES_TAG_COMBINATION: &str = "The request specified both OnPremisesTagFilters and \
     OnPremisesTagSet, but only one of these data types can be used in a single call. Remove one \
     tag specification type, and then try again.";

/// `IamUserArnAlreadyRegisteredException`'s whole sentence.
const IAM_USER_ARN_ALREADY_REGISTERED: &str = "The on-premises instance could not be registered \
     because the request included an IAM user ARN that has already been used to register an \
     instance. Include either a different IAM user ARN or IAM session ARN in the request, and \
     then try again.";

/// `IamSessionArnAlreadyRegisteredException`'s whole sentence.
const IAM_SESSION_ARN_ALREADY_REGISTERED: &str = "The on-premises instance could not be \
     registered because the request included an IAM session ARN that has already been used to \
     register a different instance. Include either a different IAM session ARN or IAM user ARN \
     in the request, and then try again.";

/// `InvalidLifecycleEventHookExecutionIdException`'s whole sentence.
const HOOK_EXECUTION_ID_INVALID: &str = "The lifecycleEventHookExecutionId is invalid";

/// The `errorInformation.message` of a deployment whose group resolved no target,
/// which goes with the `NO_INSTANCES` error code.
const NO_INSTANCES_MESSAGE: &str = "The deployment failed because no instances were found for \
     your deployment group. Check your deployment group settings to make sure the tags for your \
     Amazon EC2 instances or Auto Scaling groups correctly identify the instances you want to \
     deploy to, and then try again.";

/// One `EC2TagFilter`/`TagFilter` entry
/// (`smithy_query(query="shape",shape="EC2TagFilter")`: `Key`, `Value`, `Type`).
fn filter(key: &str, value: &str, kind: &str) -> Value {
    json!({"Key": key, "Value": value, "Type": kind})
}

/// The `$.message` of a failure body: the sentence the refusal carries.
fn message(case: &str, body: &Value) -> String {
    body["message"]
        .as_str()
        .or_else(|| body["Message"].as_str())
        .unwrap_or_else(|| panic!("case {case:?}: expected a string $.message, got {body}"))
        .to_string()
}

/// Call `operation`, require HTTP 400 with `code`, and answer the message.
async fn refused(
    emulator: &Emulator,
    case: &str,
    operation: &str,
    request: Value,
    code: &str,
) -> String {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {operation}, got {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        code,
        "case {case:?}: expected {code} from {operation}, got {body:?}"
    );
    message(case, &body)
}

/// An IAM user ARN, in the partition and with the path the caller asks for.
fn user_arn(partition: &str, path: &str, name: &str) -> String {
    format!("arn:{partition}:iam::123456789012:user/{path}{name}")
}

/// An IAM session ARN: `arn:aws:sts::<account>:assumed-role/<role>/<session>`.
fn session_arn(role: &str, session: &str) -> String {
    format!("arn:aws:sts::123456789012:assumed-role/{role}/{session}")
}

// ---------------------------------------------------------------------------
// Both forms of one tag selector in a single call
// ---------------------------------------------------------------------------

/// RULE: a deployment group selects its instances by EITHER a tag-filter list or
/// a tag set, so a call naming both forms of one selector is refused with that
/// combination error's own fixed sentence — the same one whichever operation read
/// the pair, however many entries each form carried, and whatever those entries
/// were.
#[tokio::test]
async fn naming_both_forms_of_one_tag_selector_answers_one_fixed_sentence() {
    let emulator = Emulator::new();
    let group = seeded_group(&emulator, "tag-combination-app", "Server").await;

    // Rows differ along the operation that read the pair (create, update), the
    // selector the pair belongs to (EC2, on-premises), how many entries each form
    // carries (none, one, two) and the `TagFilterType` those entries use.
    let cases: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "create, one ec2 filter and a one-list ec2 tag set",
            "CreateDeploymentGroup",
            json!({
                "applicationName": "tag-combination-app",
                "deploymentGroupName": "ec2-both-one",
                "serviceRoleArn": SERVICE_ROLE,
                "ec2TagFilters": [filter("Name", "web", "KEY_AND_VALUE")],
                "ec2TagSet": {"ec2TagSetList": [[filter("Name", "web", "KEY_AND_VALUE")]]},
            }),
            "InvalidEC2TagCombinationException",
            EC2_TAG_COMBINATION,
        ),
        (
            "create, two ec2 filters and a two-list ec2 tag set",
            "CreateDeploymentGroup",
            json!({
                "applicationName": "tag-combination-app",
                "deploymentGroupName": "ec2-both-two",
                "serviceRoleArn": SERVICE_ROLE,
                "ec2TagFilters": [
                    filter("Env", "prod", "KEY_ONLY"),
                    filter("Rack", "seven", "VALUE_ONLY"),
                ],
                "ec2TagSet": {"ec2TagSetList": [
                    [filter("Env", "prod", "KEY_ONLY")],
                    [filter("Rack", "seven", "VALUE_ONLY")],
                ]},
            }),
            "InvalidEC2TagCombinationException",
            EC2_TAG_COMBINATION,
        ),
        (
            "create, an EMPTY ec2 filter list beside an ec2 tag set",
            "CreateDeploymentGroup",
            json!({
                "applicationName": "tag-combination-app",
                "deploymentGroupName": "ec2-both-empty",
                "serviceRoleArn": SERVICE_ROLE,
                "ec2TagFilters": [],
                "ec2TagSet": {"ec2TagSetList": [[filter("Name", "web", "KEY_AND_VALUE")]]},
            }),
            "InvalidEC2TagCombinationException",
            EC2_TAG_COMBINATION,
        ),
        (
            "update, one ec2 filter and a one-list ec2 tag set",
            "UpdateDeploymentGroup",
            json!({
                "applicationName": "tag-combination-app",
                "currentDeploymentGroupName": group,
                "ec2TagFilters": [filter("Name", "web", "KEY_AND_VALUE")],
                "ec2TagSet": {"ec2TagSetList": [[filter("Name", "api", "KEY_AND_VALUE")]]},
            }),
            "InvalidEC2TagCombinationException",
            EC2_TAG_COMBINATION,
        ),
        (
            "create, one on-premises filter and a one-list on-premises tag set",
            "CreateDeploymentGroup",
            json!({
                "applicationName": "tag-combination-app",
                "deploymentGroupName": "onprem-both-one",
                "serviceRoleArn": SERVICE_ROLE,
                "onPremisesInstanceTagFilters": [filter("Rack", "seven", "KEY_AND_VALUE")],
                "onPremisesTagSet": {
                    "onPremisesTagSetList": [[filter("Rack", "seven", "KEY_AND_VALUE")]]
                },
            }),
            "InvalidOnPremisesTagCombinationException",
            ON_PREMISES_TAG_COMBINATION,
        ),
        (
            "create, two on-premises filters and a two-list on-premises tag set",
            "CreateDeploymentGroup",
            json!({
                "applicationName": "tag-combination-app",
                "deploymentGroupName": "onprem-both-two",
                "serviceRoleArn": SERVICE_ROLE,
                "onPremisesInstanceTagFilters": [
                    filter("Rack", "seven", "KEY_ONLY"),
                    filter("Row", "three", "VALUE_ONLY"),
                ],
                "onPremisesTagSet": {"onPremisesTagSetList": [
                    [filter("Rack", "seven", "KEY_ONLY")],
                    [filter("Row", "three", "VALUE_ONLY")],
                ]},
            }),
            "InvalidOnPremisesTagCombinationException",
            ON_PREMISES_TAG_COMBINATION,
        ),
        (
            "update, one on-premises filter and an EMPTY on-premises tag set",
            "UpdateDeploymentGroup",
            json!({
                "applicationName": "tag-combination-app",
                "currentDeploymentGroupName": group,
                "onPremisesInstanceTagFilters": [filter("Rack", "seven", "KEY_AND_VALUE")],
                "onPremisesTagSet": {"onPremisesTagSetList": []},
            }),
            "InvalidOnPremisesTagCombinationException",
            ON_PREMISES_TAG_COMBINATION,
        ),
    ];

    for (case, operation, request, code, expected) in cases {
        let answered = refused(&emulator, case, operation, request, code).await;
        assert_eq!(
            answered, expected,
            "case {case:?}: expected {code}'s own sentence, got {answered:?}"
        );
    }

    // The group the update rows named is unchanged: a refused update stores
    // nothing, so neither selector was written onto it.
    let read = emulator
        .ok(
            "GetDeploymentGroup",
            json!({"applicationName": "tag-combination-app", "deploymentGroupName": group}),
        )
        .await["deploymentGroupInfo"]
        .clone();
    for member in ["ec2TagFilters", "ec2TagSet", "onPremisesInstanceTagFilters", "onPremisesTagSet"]
    {
        assert!(
            read[member].is_null() || read[member].as_array().is_some_and(Vec::is_empty),
            "case {member:?}: a refused update wrote no selector, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An IAM ARN a registration already holds
// ---------------------------------------------------------------------------

/// RULE: one IAM ARN backs at most one registered on-premises instance, and the
/// second registration of one is refused with that ARN type's own fixed sentence
/// — which names neither the ARN the request carried nor the instance already
/// holding it, whatever form the ARN takes.
#[tokio::test]
async fn a_second_registration_of_one_iam_arn_answers_one_fixed_sentence() {
    let emulator = Emulator::new();

    // Rows differ along the IAM ARN's type (user, session), its form (a bare
    // user name, a user under a path, another partition, a short and a long
    // assumed-role session) and the instance already holding it.
    //
    // One row: the case name, the request member the ARN goes in, the ARN, the
    // error code the second registration answers and that class's sentence.
    let cases: Vec<(&str, &str, String, &str, &str)> = vec![
        (
            "a bare IAM user ARN",
            "iamUserArn",
            user_arn("aws", "", "deploy-user"),
            "IamUserArnAlreadyRegisteredException",
            IAM_USER_ARN_ALREADY_REGISTERED,
        ),
        (
            "an IAM user ARN under a path",
            "iamUserArn",
            user_arn("aws", "engineering/platform/", "deploy-user"),
            "IamUserArnAlreadyRegisteredException",
            IAM_USER_ARN_ALREADY_REGISTERED,
        ),
        (
            "an IAM user ARN in another partition",
            "iamUserArn",
            user_arn("aws-us-gov", "", "gov-user"),
            "IamUserArnAlreadyRegisteredException",
            IAM_USER_ARN_ALREADY_REGISTERED,
        ),
        (
            "an IAM session ARN",
            "iamSessionArn",
            session_arn("OnPremRole", "session-1"),
            "IamSessionArnAlreadyRegisteredException",
            IAM_SESSION_ARN_ALREADY_REGISTERED,
        ),
        (
            "an IAM session ARN of a long-named role",
            "iamSessionArn",
            session_arn(
                "OnPremisesFleetRegistrationRoleForTheWesternRacks",
                "session-with-a-long-name",
            ),
            "IamSessionArnAlreadyRegisteredException",
            IAM_SESSION_ARN_ALREADY_REGISTERED,
        ),
    ];

    for (index, (case, member, arn, code, expected)) in cases.iter().enumerate() {
        let holder = format!("holder-{index}");
        let second = format!("second-{index}");
        let mut first = json!({"instanceName": holder});
        first[*member] = json!(arn);
        emulator.ok("RegisterOnPremisesInstance", first).await;

        let mut again = json!({"instanceName": second});
        again[*member] = json!(arn);
        let answered = refused(&emulator, case, "RegisterOnPremisesInstance", again, code).await;
        assert_eq!(
            answered, *expected,
            "case {case:?}: expected {code}'s own sentence, got {answered:?}"
        );
        assert!(
            !answered.contains(arn.as_str()) && !answered.contains(holder.as_str()),
            "case {case:?}: the sentence names neither the ARN nor the instance holding it, \
             got {answered:?}"
        );

        // The refusal registered nothing: the name it was asked for is still
        // unregistered, and the instance holding the ARN is untouched.
        let (status, body) = emulator
            .call("GetOnPremisesInstance", json!({"instanceName": second}))
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InstanceNotRegisteredException"),
            "case {case:?}: expected {second} to be unregistered, got {status} {body:?}"
        );
        let held = emulator
            .ok("GetOnPremisesInstance", json!({"instanceName": holder}))
            .await["instanceInfo"]
            .clone();
        assert_eq!(
            held[*member].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected {holder} to still hold the ARN, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An id that names no hook execution
// ---------------------------------------------------------------------------

/// Create an application, a deployment group and a deployment on `platform`,
/// answering the minted deployment id.
async fn seeded_deployment(emulator: &Emulator, application: &str, platform: &str) -> String {
    let group = seeded_group(emulator, application, platform).await;
    let created = emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": s3_revision("my-bucket", "app.zip"),
            }),
        )
        .await;
    created["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// RULE: a `lifecycleEventHookExecutionId` that names no hook execution the
/// deployment can take a result for is refused with that class's own fixed
/// sentence — the same one whether the id was absent, empty or simply never
/// issued, and carrying neither the id nor the deployment it was reported
/// against.
#[tokio::test]
async fn an_id_naming_no_hook_execution_answers_one_fixed_sentence() {
    let emulator = Emulator::new();
    // A `Server` deployment runs its lifecycle events through the CodeDeploy
    // agent rather than through a Lambda validation function, so it issues no
    // hook execution id and NO id names a hook execution of it.
    let deployment_id = seeded_deployment(&emulator, "hook-sentence-app", "Server").await;
    // An id another deployment's hooks could have issued: reported against the
    // deployment above it still names none of ITS hook executions. Whether this
    // report landed does not matter to the row that reuses the id — the
    // deployment it is reported against is what decides.
    let elsewhere = seeded_deployment(&emulator, "hook-sentence-other-app", "Lambda").await;
    let _ = emulator
        .call(
            "PutLifecycleEventHookExecutionStatus",
            json!({
                "deploymentId": elsewhere,
                "lifecycleEventHookExecutionId": "issued-elsewhere",
                "status": "Succeeded",
            }),
        )
        .await;

    // Rows differ along the FORM of the id the report carried: omitted, empty,
    // whitespace, a plausible token, one far longer than any id, and one another
    // deployment issued.
    let ids: Vec<(&str, Option<&str>)> = vec![
        ("the member omitted", None),
        ("an empty id", Some("")),
        ("a whitespace-only id", Some("   ")),
        ("an id this deployment never issued", Some("sim2real-hook-1")),
        ("an id longer than any issued one", Some("hook-execution-0123456789abcdef")),
        ("an id another deployment issued", Some("issued-elsewhere")),
    ];

    for (case, id) in ids {
        let mut request = json!({"deploymentId": deployment_id, "status": "Succeeded"});
        if let Some(id) = id {
            request["lifecycleEventHookExecutionId"] = json!(id);
        }
        let answered = refused(
            &emulator,
            case,
            "PutLifecycleEventHookExecutionStatus",
            request,
            "InvalidLifecycleEventHookExecutionIdException",
        )
        .await;
        assert_eq!(
            answered, HOOK_EXECUTION_ID_INVALID,
            "case {case:?}: expected the class's own sentence, got {answered:?}"
        );
        assert!(
            !answered.contains(deployment_id.as_str())
                && id.is_none_or(|id| id.trim().is_empty() || !answered.contains(id)),
            "case {case:?}: the sentence names neither the id nor the deployment, \
             got {answered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A deployment whose group resolved no target
// ---------------------------------------------------------------------------

/// Create an application and a deployment group of it carrying `extras`.
async fn group_with(emulator: &Emulator, application: &str, platform: &str, extras: &Value) {
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": platform}),
        )
        .await;
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": "dg",
        "serviceRoleArn": SERVICE_ROLE,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
}

/// RULE: a deployment whose group resolves no target ends `Failed` carrying the
/// `NO_INSTANCES` error code and that failure's own fixed sentence — the same one
/// however the group was targeting and on whatever compute platform, and naming
/// neither the deployment, nor its group, nor its application.
#[tokio::test]
async fn a_deployment_that_resolves_no_target_answers_one_fixed_sentence() {
    let emulator = Emulator::new();

    // Rows differ along the compute platform and along how the group selects its
    // targets: every row selects nothing, by a different route.
    let cases: &[(&str, &str, Value)] = &[
        ("no-targeting-at-all", "Server", json!({})),
        (
            "an-ec2-tag-filter",
            "Server",
            json!({"ec2TagFilters": [filter("Name", "web", "KEY_AND_VALUE")]}),
        ),
        (
            "an-ec2-tag-set",
            "Server",
            json!({"ec2TagSet": {"ec2TagSetList": [[filter("Name", "web", "KEY_ONLY")]]}}),
        ),
        (
            "an-auto-scaling-group",
            "Server",
            json!({"autoScalingGroups": ["asg-web"]}),
        ),
        (
            "an-on-premises-filter-no-instance-carries",
            "Server",
            json!({"onPremisesInstanceTagFilters": [filter("Rack", "seven", "KEY_AND_VALUE")]}),
        ),
        ("no-ecs-service", "ECS", json!({})),
    ];

    for (case, platform, extras) in cases {
        let application = format!("no-target-{case}");
        group_with(&emulator, &application, platform, extras).await;
        let deployment_id = emulator
            .ok(
                "CreateDeployment",
                json!({
                    "applicationName": application,
                    "deploymentGroupName": "dg",
                    "revision": s3_revision("my-bucket", "app.zip"),
                }),
            )
            .await["deploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();

        let info = terminal_deployment(&emulator, &deployment_id, case).await;
        assert_eq!(
            info["status"].as_str(),
            Some("Failed"),
            "case {case:?}: expected the deployment to end Failed, got {info:?}"
        );
        assert_eq!(
            info["errorInformation"]["code"].as_str(),
            Some("NO_INSTANCES"),
            "case {case:?}: expected the NO_INSTANCES code, got {info:?}"
        );
        let answered = info["errorInformation"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            answered, NO_INSTANCES_MESSAGE,
            "case {case:?}: expected that failure's own sentence, got {answered:?}"
        );
        assert!(
            !answered.contains(&application)
                && !answered.contains("dg")
                && !answered.contains(deployment_id.as_str()),
            "case {case:?}: the sentence names neither the deployment nor its group, \
             got {answered:?}"
        );
    }
}
