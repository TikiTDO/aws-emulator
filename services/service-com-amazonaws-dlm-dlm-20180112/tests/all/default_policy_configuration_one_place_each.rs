// SPDX-License-Identifier: CC0-1.0

//! A default policy's `[Default policies only]` configuration SPLIT across its
//! two places, end to end over HTTP.
//!
//! The rule under test: the two places a default policy's configuration may be
//! stated in — the request body and the `PolicyDetails` structure — are
//! alternatives PER PARAMETER, so a request is refused for exactly the
//! parameters it states in both places, and a request that states each parameter
//! it carries in exactly ONE place is accepted however the parameters are split
//! between the two, whatever the other place states. Every parameter then takes
//! the value from whichever place stated it, and a parameter neither place
//! states takes the default its own documentation publishes.
//!
//! That is the difference the issue reported: a request carrying one body
//! parameter beside a structure that states a DIFFERENT one states each of them
//! once, so it is an accepted request, and the refusal of a request that really
//! does state one twice names that parameter alone.
//!
//! Every case drives the router, so each one exercises routing,
//! deserialization, the handler, the persisted record and serialization — the
//! path a client takes, and the only path on which a request answered 200 can be
//! seen to have taken effect.
//!
//! The rows span what the rule generalises over: which parameter goes in which
//! place, how many go in each (one, three, all six, none), the place the resource
//! type itself is stated in, the parameter's type (integer, boolean, list,
//! structure), both truth values of each boolean, both published interval bounds,
//! a list's element count, and the order the parameters are stated in on the wire.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "ca-central-1";

/// Destination Regions for the cross-Region copy values, each matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "us-west-2";
const THIRD_REGION: &str = "ap-south-1";
const FOURTH_REGION: &str = "eu-west-3";

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// A `DefaultPolicyTypeValues` member: the type of default policy every subject
/// below targets.
const DEFAULT_POLICY_TYPE: &str = "VOLUME";

/// The configuration structure the body parameters are an alternative to, the
/// structure member that states the resource type, and the BODY member that
/// states the same thing (`CreateLifecyclePolicyRequest$DefaultPolicy`).
const POLICY_DETAILS_MEMBER: &str = "PolicyDetails";
const RESOURCE_TYPE_MEMBER: &str = "ResourceType";
const DEFAULT_POLICY_MEMBER: &str = "DefaultPolicy";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The `[Default policies only]` parameters both request bodies and the
/// `PolicyDetails` structure declare, in the order the model declares them.
///
/// Source: `smithy_query(query="operation", operation="CreateLifecyclePolicy,UpdateLifecyclePolicy")`
/// read against `smithy_query(query="shape", shape="PolicyDetails")` —
/// `CreateInterval`, `RetainInterval`, `CopyTags`, `CrossRegionCopyTargets`,
/// `ExtendDeletion` and `Exclusions` are declared in both places and each of
/// their docs opens with `[Default policies only]`. `ResourceType` is declared by
/// the structure alone; the body states it as [`DEFAULT_POLICY_MEMBER`].
const SHARED_PARAMETERS: [&str; 6] = [
    "CreateInterval",
    "RetainInterval",
    "CopyTags",
    "CrossRegionCopyTargets",
    "ExtendDeletion",
    "Exclusions",
];

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

/// The documented default a `[Default policies only]` parameter takes when
/// neither place states a value: `CreateInterval` "If you do not specify a value,
/// the default is 1", `RetainInterval` 7, `CopyTags` and `ExtendDeletion` "the
/// default is false", and a policy with no destination list makes no cross-Region
/// copies and excludes nothing
/// (`smithy_query(query="shape", shape="PolicyDetails")`).
fn documented_default(member: &str) -> Value {
    match member {
        "CreateInterval" => json!(1),
        "RetainInterval" => json!(7),
        "CopyTags" | "ExtendDeletion" => json!(false),
        "CrossRegionCopyTargets" | "Exclusions" => Value::Null,
        other => panic!("{other:?} is not a [Default policies only] parameter"),
    }
}

