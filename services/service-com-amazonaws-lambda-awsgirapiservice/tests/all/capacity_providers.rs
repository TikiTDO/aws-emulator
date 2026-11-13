// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for a capacity provider: `CreateCapacityProvider` "creates a
//! capacity provider that manages compute resources for Lambda Event Functions", and
//! `GetCapacityProvider` "retrieves information about a specific capacity provider,
//! including its configuration, state, and associated resources".
//!
//! The rule under test: `CreateCapacityProvider` persists ONE capacity provider under
//! the name the request names — carrying the VPC configuration, the operator role, the
//! instance requirements, the scaling configuration, the encryption key, the tag
//! propagation and the telemetry configuration exactly as supplied, under an ARN minted
//! once from the region the caller signed for and that name — answers that stored record
//! in the `Pending` state with the instant it was last modified, and refuses a name the
//! account already holds a provider of; `GetCapacityProvider` then reports the same
//! record, by either documented name form, stably across reads, and reports `Active`
//! once the clock has made the provider's activation due.
//!
//! Every row drives real HTTP requests through the router and reads the provider back
//! through `GetCapacityProvider`, so a provider that was answered but never stored fails
//! here. The rows differ along what the rule generalises over: which optional
//! configuration the request states, the provider name (its length bounds, and the ARN
//! form), the number of subnets and security groups (each modelled bound), the scaling
//! configuration (each `ScalingMode`, `MaxVCpuCount` at each published bound, a
//! fractional `TargetValue` at each bound of its range), the instance requirements
//! (each architecture, allowed and excluded instance types) and the tags.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCapacityProvider` and `ListCapacityProviders` are bound, from
/// `smithy_query(query="operation", operation="CreateCapacityProvider")`
/// (`POST /2025-11-30/capacity-providers`).
const CAPACITY_PROVIDERS_URI: &str = "/2025-11-30/capacity-providers";

/// Where `ListTags` is bound (`smithy_query(query="operation", operation="ListTags")`:
/// `GET /2017-03-31/tags/{Resource}`), which is how a provider's tags are read back:
/// the `CapacityProvider` the create and the get answer carries none.
const TAGS_URI: &str = "/2017-03-31/tags";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in
/// `us-east-1`, which is what routes a request here and what region the ARNs carry.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed operator role ARN, satisfying the model's `RoleArn` pattern
/// (`^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$`).
const OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-capacity";

/// A second well-formed operator role ARN, so a row can differ in the role it names.
const OTHER_OPERATOR_ROLE: &str = "arn:aws:iam::000000000000:role/capacity-operator";

/// A well-formed KMS key ARN, satisfying the model's `KMSKeyArnNonEmpty` pattern
/// (`^arn:(aws[a-zA-Z-]*)?:[a-z0-9-.]+:.*$`).
const KMS_KEY: &str =
    "arn:aws:kms:us-east-1:000000000000:key/0b1c2d3e-4f56-7890-abcd-ef1234567890";

/// The `CapacityProviderState` a freshly created provider carries, from the model's
/// `CapacityProviderState` enum (`Pending | Active | Failed | Deleting`).
const STATE_PENDING: &str = "Pending";
/// See [`STATE_PENDING`].
const STATE_ACTIVE: &str = "Active";

/// The bounds the model puts on `CapacityProviderVpcConfig$SubnetIds`
/// (`@length(min: 1, max: 16)`), from
/// `smithy_query(query="shape", shape="CapacityProviderVpcConfig")`.
const SUBNETS_MIN: usize = 1;
const SUBNETS_MAX: usize = 16;

/// The bounds the model puts on `CapacityProviderVpcConfig$SecurityGroupIds`
/// (`@length(min: 0, max: 5)`), from the same shape.
const SECURITY_GROUPS_MAX: usize = 5;

/// The bounds the model puts on `CapacityProviderScalingConfig$MaxVCpuCount`
/// (`@range(min: 2, max: 15000)`), from
/// `smithy_query(query="shape", shape="CapacityProviderMaxVCpuCount")`.
const MAX_VCPU_MIN: i64 = 2;
const MAX_VCPU_MAX: i64 = 15_000;

