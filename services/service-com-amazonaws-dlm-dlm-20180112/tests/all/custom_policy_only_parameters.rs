// SPDX-License-Identifier: CC0-1.0

//! What the control plane does with a configuration member the model documents
//! for a kind of CUSTOM policy only, end to end over HTTP.
//!
//! The rule under test is the mirror image of the one
//! `default_policy_only_parameters.rs` pins: a `PolicyDetails` member the
//! documentation gives to one kind of policy is accepted only by a request for a
//! policy of that kind, so a request creating or updating a DEFAULT policy that
//! states a member a custom policy alone is configured from is refused with
//! `InvalidRequestException` (HTTP 400) and stores nothing, while a custom policy
//! states it and reports it back. A default policy therefore states its target in
//! `PolicyDetails$ResourceType` and nowhere else: one stored copy, reported once,
//! selected by the `resourceTypes` filter under that one type, and accepted back
//! unchanged by an update of the configuration a read reported.
//!
//! Which members those are, from `smithy_query(query="shape",
//! shape="PolicyDetails")`: `ResourceTypes` ("[Custom snapshot policies only]"),
//! `ResourceLocations`, `TargetTags`, `Schedules` and `Parameters` ("[Custom
//! snapshot and AMI policies only]"), `EventSource` and `Actions` ("[Event-based
//! policies only]").
//!
//! Every case drives the router, so each one exercises routing, deserialization,
//! the handler, the persisted record and serialization — the path a client takes.
//! The rows span the whole member set and differ along what the rule generalises
//! over: which member, the value's type (a list, a structure, a list of
//! structures), the value itself, how many of the members one request states,
//! which kind of default policy the request is for and where it stated the
//! policy's kind, and whether the request is a create or an update.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "ap-south-1";

/// A destination Region for the event-based policy's copy action, matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "us-west-2";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// Every `DefaultPolicyTypeValues` member, from `smithy_query(query="shape",
/// shape="DefaultPolicyTypeValues")`: the two kinds of default policy
/// `DefaultPolicy` offers.
const DEFAULT_POLICY_TYPE_VALUES: &[&str] = &["VOLUME", "INSTANCE"];

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    fn policy_id(&self) -> String {
        self.body["PolicyId"]
            .as_str()
            .unwrap_or_default()
            .to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    fn message(&self) -> &str {
        self.body["Message"].as_str().unwrap_or_default()
    }
}

/// A SigV4 credential scope naming this service — what routes the request to dlm.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/policies", Some(request)).await
}

/// A create that must be accepted, answering the new policy's id.
async fn create_ok(router: &MultiServiceRouter, case: &str, request: Value) -> String {
    let answer = create(router, request).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer.policy_id()
}

async fn get(router: &MultiServiceRouter, case: &str, id: &str) -> Answer {
    let answer = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
        answer.status, answer.body
    );
    answer
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