/// Which of the two value sets a row states its parameters with.
#[derive(Debug, Clone, Copy)]
enum Values {
    /// The first set.
    First,
    /// A second set, differing from the first in every parameter's value: the
    /// other truth value of each boolean, the other published interval bound, a
    /// longer destination list and another form of exclusion.
    Second,
}

/// A value `member` admits, from the set `values` names.
///
/// Every value is a legal one, so each case answers about WHERE the parameter was
/// stated rather than about its domain: the intervals are the bounds the member
/// docs publish (a creation frequency "can range from 1 to 7 days", a retention
/// period "from 2 to 14 days", the retention period greater than the creation
/// frequency) and `CrossRegionCopyTargetList` is `@length(min: 0, max: 3)`.
fn value_for(member: &str, values: Values) -> Value {
    match (member, values) {
        ("CreateInterval", Values::First) => json!(3),
        ("CreateInterval", Values::Second) => json!(7),
        ("RetainInterval", Values::First) => json!(10),
        ("RetainInterval", Values::Second) => json!(14),
        ("CopyTags", Values::First) => json!(true),
        ("CopyTags", Values::Second) => json!(false),
        ("ExtendDeletion", Values::First) => json!(true),
        ("ExtendDeletion", Values::Second) => json!(false),
        ("CrossRegionCopyTargets", Values::First) => json!([{"TargetRegion": OTHER_REGION}]),
        ("CrossRegionCopyTargets", Values::Second) => json!([
            {"TargetRegion": OTHER_REGION},
            {"TargetRegion": THIRD_REGION},
            {"TargetRegion": FOURTH_REGION},
        ]),
        ("Exclusions", Values::First) => json!({"ExcludeBootVolumes": true}),
        ("Exclusions", Values::Second) => json!({"ExcludeVolumeTypes": ["gp2", "io1"]}),
        (other, _) => panic!("{other:?} is not a [Default policies only] parameter"),
    }
}

/// The two places one request can state a default policy's configuration in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    Body,
    Details,
}

impl Place {
    fn label(self) -> &'static str {
        match self {
            Place::Body => "the request body",
            Place::Details => "PolicyDetails",
        }
    }
}

/// One SPLIT of the parameter set across the two places: disjoint sets, so every
/// parameter the request carries is stated exactly once.
struct Split {
    case: &'static str,
    /// The parameters stated at the request-body level, in the order the request
    /// states them on the wire.
    body: &'static [&'static str],
    /// The parameters stated inside the configuration structure.
    details: &'static [&'static str],
    /// Where the request states the resource type: the body member
    /// [`DEFAULT_POLICY_MEMBER`] or the structure's [`RESOURCE_TYPE_MEMBER`].
    resource_type_in: Place,
}

/// The splits: a single parameter on either side, a three-and-three split stated
/// out of declaration order, and the whole set on either side — each with the
/// resource type stated in one place or the other.
fn splits() -> Vec<Split> {
    vec![
        Split {
            case: "one parameter in the request body, the other five in the structure",
            body: &["CopyTags"],
            details: &[
                "CreateInterval",
                "RetainInterval",
                "CrossRegionCopyTargets",
                "ExtendDeletion",
                "Exclusions",
            ],
            resource_type_in: Place::Body,
        },
        Split {
            case: "one parameter in the structure, the other five in the request body",
            body: &[
                "CreateInterval",
                "RetainInterval",
                "CopyTags",
                "ExtendDeletion",
                "Exclusions",
            ],
            details: &["CrossRegionCopyTargets"],
            resource_type_in: Place::Details,
        },
        Split {
            case: "three parameters in each place, each run stated out of declaration order",
            body: &["Exclusions", "CopyTags", "CreateInterval"],
            details: &["ExtendDeletion", "CrossRegionCopyTargets", "RetainInterval"],
            resource_type_in: Place::Body,
        },
        Split {
            case: "every parameter in the request body, beside a structure stating none of them",
            body: &SHARED_PARAMETERS,
            details: &[],
            resource_type_in: Place::Details,
        },
        Split {
            case: "every parameter in the structure, and none in the request body",
            body: &[],
            details: &SHARED_PARAMETERS,
            resource_type_in: Place::Body,
        },
    ]
}