/// The bounds the model puts on `TargetTrackingScalingPolicy$TargetValue`
/// (`@range(min: 0, max: 100)`), from
/// `smithy_query(query="shape", shape="MetricTargetValue")`.
const TARGET_VALUE_MIN: f64 = 0.0;
const TARGET_VALUE_MAX: f64 = 100.0;

/// The one `CapacityProviderPredefinedMetricType` the model declares.
const PREDEFINED_METRIC: &str = "LambdaCapacityProviderAverageCPUUtilization";

/// The longest provider name the `Resource` member of the tag operations admits
/// (`(capacity-provider|network-connector):[a-zA-Z0-9-_]{1,64}`).
const TAG_RESOURCE_NAME_MAX: usize = 64;

/// The longest provider name whose minted ARN still satisfies `CapacityProviderArn`'s
/// `@length(max: 140)`: the ARN prefix
/// `arn:aws:lambda:us-east-1:000000000000:capacity-provider:` is 56 characters.
const LONGEST_NAME_LEN: usize = 140 - 56;

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The `CapacityProvider` member of the body, which both operations answer with.
    fn provider(&self) -> Value {
        self.json()
            .get("CapacityProvider")
            .cloned()
            .unwrap_or(Value::Null)
    }

    /// A `String` member of the answered `CapacityProvider`.
    fn provider_string(&self, member: &str) -> Option<String> {
        self.provider()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `CreateCapacityProvider`.
async fn create_provider(router: &MultiServiceRouter, body: &Value) -> Answer {
    call(router, "POST", CAPACITY_PROVIDERS_URI, body).await
}

/// `GetCapacityProvider` on the provider `reference` names.
///
/// The name goes into the path as given: both documented `CapacityProviderName` forms
/// are made of path-safe characters (a bare name, or an ARN, whose `:` separators are
/// legal in a path segment).
async fn get_provider(router: &MultiServiceRouter, reference: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("{CAPACITY_PROVIDERS_URI}/{reference}"),
        &Value::Null,
    )
    .await
}

/// `ListTags` on the resource `arn` names.
async fn list_tags(router: &MultiServiceRouter, arn: &str) -> Answer {
    call(router, "GET", &format!("{TAGS_URI}/{arn}"), &Value::Null).await
}

/// The ARN a capacity provider of `name` carries, as the model's `CapacityProviderArn`
/// pattern spells it.
fn provider_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:capacity-provider:{name}")
}

/// A subnet id satisfying the model's `SubnetId` pattern (`^subnet-[0-9a-z]*$`).
fn subnet_id(index: usize) -> String {
    format!("subnet-{:08x}", 0x1000_0000usize + index)
}

/// A security group id satisfying the model's `SecurityGroupId` pattern
/// (`^sg-[0-9a-zA-Z]*$`).
fn security_group_id(index: usize) -> String {
    format!("sg-{:08X}", 0x2000_0000usize + index)
}

/// `n` subnet ids, as a JSON list.
fn subnets(count: usize) -> Value {
    Value::Array((0..count).map(|index| json!(subnet_id(index))).collect())
}

/// `n` security group ids, as a JSON list.
fn security_groups(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!(security_group_id(index)))
            .collect(),
    )
}

/// Drop the JSON members that are absent and read every number as the one JSON number
/// type, so a row states only the members it configured — a member the emulator invented
/// shows up as a difference — and `2` and `2.0`, which JSON does not distinguish, do not.
fn normalized(value: &Value) -> Value {
    match value {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(_, member)| !member.is_null())
                .map(|(name, member)| (name.clone(), normalized(member)))
                .collect(),
        ),
        Value::Array(members) => Value::Array(members.iter().map(normalized).collect()),
        Value::Number(number) => number
            .as_f64()
            .and_then(serde_json::Number::from_f64)
            .map_or_else(|| value.clone(), Value::Number),
        other => other.clone(),
    }
}

/// The configuration members of an answered `CapacityProvider`, which the rule says are
/// the ones the request supplied.
fn reported_configuration(answer: &Answer) -> Value {
    let provider = answer.provider();
    normalized(&json!({
        "VpcConfig": provider["VpcConfig"].clone(),
        "PermissionsConfig": provider["PermissionsConfig"].clone(),
        "InstanceRequirements": provider["InstanceRequirements"].clone(),
        "CapacityProviderScalingConfig": provider["CapacityProviderScalingConfig"].clone(),
        "KmsKeyArn": provider["KmsKeyArn"].clone(),
        "PropagateTags": provider["PropagateTags"].clone(),
        "TelemetryConfig": provider["TelemetryConfig"].clone(),
    }))
}