/// How many policies the store holds, read through `GetLifecyclePolicies`.
async fn policy_count(router: &MultiServiceRouter) -> usize {
    let answer = call(router, "GET", "/policies", None).await;
    answer.body["Policies"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default()
}

/// The `LifecyclePolicySummary` `GetLifecyclePolicies` reports for `id` under
/// `query`, or `Value::Null` when the filter does not select it.
async fn summary(router: &MultiServiceRouter, case: &str, query: &str, id: &str) -> Value {
    let answer = call(router, "GET", &format!("/policies{query}"), None).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected HTTP 200 from GetLifecyclePolicies, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["Policies"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|policy| policy["PolicyId"] == json!(id))
        .cloned()
        .unwrap_or(Value::Null)
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A complete custom snapshot policy configuration — the kind of policy the
/// snapshot and AMI members belong to. The creation interval stays inside the
/// supported set (`interval_validation.rs` owns that rule).
fn custom_snapshot_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A complete event-based policy configuration — the kind the event members
/// belong to.
fn event_based_details() -> Value {
    json!({
        "PolicyType": "EVENT_BASED_POLICY",
        "EventSource": {
            "Type": "MANAGED_CWE",
            "Parameters": {
                "EventType": "shareSnapshot",
                "SnapshotOwner": ["000000000000"],
                "DescriptionRegex": "^share-.*$",
            },
        },
        "Actions": [{
            "Name": "copy",
            "CrossRegionCopy": [{
                "Target": OTHER_REGION,
                "EncryptionConfiguration": {"Encrypted": false},
                "RetainRule": {"Interval": 1, "IntervalUnit": "DAYS"},
            }],
        }],
    })
}

/// One configuration member a CUSTOM policy alone is configured from, with a
/// value for it and the kind of custom policy it belongs to.
struct Member {
    /// The case name: the member plus what is particular about this value.
    case: &'static str,
    /// The member's wire name inside `PolicyDetails`.
    member: &'static str,
    /// The value a request states for it.
    value: Value,
    /// A complete configuration of the kind of CUSTOM policy that does take this
    /// member, which the accepted side states the value inside.
    custom_details: fn() -> Value,
    /// True when a read of that custom policy reports this very value at
    /// `/PolicyDetails/<member>`; false for a member the service materializes
    /// further members onto (`policy_read_projection.rs` owns that rule).
    reported_verbatim: bool,
}

/// Every `PolicyDetails` member the model documents for a kind of custom policy
/// only, each with a value its target shape admits.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")` — `ResourceTypes`
/// opens "[Custom snapshot policies only]", `ResourceLocations`, `TargetTags`,
/// `Schedules` and `Parameters` open "[Custom snapshot and AMI policies only]",
/// and `EventSource` and `Actions` open "[Event-based policies only]".
/// `ResourceTypeValuesList` and `ResourceLocationList` are both
/// `@length(min: 1, max: 1)`, so each states one element.
fn custom_policy_only_members() -> Vec<Member> {
    vec![
        Member {
            case: "ResourceTypes targeting VOLUME",
            member: "ResourceTypes",
            value: json!(["VOLUME"]),
            custom_details: custom_snapshot_details,
            reported_verbatim: true,
        },
        Member {
            // The other `ResourceTypeValues` member: the value the issue did not
            // cite, and the one that disagrees with the default policy's own
            // target below.
            case: "ResourceTypes targeting INSTANCE",
            member: "ResourceTypes",
            value: json!(["INSTANCE"]),
            custom_details: custom_snapshot_details,
            reported_verbatim: true,
        },
        Member {
            case: "ResourceLocations in the Region",
            member: "ResourceLocations",
            value: json!(["CLOUD"]),
            custom_details: custom_snapshot_details,
            reported_verbatim: true,
        },
        Member {
            case: "ResourceLocations on an Outpost",
            member: "ResourceLocations",
            value: json!(["OUTPOST"]),
            custom_details: custom_snapshot_details,
            reported_verbatim: true,
        },
        Member {
            case: "TargetTags",
            member: "TargetTags",
            value: json!([{"Key": "backup", "Value": "daily"}]),
            custom_details: custom_snapshot_details,
            reported_verbatim: true,
        },
        Member {
            case: "Schedules",
            member: "Schedules",
            value: json!([{
                "Name": "daily",
                "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"},
                "RetainRule": {"Count": 7},
            }]),
            custom_details: custom_snapshot_details,
            // A stored schedule reports the members the service materializes onto
            // it as well, so the value does not come back verbatim.
            reported_verbatim: false,
        },
        Member {
            case: "Parameters excluding the boot volume",
            member: "Parameters",
            value: json!({"ExcludeBootVolume": true}),
            custom_details: custom_snapshot_details,
            reported_verbatim: true,
        },
        Member {
            case: "EventSource",
            member: "EventSource",
            value: event_based_details()["EventSource"].clone(),
            custom_details: event_based_details,
            reported_verbatim: true,
        },
        Member {
            case: "Actions",
            member: "Actions",
            value: event_based_details()["Actions"].clone(),
            custom_details: event_based_details,
            reported_verbatim: true,
        },
    ]
}

/// `details` with `member` stated inside it.
fn stating(details: &Value, member: &str, value: &Value) -> Value {
    let mut details = details.clone();
    details[member] = value.clone();
    details
}

/// The refusal wording this service answers for `parameters` stated by a request
/// for a default policy: the parameters a custom policy alone takes, each named
/// by where the caller stated it.
fn custom_only_refusal(parameters: &[&str]) -> String {
    let (parameter, is) = if parameters.len() == 1 {
        ("parameter", "is")
    } else {
        ("parameters", "are")
    };
    format!(
        "The {parameter} {} {is} supported for custom policies only.",
        parameters
            .iter()
            .map(|member| format!("PolicyDetails.{member}"))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Assert `answer` is the modelled refusal naming exactly `parameters`.
fn assert_refused(case: &str, parameters: &[&str], answer: &Answer) {
    assert_eq!(
        answer.status, INVALID_REQUEST_STATUS,
        "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
        "case {case:?}: expected {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        at(&answer.body, "/Code"),
        json!(INVALID_PARAMETER_CODE),
        "case {case:?}: expected the {INVALID_PARAMETER_CODE:?} code, got {:?}",
        at(&answer.body, "/Code")
    );
    let expected = custom_only_refusal(parameters);
    assert_eq!(
        answer.message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        answer.message()
    );
}

/// A create of a default policy of the kind `target` names, stating its kind in
/// the request body as `DefaultPolicy`, with `details` as its configuration.
fn default_create_from_body(target: &str, details: Value) -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a default policy from the body",
        "State": "ENABLED",
        "DefaultPolicy": target,
        "PolicyDetails": details,
    })
}