/// Where this split states `member`, or `None` when it states it nowhere.
fn place_of(split: &Split, member: &str, values: Values) -> Option<(Place, Value)> {
    if split.body.contains(&member) {
        return Some((Place::Body, value_for(member, values)));
    }
    if split.details.contains(&member) {
        return Some((Place::Details, value_for(member, values)));
    }
    None
}

/// What a read of a policy configured by this split must report for `member`: the
/// value whichever place stated it, else the default the member documents.
fn expected_report(split: &Split, member: &str, values: Values) -> Value {
    place_of(split, member, values)
        .map(|(_, value)| value)
        .unwrap_or_else(|| documented_default(member))
}

/// The configuration structure this split states, with the language that makes a
/// default policy and, where the split puts it there, the resource type.
fn details_of(split: &Split, values: Values) -> Value {
    let mut details = json!({"PolicyLanguage": DEFAULT_POLICY_LANGUAGE});
    if split.resource_type_in == Place::Details {
        details[RESOURCE_TYPE_MEMBER] = json!(DEFAULT_POLICY_TYPE);
    }
    for member in split.details {
        details[*member] = value_for(member, values);
    }
    details
}

/// A create of a default policy stating its configuration as this split does.
fn create_request(split: &Split, values: Values) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a default policy stating each parameter once",
        "State": "ENABLED",
    });
    if split.resource_type_in == Place::Body {
        request[DEFAULT_POLICY_MEMBER] = json!(DEFAULT_POLICY_TYPE);
    }
    for member in split.body {
        request[*member] = value_for(member, values);
    }
    request[POLICY_DETAILS_MEMBER] = details_of(split, values);
    request
}

/// An update stating the configuration as this split does. An update's body
/// declares no resource-type member, so the structure is the only place it can
/// restate the type — and restating the type it already holds asks for no change.
fn update_request(split: &Split, values: Values) -> Value {
    let mut request = json!({});
    for member in split.body {
        request[*member] = value_for(member, values);
    }
    let mut details = details_of(split, values);
    if split.resource_type_in == Place::Body {
        // The body cannot carry it on an update, so the structure states it.
        details[RESOURCE_TYPE_MEMBER] = json!(DEFAULT_POLICY_TYPE);
    }
    request[POLICY_DETAILS_MEMBER] = details;
    request
}

/// A default policy holding no configuration of its own: the subject the update
/// cases patch.
fn bare_default_policy() -> Value {
    json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a default policy to patch",
        "State": "ENABLED",
        DEFAULT_POLICY_MEMBER: DEFAULT_POLICY_TYPE,
    })
}

/// Assert a read of `id` reports exactly what this split stated, each parameter
/// from the place that stated it.
async fn assert_reports_the_split(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
    split: &Split,
    values: Values,
) {
    let read = get(router, id).await;
    for member in SHARED_PARAMETERS {
        let expected = expected_report(split, member, values);
        let pointer = format!("/{POLICY_DETAILS_MEMBER}/{member}");
        let actual = at(read.policy(), &pointer);
        let stated = place_of(split, member, values)
            .map(|(place, _)| place.label())
            .unwrap_or("neither place");
        assert_eq!(
            actual, expected,
            "case {case:?}: {member} was stated in {stated}, so a read must report \
             {expected:?}, got {actual:?}"
        );
    }
    for (pointer, expected) in [
        ("/DefaultPolicy", json!(true)),
        (
            &format!("/{POLICY_DETAILS_MEMBER}/{RESOURCE_TYPE_MEMBER}") as &str,
            json!(DEFAULT_POLICY_TYPE),
        ),
    ] {
        let actual = at(read.policy(), pointer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {pointer} to report {expected:?}, got {actual:?}"
        );
    }
}

