// SPDX-License-Identifier: CC0-1.0

//! How a refusal built from the `[Default policies only]` parameter set is
//! COMPOSED — which halves of the set it reads, in which order, and under which
//! prefix — as `CreateLifecyclePolicy` and `UpdateLifecyclePolicy` both answer it,
//! end to end over HTTP.
//!
//! The rule under test: one request's `[Default policies only]` parameters are
//! judged as ONE set however the request states them, so both operations compose
//! their refusals the same way — the request-body members first, under their own
//! names, then the members stated inside the configuration structure, each
//! qualified by `PolicyDetails.`, every one in the order the model declares them.
//! The set feeds two refusals: a policy that is not a default policy may carry
//! none of those members (so stating one anywhere is refused), and a default
//! policy's request states each of them "either in the request body, or in the
//! PolicyDetails request structure, but not both" (so stating one parameter in
//! both places is refused, naming the body members it duplicated beside the
//! structure).
//!
//! Every case drives the router, so each one exercises routing, deserialization,
//! the handler, the persisted record and serialization — the path a client takes.
//! The rows differ along what the composition generalises over: which member,
//! which half of the set states it, how many members one request states, the
//! value's type and truthiness, a list's element count, and the order the
//! request states them in on the wire.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-north-1";

/// A destination Region for the cross-Region copy rows, matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "us-west-2";

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// A `DefaultPolicyTypeValues` member: the type of default policy the subjects
/// below target.
const DEFAULT_POLICY_TYPE: &str = "VOLUME";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The configuration structure the body parameters are an alternative to, and
/// the prefix a member stated INSIDE it is reported under.
const POLICY_DETAILS_MEMBER: &str = "PolicyDetails";
const POLICY_DETAILS_PREFIX: &str = "PolicyDetails.";

/// The request-BODY member that states a default policy's resource type:
/// `CreateLifecyclePolicyRequest$DefaultPolicy`, whose documentation is
/// `PolicyDetails$ResourceType`'s.
const DEFAULT_POLICY_MEMBER: &str = "DefaultPolicy";

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

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/policies/{id}"), None).await
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

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A custom EBS snapshot policy's `PolicyDetails`, as a client sends it: no
/// `[Default policies only]` member anywhere in it.
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

/// The `[Default policies only]` members the request BODY of both operations
/// declares, in the order the model declares them.
///
/// Source: `smithy_query(query="operation", operation="CreateLifecyclePolicy,UpdateLifecyclePolicy")`
/// — both request shapes declare `CreateInterval`, `RetainInterval`, `CopyTags`,
/// `CrossRegionCopyTargets`, `ExtendDeletion` and `Exclusions`, and each of their
/// docs opens with `[Default policies only]`.
const BODY_PARAMETERS: [&str; 6] = [
    "CreateInterval",
    "RetainInterval",
    "CopyTags",
    "CrossRegionCopyTargets",
    "ExtendDeletion",
    "Exclusions",
];

/// The same set as the `PolicyDetails` structure declares it, in the order the
/// model declares them.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")` — `ResourceType`,
/// `CreateInterval`, `RetainInterval`, `CopyTags`, `CrossRegionCopyTargets`,
/// `ExtendDeletion` and `Exclusions` each open with `[Default policies only]`.
/// `ResourceType` leads it and has no same-named body member: a create states the
/// resource type in the body as `DefaultPolicy` instead.
const DETAILS_PARAMETERS: [&str; 7] = [
    "ResourceType",
    "CreateInterval",
    "RetainInterval",
    "CopyTags",
    "CrossRegionCopyTargets",
    "ExtendDeletion",
    "Exclusions",
];

