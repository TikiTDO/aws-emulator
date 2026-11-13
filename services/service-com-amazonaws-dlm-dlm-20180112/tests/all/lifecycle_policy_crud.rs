// SPDX-License-Identifier: CC0-1.0

//! The Amazon Data Lifecycle Manager lifecycle-policy loop, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same
//! path a client takes. The rule under test is the one the control plane states
//! for its primary resource: a lifecycle policy's members are whatever the create,
//! or a later update, stored, and every operation computes its answer from that
//! record rather than from the request it is serving.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region the requests in this file are signed for, and so the region every
/// policy ARN they mint must name.
const REGION: &str = "eu-west-1";

/// A second signing region, so the ARN case spans more than one.
const OTHER_REGION: &str = "us-west-2";

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerDefaultRole";

/// A second role, for the update case that replaces it.
const OTHER_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerCustomRole";

/// `DefaultPolicyTypeValues` -> the `PolicyTypeValues` a default policy of that
/// type carries.
///
/// Source: `CreateLifecyclePolicyRequest$DefaultPolicy` ("specify `VOLUME`" to
/// create snapshots of all volumes, "specify `INSTANCE`" to create EBS-backed
/// AMIs from all instances) read against `LifecyclePolicySummary$PolicyType`
/// (`EBS_SNAPSHOT_MANAGEMENT` "manages the lifecycle of Amazon EBS snapshots",
/// `IMAGE_MANAGEMENT` "manages the lifecycle of EBS-backed AMIs"). Both entries
/// are exercised; the issue cited neither.
const DEFAULT_POLICY_TYPES: &[(&str, &str)] = &[
    ("VOLUME", "EBS_SNAPSHOT_MANAGEMENT"),
    ("INSTANCE", "IMAGE_MANAGEMENT"),
];

/// `CreateInterval`: "If you do not specify a value, the default is 1."
const PUBLISHED_CREATE_INTERVAL_DEFAULT: i64 = 1;

/// `RetainInterval`: "If you do not specify a value, the default is 7."
const PUBLISHED_RETAIN_INTERVAL_DEFAULT: i64 = 7;

/// `PolicyId`'s `^policy-[a-f0-9]+$`, checked here rather than through the
/// crate's own predicate so a wrong predicate cannot make this test pass.
fn matches_policy_id_pattern(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("policy-") else {
        return false;
    };
    !hex.is_empty() && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
    /// The `Policy` member `GetLifecyclePolicy` returns.
    fn policy(&self) -> &Value {
        &self.body["Policy"]
    }

    /// The `PolicyId` `CreateLifecyclePolicy` returns.
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

/// A SigV4 credential scope naming this service in `region`. It is what routes a
/// request to dlm, and its region is what a policy ARN is built for.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{region}.amazonaws.com"))
        .header("authorization", signed_for(region));
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

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    call_in(router, REGION, method, uri, body).await
}

async fn create(router: &MultiServiceRouter, request: Value) -> Answer {
    call(router, "POST", "/policies", Some(request)).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "GET", &format!("/policies/{id}"), None).await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    call(router, "GET", &format!("/policies{query}"), None).await
}

async fn update(router: &MultiServiceRouter, id: &str, request: Value) -> Answer {
    call(router, "PATCH", &format!("/policies/{id}"), Some(request)).await
}

async fn delete(router: &MultiServiceRouter, id: &str) -> Answer {
    call(router, "DELETE", &format!("/policies/{id}"), None).await
}

/// `/tags/{ResourceArn}` for `arn`. The label is a single path segment, so the
/// ARN's `/` is percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// The ids `GetLifecyclePolicies` reported, in the order it reported them.
fn listed_ids(answer: &Answer) -> Vec<String> {
    answer.body["Policies"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "GetLifecyclePolicies must return a Policies list: {:?}",
                answer.body
            )
        })
        .iter()
        .map(|summary| summary["PolicyId"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// A comparable key for a `DateCreated` / `DateModified` timestamp, whichever
/// of the two representations the codec renders it in.
///
/// The wire form is the framework's to choose from the schema (this scaffold's
/// JSON codec renders an epoch-seconds number unless the member schema carries
/// `@timestampFormat`), so the test compares instants rather than spellings:
/// a number is padded to a fixed width and an RFC-3339 string's
/// fractional-second part to nine digits, after which either sorts
/// lexicographically in the order the instants fall.
pub(crate) fn timestamp_key(value: &Value) -> String {
    if let Some(seconds) = value.as_f64() {
        return format!("{seconds:030.9}");
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("a timestamp must be a number or a string, got {value:?}"));
    let body = text.trim_end_matches('Z');
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    format!("{whole}.{fraction:0<9}")
}

/// Where a `CreateLifecyclePolicy` input member lands in `LifecyclePolicy`, as a
/// JSON pointer.
///
/// The table is the crate's `OPERATIONS.md` CreateLifecyclePolicy input list read
/// against the `LifecyclePolicy` shape; `None` marks a member that shape reports
/// somewhere a pointer cannot name on its own. An unmapped member panics rather
/// than being skipped, so a case cannot quietly assert nothing.
fn reported_as(member: &str) -> Option<&'static str> {
    Some(match member {
        "Description" => "/Description",
        "ExecutionRoleArn" => "/ExecutionRoleArn",
        "State" => "/State",
        "Tags" => "/Tags",
        "PolicyDetails" => "/PolicyDetails",
        // The `[Default policies only]` body parameters are the alternative way
        // of stating a default policy's configuration, so they are reported in
        // `PolicyDetails` — see `effective_policy_details` in `src/state.rs`.
        "CreateInterval" => "/PolicyDetails/CreateInterval",
        "RetainInterval" => "/PolicyDetails/RetainInterval",
        "CopyTags" => "/PolicyDetails/CopyTags",
        "ExtendDeletion" => "/PolicyDetails/ExtendDeletion",
        "CrossRegionCopyTargets" => "/PolicyDetails/CrossRegionCopyTargets",
        "Exclusions" => "/PolicyDetails/Exclusions",
        // `DefaultPolicy` names the TYPE of default policy to create; the shape
        // reports it as the `DefaultPolicy` boolean plus
        // `PolicyDetails$ResourceType`, both asserted explicitly below.
        "DefaultPolicy" => return None,
        other => {
            panic!("this test names a CreateLifecyclePolicy member with no mapping: {other:?}")
        }
    })
}

/// A custom EBS snapshot policy's `PolicyDetails`, as a client sends it.
fn custom_snapshot_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "TagsToAdd": [{"Key": "made-by", "Value": "dlm"}],
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A custom EBS-backed AMI policy's `PolicyDetails`.
fn custom_image_details() -> Value {
    json!({
        "PolicyType": "IMAGE_MANAGEMENT",
        "ResourceTypes": ["INSTANCE"],
        "TargetTags": [{"Key": "backup", "Value": "weekly"}],
        "Schedules": [{
            "Name": "weekly",
            // `CreateRule$Interval`'s "supported values are 1, 2, 3, 4, 6, 8, 12,
            // and 24" (`smithy_query(query="doc", shape="CreateRule",
            // member="Interval")`), and `IntervalUnit` offers `HOURS` alone, so
            // 12 hours is the cadence here; `interval_validation.rs` pins the set.
            "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 2},
        }],
    })
}

/// A second custom EBS snapshot configuration, for an update that replaces
/// [`custom_snapshot_details`] wholesale: the same policy type and target
/// resource type, since `UpdateLifecyclePolicyRequest$PolicyDetails` is "the
/// configuration of the lifecycle policy. You cannot update the policy type or
/// the resource type", and every other member different.
fn replacement_snapshot_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "weekly"}],
        "Schedules": [{
            "Name": "weekly",
            "CreateRule": {"Interval": 12, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 2},
        }],
    })
}

