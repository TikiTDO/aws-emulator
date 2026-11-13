// SPDX-License-Identifier: CC0-1.0

//! WHICH request-body members are the `[Default policies only]` parameters, as
//! `CreateLifecyclePolicy` and `UpdateLifecyclePolicy` both answer it, end to end
//! over HTTP.
//!
//! The rule under test: the two operations declare the same body members as the
//! alternative to the `PolicyDetails` configuration structure, so that set is ONE
//! set — every refusal built from it (a default-only parameter stated for a policy
//! that is not a default policy, and a configuration stated both in the body and in
//! the structure) names the same members, in the order the model declares them, on
//! the create and on the update alike. A body member the model does not document
//! `[Default policies only]` is not in the set and neither operation refuses it.
//!
//! Every case drives the router, so each one exercises routing, deserialization,
//! the handler, the persisted record and serialization — the path a client takes.
//!
//! The rows span what the rule generalises over: which member, the value's type
//! (integer, boolean, structure, list), both truth values of a boolean (a stated
//! `false` is still stated), a list's element count from the empty list to the
//! model's maximum, each published interval bound, how many members one request
//! states (one, through all six at once), and which of the two operations states
//! them.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "ap-southeast-2";

/// Destination Regions for the cross-Region copy rows, each matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "us-west-2";
const THIRD_REGION: &str = "eu-central-1";
const FOURTH_REGION: &str = "ap-south-1";

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The type of default policy the stated-twice subjects target, a
/// `DefaultPolicyTypeValues` member.
const DEFAULT_POLICY_TYPE: &str = "VOLUME";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The configuration structure the body parameters are an alternative to.
const POLICY_DETAILS_MEMBER: &str = "PolicyDetails";

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
/// `CrossRegionCopyTargets`, `ExtendDeletion` and `Exclusions`, and
/// `smithy_query(query="shape", shape="PolicyDetails")` opens every one of their
/// docs with `[Default policies only]`. `PolicyDetails$ResourceType` is NOT among
/// them: it has no same-named body member (the create states the type of default
/// policy as `DefaultPolicy`, and an update may not change it), which
/// `default_policy_resource_type.rs` owns.
const BODY_PARAMETERS: [&str; 6] = [
    "CreateInterval",
    "RetainInterval",
    "CopyTags",
    "CrossRegionCopyTargets",
    "ExtendDeletion",
    "Exclusions",
];

/// One request's worth of body parameters: what the case is, and the members it
/// states with their values.
struct Row {
    case: String,
    stated: Vec<(&'static str, Value)>,
}

/// The rows: every body member on its own, over values spanning what its target
/// shape admits, then all six at once.
///
/// The interval values are the bounds the member docs publish — a creation
/// frequency "can range from 1 to 7 days", a retention period "from 2 to 14 days"
/// — and `CrossRegionCopyTargetList` is `@length(min: 0, max: 3)`, so the empty
/// list and three destinations are both values it admits. Whether a value is
/// INSIDE those domains is `interval_validation.rs`' rule, not this one: every
/// value here is a legal one, so each case answers about the member being stated.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();
    let values: Vec<(&'static str, &'static str, Value)> = vec![
        ("CreateInterval", "at the documented minimum of 1 day", json!(1)),
        ("CreateInterval", "at the documented maximum of 7 days", json!(7)),
        (
            "RetainInterval",
            "at the documented minimum of 2 days",
            json!(2),
        ),
        (
            "RetainInterval",
            "at the documented maximum of 14 days",
            json!(14),
        ),
        ("CopyTags", "true", json!(true)),
        // A stated `false` is still stated: the set is about which members the
        // request carries, not about the values they carry.
        ("CopyTags", "false", json!(false)),
        ("ExtendDeletion", "true", json!(true)),
        ("ExtendDeletion", "false", json!(false)),
        (
            "CrossRegionCopyTargets",
            "with one destination Region",
            json!([{"TargetRegion": OTHER_REGION}]),
        ),
        (
            "CrossRegionCopyTargets",
            "with the maximum of three destination Regions",
            json!([
                {"TargetRegion": OTHER_REGION},
                {"TargetRegion": THIRD_REGION},
                {"TargetRegion": FOURTH_REGION},
            ]),
        ),
        (
            "CrossRegionCopyTargets",
            "stated as the empty list",
            json!([]),
        ),
        (
            "Exclusions",
            "excluding boot volumes",
            json!({"ExcludeBootVolumes": true}),
        ),
        (
            "Exclusions",
            "excluding volume types",
            json!({"ExcludeVolumeTypes": ["gp2", "io1"]}),
        ),
    ];
    for (member, what, value) in values {
        rows.push(Row {
            case: format!("{member} {what}"),
            stated: vec![(member, value)],
        });
    }
    // Two members at once, then the whole set: a request may state any number of
    // them, and the refusal names exactly those, in declaration order. The pair
    // is stated out of declaration order on the wire, which only the one shared
    // list can put back in order.
    rows.push(Row {
        case: "the retention period and the creation frequency, stated in that order".to_string(),
        stated: vec![("RetainInterval", json!(10)), ("CreateInterval", json!(3))],
    });
    rows.push(Row {
        case: "every body parameter at once".to_string(),
        stated: whole_set(),
    });
    rows
}