/// One request's worth of `[Default policies only]` parameters: what the case is,
/// which members it states in the request BODY, and which inside the
/// configuration structure.
struct Row {
    case: &'static str,
    body: Vec<(&'static str, Value)>,
    details: Vec<(&'static str, Value)>,
}

/// A value each member admits, used wherever a row states that member.
///
/// Every value here is a legal one — the interval bounds are the ones the member
/// docs publish (a creation frequency "can range from 1 to 7 days", a retention
/// period "from 2 to 14 days") and `CrossRegionCopyTargetList` is
/// `@length(min: 0, max: 3)` — so each case answers about the member being
/// STATED, not about its domain.
fn value_for(member: &str) -> Value {
    match member {
        "ResourceType" => json!(DEFAULT_POLICY_TYPE),
        "CreateInterval" => json!(3),
        "RetainInterval" => json!(10),
        "CopyTags" => json!(true),
        "CrossRegionCopyTargets" => json!([{"TargetRegion": OTHER_REGION}]),
        "ExtendDeletion" => json!(true),
        "Exclusions" => json!({"ExcludeVolumeTypes": ["gp3"]}),
        other => panic!("{other:?} is not a [Default policies only] member"),
    }
}

/// `members` with the value each admits, in the order given.
fn stating(members: &[&'static str]) -> Vec<(&'static str, Value)> {
    members
        .iter()
        .map(|member| (*member, value_for(member)))
        .collect()
}

/// The rows: one member in one half, then members in both halves at once, each
/// stated on the wire in an order the composition has to put back.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the creation frequency in the request body alone",
            body: stating(&["CreateInterval"]),
            details: Vec::new(),
        },
        Row {
            case: "a boolean stated false in the request body alone",
            body: vec![("CopyTags", json!(false))],
            details: Vec::new(),
        },
        Row {
            case: "an empty cross-Region copy list in the request body alone",
            body: vec![("CrossRegionCopyTargets", json!([]))],
            details: Vec::new(),
        },
        Row {
            case: "the retention period inside the configuration structure alone",
            body: Vec::new(),
            details: stating(&["RetainInterval"]),
        },
        Row {
            // The member only the structure declares, and the one that leads its
            // half of the set.
            case: "the resource type inside the configuration structure alone",
            body: Vec::new(),
            details: stating(&["ResourceType"]),
        },
        Row {
            case: "the exclusions structure inside the configuration structure alone",
            body: Vec::new(),
            details: vec![("Exclusions", json!({"ExcludeBootVolumes": true}))],
        },
        Row {
            case: "one member in each half",
            body: stating(&["ExtendDeletion"]),
            details: stating(&["CopyTags"]),
        },
        Row {
            case: "two members in each half, each pair stated out of declaration order",
            body: stating(&["RetainInterval", "CreateInterval"]),
            details: stating(&["Exclusions", "CrossRegionCopyTargets"]),
        },
        Row {
            case: "every member of both halves at once, stated in reverse declaration order",
            body: stating(&[
                "Exclusions",
                "ExtendDeletion",
                "CrossRegionCopyTargets",
                "CopyTags",
                "RetainInterval",
                "CreateInterval",
            ]),
            details: stating(&[
                "Exclusions",
                "ExtendDeletion",
                "CrossRegionCopyTargets",
                "CopyTags",
                "RetainInterval",
                "CreateInterval",
                "ResourceType",
            ]),
        },
    ]
}

