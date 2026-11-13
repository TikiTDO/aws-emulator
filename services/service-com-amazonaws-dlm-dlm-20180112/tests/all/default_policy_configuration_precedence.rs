// SPDX-License-Identifier: CC0-1.0

//! Where a DEFAULT policy's configuration is stated, and what a later read
//! reports for it, end to end over HTTP.
//!
//! The rule under test: a default policy's `[Default policies only]`
//! configuration may be stated in the request body or inside the `PolicyDetails`
//! structure, and the two are ALTERNATIVES — one request may not state a member
//! in both places, and across requests the value the most recent accepted
//! request stated is what every later read reports, whichever of the two places
//! that request or the one before it used. A `PolicyDetails` an update states is
//! "the configuration of the lifecycle policy", so it replaces that
//! configuration wholesale: the members it omits go back to the default their own
//! documentation publishes.
//!
//! Every case drives the router, so each one exercises routing, deserialization,
//! the handler, the persisted record and serialization — the path a client takes,
//! and the only path on which an update that is answered 200 can be seen to have
//! taken effect.
//!
//! The rows span what the rule generalises over: every member the rule covers,
//! each of the four create-place/update-place combinations, the value's type
//! (integer, boolean, structure, list), both truth values of each boolean (a
//! stated `false` replacing a stored `true` is the case a fixed precedence hides),
//! a list's element count, and both directions of a numeric change.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region these requests are signed for.
const REGION: &str = "eu-central-1";