/// `CreateLifecyclePolicy` persists what it was given and `GetLifecyclePolicy`
/// reports that record: for every input member the policy shape carries, the
/// value read back is the created one.
///
/// The rows differ along what the rule generalises over — which optional members
/// are present at all, the activation state, how many tags, custom versus
/// default policy, which default-policy type, and whether the configuration was
/// stated as a `PolicyDetails` structure or as body parameters — so no row's
/// shape can be special-cased.
#[tokio::test]
async fn a_created_policy_is_read_back_from_what_was_stored() {
    let mut cases = vec![
        (
            // The smallest create the service accepts: the three `@required`
            // members plus the one parameter that says what the policy would
            // do. `DefaultPolicy` is the shortest way to say it — a custom
            // policy's configuration is a whole `PolicyDetails` structure.
            "only the required members and the smallest configuration",
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "nightly ebs",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
            }),
        ),
        (
            "disabled, with several tags",
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "paused ebs",
                "State": "DISABLED",
                "DefaultPolicy": "INSTANCE",
                "Tags": {"team": "backups", "env": "prod", "cost-centre": "42"},
            }),
        ),
        (
            "a custom snapshot policy stated as PolicyDetails",
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "custom volume snapshots",
                "State": "ENABLED",
                "Tags": {"team": "backups"},
                "PolicyDetails": custom_snapshot_details(),
            }),
        ),
        (
            "a custom AMI policy stated as PolicyDetails",
            json!({
                "ExecutionRoleArn": OTHER_ROLE_ARN,
                "Description": "custom image management",
                "State": "ENABLED",
                "PolicyDetails": custom_image_details(),
            }),
        ),
    ];
    // One row per `DefaultPolicyTypeValues` member, each stating its
    // configuration in the request body rather than in `PolicyDetails`.
    for (default_type, _) in DEFAULT_POLICY_TYPES {
        cases.push((
            "a default policy stated as body parameters",
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": format!("default {default_type} policy"),
                "State": "ENABLED",
                "DefaultPolicy": default_type,
                "CreateInterval": 3,
                "RetainInterval": 14,
                "CopyTags": true,
                "ExtendDeletion": true,
                "CrossRegionCopyTargets": [{"TargetRegion": OTHER_REGION}],
                "Exclusions": {"ExcludeBootVolumes": true},
            }),
        ));
    }
    // And one that names only the default-policy type, so the documented
    // defaults are what the policy must carry.
    cases.push((
        "a default policy with every optional parameter left out",
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "default volume policy with defaults",
            "State": "ENABLED",
            "DefaultPolicy": "VOLUME",
        }),
    ));

    let router = emulator();
    let mut seen_ids: Vec<String> = Vec::new();
    for (case, request) in cases {
        let created = create(&router, request.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );

        let id = created.policy_id();
        assert!(
            matches_policy_id_pattern(&id),
            "case {case:?}: expected an id matching ^policy-[a-f0-9]+$, got {id:?}"
        );
        assert!(
            !seen_ids.contains(&id),
            "case {case:?}: CreateLifecyclePolicy minted an id it had already used: {id:?}"
        );
        seen_ids.push(id.clone());

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            at(read.policy(), "/PolicyId"),
            json!(id),
            "case {case:?}: GetLifecyclePolicy must report the id it was asked about"
        );
        assert_eq!(
            at(read.policy(), "/PolicyArn"),
            json!(format!(
                "arn:aws:dlm:{REGION}:{}:policy/{id}",
                engine::emulator_account_id()
            )),
            "case {case:?}: the ARN must name dlm, the signed region, the emulator account \
             and the policy, got {:?}",
            at(read.policy(), "/PolicyArn")
        );

        for (member, expected) in request.as_object().expect("each case is a JSON object") {
            let Some(pointer) = reported_as(member) else {
                continue;
            };
            let observed = at(read.policy(), pointer);
            // `PolicyDetails` is reported with the members the service
            // MATERIALIZES stamped onto it beside the submitted ones, so what it
            // owes the client is that nothing submitted was lost; which members
            // the service fills in is pinned by `policy_read_projection.rs`.
            if member == "PolicyDetails" {
                use crate::policy_read_projection::reports_everything_sent;
                if let Err(problem) = reports_everything_sent(expected, &observed, pointer) {
                    panic!("case {case:?}: {problem}");
                }
                continue;
            }
            assert_eq!(
                &observed, expected,
                "case {case:?}: member {member:?} was stored as {expected:?} but {pointer} \
                 reports {observed:?}"
            );
        }

        // A policy with no tags reports the EMPTY map: the read path reports the
        // tag set the policy holds, so "no tags" is distinguishable from "not
        // reported" without a second call.
        if request.get("Tags").is_none() {
            assert_eq!(
                at(read.policy(), "/Tags"),
                json!({}),
                "case {case:?}: a policy created with no tags must report an empty Tags map"
            );
        }

        // `DefaultPolicy` is the boolean "whether the policy is a default
        // lifecycle policy or a custom lifecycle policy".
        let default_type = request.get("DefaultPolicy").and_then(Value::as_str);
        assert_eq!(
            at(read.policy(), "/DefaultPolicy"),
            json!(default_type.is_some()),
            "case {case:?}: DefaultPolicy must say whether this is a default policy"
        );
        if let Some(default_type) = default_type {
            let expected_policy_type = DEFAULT_POLICY_TYPES
                .iter()
                .find(|(name, _)| *name == default_type)
                .map(|(_, policy_type)| *policy_type)
                .unwrap_or_else(|| panic!("case {case:?}: unmapped default type {default_type:?}"));
            assert_eq!(
                at(read.policy(), "/PolicyDetails/ResourceType"),
                json!(default_type),
                "case {case:?}: a default policy's configuration names its resource type"
            );
            assert_eq!(
                at(read.policy(), "/PolicyDetails/PolicyLanguage"),
                json!("SIMPLIFIED"),
                "case {case:?}: SIMPLIFIED is the policy language of a default policy"
            );
            assert_eq!(
                at(read.policy(), "/PolicyDetails/PolicyType"),
                json!(expected_policy_type),
                "case {case:?}: a {default_type} default policy is a {expected_policy_type} \
                 policy, got {:?}",
                at(read.policy(), "/PolicyDetails/PolicyType")
            );
            // The documented defaults, for the row that names none of them.
            if request.get("CreateInterval").is_none() {
                assert_eq!(
                    at(read.policy(), "/PolicyDetails/CreateInterval"),
                    json!(PUBLISHED_CREATE_INTERVAL_DEFAULT),
                    "case {case:?}: an unspecified CreateInterval defaults to \
                     {PUBLISHED_CREATE_INTERVAL_DEFAULT}"
                );
                assert_eq!(
                    at(read.policy(), "/PolicyDetails/RetainInterval"),
                    json!(PUBLISHED_RETAIN_INTERVAL_DEFAULT),
                    "case {case:?}: an unspecified RetainInterval defaults to \
                     {PUBLISHED_RETAIN_INTERVAL_DEFAULT}"
                );
                assert_eq!(
                    at(read.policy(), "/PolicyDetails/CopyTags"),
                    json!(false),
                    "case {case:?}: an unspecified CopyTags defaults to false"
                );
                assert_eq!(
                    at(read.policy(), "/PolicyDetails/ExtendDeletion"),
                    json!(false),
                    "case {case:?}: an unspecified ExtendDeletion defaults to false"
                );
            }
        }

        // A policy that nothing has modified yet was last modified when it was
        // created.
        assert_eq!(
            timestamp_key(&at(read.policy(), "/DateModified")),
            timestamp_key(&at(read.policy(), "/DateCreated")),
            "case {case:?}: an unmodified policy's DateModified is its DateCreated"
        );
        // "The description of the status" describes the policy's CURRENT status;
        // the emulator never runs a policy, so none of them is in ERROR and each
        // reports its activation state.
        assert_eq!(
            at(read.policy(), "/StatusMessage"),
            at(read.policy(), "/State"),
            "case {case:?}: StatusMessage must describe the policy's status, got {:?}",
            at(read.policy(), "/StatusMessage")
        );
    }

    // Every created policy is still its own record, read back by its own id.
    assert_eq!(
        seen_ids.len(),
        7,
        "every row must have produced a policy: {seen_ids:?}"
    );
    for id in &seen_ids {
        assert_eq!(
            at(get(&router, id).await.policy(), "/PolicyId"),
            json!(id),
            "policy {id:?} must still be readable by its own id"
        );
    }
}