/// The same, stating the kind inside a `SIMPLIFIED` configuration structure.
fn default_create_from_details(target: &str, details: Value) -> Value {
    let mut details = details;
    details["PolicyLanguage"] = json!("SIMPLIFIED");
    details["ResourceType"] = json!(target);
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a default policy from PolicyDetails",
        "State": "ENABLED",
        "PolicyDetails": details,
    })
}

/// A create that states `member` for a DEFAULT policy, in each shape a client can
/// send one: both kinds of default policy, and the two places a request states
/// which kind it asks for.
fn default_create_requests(member: &Member) -> Vec<(String, Value)> {
    let mut requests = Vec::new();
    for target in DEFAULT_POLICY_TYPE_VALUES {
        let details = stating(&json!({}), member.member, &member.value);
        requests.push((
            format!(
                "{}: on a default {target} policy stating its kind as DefaultPolicy",
                member.case
            ),
            default_create_from_body(target, details.clone()),
        ));
        requests.push((
            format!(
                "{}: on a default {target} policy stating its kind inside PolicyDetails",
                member.case
            ),
            default_create_from_details(target, details),
        ));
    }
    requests
}

/// A member a custom policy alone is configured from, stated by a create for a
/// DEFAULT policy, is refused and no policy is minted for it.
#[tokio::test]
async fn a_create_refuses_custom_only_parameters_for_a_default_policy() {
    let router = emulator();
    for member in custom_policy_only_members() {
        for (case, request) in default_create_requests(&member) {
            let before = policy_count(&router).await;
            let answer = create(&router, request).await;
            assert_refused(&case, &[member.member], &answer);
            assert_eq!(
                policy_count(&router).await,
                before,
                "case {case:?}: a refused create must leave no policy behind"
            );
        }
    }
}

/// A request stating SEVERAL of them is refused naming every one it stated, in
/// the order `PolicyDetails` declares them.
#[tokio::test]
async fn the_refusal_names_every_custom_only_parameter_the_request_stated() {
    let router = emulator();
    let case = "a default policy stating a whole custom snapshot configuration";
    let before = policy_count(&router).await;
    let answer = create(
        &router,
        default_create_from_body("VOLUME", custom_snapshot_details()),
    )
    .await;
    assert_refused(
        case,
        &["ResourceTypes", "TargetTags", "Schedules"],
        &answer,
    );
    assert_eq!(
        policy_count(&router).await,
        before,
        "case {case:?}: a refused create must leave no policy behind"
    );
}

/// The other side of the rule: a CUSTOM policy of the kind each member belongs to
/// states it and reports it back.
#[tokio::test]
async fn a_custom_policy_states_and_reports_every_custom_only_parameter() {
    let router = emulator();
    for member in custom_policy_only_members() {
        let case = format!("{}: on a custom policy of its own kind", member.case);
        let case = case.as_str();
        let details = stating(&(member.custom_details)(), member.member, &member.value);
        let id = create_ok(
            &router,
            case,
            json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "a custom policy",
                "State": "ENABLED",
                "PolicyDetails": details,
            }),
        )
        .await;
        let read = get(&router, case, &id).await;
        assert_eq!(
            at(read.policy(), "/DefaultPolicy"),
            json!(false),
            "case {case:?}: the policy must be reported as a custom policy, got {:?}",
            at(read.policy(), "/DefaultPolicy")
        );
        let pointer = format!("/PolicyDetails/{}", member.member);
        if member.reported_verbatim {
            assert_eq!(
                at(read.policy(), &pointer),
                member.value,
                "case {case:?}: expected {pointer} to report {:?}, got {:?}",
                member.value,
                at(read.policy(), &pointer)
            );
        } else {
            assert_ne!(
                at(read.policy(), &pointer),
                Value::Null,
                "case {case:?}: expected {pointer} to be reported"
            );
        }
    }
}