/// The same members, taken from the request that created the provider: the rule is that
/// the record holds what was supplied, so this is what the answer has to report.
fn requested_configuration(request: &Value) -> Value {
    normalized(&json!({
        "VpcConfig": request["VpcConfig"].clone(),
        "PermissionsConfig": request["PermissionsConfig"].clone(),
        "InstanceRequirements": request["InstanceRequirements"].clone(),
        "CapacityProviderScalingConfig": request["CapacityProviderScalingConfig"].clone(),
        "KmsKeyArn": request["KmsKeyArn"].clone(),
        "PropagateTags": request["PropagateTags"].clone(),
        "TelemetryConfig": request["TelemetryConfig"].clone(),
    }))
}

/// One row of the capacity-provider table: the request, the bare name the provider is
/// stored under, the reference `GetCapacityProvider` addresses it by and the tags the
/// record must then hold (they are not reported on the wire).
struct ProviderCase {
    name: &'static str,
    provider: String,
    /// The `CapacityProviderName` form the request creates it under; `None` is the bare
    /// name.
    created_as: Option<String>,
    /// The form `GetCapacityProvider` reads it back by; `None` is the bare name.
    read_as: Option<String>,
    request_extras: Value,
    expected_tags: Vec<(String, String)>,
}

/// Merge the members of `extras` into a create request for `provider`.
fn create_request(name: &str, extras: &Value) -> Value {
    let mut body = json!({ "CapacityProviderName": name });
    if let (Value::Object(target), Value::Object(members)) = (&mut body, extras) {
        for (member, value) in members {
            target.insert(member.clone(), value.clone());
        }
    }
    body
}