/// Two creates with the same request body still make two distinct policies: the
/// id is minted per call, and each record is its own.
#[tokio::test]
async fn two_creates_make_two_distinct_policies() {
    let router = emulator();
    let request = json!({
        "ExecutionRoleArn": ROLE_ARN,
        "Description": "identical requests",
        "State": "ENABLED",
        "DefaultPolicy": "VOLUME",
    });

    let first = create(&router, request.clone()).await.policy_id();
    let second = create(&router, request).await.policy_id();
    assert_ne!(
        first, second,
        "two creates must mint two ids, got {first:?} twice"
    );

    // Changing one must not change the other.
    let updated = update(&router, &first, json!({"Description": "only the first"})).await;
    assert_eq!(
        updated.status, 200,
        "expected HTTP 200 from UpdateLifecyclePolicy, got {} with {:?}",
        updated.status, updated.body
    );
    assert_eq!(
        at(get(&router, &first).await.policy(), "/Description"),
        json!("only the first"),
        "the updated policy must carry its new description"
    );
    assert_eq!(
        at(get(&router, &second).await.policy(), "/Description"),
        json!("identical requests"),
        "the other policy must be untouched"
    );
    assert_eq!(
        at(get(&router, &second).await.policy(), "/PolicyId"),
        json!(second),
        "the other policy must still be its own record"
    );
}