/// `request` with each of `stated` set at its top level.
fn with_body(mut request: Value, stated: &[(&'static str, Value)]) -> Value {
    for (member, value) in stated {
        request[*member] = value.clone();
    }
    request
}

/// `details` with each of `stated` set inside it.
fn with_details(mut details: Value, stated: &[(&'static str, Value)]) -> Value {
    for (member, value) in stated {
        details[*member] = value.clone();
    }
    details
}

/// The names a refusal composed from this row must emit, in order: the body half
/// under the members' own names, then the structure half under
/// [`POLICY_DETAILS_PREFIX`], each half in declaration order.
fn composed_names(row: &Row) -> Vec<String> {
    let mut names: Vec<String> = BODY_PARAMETERS
        .into_iter()
        .filter(|member| row.body.iter().any(|(stated, _)| stated == member))
        .map(str::to_string)
        .collect();
    names.extend(
        DETAILS_PARAMETERS
            .into_iter()
            .filter(|member| row.details.iter().any(|(stated, _)| stated == member))
            .map(|member| format!("{POLICY_DETAILS_PREFIX}{member}")),
    );
    names
}

/// Assert `answer` is the modelled refusal.
fn assert_modelled_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status, INVALID_REQUEST_STATUS,
        "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        answer.status, answer.body
    );
    assert!(
        answer.error_code().ends_with(INVALID_REQUEST_EXCEPTION),
        "case {case:?}: expected the modelled {INVALID_REQUEST_EXCEPTION}, got {:?}",
        answer.error_code()
    );
    assert_eq!(
        at(&answer.body, "/Code"),
        json!(INVALID_PARAMETER_CODE),
        "case {case:?}: expected Code {INVALID_PARAMETER_CODE:?}, got {:?}",
        at(&answer.body, "/Code")
    );
}

/// Assert `answer`'s message names exactly `expected`, as one comma-separated run
/// in that order.
fn assert_names_composed(case: &str, expected: &[String], answer: &Answer) {
    let message = answer.message();
    let run = expected.join(", ");
    assert!(
        message.contains(&run),
        "case {case:?}: expected the refusal to name {run:?}, got {message:?}"
    );
    for member in DETAILS_PARAMETERS {
        let qualified = format!("{POLICY_DETAILS_PREFIX}{member}");
        for name in [member.to_string(), qualified] {
            if expected.contains(&name) {
                continue;
            }
            // A body name is a substring of its qualified form, so an unstated
            // name counts only outside the run the composition emitted.
            assert!(
                !message.replace(&run, "").contains(&name),
                "case {case:?}: the refusal must not name the unstated {name:?}, got {message:?}"
            );
        }
    }
}

/// Both operations compose the refusal of a policy that is NOT a default policy
/// from the same halves, in the same order, under the same prefix: for the same
/// stated members, a create of a custom policy and an update of a custom policy
/// answer the very same message.
#[tokio::test]
async fn the_create_and_the_update_compose_one_refusal_for_a_custom_policy() {
    let router = emulator();
    let custom = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a custom policy to patch",
            "State": "ENABLED",
            "PolicyDetails": custom_snapshot_details(),
        }),
    )
    .await;
    assert_eq!(
        custom.status, 200,
        "the custom subject policy must be created, got {} with {:?}",
        custom.status, custom.body
    );
    let custom = custom.policy_id();
    let stored = at(get(&router, &custom).await.policy(), "/PolicyDetails");

    for row in rows() {
        let expected = composed_names(&row);
        let mut request = with_body(
            json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "not a default policy",
                "State": "ENABLED",
            }),
            &row.body,
        );
        let mut patch = with_body(json!({}), &row.body);
        if !row.details.is_empty() {
            let details = with_details(custom_snapshot_details(), &row.details);
            request[POLICY_DETAILS_MEMBER] = details.clone();
            patch[POLICY_DETAILS_MEMBER] = details;
        }

        let before = policy_count(&router).await;
        let created = create(&router, request).await;
        let create_case = format!("{}: stated by a create of a custom policy", row.case);
        assert_modelled_refusal(&create_case, &created);
        assert_names_composed(&create_case, &expected, &created);
        assert_eq!(
            policy_count(&router).await,
            before,
            "case {create_case:?}: a refused create must leave no policy behind"
        );

        let patched = update(&router, &custom, patch).await;
        let update_case = format!("{}: patched onto a custom policy", row.case);
        assert_modelled_refusal(&update_case, &patched);
        assert_names_composed(&update_case, &expected, &patched);

        assert_eq!(
            created.message(),
            patched.message(),
            "case {:?}: the create and the update must compose one refusal; the create said \
             {:?} and the update said {:?}",
            row.case,
            created.message(),
            patched.message()
        );
        assert_eq!(
            at(get(&router, &custom).await.policy(), "/PolicyDetails"),
            stored,
            "case {:?}: a refused update must leave the stored configuration alone",
            row.case
        );
    }
}