/// Destination Regions for the cross-Region copy rows, each matching
/// `TargetRegion`'s `^([a-z]+-){2,3}\d$`.
const OTHER_REGION: &str = "us-west-2";
const THIRD_REGION: &str = "ap-south-1";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`
/// in the emulator's own account, so `iam:PassRole` is granted.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/AWSDataLifecycleManagerDefaultRole",
        engine::emulator_account_id()
    )
}

/// `PolicyDetails$PolicyLanguage`: "`SIMPLIFIED` To create a default policy."
const DEFAULT_POLICY_LANGUAGE: &str = "SIMPLIFIED";

/// The type of default policy every subject below targets, a
/// `DefaultPolicyTypeValues` member.
const DEFAULT_POLICY_TYPE: &str = "VOLUME";

/// The modelled client error this service answers for a refused parameter, its
/// `@httpError` status and its `Code`.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";
const INVALID_REQUEST_STATUS: u16 = 400;
const INVALID_PARAMETER_CODE: &str = "InvalidParameter";

/// The documented default each `[Default policies only]` member takes when
/// nothing states a value: `CreateInterval` "the default is 1",
/// `RetainInterval` "the default is 7", `CopyTags` and `ExtendDeletion` "the
/// default is false", and a policy with no destination list makes no
/// cross-Region copies and excludes nothing
/// (`smithy_query(query="shape", shape="PolicyDetails")`).
fn documented_default(member: &str) -> Value {
    match member {
        "CreateInterval" => json!(1),
        "RetainInterval" => json!(7),
        "CopyTags" | "ExtendDeletion" => json!(false),
        "CrossRegionCopyTargets" | "Exclusions" => Value::Null,
        other => panic!("{other} is not a [Default policies only] member"),
    }
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

/// What a read reports at `/PolicyDetails/<member>`.
async fn reported(router: &MultiServiceRouter, id: &str, member: &str) -> Value {
    let read = get(router, id).await;
    at(read.policy(), &format!("/PolicyDetails/{member}"))
}

/// The two places a default policy's configuration can be stated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Place {
    /// The request body of the create or the update.
    Body,
    /// Inside the `PolicyDetails` configuration structure.
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

/// A create of a default `VOLUME` policy stating `member` as `value` in `place`.
fn create_request(place: Place, member: &str, value: &Value) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a default policy",
        "State": "ENABLED",
    });
    match place {
        Place::Body => {
            request["DefaultPolicy"] = json!(DEFAULT_POLICY_TYPE);
            request[member] = value.clone();
        }
        Place::Details => {
            request["PolicyDetails"] = json!({
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
                member: value.clone(),
            });
        }
    }
    request
}

/// An update stating `member` as `value` in `place`.
fn update_request(place: Place, member: &str, value: &Value) -> Value {
    match place {
        Place::Body => json!({member: value.clone()}),
        Place::Details => json!({
            "PolicyDetails": {
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
                member: value.clone(),
            },
        }),
    }
}

/// One configuration member, with two values for it: what the create states and
/// what the update replaces it with.
struct Replacement {
    /// What is particular about this row.
    case: &'static str,
    /// The member's wire name, as both the request body and `PolicyDetails`
    /// declare it.
    member: &'static str,
    created: Value,
    updated: Value,
}

/// Every `[Default policies only]` member a request can state in both places,
/// with a pair of values for each.
///
/// Source: `smithy_query(query="shape", shape="PolicyDetails")` — `CreateInterval`,
/// `RetainInterval`, `CopyTags`, `ExtendDeletion`, `CrossRegionCopyTargets` and
/// `Exclusions` each open `[Default policies only]` — read against
/// `smithy_query(query="operation", operation="UpdateLifecyclePolicy")`, whose
/// request body declares the same six. The interval values stay inside the
/// documented ranges (a creation frequency of 1 to 7 days, a retention period of
/// 2 to 14 days), which `interval_validation.rs` owns.
fn replacements() -> Vec<Replacement> {
    vec![
        Replacement {
            case: "a longer creation frequency",
            member: "CreateInterval",
            created: json!(3),
            updated: json!(5),
        },
        Replacement {
            case: "a shorter creation frequency (the change in the other direction)",
            member: "CreateInterval",
            created: json!(6),
            updated: json!(2),
        },
        Replacement {
            case: "a longer retention period",
            member: "RetainInterval",
            created: json!(10),
            updated: json!(14),
        },
        Replacement {
            case: "a shorter retention period, at its documented minimum",
            member: "RetainInterval",
            created: json!(14),
            updated: json!(2),
        },
        Replacement {
            // A stated `false` is a stated value: a read must report it in place
            // of the stored `true`.
            case: "copying tags turned off",
            member: "CopyTags",
            created: json!(true),
            updated: json!(false),
        },
        Replacement {
            case: "copying tags turned on",
            member: "CopyTags",
            created: json!(false),
            updated: json!(true),
        },
        Replacement {
            case: "extended deletion turned on",
            member: "ExtendDeletion",
            created: json!(false),
            updated: json!(true),
        },
        Replacement {
            case: "extended deletion turned off",
            member: "ExtendDeletion",
            created: json!(true),
            updated: json!(false),
        },
        Replacement {
            case: "a second destination Region added",
            member: "CrossRegionCopyTargets",
            created: json!([{"TargetRegion": OTHER_REGION}]),
            updated: json!([{"TargetRegion": OTHER_REGION}, {"TargetRegion": THIRD_REGION}]),
        },
        Replacement {
            case: "a different destination Region",
            member: "CrossRegionCopyTargets",
            created: json!([{"TargetRegion": OTHER_REGION}, {"TargetRegion": THIRD_REGION}]),
            updated: json!([{"TargetRegion": THIRD_REGION}]),
        },
        Replacement {
            case: "exclusions by volume type in place of boot volumes",
            member: "Exclusions",
            created: json!({"ExcludeBootVolumes": true}),
            updated: json!({"ExcludeVolumeTypes": ["gp2", "io1"]}),
        },
        Replacement {
            case: "exclusions by instance tag",
            member: "Exclusions",
            created: json!({"ExcludeVolumeTypes": ["gp2"]}),
            updated: json!({"ExcludeTags": [{"Key": "backup", "Value": "never"}]}),
        },
    ]
}

/// The configuration an accepted update states is what every later read reports,
/// whichever of the two places that update and the create before it stated it in.
///
/// This is the shadowing the issue reported: a create stating the configuration
/// in one place and an update stating it in the other must not leave the read
/// reporting the pre-update value.
#[tokio::test]
async fn an_accepted_update_is_what_a_later_read_reports_wherever_it_was_stated() {
    let router = emulator();
    for row in replacements() {
        for created_in in [Place::Body, Place::Details] {
            for updated_in in [Place::Body, Place::Details] {
                let case = format!(
                    "{} ({} created in {}, updated in {})",
                    row.case,
                    row.member,
                    created_in.label(),
                    updated_in.label()
                );

                // A FRESH policy per case, so each one answers about its own
                // create/update pair and nothing a previous case left stored.
                let created = create(
                    &router,
                    create_request(created_in, row.member, &row.created),
                )
                .await;
                assert_eq!(
                    created.status, 200,
                    "case {case:?}: the create must be accepted, got {} with {:?}",
                    created.status, created.body
                );
                let id = created.policy_id();
                let stored = reported(&router, &id, row.member).await;
                assert_eq!(
                    stored, row.created,
                    "case {case:?}: the create's value must be what a read reports, got {stored:?}"
                );

                let patched = update(
                    &router,
                    &id,
                    update_request(updated_in, row.member, &row.updated),
                )
                .await;
                assert_eq!(
                    patched.status, 200,
                    "case {case:?}: the update must be accepted, got {} with {:?}",
                    patched.status, patched.body
                );

                let after = get(&router, &id).await;
                let pointer = format!("/PolicyDetails/{}", row.member);
                assert_eq!(
                    at(after.policy(), &pointer),
                    row.updated,
                    "case {case:?}: expected {pointer} to report the value the update stated \
                     ({:?}), got {:?}",
                    row.updated,
                    at(after.policy(), &pointer)
                );
                // The update changed the configuration, not the kind of policy.
                assert_eq!(
                    at(after.policy(), "/DefaultPolicy"),
                    json!(true),
                    "case {case:?}: the policy must still be a default policy, got {:?}",
                    at(after.policy(), "/DefaultPolicy")
                );
                assert_eq!(
                    at(after.policy(), "/PolicyDetails/ResourceType"),
                    json!(DEFAULT_POLICY_TYPE),
                    "case {case:?}: the resource type cannot be updated, got {:?}",
                    at(after.policy(), "/PolicyDetails/ResourceType")
                );
            }
        }
    }
}

/// Every member of a default policy's configuration, as one request states them.
fn whole_configuration() -> Vec<(&'static str, Value)> {
    vec![
        ("CreateInterval", json!(3)),
        ("RetainInterval", json!(10)),
        ("CopyTags", json!(true)),
        ("ExtendDeletion", json!(true)),
        (
            "CrossRegionCopyTargets",
            json!([{"TargetRegion": OTHER_REGION}]),
        ),
        ("Exclusions", json!({"ExcludeBootVolumes": true})),
    ]
}

/// A create of a default policy stating its whole configuration in `place`.
fn fully_configured_create(place: Place) -> Value {
    let mut request = json!({
        "ExecutionRoleArn": role_arn(),
        "Description": "a fully configured default policy",
        "State": "ENABLED",
    });
    let mut details = json!({
        "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
        "ResourceType": DEFAULT_POLICY_TYPE,
    });
    for (member, value) in whole_configuration() {
        match place {
            Place::Body => request[member] = value,
            Place::Details => details[member] = value,
        }
    }
    match place {
        Place::Body => request["DefaultPolicy"] = json!(DEFAULT_POLICY_TYPE),
        Place::Details => request["PolicyDetails"] = details,
    }
    request
}

/// `UpdateLifecyclePolicyRequest$PolicyDetails` is "the configuration of the
/// lifecycle policy", so stating it replaces that configuration WHOLESALE: the
/// members it states take effect and every member it omits goes back to the
/// default its own documentation publishes — from either place the stored
/// configuration was stated in.
#[tokio::test]
async fn a_policy_details_update_replaces_the_whole_configuration() {
    let router = emulator();
    // Both ways of stating the configuration being replaced: it is the
    // replacement that decides, not where the value it replaces came from.
    for stored_in in [Place::Body, Place::Details] {
        let case = format!("configuration created in {}", stored_in.label());
        let id = create(&router, fully_configured_create(stored_in))
            .await
            .policy_id();
        for (member, value) in whole_configuration() {
            let stored = reported(&router, &id, member).await;
            assert_eq!(
                stored, value,
                "case {case:?}: the create's {member} must be stored, got {stored:?}"
            );
        }

        let patched = update(
            &router,
            &id,
            json!({
                "PolicyDetails": {
                    "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                    "ResourceType": DEFAULT_POLICY_TYPE,
                    "CreateInterval": 5,
                    "RetainInterval": 14,
                },
            }),
        )
        .await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: the update must be accepted, got {} with {:?}",
            patched.status, patched.body
        );

        for (member, expected) in [
            ("CreateInterval", json!(5)),
            ("RetainInterval", json!(14)),
            ("CopyTags", documented_default("CopyTags")),
            ("ExtendDeletion", documented_default("ExtendDeletion")),
            (
                "CrossRegionCopyTargets",
                documented_default("CrossRegionCopyTargets"),
            ),
            ("Exclusions", documented_default("Exclusions")),
        ] {
            let actual = reported(&router, &id, member).await;
            assert_eq!(
                actual, expected,
                "case {case:?}: the replacement states no {member}, so a read must report \
                 {expected:?}, got {actual:?}"
            );
        }
    }
}

/// The request-body members are a PARTIAL update of the configuration the policy
/// holds, wherever it was stated: a patch of one member leaves every other member
/// of the stored configuration exactly as it was.
#[tokio::test]
async fn a_request_body_update_changes_only_the_member_it_states() {
    let router = emulator();
    for stored_in in [Place::Body, Place::Details] {
        for (patched_member, patched_value) in [
            ("CopyTags", json!(false)),
            ("CreateInterval", json!(6)),
            (
                "CrossRegionCopyTargets",
                json!([{"TargetRegion": THIRD_REGION}]),
            ),
        ] {
            let case = format!(
                "{patched_member} patched in the request body (configuration created in {})",
                stored_in.label()
            );
            let id = create(&router, fully_configured_create(stored_in))
                .await
                .policy_id();

            let patched = update(
                &router,
                &id,
                json!({patched_member: patched_value.clone()}),
            )
            .await;
            assert_eq!(
                patched.status, 200,
                "case {case:?}: the update must be accepted, got {} with {:?}",
                patched.status, patched.body
            );

            for (member, created_value) in whole_configuration() {
                let expected = if member == patched_member {
                    patched_value.clone()
                } else {
                    created_value
                };
                let actual = reported(&router, &id, member).await;
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {member} to report {expected:?}, got {actual:?}"
                );
            }
        }
    }
}

/// One request may not state a default policy's configuration in both places:
/// `CreateLifecyclePolicy` documents them as alternatives, "but not both", and
/// the update carries the very same parameters.
#[tokio::test]
async fn an_update_may_not_state_the_configuration_twice() {
    let router = emulator();
    let id = create(&router, fully_configured_create(Place::Body))
        .await
        .policy_id();
    let stored = at(get(&router, &id).await.policy(), "/PolicyDetails");

    // Each member on its own, then every one of them at once: one refusal
    // naming them all.
    let mut cases: Vec<(String, Value)> = Vec::new();
    let mut everything = json!({
        "PolicyDetails": {
            "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
            "ResourceType": DEFAULT_POLICY_TYPE,
        },
    });
    for (member, value) in whole_configuration() {
        let mut patch = json!({
            "PolicyDetails": {
                "PolicyLanguage": DEFAULT_POLICY_LANGUAGE,
                "ResourceType": DEFAULT_POLICY_TYPE,
                member: value.clone(),
            },
        });
        patch[member] = value.clone();
        cases.push((format!("{member} stated in both places"), patch));
        everything["PolicyDetails"][member] = value.clone();
        everything[member] = value;
    }
    cases.push(("every member stated in both places".to_string(), everything));

    for (case, patch) in cases {
        let answer = update(&router, &id, patch).await;
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
            answer.body
        );
        let conflicting = at(&answer.body, "/MutuallyExclusiveParameters");
        assert!(
            conflicting
                .as_array()
                .is_some_and(|names| names.contains(&json!("PolicyDetails"))),
            "case {case:?}: the refusal must name PolicyDetails, got {conflicting:?}"
        );
        assert_eq!(
            at(get(&router, &id).await.policy(), "/PolicyDetails"),
            stored,
            "case {case:?}: a refused update must leave the stored configuration alone"
        );
    }

    // And the request the rule leaves alone: the configuration in ONE place.
    let accepted = update(
        &router,
        &id,
        json!({"CreateInterval": 4, "RetainInterval": 12}),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "the configuration in one place is accepted, got {} with {:?}",
        accepted.status, accepted.body
    );
    let actual = reported(&router, &id, "CreateInterval").await;
    assert_eq!(
        actual,
        json!(4),
        "the accepted update must be what a read reports, got {actual:?}"
    );
}