/// A policy ARN names the region the request was signed for — the rule holds for
/// any region, not just one.
#[tokio::test]
async fn a_policy_arn_names_the_signed_region() {
    let router = emulator();
    for region in [REGION, OTHER_REGION, "ap-northeast-1"] {
        let created = call_in(
            &router,
            region,
            "POST",
            "/policies",
            Some(json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "region scoped",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
            })),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {region:?}: expected HTTP 200, got {} with {:?}",
            created.status, created.body
        );
        let id = created.policy_id();
        let read = call_in(&router, region, "GET", &format!("/policies/{id}"), None).await;
        assert_eq!(
            at(read.policy(), "/PolicyArn"),
            json!(format!(
                "arn:aws:dlm:{region}:{}:policy/{id}",
                engine::emulator_account_id()
            )),
            "case {region:?}: got {:?}",
            at(read.policy(), "/PolicyArn")
        );
    }
}

/// The four policies the filter case lists over, by id.
struct Fixture {
    enabled_volume: String,
    disabled_instance: String,
    default_volume: String,
    default_instance: String,
}

async fn filter_fixture(router: &MultiServiceRouter) -> Fixture {
    let enabled_volume = create(
        router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "enabled volume snapshots",
            "State": "ENABLED",
            "Tags": {"team": "backups"},
            "PolicyDetails": custom_snapshot_details(),
        }),
    )
    .await
    .policy_id();
    let disabled_instance = create(
        router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "disabled image management",
            "State": "DISABLED",
            "PolicyDetails": custom_image_details(),
        }),
    )
    .await
    .policy_id();
    let default_volume = create(
        router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "default volume policy",
            "State": "ENABLED",
            "DefaultPolicy": "VOLUME",
        }),
    )
    .await
    .policy_id();
    let default_instance = create(
        router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "default instance policy",
            "State": "ENABLED",
            "DefaultPolicy": "INSTANCE",
        }),
    )
    .await
    .policy_id();
    Fixture {
        enabled_volume,
        disabled_instance,
        default_volume,
        default_instance,
    }
}