/// The rows. Each differs from the others in the optional configuration it states, the
/// name form, the subnet and security-group counts, the scaling configuration, the
/// instance requirements or the tags — the dimensions the rule generalises over.
fn provider_cases() -> Vec<ProviderCase> {
    vec![
        // Nothing but the required members, each list at its modelled minimum: the
        // answer carries no optional configuration at all.
        ProviderCase {
            name: "only the required configuration, with the fewest subnets",
            provider: "minimal-provider".to_string(),
            created_as: None,
            read_as: None,
            request_extras: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(SUBNETS_MIN),
                    "SecurityGroupIds": security_groups(0),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
            }),
            expected_tags: Vec::new(),
        },
        // Every optional member stated, each list at its modelled maximum, automatic
        // scaling with a fractional target value, tags and explicit tag propagation.
        ProviderCase {
            name: "every optional member, with the most subnets and security groups",
            provider: "full-provider".to_string(),
            created_as: None,
            read_as: None,
            request_extras: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(SUBNETS_MAX),
                    "SecurityGroupIds": security_groups(SECURITY_GROUPS_MAX),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OTHER_OPERATOR_ROLE },
                "InstanceRequirements": {
                    "Architectures": ["arm64"],
                    "AllowedInstanceTypes": ["c7g.large", "m7g.*"],
                },
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": 128,
                    "ScalingMode": "Auto",
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": 62.5,
                    }],
                },
                "KmsKeyArn": KMS_KEY,
                "Tags": { "team": "platform", "env": "test" },
                "PropagateTags": {
                    "Mode": "Explicit",
                    "ExplicitTags": { "cost-center": "4711" },
                },
                "TelemetryConfig": {
                    "LoggingConfig": { "SystemLogLevel": "DEBUG", "LogGroup": "/aws/lambda/capacity" },
                },
            }),
            expected_tags: vec![
                ("env".to_string(), "test".to_string()),
                ("team".to_string(), "platform".to_string()),
            ],
        },
        // Manual scaling at the bottom of the `MaxVCpuCount` range, the other
        // architecture, excluded instance types, and the target value at its minimum.
        ProviderCase {
            name: "manual scaling at the smallest vcpu count, excluding instance types",
            provider: "manual_provider".to_string(),
            created_as: None,
            read_as: None,
            request_extras: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(2),
                    "SecurityGroupIds": security_groups(1),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
                "InstanceRequirements": {
                    "Architectures": ["x86_64"],
                    "ExcludedInstanceTypes": ["t2.nano"],
                },
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": MAX_VCPU_MIN,
                    "ScalingMode": "Manual",
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MIN,
                    }],
                },
                "Tags": { "single": "" },
            }),
            expected_tags: vec![("single".to_string(), String::new())],
        },
        // The top of the `MaxVCpuCount` range and of the target-value range, with the
        // most scaling policies one `PredefinedMetricType` can be stated with.
        ProviderCase {
            name: "automatic scaling at the largest vcpu count and target value",
            provider: "ceiling-provider".to_string(),
            created_as: None,
            read_as: None,
            request_extras: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(3),
                    "SecurityGroupIds": security_groups(2),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
                "CapacityProviderScalingConfig": {
                    "MaxVCpuCount": MAX_VCPU_MAX,
                    "ScalingMode": "Auto",
                    "ScalingPolicies": [{
                        "PredefinedMetricType": PREDEFINED_METRIC,
                        "TargetValue": TARGET_VALUE_MAX,
                    }],
                },
            }),
            expected_tags: Vec::new(),
        },
        // A one-character name: the shortest `CapacityProviderName` the model admits.
        ProviderCase {
            name: "the shortest name the model admits",
            provider: "p".to_string(),
            created_as: None,
            read_as: None,
            request_extras: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(1),
                    "SecurityGroupIds": security_groups(0),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
                "PropagateTags": { "Mode": "None" },
            }),
            expected_tags: Vec::new(),
        },
        // The longest name whose ARN still fits `CapacityProviderArn`, read back by that
        // ARN: the other documented `CapacityProviderName` form names the same record.
        ProviderCase {
            name: "the longest name, read back by its arn",
            provider: "l".repeat(LONGEST_NAME_LEN),
            created_as: None,
            read_as: Some(provider_arn(&"l".repeat(LONGEST_NAME_LEN))),
            request_extras: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(1),
                    "SecurityGroupIds": security_groups(0),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
            }),
            expected_tags: Vec::new(),
        },
        // Created BY its ARN, read back by the bare name: the same record either way.
        ProviderCase {
            name: "created by its arn, read back by its name",
            provider: "arn-created".to_string(),
            created_as: Some(provider_arn("arn-created")),
            read_as: None,
            request_extras: json!({
                "VpcConfig": {
                    "SubnetIds": subnets(1),
                    "SecurityGroupIds": security_groups(0),
                },
                "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
                "Tags": { "created-by": "arn" },
            }),
            expected_tags: vec![("created-by".to_string(), "arn".to_string())],
        },
    ]
}