/// All six body parameters with a legal value each, stated in REVERSE
/// declaration order so the order a refusal reports is the shared list's, not the
/// request's.
fn whole_set() -> Vec<(&'static str, Value)> {
    vec![
        (
            "Exclusions",
            json!({"ExcludeVolumeTypes": ["gp3"]}),
        ),
        ("ExtendDeletion", json!(true)),
        (
            "CrossRegionCopyTargets",
            json!([{"TargetRegion": OTHER_REGION}]),
        ),
        ("CopyTags", json!(true)),
        ("RetainInterval", json!(10)),
        ("CreateInterval", json!(3)),
    ]
}

/// `request` with each of `stated` set at its top level.
fn with_body_parameters(mut request: Value, stated: &[(&'static str, Value)]) -> Value {
    for (member, value) in stated {
        request[*member] = value.clone();
    }
    request
}

/// `details` with each of `stated` set INSIDE it — the OTHER place a request can
/// state the same parameter, which is what makes it stated twice.
fn with_details_parameters(mut details: Value, stated: &[(&'static str, Value)]) -> Value {
    for (member, value) in stated {
        details[*member] = value.clone();
    }
    details
}

/// The members of `stated`, in the order the model declares them — what a
/// refusal built from the one shared set must name.
fn declared_order(stated: &[(&'static str, Value)]) -> Vec<&'static str> {
    BODY_PARAMETERS
        .into_iter()
        .filter(|member| stated.iter().any(|(stated, _)| stated == member))
        .collect()
}

/// Assert `answer` is the modelled refusal, and that `message` names exactly the
/// members of `stated`, in declaration order.
fn assert_refusal_names(case: &str, stated: &[(&'static str, Value)], answer: &Answer) {
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

    let message = answer.message();
    let mut previous = 0usize;
    for member in declared_order(stated) {
        let found = message.find(member).unwrap_or_else(|| {
            panic!("case {case:?}: the refusal must name {member:?}, got {message:?}")
        });
        assert!(
            found >= previous,
            "case {case:?}: expected {member:?} after position {previous}, got {found} in \
             {message:?}"
        );
        previous = found;
    }
    // And no member the request did NOT state.
    for member in BODY_PARAMETERS {
        if stated.iter().any(|(stated, _)| *stated == member) {
            continue;
        }
        assert!(
            !message.contains(member),
            "case {case:?}: the refusal must not name the unstated {member:?}, got {message:?}"
        );
    }
}

/// Both operations read ONE body-parameter set: for the same stated members, the
/// create of a custom policy and the update of a custom policy refuse with the
/// very same `InvalidRequestException` message.
#[tokio::test]
async fn the_create_and_the_update_refuse_the_same_body_parameter_set() {
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
        let created = create(
            &router,
            with_body_parameters(
                json!({
                    "ExecutionRoleArn": role_arn(),
                    "Description": "not a default policy",
                    "State": "ENABLED",
                }),
                &row.stated,
            ),
        )
        .await;
        let create_case = format!("{}: stated by a create of a custom policy", row.case);
        assert_refusal_names(&create_case, &row.stated, &created);

        let patched = update(
            &router,
            &custom,
            with_body_parameters(json!({}), &row.stated),
        )
        .await;
        let update_case = format!("{}: patched onto a custom policy", row.case);
        assert_refusal_names(&update_case, &row.stated, &patched);

        assert_eq!(
            created.message(),
            patched.message(),
            "case {:?}: the create and the update must refuse the same set; the create said \
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

/// The other refusal the same set feeds: a default policy's configuration stated
/// in the request body AND in `PolicyDetails`. Every row states each member in
/// BOTH places — the two are alternatives per member — and the create and the
/// update name the same body parameters beside the structure they cannot be
/// combined with.
#[tokio::test]
async fn the_create_and_the_update_name_the_same_set_when_the_configuration_is_stated_twice() {
    let router = emulator();
    let default_policy = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy to patch",
            "State": "ENABLED",
            "DefaultPolicy": DEFAULT_POLICY_TYPE,
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
        // The create states the configuration in both places: the body members,
        // and a `SIMPLIFIED` structure — which is what makes it a default policy.
        let mut create_request = with_body_parameters(
            json!({
                "ExecutionRoleArn": role_arn(),
                "Description": "a default policy stating it twice",
                "State": "ENABLED",
            }),
            &row.stated,
        );
        create_request[POLICY_DETAILS_MEMBER] = with_details_parameters(
            json!({
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
            }),
            &row.stated,
        );
        let created = create(&router, create_request).await;
        let create_case = format!("{}: stated twice by a create", row.case);
        assert_refusal_names(&create_case, &row.stated, &created);

        let mut patch = with_body_parameters(json!({}), &row.stated);
        patch[POLICY_DETAILS_MEMBER] = with_details_parameters(
            json!({
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
            }),
            &row.stated,
        );
        let patched = update(&router, &default_policy, patch).await;
        let update_case = format!("{}: stated twice by an update", row.case);
        assert_refusal_names(&update_case, &row.stated, &patched);

        assert_eq!(
            created.message(),
            patched.message(),
            "case {:?}: the create and the update must name the same set; the create said {:?} \
             and the update said {:?}",
            row.case,
            created.message(),
            patched.message()
        );

        // `MutuallyExclusiveParameters` is the structure plus those very members,
        // identically on both operations.
        let expected: Vec<Value> = std::iter::once(json!(POLICY_DETAILS_MEMBER))
            .chain(
                declared_order(&row.stated)
                    .into_iter()
                    .map(|member| json!(member)),
            )
            .collect();
        for (case, answer) in [(&create_case, &created), (&update_case, &patched)] {
            let listed = at(&answer.body, "/MutuallyExclusiveParameters");
            assert_eq!(
                listed,
                Value::Array(expected.clone()),
                "case {case:?}: expected MutuallyExclusiveParameters {:?}, got {listed:?}",
                Value::Array(expected.clone())
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

/// The set's boundary: a body member the model does not document `[Default
/// policies only]` is not in it, so neither operation refuses one for a policy
/// that is not a default policy — and the update takes effect.
#[tokio::test]
async fn a_body_member_outside_the_set_is_refused_by_neither_operation() {
    let router = emulator();
    let created = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a custom policy",
            "State": "ENABLED",
            "Tags": {"owner": "platform"},
            "PolicyDetails": custom_snapshot_details(),
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "a custom create stating no default-only parameter must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let id = created.policy_id();

    for (case, patch, pointer, expected) in [
        (
            "Description",
            json!({"Description": "patched description"}),
            "/Description",
            json!("patched description"),
        ),
        (
            "State",
            json!({"State": "DISABLED"}),
            "/State",
            json!("DISABLED"),
        ),
        (
            "ExecutionRoleArn",
            json!({"ExecutionRoleArn": role_arn()}),
            "/ExecutionRoleArn",
            json!(role_arn()),
        ),
    ] {
        let patched = update(&router, &id, patch).await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: a body member outside the set must be accepted, got {} with {:?}",
            patched.status, patched.body
        );
        let read = get(&router, &id).await;
        assert_eq!(
            at(read.policy(), pointer),
            expected,
            "case {case:?}: expected {pointer} to report {expected:?}, got {:?}",
            at(read.policy(), pointer)
        );
    }
}