/// `GetLifecyclePolicies` builds its answer by iterating the store and keeping
/// the policies every filter in the request admits.
///
/// The rows span each filter the operation models, each filter's values, a
/// combination of two filters, and the no-filter case.
#[tokio::test]
async fn get_lifecycle_policies_applies_every_filter_to_the_store() {
    let router = emulator();
    let fixture = filter_fixture(&router).await;
    let Fixture {
        enabled_volume,
        disabled_instance,
        default_volume,
        default_instance,
    } = &fixture;

    let cases: Vec<(&str, String, Vec<&String>)> = vec![
        (
            "no filter at all",
            String::new(),
            vec![
                enabled_volume,
                disabled_instance,
                default_volume,
                default_instance,
            ],
        ),
        (
            "state=ENABLED",
            "?state=ENABLED".to_string(),
            vec![enabled_volume, default_volume, default_instance],
        ),
        (
            "state=DISABLED",
            "?state=DISABLED".to_string(),
            vec![disabled_instance],
        ),
        ("state=ERROR", "?state=ERROR".to_string(), vec![]),
        (
            "policyIds naming one policy",
            format!("?policyIds={default_instance}"),
            vec![default_instance],
        ),
        (
            "policyIds naming two policies",
            format!("?policyIds={enabled_volume}&policyIds={disabled_instance}"),
            vec![enabled_volume, disabled_instance],
        ),
        (
            "resourceTypes=VOLUME",
            "?resourceTypes=VOLUME".to_string(),
            vec![enabled_volume, default_volume],
        ),
        (
            "resourceTypes=INSTANCE",
            "?resourceTypes=INSTANCE".to_string(),
            vec![disabled_instance, default_instance],
        ),
        (
            "targetTags naming a tag one policy targets",
            "?targetTags=backup%3Ddaily".to_string(),
            vec![enabled_volume],
        ),
        (
            "targetTags naming a tag no policy targets",
            "?targetTags=backup%3Dhourly".to_string(),
            vec![],
        ),
        (
            "tagsToAdd naming a tag one policy adds",
            "?tagsToAdd=made-by%3Ddlm".to_string(),
            vec![enabled_volume],
        ),
        (
            "defaultPolicyType=ALL",
            "?defaultPolicyType=ALL".to_string(),
            vec![default_volume, default_instance],
        ),
        (
            "defaultPolicyType=VOLUME",
            "?defaultPolicyType=VOLUME".to_string(),
            vec![default_volume],
        ),
        (
            "defaultPolicyType=INSTANCE",
            "?defaultPolicyType=INSTANCE".to_string(),
            vec![default_instance],
        ),
        (
            "two filters at once",
            "?state=ENABLED&resourceTypes=VOLUME".to_string(),
            vec![enabled_volume, default_volume],
        ),
    ];

    for (case, query, expected) in cases {
        let answer = list(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let mut observed = listed_ids(&answer);
        observed.sort();
        let mut wanted: Vec<String> = expected.into_iter().cloned().collect();
        wanted.sort();
        assert_eq!(
            observed, wanted,
            "case {case:?}: GetLifecyclePolicies answered {observed:?}, expected {wanted:?}"
        );
    }
}

/// A `LifecyclePolicySummary` is the stored policy projected: its `PolicyType`
/// is the policy's own type, and its `Tags` and `State` are what is stored.
#[tokio::test]
async fn a_summary_reports_the_stored_policys_type_state_and_tags() {
    let router = emulator();
    let fixture = filter_fixture(&router).await;

    let expected: Vec<(&str, &String, &str, &str, Value)> = vec![
        (
            "a custom snapshot policy",
            &fixture.enabled_volume,
            "EBS_SNAPSHOT_MANAGEMENT",
            "ENABLED",
            json!({"team": "backups"}),
        ),
        (
            "a custom AMI policy",
            &fixture.disabled_instance,
            "IMAGE_MANAGEMENT",
            "DISABLED",
            json!({}),
        ),
        (
            "a default VOLUME policy",
            &fixture.default_volume,
            "EBS_SNAPSHOT_MANAGEMENT",
            "ENABLED",
            json!({}),
        ),
        (
            "a default INSTANCE policy",
            &fixture.default_instance,
            "IMAGE_MANAGEMENT",
            "ENABLED",
            json!({}),
        ),
    ];

    let answer = list(&router, "").await;
    let summaries = answer.body["Policies"]
        .as_array()
        .expect("GetLifecyclePolicies returns a Policies list")
        .clone();

    for (case, id, policy_type, state, tags) in expected {
        let summary = summaries
            .iter()
            .find(|summary| summary["PolicyId"] == json!(id))
            .unwrap_or_else(|| panic!("case {case:?}: {id} is missing from {summaries:?}"));
        assert_eq!(
            at(summary, "/PolicyType"),
            json!(policy_type),
            "case {case:?}: expected PolicyType {policy_type:?}, got {:?}",
            at(summary, "/PolicyType")
        );
        assert_eq!(
            at(summary, "/State"),
            json!(state),
            "case {case:?}: expected State {state:?}, got {:?}",
            at(summary, "/State")
        );
        assert_eq!(
            at(summary, "/Tags"),
            tags,
            "case {case:?}: expected Tags {tags:?}, got {:?}",
            at(summary, "/Tags")
        );
        assert_eq!(
            at(summary, "/DefaultPolicy"),
            json!(id == &fixture.default_volume || id == &fixture.default_instance),
            "case {case:?}: DefaultPolicy must say whether this is a default policy"
        );
    }
}

/// One update case: what to create, what to patch, what the patch must change,
/// and what it must leave alone.
struct UpdateCase {
    case: &'static str,
    create: Value,
    patch: Value,
    /// `(pointer, value)` the policy must report after the patch.
    changed: Vec<(&'static str, Value)>,
    /// Pointers whose value must be exactly what they were before the patch.
    preserved: Vec<&'static str>,
}

/// `UpdateLifecyclePolicy` is a PARTIAL update: only the members the request
/// carries overwrite the stored policy, every absent member keeps its stored
/// value, and `DateModified` is bumped.
///
/// The rows span each updatable member, a request carrying several at once, and
/// a request carrying none.
#[tokio::test]
async fn update_overwrites_only_the_members_the_request_carries() {
    let custom = json!({
        "ExecutionRoleArn": ROLE_ARN,
        "Description": "nightly ebs",
        "State": "ENABLED",
        "Tags": {"team": "backups"},
        "PolicyDetails": custom_snapshot_details(),
    });
    let default_volume = json!({
        "ExecutionRoleArn": ROLE_ARN,
        "Description": "default volume policy",
        "State": "ENABLED",
        "DefaultPolicy": "VOLUME",
    });

    let cases = vec![
        UpdateCase {
            case: "State only",
            create: custom.clone(),
            patch: json!({"State": "DISABLED"}),
            changed: vec![("/State", json!("DISABLED"))],
            preserved: vec![
                "/Description",
                "/ExecutionRoleArn",
                "/PolicyDetails",
                "/Tags",
                "/DateCreated",
            ],
        },
        UpdateCase {
            case: "Description only",
            create: custom.clone(),
            patch: json!({"Description": "nightly ebs v2"}),
            changed: vec![("/Description", json!("nightly ebs v2"))],
            preserved: vec![
                "/State",
                "/ExecutionRoleArn",
                "/PolicyDetails",
                "/Tags",
                "/DateCreated",
            ],
        },
        UpdateCase {
            case: "ExecutionRoleArn only",
            create: custom.clone(),
            patch: json!({"ExecutionRoleArn": OTHER_ROLE_ARN}),
            changed: vec![("/ExecutionRoleArn", json!(OTHER_ROLE_ARN))],
            preserved: vec!["/State", "/Description", "/PolicyDetails", "/DateCreated"],
        },
        UpdateCase {
            case: "PolicyDetails replaced wholesale",
            create: custom.clone(),
            patch: json!({"PolicyDetails": replacement_snapshot_details()}),
            // The replacement configuration, with the members the service
            // materializes stamped onto it as at create — and with the members
            // the created configuration carried and the replacement does not
            // (`Schedules[0].TagsToAdd`) gone, since the structure replaces the
            // configuration rather than patching it.
            changed: vec![
                (
                    "/PolicyDetails/TargetTags",
                    json!([{"Key": "backup", "Value": "weekly"}]),
                ),
                ("/PolicyDetails/Schedules/0/Name", json!("weekly")),
                ("/PolicyDetails/Schedules/0/TagsToAdd", Value::Null),
                ("/PolicyDetails/Schedules/0/CreateRule/Interval", json!(12)),
                (
                    // `CreateRule$Location` is `[Custom snapshot policies only]`,
                    // so a snapshot policy's creation rule carries the
                    // materialized location.
                    "/PolicyDetails/Schedules/0/CreateRule/Location",
                    json!("CLOUD"),
                ),
                ("/PolicyDetails/Schedules/0/CopyTags", json!(false)),
            ],
            preserved: vec![
                "/State",
                "/Description",
                "/ExecutionRoleArn",
                "/Tags",
                "/DateCreated",
                // "You cannot update the policy type or the resource type":
                // `policy_identity_immutability.rs` owns that rule, and a
                // wholesale replacement does not touch either member.
                "/PolicyDetails/PolicyType",
                "/PolicyDetails/ResourceTypes",
            ],
        },
        UpdateCase {
            case: "the default-policy intervals and flags",
            create: default_volume.clone(),
            patch: json!({
                "CreateInterval": 3,
                "RetainInterval": 14,
                "CopyTags": true,
                "ExtendDeletion": true,
            }),
            changed: vec![
                ("/PolicyDetails/CreateInterval", json!(3)),
                ("/PolicyDetails/RetainInterval", json!(14)),
                ("/PolicyDetails/CopyTags", json!(true)),
                ("/PolicyDetails/ExtendDeletion", json!(true)),
            ],
            preserved: vec![
                "/State",
                "/Description",
                "/ExecutionRoleArn",
                "/DefaultPolicy",
                "/PolicyDetails/ResourceType",
                "/DateCreated",
            ],
        },
        UpdateCase {
            case: "the default-policy copy targets and exclusions",
            create: default_volume.clone(),
            patch: json!({
                "CrossRegionCopyTargets": [{"TargetRegion": OTHER_REGION}],
                "Exclusions": {"ExcludeBootVolumes": true},
            }),
            changed: vec![
                (
                    "/PolicyDetails/CrossRegionCopyTargets",
                    json!([{"TargetRegion": OTHER_REGION}]),
                ),
                (
                    "/PolicyDetails/Exclusions",
                    json!({"ExcludeBootVolumes": true}),
                ),
            ],
            preserved: vec![
                "/State",
                "/Description",
                "/PolicyDetails/CreateInterval",
                "/DateCreated",
            ],
        },
        UpdateCase {
            case: "no member at all",
            create: custom.clone(),
            patch: json!({}),
            changed: vec![],
            preserved: vec![
                "/State",
                "/Description",
                "/ExecutionRoleArn",
                "/PolicyDetails",
                "/Tags",
                "/DateCreated",
            ],
        },
    ];

    let router = emulator();
    for UpdateCase {
        case,
        create: create_request,
        patch,
        changed,
        preserved,
    } in cases
    {
        let id = create(&router, create_request).await.policy_id();
        let before = get(&router, &id).await;
        let before_policy = before.policy().clone();

        let patched = update(&router, &id, patch).await;
        assert_eq!(
            patched.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            patched.status, patched.body
        );
        assert_eq!(
            patched.body,
            json!({}),
            "case {case:?}: UpdateLifecyclePolicy has no output members, got {:?}",
            patched.body
        );

        let after = get(&router, &id).await;
        for (pointer, expected) in changed {
            assert_eq!(
                at(after.policy(), pointer),
                expected,
                "case {case:?}: {pointer} should now be {expected:?}, got {:?}",
                at(after.policy(), pointer)
            );
        }
        for pointer in preserved {
            assert_eq!(
                at(after.policy(), pointer),
                at(&before_policy, pointer),
                "case {case:?}: {pointer} was not in the request, so it must still be {:?}",
                at(&before_policy, pointer)
            );
        }
        assert!(
            timestamp_key(&at(after.policy(), "/DateModified"))
                >= timestamp_key(&at(after.policy(), "/DateCreated")),
            "case {case:?}: DateModified {:?} must not precede DateCreated {:?}",
            at(after.policy(), "/DateModified"),
            at(after.policy(), "/DateCreated")
        );
    }
}

/// `DateModified` is "the local date and time when the lifecycle policy was last
/// modified", so an update moves it forward while `DateCreated` stays put.
///
/// The wait is what makes the two instants distinguishable at any timestamp
/// precision the `date-time` format renders.
#[tokio::test]
async fn an_update_advances_date_modified_and_leaves_date_created() {
    let router = emulator();
    let id = create(
        &router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "nightly ebs",
            "State": "ENABLED",
            "PolicyDetails": custom_snapshot_details(),
        }),
    )
    .await
    .policy_id();
    let before = get(&router, &id).await;
    let created_at = timestamp_key(&at(before.policy(), "/DateCreated"));
    let modified_before = timestamp_key(&at(before.policy(), "/DateModified"));

    tokio::time::sleep(Duration::from_millis(1_050)).await;
    update(&router, &id, json!({"State": "DISABLED"})).await;

    let after = get(&router, &id).await;
    let modified_after = timestamp_key(&at(after.policy(), "/DateModified"));
    assert!(
        modified_after > modified_before,
        "DateModified must advance past {modified_before:?}, got {modified_after:?}"
    );
    assert_eq!(
        timestamp_key(&at(after.policy(), "/DateCreated")),
        created_at,
        "DateCreated must not move"
    );
}