#[tokio::test]
async fn a_created_capacity_provider_is_persisted_and_reported() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let mut arns: BTreeSet<String> = BTreeSet::new();

    for case in provider_cases() {
        let label = case.name;
        let request = create_request(
            case.created_as.as_deref().unwrap_or(&case.provider),
            &case.request_extras,
        );
        let created = create_provider(&router, &request).await;
        assert_eq!(
            created.status,
            202,
            "case {label:?}: expected 202 from CreateCapacityProvider, got {} {}",
            created.status,
            created.text()
        );

        let expected = requested_configuration(&request);
        assert_eq!(
            reported_configuration(&created),
            expected,
            "case {label:?}: expected the configuration the request described, {expected}, \
             got {}",
            created.text()
        );

        let arn = provider_arn(&case.provider);
        assert_eq!(
            created.provider_string("CapacityProviderArn"),
            Some(arn.clone()),
            "case {label:?}: expected the ARN {arn:?}, got {:?}",
            created.provider_string("CapacityProviderArn")
        );
        assert!(
            arns.insert(arn.clone()),
            "case {label:?}: every provider gets its own ARN, but {arn:?} was minted twice"
        );
        assert_eq!(
            created.provider_string("State").as_deref(),
            Some(STATE_PENDING),
            "case {label:?}: a created provider is {STATE_PENDING}, got {:?}",
            created.provider_string("State")
        );
        let last_modified = created.provider_string("LastModified").unwrap_or_default();
        assert!(
            last_modified.starts_with("20") && last_modified.ends_with("+0000"),
            "case {label:?}: expected an ISO-8601 LastModified carrying the numeric UTC \
             offset this service renders (the form the model's own instant example, \
             `2018-11-27T15:10:45.123+0000`, spells), got {last_modified:?}"
        );

        // Read the provider back: one that was answered but never persisted fails here.
        let reference = case.read_as.clone().unwrap_or_else(|| case.provider.clone());
        let fetched = get_provider(&router, &reference).await;
        assert_eq!(
            fetched.status,
            200,
            "case {label:?}: expected 200 from GetCapacityProvider on {reference:?}, got {} {}",
            fetched.status,
            fetched.text()
        );
        assert_eq!(
            reported_configuration(&fetched),
            expected,
            "case {label:?}: GetCapacityProvider must report the created configuration \
             {expected}, got {}",
            fetched.text()
        );
        assert_eq!(
            (
                fetched.provider_string("CapacityProviderArn"),
                fetched.provider_string("LastModified")
            ),
            (Some(arn.clone()), Some(last_modified.clone())),
            "case {label:?}: expected the stored ARN {arn:?} and LastModified \
             {last_modified:?}, got {}",
            fetched.text()
        );

        // A read changes nothing a later caller sees: the same provider comes back, and
        // its state only ever moves the way the clock owed it.
        let again = get_provider(&router, &reference).await;
        assert_eq!(
            (again.status, reported_configuration(&again)),
            (200, expected.clone()),
            "case {label:?}: the provider must be stable across reads, got {} {}",
            again.status,
            again.text()
        );
        let states = [
            fetched.provider_string("State").unwrap_or_default(),
            again.provider_string("State").unwrap_or_default(),
        ];
        assert!(
            states
                .iter()
                .all(|state| state == STATE_PENDING || state == STATE_ACTIVE),
            "case {label:?}: a read reports the state the clock owed, got {states:?}"
        );

        // The store itself holds exactly one record, under the bare name, with the tags
        // the request associated with it (they are not reported on the wire).
        let (stored_name, stored_arn, stored_tags) = {
            let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
            let store = state.lock();
            let record = store
                .capacity_providers
                .get(&case.provider)
                .unwrap_or_else(|| panic!("case {label:?}: the provider must be stored"));
            (
                record.name.clone(),
                record.arn.clone(),
                record
                    .tags
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect::<Vec<(String, String)>>(),
            )
        };
        assert_eq!(
            (stored_name, stored_arn, stored_tags),
            (case.provider.clone(), arn.clone(), case.expected_tags.clone()),
            "case {label:?}: expected the record {:?} under {arn:?} with tags {:?}",
            case.provider,
            case.expected_tags
        );

        // The tags the request associated with the provider are the ones `ListTags`
        // reports for its ARN: they are held on the record, not on the wire answer. Its
        // `Resource` member admits a provider name of at most 64 characters
        // (`(capacity-provider|network-connector):[a-zA-Z0-9-_]{1,64}`, from
        // `smithy_query(query="operation", operation="ListTags")`), so a longer name —
        // which `CapacityProviderName` itself allows — names no tag ARN to ask for.
        if case.provider.len() <= TAG_RESOURCE_NAME_MAX {
            let tagged = list_tags(&router, &arn).await;
            let expected_tags = Value::Object(
                case.expected_tags
                    .iter()
                    .map(|(key, value)| (key.clone(), json!(value)))
                    .collect(),
            );
            assert_eq!(
                (tagged.status, tagged.json().get("Tags").cloned()),
                (200, Some(expected_tags.clone())),
                "case {label:?}: expected ListTags on {arn:?} to report {expected_tags}, got \
                 {} {}",
                tagged.status,
                tagged.text()
            );
        }
    }
}