/// `UpdateLifecyclePolicy` follows the same rule: a replacement configuration
/// stating one of those members for a DEFAULT policy is refused and the stored
/// policy is left exactly as it was, while the same member is applied to a custom
/// policy of its own kind.
#[tokio::test]
async fn an_update_refuses_custom_only_parameters_for_a_default_policy() {
    let router = emulator();
    for member in custom_policy_only_members() {
        for target in DEFAULT_POLICY_TYPE_VALUES {
            let case = format!("{}: patched onto a default {target} policy", member.case);
            let case = case.as_str();
            let id = create_ok(
                &router,
                case,
                json!({
                    "ExecutionRoleArn": role_arn(),
                    "Description": "a default policy to patch",
                    "State": "ENABLED",
                    "DefaultPolicy": target,
                }),
            )
            .await;
            let before = get(&router, case, &id).await;

            let replacement = stating(
                &json!({"PolicyLanguage": "SIMPLIFIED", "ResourceType": target}),
                member.member,
                &member.value,
            );
            let answer = update(&router, &id, json!({"PolicyDetails": replacement})).await;
            assert_refused(case, &[member.member], &answer);
            assert_eq!(
                get(&router, case, &id).await.policy(),
                before.policy(),
                "case {case:?}: a refused update must leave the stored policy exactly as it was"
            );
        }

        // The same member stated for a CUSTOM policy of the kind it belongs to is
        // accepted by an update too, so the refusal above is about the kind of
        // policy addressed and not about the member being unwritable. The policy
        // is created stating this row's value, because an update may not change
        // what a policy targets (`policy_identity_immutability.rs` owns that
        // rule).
        let case = format!("{}: patched onto a custom policy of its own kind", member.case);
        let case = case.as_str();
        let details = stating(&(member.custom_details)(), member.member, &member.value);
        let id = create_ok(
            &router,
            case,
            json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "a custom policy to patch",
                "State": "ENABLED",
                "PolicyDetails": details.clone(),
            }),
        )
        .await;
        let answer = update(&router, &id, json!({"PolicyDetails": details})).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let pointer = format!("/PolicyDetails/{}", member.member);
        let read = get(&router, case, &id).await;
        assert_ne!(
            at(read.policy(), &pointer),
            Value::Null,
            "case {case:?}: expected {pointer} to be reported after the update"
        );
    }
}

/// What follows from the rule for a DEFAULT policy: it holds ONE copy of its
/// target type, reports it as `PolicyDetails$ResourceType` and nowhere else, is
/// selected by the `resourceTypes` filter under that one type and under no other,
/// and the configuration a read reports is accepted back by an update — a
/// read-modify-write of the policy.
#[tokio::test]
async fn a_default_policy_states_its_target_once_and_survives_a_read_modify_write() {
    let router = emulator();
    for target in DEFAULT_POLICY_TYPE_VALUES {
        for (where_stated, request) in [
            (
                "stating its kind as DefaultPolicy",
                default_create_from_body(target, json!({})),
            ),
            (
                "stating its kind inside PolicyDetails",
                default_create_from_details(target, json!({})),
            ),
        ] {
            let case = format!("a default {target} policy {where_stated}");
            let case = case.as_str();
            let id = create_ok(&router, case, request).await;

            let read = get(&router, case, &id).await;
            assert_eq!(
                at(read.policy(), "/PolicyDetails/ResourceType"),
                json!(target),
                "case {case:?}: expected ResourceType {target:?}, got {:?}",
                at(read.policy(), "/PolicyDetails/ResourceType")
            );
            assert_eq!(
                at(read.policy(), "/PolicyDetails/ResourceTypes"),
                Value::Null,
                "case {case:?}: a default policy reports no ResourceTypes, got {:?}",
                at(read.policy(), "/PolicyDetails/ResourceTypes")
            );

            // The filter selects it under its one target type and under no other
            // member of the catalog.
            for candidate in DEFAULT_POLICY_TYPE_VALUES {
                let selected =
                    summary(&router, case, &format!("?resourceTypes={candidate}"), &id).await;
                let expected_selected = candidate == target;
                assert_eq!(
                    selected != Value::Null,
                    expected_selected,
                    "case {case:?}: resourceTypes={candidate} must {}select the policy, got {:?}",
                    if expected_selected { "" } else { "not " },
                    selected
                );
            }

            // The configuration the read reported, sent straight back.
            let reported = at(read.policy(), "/PolicyDetails");
            let answer = update(&router, &id, json!({"PolicyDetails": reported.clone()})).await;
            assert_eq!(
                answer.status, 200,
                "case {case:?}: the configuration the read reported must be accepted back, got \
                 {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                at(get(&router, case, &id).await.policy(), "/PolicyDetails"),
                reported,
                "case {case:?}: a read-modify-write must leave the configuration as it was"
            );
        }
    }
}