/// A delete removes the record, and nothing afterwards reports the policy: the
/// get 404s, the list omits it, and a second delete 404s too. Every other policy
/// is untouched.
#[tokio::test]
async fn a_deleted_policy_is_gone_from_every_reader() {
    let router = emulator();
    let fixture = filter_fixture(&router).await;
    let doomed = fixture.enabled_volume.clone();

    let deleted = delete(&router, &doomed).await;
    assert_eq!(
        deleted.status, 200,
        "expected HTTP 200 from DeleteLifecyclePolicy, got {} with {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.body,
        json!({}),
        "DeleteLifecyclePolicy has no output members, got {:?}",
        deleted.body
    );

    let read = get(&router, &doomed).await;
    assert_eq!(
        read.status, 404,
        "a deleted policy must not be readable, got {} with {:?}",
        read.status, read.body
    );
    assert!(
        read.error_code().ends_with("ResourceNotFoundException"),
        "expected ResourceNotFoundException, got {:?}",
        read.error_code()
    );

    let listed = listed_ids(&list(&router, "").await);
    assert!(
        !listed.contains(&doomed),
        "a deleted policy must not be listed, got {listed:?}"
    );
    for survivor in [
        &fixture.disabled_instance,
        &fixture.default_volume,
        &fixture.default_instance,
    ] {
        assert!(
            listed.contains(survivor),
            "deleting one policy must not remove {survivor:?}: {listed:?}"
        );
    }

    let again = delete(&router, &doomed).await;
    assert_eq!(
        again.status, 404,
        "a second delete of the same id must 404, got {} with {:?}",
        again.status, again.body
    );
}