/// A CREATE that states each `[Default policies only]` parameter in exactly one
/// place is accepted however the parameters are split between the two, and every
/// one of them takes the value the place that stated it gave.
#[tokio::test]
async fn a_create_may_split_the_configuration_across_the_two_places() {
    let router = emulator();
    for split in splits() {
        for values in [Values::First, Values::Second] {
            let case = format!("{} (value set {values:?}) on a create", split.case);
            let created = create(&router, create_request(&split, values)).await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: each parameter is stated once, so the create must be accepted, \
                 got {} with {:?}",
                created.status, created.body
            );
            assert_reports_the_split(&router, &created.policy_id(), &case, &split, values).await;
        }
    }
}

/// And an UPDATE by the same rule: body parameters beside a structure that states
/// the others is one statement of each, so the update is applied and a read
/// reports every parameter from the place that stated it — a parameter neither
/// place states going back to its documented default, since the structure is "the
/// configuration of the lifecycle policy".
#[tokio::test]
async fn an_update_may_split_the_configuration_across_the_two_places() {
    let router = emulator();
    for split in splits() {
        for values in [Values::First, Values::Second] {
            let case = format!("{} (value set {values:?}) on an update", split.case);
            // A FRESH subject per case, so each one answers about its own patch.
            let subject = create(&router, bare_default_policy()).await;
            assert_eq!(
                subject.status, 200,
                "case {case:?}: the subject policy must be created, got {} with {:?}",
                subject.status, subject.body
            );
            let id = subject.policy_id();

            let patched = update(&router, &id, update_request(&split, values)).await;
            assert_eq!(
                patched.status, 200,
                "case {case:?}: each parameter is stated once, so the update must be accepted, \
                 got {} with {:?}",
                patched.status, patched.body
            );
            assert_reports_the_split(&router, &id, &case, &split, values).await;
        }
    }
}

/// Assert `answer` is the modelled refusal naming `duplicated` and nothing else:
/// the parameter the request really stated twice, beside the structure it cannot
/// be combined with for it.
fn assert_refuses_only(case: &str, duplicated: &str, answer: &Answer) {
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
    assert!(
        answer.message().contains(duplicated),
        "case {case:?}: the refusal must name {duplicated:?}, got {:?}",
        answer.message()
    );
    for member in SHARED_PARAMETERS {
        if member == duplicated {
            continue;
        }
        assert!(
            !answer.message().contains(member),
            "case {case:?}: {member:?} was stated in one place, so the refusal must not name it, \
             got {:?}",
            answer.message()
        );
    }
    let listed = at(&answer.body, "/MutuallyExclusiveParameters");
    let expected = json!([POLICY_DETAILS_MEMBER, duplicated]);
    assert_eq!(
        listed, expected,
        "case {case:?}: MutuallyExclusiveParameters must list the parameters that cannot be \
         provided together, expected {expected:?}, got {listed:?}"
    );
}

/// The refusal is decided PER PARAMETER: of a request that splits the set across
/// the two places and states ONE of them twice, only that one is refused — named
/// in the message and in `MutuallyExclusiveParameters` — and the request is
/// refused on both operations, leaving the store as it was.
#[tokio::test]
async fn only_the_parameter_stated_in_both_places_is_refused() {
    let router = emulator();
    let subject = create(&router, bare_default_policy()).await;
    assert_eq!(
        subject.status, 200,
        "the subject policy must be created, got {} with {:?}",
        subject.status, subject.body
    );
    let subject = subject.policy_id();
    let stored = at(
        get(&router, &subject).await.policy(),
        &format!("/{POLICY_DETAILS_MEMBER}"),
    );

    for split in splits() {
        for values in [Values::First, Values::Second] {
            // Every parameter the split puts in the request body, each in turn
            // restated inside the structure: the only parameter stated twice.
            for duplicated in split.body {
                let case = format!(
                    "{} (value set {values:?}), with {duplicated} restated in the structure",
                    split.case
                );

                let before = policy_count(&router).await;
                let mut request = create_request(&split, values);
                request[POLICY_DETAILS_MEMBER][*duplicated] = value_for(duplicated, values);
                let created = create(&router, request).await;
                assert_refuses_only(&format!("{case}: on a create"), duplicated, &created);
                assert_eq!(
                    policy_count(&router).await,
                    before,
                    "case {case:?}: a refused create must leave no policy behind"
                );

                let mut patch = update_request(&split, values);
                patch[POLICY_DETAILS_MEMBER][*duplicated] = value_for(duplicated, values);
                let patched = update(&router, &subject, patch).await;
                assert_refuses_only(&format!("{case}: on an update"), duplicated, &patched);
                assert_eq!(
                    at(
                        get(&router, &subject).await.policy(),
                        &format!("/{POLICY_DETAILS_MEMBER}")
                    ),
                    stored,
                    "case {case:?}: a refused update must leave the stored configuration alone"
                );
            }
        }
    }
}