/// And both compose the other refusal the same set feeds — a default policy's
/// configuration stated in the request body AND in the structure, member for
/// member — naming the body members it stated twice beside the structure they
/// cannot be combined with.
#[tokio::test]
async fn the_create_and_the_update_compose_one_stated_twice_refusal() {
    let router = emulator();
    let default_policy = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy to patch",
            "State": "ENABLED",
            DEFAULT_POLICY_MEMBER: DEFAULT_POLICY_TYPE,
        }),
    )
    .await;
    assert_eq!(
        default_policy.status, 200,
        "the default subject policy must be created, got {} with {:?}",
        default_policy.status, default_policy.body
    );
    let default_policy = default_policy.policy_id();
    let stored = at(get(&router, &default_policy).await.policy(), "/PolicyDetails");

    for row in rows() {
        // The body half is what this refusal names; a row stating none of it
        // states the configuration once, which the rows above cover.
        if row.body.is_empty() {
            continue;
        }
        let expected: Vec<String> = BODY_PARAMETERS
            .into_iter()
            .filter(|member| row.body.iter().any(|(stated, _)| stated == member))
            .map(str::to_string)
            .collect();

        // The create states the configuration in both places: the body members,
        // and a `SIMPLIFIED` structure restating every one of them.
        let mut create_request = with_body(
            json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "a default policy stating it twice",
                "State": "ENABLED",
            }),
            &row.body,
        );
        create_request[POLICY_DETAILS_MEMBER] = with_details(
            json!({
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
            }),
            &row.body,
        );
        let created = create(&router, create_request).await;
        let create_case = format!("{}: stated twice by a create", row.case);
        assert_modelled_refusal(&create_case, &created);
        assert_names_composed(&create_case, &expected, &created);

        let mut patch = with_body(json!({}), &row.body);
        patch[POLICY_DETAILS_MEMBER] = with_details(
            json!({
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
            }),
            &row.body,
        );
        let patched = update(&router, &default_policy, patch).await;
        let update_case = format!("{}: stated twice by an update", row.case);
        assert_modelled_refusal(&update_case, &patched);
        assert_names_composed(&update_case, &expected, &patched);

        assert_eq!(
            created.message(),
            patched.message(),
            "case {:?}: the create and the update must compose one refusal; the create said \
             {:?} and the update said {:?}",
            row.case,
            created.message(),
            patched.message()
        );

        // `MutuallyExclusiveParameters` is the structure plus those very members,
        // identically on both operations.
        let listed: Vec<Value> = std::iter::once(json!(POLICY_DETAILS_MEMBER))
            .chain(expected.iter().map(|member| json!(member)))
            .collect();
        for (case, answer) in [(&create_case, &created), (&update_case, &patched)] {
            let reported = at(&answer.body, "/MutuallyExclusiveParameters");
            assert_eq!(
                reported,
                Value::Array(listed.clone()),
                "case {case:?}: expected MutuallyExclusiveParameters {:?}, got {reported:?}",
                Value::Array(listed.clone())
            );
        }
        assert_eq!(
            at(get(&router, &default_policy).await.policy(), "/PolicyDetails"),
            stored,
            "case {:?}: a refused update must leave the stored configuration alone",
            row.case
        );
    }
}

/// The one asymmetry of that composition: the resource type is stated in the body
/// as `DefaultPolicy`, so a CREATE naming it beside a structure that also states
/// `ResourceType` states it twice and the refusal leads with that body member. An
/// update's body declares no such member, so a structure restating the resource
/// type states it once and the update is applied.
#[tokio::test]
async fn the_resource_type_is_stated_twice_only_where_the_body_declares_it() {
    let router = emulator();

    let before = policy_count(&router).await;
    let case = "the resource type stated in the body and in the structure by a create";
    let created = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy naming the resource type twice",
            "State": "ENABLED",
            DEFAULT_POLICY_MEMBER: DEFAULT_POLICY_TYPE,
            POLICY_DETAILS_MEMBER: {
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
            },
        }),
    )
    .await;
    assert_modelled_refusal(case, &created);
    assert!(
        created.message().contains(DEFAULT_POLICY_MEMBER),
        "case {case:?}: the refusal must name the body member {DEFAULT_POLICY_MEMBER:?}, got {:?}",
        created.message()
    );
    assert_eq!(
        at(&created.body, "/MutuallyExclusiveParameters"),
        json!([POLICY_DETAILS_MEMBER, DEFAULT_POLICY_MEMBER]),
        "case {case:?}: expected the structure and the body member, got {:?}",
        at(&created.body, "/MutuallyExclusiveParameters")
    );
    assert_eq!(
        policy_count(&router).await,
        before,
        "case {case:?}: a refused create must leave no policy behind"
    );

    // The same resource type, stated once: by the body member on the create, and
    // by the structure on the update that follows.
    let subject = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy to patch",
            "State": "ENABLED",
            DEFAULT_POLICY_MEMBER: DEFAULT_POLICY_TYPE,
        }),
    )
    .await;
    assert_eq!(
        subject.status, 200,
        "the default subject policy must be created, got {} with {:?}",
        subject.status, subject.body
    );
    let subject = subject.policy_id();

    let case = "the resource type restated by an update's structure, with no body member";
    let patched = update(
        &router,
        &subject,
        json!({
            POLICY_DETAILS_MEMBER: {
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
                "CopyTags": true,
            },
        }),
    )
    .await;
    assert_eq!(
        patched.status, 200,
        "case {case:?}: expected HTTP 200, got {} with {:?}",
        patched.status, patched.body
    );
    let read = get(&router, &subject).await;
    for (pointer, expected) in [
        ("/DefaultPolicy", json!(true)),
        ("/PolicyDetails/ResourceType", json!(DEFAULT_POLICY_TYPE)),
        ("/PolicyDetails/CopyTags", json!(true)),
    ] {
        assert_eq!(
            at(read.policy(), pointer),
            expected,
            "case {case:?}: expected {pointer} to report {expected:?}, got {:?}",
            at(read.policy(), pointer)
        );
    }
}