/// An id that names no policy is a `ResourceNotFoundException` (HTTP 404) from
/// every operation that looks one up — never a 501 and never a 200.
///
/// The rows span the three id-addressed operations, the list's `policyIds`
/// filter, and more than one shape of unknown id.
#[tokio::test]
async fn an_unknown_policy_id_is_not_found_everywhere() {
    let router = emulator();
    // A live policy, so the store is not empty and a 404 cannot come from that.
    let live = create(
        &router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "a policy that does exist",
            "State": "ENABLED",
            "PolicyDetails": custom_snapshot_details(),
        }),
    )
    .await
    .policy_id();

    // Both satisfy `PolicyId`'s `^policy-[a-f0-9]+$`, so the boundary validator
    // admits them and the handler is what must refuse them.
    for unknown in ["policy-deadbeef", "policy-0"] {
        let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
            (
                "GetLifecyclePolicy",
                "GET",
                format!("/policies/{unknown}"),
                None,
            ),
            (
                "UpdateLifecyclePolicy",
                "PATCH",
                format!("/policies/{unknown}"),
                Some(json!({"State": "DISABLED"})),
            ),
            (
                "DeleteLifecyclePolicy",
                "DELETE",
                format!("/policies/{unknown}"),
                None,
            ),
            (
                "GetLifecyclePolicies with policyIds",
                "GET",
                format!("/policies?policyIds={unknown}"),
                None,
            ),
            (
                "GetLifecyclePolicies with a known and an unknown id",
                "GET",
                format!("/policies?policyIds={live}&policyIds={unknown}"),
                None,
            ),
        ];

        for (case, method, uri, body) in cases {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} with {unknown:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with("ResourceNotFoundException"),
                "case {case:?} with {unknown:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }

    // And the live policy is still readable: a refused lookup changed nothing.
    assert_eq!(
        at(get(&router, &live).await.policy(), "/PolicyId"),
        json!(live),
        "a 404 for another id must not disturb the policy that does exist"
    );
}

/// The tag operations read and write the very tags the create stored, addressing
/// the policy by its stored `PolicyArn`.
#[tokio::test]
async fn tag_operations_read_and_write_the_stored_tags() {
    let router = emulator();
    let created_with_tags = create(
        &router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "tagged policy",
            "State": "ENABLED",
            "PolicyDetails": custom_snapshot_details(),
            "Tags": {"team": "backups", "env": "prod"},
        }),
    )
    .await
    .policy_id();
    let created_bare = create(
        &router,
        json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "untagged policy",
            "State": "ENABLED",
            "PolicyDetails": custom_snapshot_details(),
        }),
    )
    .await
    .policy_id();

    let tagged_arn = at(get(&router, &created_with_tags).await.policy(), "/PolicyArn")
        .as_str()
        .expect("a created policy reports its ARN")
        .to_string();
    let bare_arn = at(get(&router, &created_bare).await.policy(), "/PolicyArn")
        .as_str()
        .expect("a created policy reports its ARN")
        .to_string();

    // What the create stored, read through the ARN.
    let listed = call(&router, "GET", &tags_uri(&tagged_arn), None).await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 from ListTagsForResource, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        at(&listed.body, "/Tags"),
        json!({"team": "backups", "env": "prod"}),
        "ListTagsForResource must return exactly the tags the create stored, got {:?}",
        at(&listed.body, "/Tags")
    );

    // An untagged policy reports the EMPTY map, by the same rule the policy read
    // follows: the tag set it holds, which is empty.
    let bare = call(&router, "GET", &tags_uri(&bare_arn), None).await;
    assert_eq!(
        at(&bare.body, "/Tags"),
        json!({}),
        "an untagged policy reports an empty Tags map, got {:?}",
        at(&bare.body, "/Tags")
    );

    // Adding a new key keeps the old ones; reusing a key replaces its value.
    let tagged = call(
        &router,
        "POST",
        &tags_uri(&tagged_arn),
        Some(json!({"Tags": {"env": "staging", "owner": "platform"}})),
    )
    .await;
    assert_eq!(
        tagged.status, 204,
        "expected HTTP 204 from TagResource, got {} with {:?}",
        tagged.status, tagged.body
    );
    let after_tag = call(&router, "GET", &tags_uri(&tagged_arn), None).await;
    assert_eq!(
        at(&after_tag.body, "/Tags"),
        json!({"team": "backups", "env": "staging", "owner": "platform"}),
        "TagResource adds keys and replaces the value of a key already held, got {:?}",
        at(&after_tag.body, "/Tags")
    );
    // And the policy itself reports the same tags, from the same record.
    assert_eq!(
        at(get(&router, &created_with_tags).await.policy(), "/Tags"),
        json!({"team": "backups", "env": "staging", "owner": "platform"}),
        "GetLifecyclePolicy and ListTagsForResource must read one record"
    );

    // Untagging removes the named keys and leaves the rest; a key the policy
    // does not hold is not an error.
    let untagged = call(
        &router,
        "DELETE",
        &format!("{}?tagKeys=env&tagKeys=absent", tags_uri(&tagged_arn)),
        None,
    )
    .await;
    assert_eq!(
        untagged.status, 200,
        "expected HTTP 200 from UntagResource, got {} with {:?}",
        untagged.status, untagged.body
    );
    let after_untag = call(&router, "GET", &tags_uri(&tagged_arn), None).await;
    assert_eq!(
        at(&after_untag.body, "/Tags"),
        json!({"team": "backups", "owner": "platform"}),
        "UntagResource removes the named keys and leaves the rest, got {:?}",
        at(&after_untag.body, "/Tags")
    );

    // An ARN naming no policy is a 404 from each of the three operations —
    // including the ARN of a policy that has been deleted.
    delete(&router, &created_bare).await;
    let unknown_arn = format!(
        "arn:aws:dlm:{REGION}:{}:policy/policy-deadbeef",
        engine::emulator_account_id()
    );
    for arn in [bare_arn.as_str(), unknown_arn.as_str()] {
        let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
            ("ListTagsForResource", "GET", tags_uri(arn), None),
            (
                "TagResource",
                "POST",
                tags_uri(arn),
                Some(json!({"Tags": {"team": "backups"}})),
            ),
            (
                "UntagResource",
                "DELETE",
                format!("{}?tagKeys=team", tags_uri(arn)),
                None,
            ),
        ];
        for (case, method, uri, body) in cases {
            let answer = call(&router, method, &uri, body).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?} with {arn:?}: expected HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert!(
                answer.error_code().ends_with("ResourceNotFoundException"),
                "case {case:?} with {arn:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }
}