#[tokio::test]
async fn a_provider_becomes_active_when_the_clock_makes_it_due() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let request = create_request(
        "activating-provider",
        &json!({
            "VpcConfig": {
                "SubnetIds": subnets(1),
                "SecurityGroupIds": security_groups(0),
            },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        }),
    );
    let created = create_provider(&router, &request).await;
    assert_eq!(
        (created.status, created.provider_string("State")),
        (202, Some(STATE_PENDING.to_string())),
        "a created provider starts {STATE_PENDING}, got {} {}",
        created.status,
        created.text()
    );

    // The activation is owed by the clock, not by anybody calling: nothing here sleeps
    // on the emulator's behalf, it just waits past the instant the create made due.
    let delay = service_com_amazonaws_lambda_awsgirapiservice::state::CAPACITY_PROVIDER_ACTIVATION_DELAY;
    tokio::time::sleep(delay + std::time::Duration::from_millis(100)).await;

    for read in 1..=2 {
        let fetched = get_provider(&router, "activating-provider").await;
        assert_eq!(
            (fetched.status, fetched.provider_string("State")),
            (200, Some(STATE_ACTIVE.to_string())),
            "read {read}: expected {STATE_ACTIVE} once the activation was due, got {} {}",
            fetched.status,
            fetched.text()
        );
    }
}

/// One refusal row: what to call, and the status (and, for a modelled error, the code)
/// it must be answered with.
struct RefusalCase {
    name: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    status: u16,
    code: Option<&'static str>,
}

#[tokio::test]
async fn a_request_no_capacity_provider_can_satisfy_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let taken = create_request(
        "taken-provider",
        &json!({
            "VpcConfig": {
                "SubnetIds": subnets(1),
                "SecurityGroupIds": security_groups(0),
            },
            "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
        }),
    );
    let created = create_provider(&router, &taken).await;
    assert_eq!(
        created.status,
        202,
        "the first create must succeed, got {} {}",
        created.status,
        created.text()
    );

    let cases = vec![
        // A name the account already holds a provider of: the resource already exists.
        RefusalCase {
            name: "a provider name that is already taken",
            method: "POST",
            uri: CAPACITY_PROVIDERS_URI.to_string(),
            body: taken.clone(),
            status: 409,
            code: Some("ResourceConflictException"),
        },
        // The same, addressed by the ARN of that provider: the same record, so the same
        // conflict.
        RefusalCase {
            name: "the arn of a provider that already exists",
            method: "POST",
            uri: CAPACITY_PROVIDERS_URI.to_string(),
            body: create_request(
                &provider_arn("taken-provider"),
                &json!({
                    "VpcConfig": {
                        "SubnetIds": subnets(1),
                        "SecurityGroupIds": security_groups(0),
                    },
                    "PermissionsConfig": { "CapacityProviderOperatorRoleArn": OPERATOR_ROLE },
                }),
            ),
            status: 409,
            code: Some("ResourceConflictException"),
        },
        // A provider this account holds none of.
        RefusalCase {
            name: "a get of a provider that does not exist",
            method: "GET",
            uri: format!("{CAPACITY_PROVIDERS_URI}/absent-provider"),
            body: Value::Null,
            status: 404,
            code: Some("ResourceNotFoundException"),
        },
        // An ARN of ANOTHER Lambda resource names no capacity provider at all. The
        // modelled `CapacityProviderName` pattern is what first refuses a value this far
        // from either documented form, so only the 400 is asserted here.
        RefusalCase {
            name: "an arn of another lambda resource",
            method: "GET",
            uri: format!(
                "{CAPACITY_PROVIDERS_URI}/arn:aws:lambda:{REGION}:{ACCOUNT}:function:not-a-provider"
            ),
            body: Value::Null,
            status: 400,
            code: None,
        },
    ];

    for case in cases {
        let answer = call(&router, case.method, &case.uri, &case.body).await;
        assert_eq!(
            answer.status,
            case.status,
            "case {:?}: expected HTTP {}, got {} {}",
            case.name,
            case.status,
            answer.status,
            answer.text()
        );
        if let Some(code) = case.code {
            assert!(
                answer.error_code().contains(code),
                "case {:?}: expected the {code} error code, got {:?}",
                case.name,
                answer.error_code()
            );
        }
    }

    // Every refusal left the store as it was: one provider, the one that was created.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let names: Vec<&String> = store.capacity_providers.keys().collect();
    assert_eq!(
        names,
        vec![&"taken-provider".to_string()],
        "a refused request persists nothing, got {names:?}"
    );
}