/// The resource type follows the same per-parameter rule, through the one body
/// member that states it: a create naming `DefaultPolicy` beside a structure that
/// states no `ResourceType` states the type once and is accepted, and the pair is
/// refused only when the structure states it too.
#[tokio::test]
async fn the_resource_type_is_refused_only_when_both_places_state_it() {
    let router = emulator();

    let case = "DefaultPolicy beside a structure stating no ResourceType";
    let created = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy naming its type once",
            "State": "ENABLED",
            DEFAULT_POLICY_MEMBER: DEFAULT_POLICY_TYPE,
            POLICY_DETAILS_MEMBER: {
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "CreateInterval": 2,
            },
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the type is stated once, so the create must be accepted, got {} with {:?}",
        created.status, created.body
    );
    let id = created.policy_id();
    let read = get(&router, &id).await;
    for (pointer, expected) in [
        ("/DefaultPolicy", json!(true)),
        (
            &format!("/{POLICY_DETAILS_MEMBER}/{RESOURCE_TYPE_MEMBER}") as &str,
            json!(DEFAULT_POLICY_TYPE),
        ),
        (
            &format!("/{POLICY_DETAILS_MEMBER}/CreateInterval") as &str,
            json!(2),
        ),
    ] {
        let actual = at(read.policy(), pointer);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected {pointer} to report {expected:?}, got {actual:?}"
        );
    }

    let case = "DefaultPolicy beside a structure stating ResourceType";
    let before = policy_count(&router).await;
    let refused = create(
        &router,
        json!({
            "ExecutionRoleArn": role_arn(),
            "Description": "a default policy naming its type twice",
            "State": "ENABLED",
            DEFAULT_POLICY_MEMBER: DEFAULT_POLICY_TYPE,
            POLICY_DETAILS_MEMBER: {
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                RESOURCE_TYPE_MEMBER: DEFAULT_POLICY_TYPE,
                "CreateInterval": 2,
            },
        }),
    )
    .await;
    assert_eq!(
        refused.status, INVALID_REQUEST_STATUS,
        "case {case:?}: expected HTTP {INVALID_REQUEST_STATUS}, got {} with {:?}",
        refused.status, refused.body
    );
    assert!(
        refused.message().contains(DEFAULT_POLICY_MEMBER),
        "case {case:?}: the refusal must name the body member {DEFAULT_POLICY_MEMBER:?}, got {:?}",
        refused.message()
    );
    assert!(
        !refused.message().contains("CreateInterval"),
        "case {case:?}: CreateInterval was stated in one place, so the refusal must not name it, \
         got {:?}",
        refused.message()
    );
    let listed = at(&refused.body, "/MutuallyExclusiveParameters");
    assert_eq!(
        listed,
        json!([POLICY_DETAILS_MEMBER, DEFAULT_POLICY_MEMBER]),
        "case {case:?}: expected the structure and the body member, got {listed:?}"
    );
    assert_eq!(
        policy_count(&router).await,
        before,
        "case {case:?}: a refused create must leave no policy behind"
    );
}