/// `CreateLifecyclePolicy` documents one cross-member rule the model cannot
/// express: a DEFAULT policy states its configuration "either in the request
/// body, or in the PolicyDetails request structure, but not both".
///
/// The rows span each body parameter the rule covers, the combination of them,
/// and the two requests the rule must NOT refuse.
#[tokio::test]
async fn a_default_policy_may_not_state_its_configuration_twice() {
    let router = emulator();
    let refused: Vec<(&str, Value)> = vec![
        ("CreateInterval", json!(3)),
        ("RetainInterval", json!(14)),
        ("CopyTags", json!(true)),
        ("ExtendDeletion", json!(true)),
        (
            "CrossRegionCopyTargets",
            json!([{"TargetRegion": OTHER_REGION}]),
        ),
        ("Exclusions", json!({"ExcludeBootVolumes": true})),
    ];

    for (member, value) in &refused {
        let mut request = json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "default policy stated twice",
            "State": "ENABLED",
            "DefaultPolicy": "VOLUME",
            "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME"},
        });
        // The same parameter in BOTH places: the request body, and the
        // configuration structure beside it.
        request[*member] = value.clone();
        request["PolicyDetails"][*member] = value.clone();
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 400,
            "case {member:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().ends_with("InvalidRequestException"),
            "case {member:?}: expected InvalidRequestException, got {:?}",
            answer.error_code()
        );
        let conflicting = at(&answer.body, "/MutuallyExclusiveParameters");
        assert!(
            conflicting
                .as_array()
                .is_some_and(|names| names.contains(&json!(member))
                    && names.contains(&json!("PolicyDetails"))),
            "case {member:?}: the refusal must name both parameters, got {conflicting:?}"
        );
    }

    // Every body parameter at once is still one refusal, naming them all.
    let mut all = json!({
        "ExecutionRoleArn": ROLE_ARN,
        "Description": "default policy stated twice",
        "State": "ENABLED",
        "DefaultPolicy": "VOLUME",
        "PolicyDetails": {"PolicyLanguage": "SIMPLIFIED", "ResourceType": "VOLUME"},
    });
    for (member, value) in &refused {
        all[*member] = value.clone();
        all["PolicyDetails"][*member] = value.clone();
    }
    let answer = create(&router, all).await;
    assert_eq!(
        answer.status, 400,
        "case \"every body parameter\": expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    let conflicting = at(&answer.body, "/MutuallyExclusiveParameters");
    for (member, _) in &refused {
        assert!(
            conflicting
                .as_array()
                .is_some_and(|names| names.contains(&json!(member))),
            "case \"every body parameter\": the refusal must name {member:?}, got {conflicting:?}"
        );
    }

    // The two requests the rule leaves alone: the configuration in ONE place.
    let accepted: Vec<(&str, Value)> = vec![
        (
            "configuration in the request body only",
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "default policy from the body",
                "State": "ENABLED",
                "DefaultPolicy": "VOLUME",
                "CreateInterval": 3,
                "RetainInterval": 14,
            }),
        ),
        (
            "configuration in PolicyDetails only",
            json!({
                "ExecutionRoleArn": ROLE_ARN,
                "Description": "default policy from PolicyDetails",
                "State": "ENABLED",
                "PolicyDetails": {
                    "PolicyLanguage": "SIMPLIFIED",
                    "ResourceType": "VOLUME",
                    "CreateInterval": 3,
                    "RetainInterval": 14,
                },
            }),
        ),
    ];
    for (case, request) in accepted {
        let answer = create(&router, request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected HTTP 200, got {} with {:?}",
            answer.status, answer.body
        );
        let read = get(&router, &answer.policy_id()).await;
        assert_eq!(
            at(read.policy(), "/DefaultPolicy"),
            json!(true),
            "case {case:?}: both ways of stating it make a DEFAULT policy, got {:?}",
            at(read.policy(), "/DefaultPolicy")
        );
        assert_eq!(
            at(read.policy(), "/PolicyDetails/CreateInterval"),
            json!(3),
            "case {case:?}: the configuration is reported wherever it was stated"
        );
    }
}
