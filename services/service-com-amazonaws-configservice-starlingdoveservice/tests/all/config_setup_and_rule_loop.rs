// SPDX-License-Identifier: CC0-1.0

//! The AWS Config setup-and-rule loop, end to end over the wire.
//!
//! Every case here drives the emulator the way a client does: an `awsJson1_1` `POST /`
//! with `X-Amz-Target: StarlingDoveService.<Operation>`, dispatched through the same
//! `MultiServiceRouter` the production router builds. The assertions are about what the
//! service PERSISTED — a create followed by a describe must return what was created,
//! including the server-generated ARNs, ids and timestamps.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A role ARN that is well formed; its value is never asserted on, only read back.
pub(crate) const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/config-role";

/// Every published `MaximumExecutionFrequency` value, from
/// `smithy_query(query="shape", shape="MaximumExecutionFrequency")` on this service's
/// model: `One_Hour`, `Three_Hours`, `Six_Hours`, `Twelve_Hours`, `TwentyFour_Hours`.
const MAXIMUM_EXECUTION_FREQUENCIES: [&str; 5] = [
    "One_Hour",
    "Three_Hours",
    "Six_Hours",
    "Twelve_Hours",
    "TwentyFour_Hours",
];

/// Every published `RecordingStrategyType` value, from the same model query:
/// `ALL_SUPPORTED_RESOURCE_TYPES`, `INCLUSION_BY_RESOURCE_TYPES`,
/// `EXCLUSION_BY_RESOURCE_TYPES`.
const EXCLUSION_STRATEGY: &str = "EXCLUSION_BY_RESOURCE_TYPES";

/// The page size `DescribeConfigRules` serves (the `ConfigRuleNames` length maximum the
/// same operation declares, which is what this emulator pages at).
const PAGE_SIZE: usize = 25;

/// One emulator instance: a shared engine behind the router, plus the Region its
/// requests are signed for, so an ARN this emulator mints can be checked against it.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) region: String,
    /// The SINGLE shared engine the router hands every handler of this service, kept
    /// so a suite can reach this service's store the way a handler does — for state an
    /// operation of this API mints but hands to nobody (the `ResultToken` Config mints
    /// when it invokes a rule), or that another operation of the service writes.
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    /// An emulator serving requests signed for `region`.
    pub(crate) fn in_region(region: &str) -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self {
            router,
            region: region.to_string(),
            engine,
        }
    }

    /// An emulator in the default test Region.
    pub(crate) fn new() -> Self {
        Self::in_region("us-east-1")
    }

    /// Another emulator over the SAME engine, serving requests signed for `region`:
    /// one account's store reached from two Regions, which is how a suite checks that
    /// a regional resource is served only to the Region it belongs to.
    pub(crate) fn in_other_region(&self, region: &str) -> Self {
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&self.engine)));
        Self {
            router,
            region: region.to_string(),
            engine: Arc::clone(&self.engine),
        }
    }

    /// The SigV4 credential scope that routes the request to this service and names
    /// the Region its ARNs are minted in.
    fn authorization(&self) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{}/config/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef",
            self.region
        )
    }

    /// Call `operation`, returning the HTTP status and the parsed response body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("StarlingDoveService.{operation}"))
            .header("authorization", self.authorization())
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// Call `operation` and require a 200, returning the response body.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with body {parsed}"
        );
        parsed
    }

    /// Call `operation` and require a refusal, returning its status and error code.
    pub(crate) async fn refused(&self, operation: &str, body: &Value) -> (u16, String) {
        let (status, parsed) = self.call(operation, body).await;
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .map(|wire| wire.rsplit('#').next().unwrap_or(wire).to_string())
            .unwrap_or_else(|| format!("<no __type in {parsed}>"));
        (status, code)
    }
}

/// `key` as a bool, defaulting to false the way the modelled `recording` member does.
fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// `recorder`'s ARN, checked against the form Config mints for a configuration
/// recorder — `arn:aws:config:<region>:<account>:configuration-recorder/<name>/<id>`,
/// whose id segment is assigned by the server.
fn recorder_arn_of(recorder: &Value, region: &str, name: &str) -> String {
    let prefix = emulator_server::emulator_arn(
        "config",
        region,
        &format!("configuration-recorder/{name}/"),
    );
    let arn = recorder["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("the recorder must carry an arn, got {recorder}"))
        .to_string();
    assert!(
        arn.starts_with(&prefix),
        "case {name:?}: expected the recorder ARN to start with {prefix:?}, got {arn:?}"
    );
    assert!(
        !arn[prefix.len()..].is_empty(),
        "case {name:?}: expected the recorder ARN to carry the assigned recorder id, got {arn:?}"
    );
    arn
}

/// The recorder body for `PutConfigurationRecorder`.
pub(crate) fn recorder_request(recorder: Value) -> Value {
    json!({ "ConfigurationRecorder": recorder })
}

/// A minimal managed Config rule named `name`.
pub(crate) fn managed_rule(name: &str) -> Value {
    json!({
        "ConfigRule": {
            "ConfigRuleName": name,
            "Source": { "Owner": "AWS", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
        }
    })
}

/// Put the recorder and the delivery channel every rule case needs.
pub(crate) async fn with_recorder_and_channel(emulator: &Emulator) {
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
        .await;
    emulator
        .ok(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
        )
        .await;
}

#[tokio::test]
async fn the_setup_and_rule_loop_round_trips_through_state() {
    let emulator = Emulator::in_region("eu-west-1");

    // 1. The recorder is created and read back with the ARN the service assigned.
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({
                "name": "default",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": true, "includeGlobalResourceTypes": true },
            })),
        )
        .await;

    let described = emulator.ok("DescribeConfigurationRecorders", &json!({})).await;
    let recorders = described["ConfigurationRecorders"]
        .as_array()
        .unwrap_or_else(|| panic!("ConfigurationRecorders must be a list, got {described}"));
    assert_eq!(
        recorders.len(),
        1,
        "one recorder was created, got {described}"
    );
    let recorder = &recorders[0];
    assert_eq!(
        recorder["name"], "default",
        "the stored recorder name must come back, got {recorder}"
    );
    assert_eq!(
        recorder["roleARN"], ROLE_ARN,
        "the stored roleARN must come back, got {recorder}"
    );
    let recorder_arn = recorder_arn_of(recorder, &emulator.region, "default");
    assert_eq!(
        recorder["recordingGroup"]["includeGlobalResourceTypes"],
        json!(true),
        "the stored recording group must come back, got {recorder}"
    );

    // 2. Its status exists and says the recorder is not recording yet.
    let status = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    let first = &status["ConfigurationRecordersStatus"][0];
    assert!(
        !flag(first, "recording"),
        "a recorder that was never started is not recording, got {status}"
    );
    assert!(
        first.get("lastStartTime").is_none(),
        "a recorder that was never started has no lastStartTime, got {status}"
    );
    assert_eq!(
        first["arn"], json!(recorder_arn),
        "the status must report the recorder's stored ARN, got {status}"
    );

    // 3. The delivery channel is persisted and read back.
    emulator
        .ok(
            "PutDeliveryChannel",
            &json!({
                "DeliveryChannel": {
                    "name": "default",
                    "s3BucketName": "config-bucket",
                    "s3KeyPrefix": "config/",
                }
            }),
        )
        .await;
    let channels = emulator.ok("DescribeDeliveryChannels", &json!({})).await;
    assert_eq!(
        channels["DeliveryChannels"][0]["s3BucketName"], "config-bucket",
        "the stored delivery channel must come back, got {channels}"
    );
    assert_eq!(
        channels["DeliveryChannels"][0]["s3KeyPrefix"], "config/",
        "the stored s3KeyPrefix must come back, got {channels}"
    );

    // 4. Starting the recorder flips the persisted flag.
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "default" }),
        )
        .await;
    let status = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    let first = &status["ConfigurationRecordersStatus"][0];
    assert!(
        flag(first, "recording"),
        "a started recorder is recording, got {status}"
    );
    assert!(
        first.get("lastStartTime").is_some(),
        "a started recorder has a lastStartTime, got {status}"
    );

    // 5. A rule is created with a server-assigned id, ARN and ACTIVE state.
    emulator
        .ok("PutConfigRule", &managed_rule("s3-bucket-versioning-enabled"))
        .await;
    let rules = emulator.ok("DescribeConfigRules", &json!({})).await;
    let rule = &rules["ConfigRules"][0];
    let rule_id = rule["ConfigRuleId"]
        .as_str()
        .unwrap_or_else(|| panic!("the rule must carry a ConfigRuleId, got {rules}"));
    assert_eq!(
        rule["ConfigRuleName"], "s3-bucket-versioning-enabled",
        "the stored rule name must come back, got {rules}"
    );
    assert_eq!(
        rule["ConfigRuleState"], "ACTIVE",
        "a new rule is ACTIVE, got {rules}"
    );
    assert_eq!(
        rule["ConfigRuleArn"],
        json!(emulator_server::emulator_arn(
            "config",
            &emulator.region,
            &format!("config-rule/{rule_id}")
        )),
        "the rule ARN must be built from the stored rule id, got {rules}"
    );
    assert_eq!(
        rule["Source"]["SourceIdentifier"], "S3_BUCKET_VERSIONING_ENABLED",
        "the stored rule source must come back, got {rules}"
    );
    assert_eq!(
        rule["EvaluationModes"][0]["Mode"], "DETECTIVE",
        "a rule put without EvaluationModes evaluates in Detective mode only, got {rules}"
    );
    assert!(
        rules.get("NextToken").is_none(),
        "a single page carries no NextToken, got {rules}"
    );

    // 6. Deleting the rule removes it from state; the delete is not idempotent.
    emulator
        .ok(
            "DeleteConfigRule",
            &json!({ "ConfigRuleName": "s3-bucket-versioning-enabled" }),
        )
        .await;
    let rules = emulator.ok("DescribeConfigRules", &json!({})).await;
    assert_eq!(
        rules["ConfigRules"],
        json!([]),
        "the deleted rule must be gone, got {rules}"
    );
    let (status_code, code) = emulator
        .refused(
            "DeleteConfigRule",
            &json!({ "ConfigRuleName": "s3-bucket-versioning-enabled" }),
        )
        .await;
    assert_eq!(
        (status_code, code.as_str()),
        (400, "NoSuchConfigRuleException"),
        "deleting a rule twice must be refused"
    );

    // 7. Stop, then delete the recorder: the describes go back to empty and a start
    //    now fails.
    emulator
        .ok(
            "StopConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "default" }),
        )
        .await;
    emulator
        .ok(
            "DeleteConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "default" }),
        )
        .await;
    let described = emulator.ok("DescribeConfigurationRecorders", &json!({})).await;
    assert_eq!(
        described["ConfigurationRecorders"],
        json!([]),
        "the deleted recorder must be gone, got {described}"
    );
    let status = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    assert_eq!(
        status["ConfigurationRecordersStatus"],
        json!([]),
        "the deleted recorder's status must be gone, got {status}"
    );
    let (status_code, code) = emulator
        .refused(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "default" }),
        )
        .await;
    assert_eq!(
        (status_code, code.as_str()),
        (400, "NoSuchConfigurationRecorderException"),
        "starting a deleted recorder must be refused"
    );
}

#[tokio::test]
async fn a_recorder_is_read_back_exactly_as_it_was_put() {
    // Rows differ along the members `PutConfigurationRecorder` persists: whether the
    // name is defaulted, which recording strategy the group uses, and whether a
    // recording mode is carried.
    let cases: [(&str, Value, &str, Value); 5] = [
        (
            "no name defaults to \"default\"",
            json!({ "roleARN": ROLE_ARN, "recordingGroup": { "allSupported": true } }),
            "default",
            json!({ "allSupported": true }),
        ),
        (
            "an explicit name is kept",
            json!({
                "name": "my-recorder",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": true },
            }),
            "my-recorder",
            json!({ "allSupported": true }),
        ),
        (
            "an inclusion list is stored verbatim",
            json!({
                "name": "inclusion",
                "roleARN": ROLE_ARN,
                "recordingGroup": {
                    "resourceTypes": ["AWS::S3::Bucket", "AWS::EC2::Volume"],
                    "recordingStrategy": { "useOnly": "INCLUSION_BY_RESOURCE_TYPES" },
                },
            })
            ,
            "inclusion",
            json!({
                "resourceTypes": ["AWS::S3::Bucket", "AWS::EC2::Volume"],
                "recordingStrategy": { "useOnly": "INCLUSION_BY_RESOURCE_TYPES" },
            }),
        ),
        (
            "an exclusion list is stored verbatim",
            json!({
                "name": "exclusion",
                "roleARN": ROLE_ARN,
                "recordingGroup": {
                    "exclusionByResourceTypes": { "resourceTypes": ["AWS::IAM::Role"] },
                    "recordingStrategy": { "useOnly": EXCLUSION_STRATEGY },
                },
            }),
            "exclusion",
            json!({
                "exclusionByResourceTypes": { "resourceTypes": ["AWS::IAM::Role"] },
                "recordingStrategy": { "useOnly": EXCLUSION_STRATEGY },
            }),
        ),
        (
            // No recording group: Config records all supported resource types,
            // excluding the global IAM resource types.
            "an absent recording group takes the documented default",
            json!({ "name": "defaulted-group", "roleARN": ROLE_ARN }),
            "defaulted-group",
            json!({
                "allSupported": true,
                "recordingStrategy": { "useOnly": "ALL_SUPPORTED_RESOURCE_TYPES" },
            }),
        ),
    ];

    for (case, requested, expected_name, expected_group) in cases {
        let emulator = Emulator::in_region("ap-south-1");
        emulator
            .ok("PutConfigurationRecorder", &recorder_request(requested))
            .await;

        let described = emulator.ok("DescribeConfigurationRecorders", &json!({})).await;
        let recorder = &described["ConfigurationRecorders"][0];
        assert_eq!(
            recorder["name"], expected_name,
            "case {case:?}: expected name {expected_name:?}, got {described}"
        );
        assert_eq!(
            recorder["roleARN"], ROLE_ARN,
            "case {case:?}: expected the stored roleARN, got {described}"
        );
        recorder_arn_of(recorder, &emulator.region, expected_name);
        for (member, expected) in expected_group
            .as_object()
            .expect("the expected recording group is an object")
        {
            assert_eq!(
                &recorder["recordingGroup"][member], expected,
                "case {case:?}: expected recordingGroup.{member} = {expected}, got {described}"
            );
        }

        // Reading twice must not change what the second read sees.
        let again = emulator.ok("DescribeConfigurationRecorders", &json!({})).await;
        assert_eq!(
            again, described,
            "case {case:?}: a describe must not change the recorder, got {again}"
        );
    }
}

#[tokio::test]
async fn a_recorder_update_keeps_the_arn_and_replaces_the_configuration() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({
                "name": "default",
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": true },
            })),
        )
        .await;
    let created = emulator.ok("DescribeConfigurationRecorders", &json!({})).await;

    let updated_role = "arn:aws:iam::000000000000:role/other-config-role";
    emulator
        .ok(
            "PutConfigurationRecorder",
            &recorder_request(json!({
                "name": "default",
                "roleARN": updated_role,
                "recordingGroup": {
                    "resourceTypes": ["AWS::S3::Bucket"],
                    "recordingStrategy": { "useOnly": "INCLUSION_BY_RESOURCE_TYPES" },
                },
                "recordingMode": { "recordingFrequency": "DAILY" },
            })),
        )
        .await;

    let after = emulator.ok("DescribeConfigurationRecorders", &json!({})).await;
    let recorder = &after["ConfigurationRecorders"][0];
    assert_eq!(
        recorder["arn"], created["ConfigurationRecorders"][0]["arn"],
        "case \"update\": the recorder keeps the ARN it was created with, got {after}"
    );
    assert_eq!(
        recorder["roleARN"], updated_role,
        "case \"update\": the roleARN must be replaced, got {after}"
    );
    assert_eq!(
        recorder["recordingGroup"]["resourceTypes"],
        json!(["AWS::S3::Bucket"]),
        "case \"update\": the recording group must be replaced, got {after}"
    );
    assert_eq!(
        recorder["recordingMode"]["recordingFrequency"], "DAILY",
        "case \"update\": the recording mode must be stored, got {after}"
    );
}

#[tokio::test]
async fn the_recording_status_moves_from_pending_to_success_on_its_own() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator
        .ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "default" }),
        )
        .await;

    // Straight after the start the recording event is still being processed.
    let status = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    let first = &status["ConfigurationRecordersStatus"][0];
    assert_eq!(
        first["lastStatus"], "Pending",
        "case \"just started\": the recording event is Pending, got {status}"
    );
    assert!(
        flag(first, "recording"),
        "case \"just started\": the recorder is recording, got {status}"
    );

    // The move to Success is owed by the clock, not by the describe.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let status = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    let first = &status["ConfigurationRecordersStatus"][0];
    assert_eq!(
        first["lastStatus"], "Success",
        "case \"settled\": the recording event has been processed, got {status}"
    );
    assert!(
        first.get("lastStatusChangeTime").is_some(),
        "case \"settled\": the status change is stamped, got {status}"
    );
    assert!(
        flag(first, "recording"),
        "case \"settled\": the recorder is still recording, got {status}"
    );

    // Stopping stamps a stop time and clears the flag.
    emulator
        .ok(
            "StopConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "default" }),
        )
        .await;
    let status = emulator
        .ok("DescribeConfigurationRecorderStatus", &json!({}))
        .await;
    let first = &status["ConfigurationRecordersStatus"][0];
    assert!(
        !flag(first, "recording"),
        "case \"stopped\": the recorder is not recording, got {status}"
    );
    assert!(
        first.get("lastStopTime").is_some(),
        "case \"stopped\": the stop is stamped, got {status}"
    );
    assert!(
        first.get("lastStartTime").is_some(),
        "case \"stopped\": the earlier start stays stamped, got {status}"
    );
}

#[tokio::test]
async fn a_rule_is_read_back_exactly_as_it_was_put() {
    // One row per published `MaximumExecutionFrequency` value, plus rows that vary the
    // scope, the input parameters, the description and the evaluation modes.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for frequency in MAXIMUM_EXECUTION_FREQUENCIES {
        cases.push((
            format!("MaximumExecutionFrequency {frequency}"),
            json!({
                "ConfigRuleName": format!("periodic-{}", frequency.to_lowercase()),
                "Source": { "Owner": "AWS", "SourceIdentifier": "IAM_PASSWORD_POLICY" },
                "MaximumExecutionFrequency": frequency,
            }),
        ));
    }
    cases.push((
        "a scoped rule".to_string(),
        json!({
            "ConfigRuleName": "scoped-rule",
            "Description": "resources in scope only",
            "Source": { "Owner": "AWS", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
            "Scope": {
                "ComplianceResourceTypes": ["AWS::S3::Bucket"],
                "TagKey": "env",
                "TagValue": "prod",
            },
            "InputParameters": "{\"minimumCount\":\"1\"}",
        }),
    ));
    cases.push((
        "explicit proactive and detective evaluation modes".to_string(),
        json!({
            "ConfigRuleName": "both-modes",
            "Source": { "Owner": "AWS", "SourceIdentifier": "S3_BUCKET_VERSIONING_ENABLED" },
            "EvaluationModes": [{ "Mode": "PROACTIVE" }, { "Mode": "DETECTIVE" }],
        }),
    ));
    cases.push((
        "a custom policy rule".to_string(),
        json!({
            "ConfigRuleName": "guard-rule",
            "Source": {
                "Owner": "CUSTOM_POLICY",
                "CustomPolicyDetails": {
                    "PolicyRuntime": "guard-2.x.x",
                    "PolicyText": "rule check { true }",
                    "EnableDebugLogDelivery": true,
                },
            },
        }),
    ));

    for (case, rule) in cases {
        let emulator = Emulator::in_region("us-west-2");
        with_recorder_and_channel(&emulator).await;
        let name = rule["ConfigRuleName"]
            .as_str()
            .expect("every case names its rule")
            .to_string();

        emulator
            .ok("PutConfigRule", &json!({ "ConfigRule": rule.clone() }))
            .await;

        let described = emulator
            .ok(
                "DescribeConfigRules",
                &json!({ "ConfigRuleNames": [name.clone()] }),
            )
            .await;
        let stored = &described["ConfigRules"][0];

        // Every member the request carried comes back as it was stored, except the
        // Guard policy body: `DescribeConfigRules` reports
        // `Source.CustomPolicyDetails.PolicyText` empty rather than handing the stored
        // policy back.
        for (member, expected) in rule.as_object().expect("the case rule is an object") {
            if member == "EvaluationModes" {
                continue;
            }
            let mut expected = expected.clone();
            if let Some(policy_text) = expected.pointer_mut("/CustomPolicyDetails/PolicyText") {
                *policy_text = json!("");
            }
            assert_eq!(
                &stored[member], &expected,
                "case {case:?}: expected {member} = {expected}, got {described}"
            );
        }

        let id = stored["ConfigRuleId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ConfigRuleId, got {described}"));
        assert_eq!(
            stored["ConfigRuleArn"],
            json!(emulator_server::emulator_arn(
                "config",
                &emulator.region,
                &format!("config-rule/{id}")
            )),
            "case {case:?}: expected the ARN to be built from the stored id, got {described}"
        );
        assert_eq!(
            stored["ConfigRuleState"], "ACTIVE",
            "case {case:?}: expected a new rule to be ACTIVE, got {described}"
        );

        let expected_modes = rule
            .get("EvaluationModes")
            .cloned()
            .unwrap_or_else(|| json!([{ "Mode": "DETECTIVE" }]));
        assert_eq!(
            stored["EvaluationModes"], expected_modes,
            "case {case:?}: expected EvaluationModes {expected_modes}, got {described}"
        );
    }
}

#[tokio::test]
async fn a_rule_update_keeps_the_identity_it_was_created_with() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator.ok("PutConfigRule", &managed_rule("the-rule")).await;
    let created = emulator.ok("DescribeConfigRules", &json!({})).await;

    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": "the-rule",
                    "Description": "now described",
                    "Source": {
                        "Owner": "AWS",
                        "SourceIdentifier": "S3_BUCKET_PUBLIC_READ_PROHIBITED",
                    },
                }
            }),
        )
        .await;

    let after = emulator.ok("DescribeConfigRules", &json!({})).await;
    assert_eq!(
        after["ConfigRules"].as_array().map(Vec::len),
        Some(1),
        "case \"update\": updating a rule must not add one, got {after}"
    );
    for member in ["ConfigRuleId", "ConfigRuleArn"] {
        assert_eq!(
            after["ConfigRules"][0][member], created["ConfigRules"][0][member],
            "case \"update\": {member} must survive the update, got {after}"
        );
    }
    assert_eq!(
        after["ConfigRules"][0]["Description"], "now described",
        "case \"update\": the description must be replaced, got {after}"
    );
    assert_eq!(
        after["ConfigRules"][0]["Source"]["SourceIdentifier"],
        "S3_BUCKET_PUBLIC_READ_PROHIBITED",
        "case \"update\": the source must be replaced, got {after}"
    );
}

#[tokio::test]
async fn every_rule_gets_its_own_identity() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for index in 0..5 {
        emulator
            .ok("PutConfigRule", &managed_rule(&format!("rule-{index}")))
            .await;
    }

    let described = emulator.ok("DescribeConfigRules", &json!({})).await;
    let rules = described["ConfigRules"]
        .as_array()
        .unwrap_or_else(|| panic!("ConfigRules must be a list, got {described}"));
    let mut ids: Vec<&str> = rules
        .iter()
        .map(|rule| {
            rule["ConfigRuleId"]
                .as_str()
                .unwrap_or_else(|| panic!("every rule carries an id, got {described}"))
        })
        .collect();
    ids.sort_unstable();
    let distinct = ids.len();
    ids.dedup();
    assert_eq!(
        ids.len(),
        distinct,
        "case \"five rules\": every rule id must be distinct, got {described}"
    );
    assert!(
        ids.iter().all(|id| id.starts_with("config-rule-")),
        "case \"five rules\": every rule id must be a Config rule id, got {described}"
    );
}

#[tokio::test]
async fn describe_config_rules_pages_over_a_stable_ordering() {
    // Rule counts around the page size: empty, one, one short of a page, exactly a
    // page, one past it, and several pages.
    for count in [0_usize, 1, PAGE_SIZE - 1, PAGE_SIZE, PAGE_SIZE + 1, 53] {
        let emulator = Emulator::new();
        with_recorder_and_channel(&emulator).await;
        let mut expected: Vec<String> = Vec::with_capacity(count);
        for index in 0..count {
            let name = format!("rule-{index:03}");
            emulator.ok("PutConfigRule", &managed_rule(&name)).await;
            expected.push(name);
        }
        expected.sort();

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0_usize;
        loop {
            let request = match &token {
                Some(token) => json!({ "NextToken": token }),
                None => json!({}),
            };
            let page = emulator.ok("DescribeConfigRules", &request).await;
            let rules = page["ConfigRules"]
                .as_array()
                .unwrap_or_else(|| panic!("count {count}: ConfigRules must be a list, got {page}"))
                .clone();
            pages += 1;
            assert!(
                rules.len() <= PAGE_SIZE,
                "count {count}: a page carries at most {PAGE_SIZE} rules, got {}",
                rules.len()
            );
            for rule in &rules {
                seen.push(
                    rule["ConfigRuleName"]
                        .as_str()
                        .unwrap_or_else(|| panic!("count {count}: a rule has a name, got {page}"))
                        .to_string(),
                );
            }
            token = page
                .get("NextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            let remaining = count - seen.len();
            assert_eq!(
                token.is_some(),
                remaining > 0,
                "count {count}: NextToken is present exactly while rules remain \
                 ({remaining} left), got {page}"
            );
            if token.is_none() {
                break;
            }
            assert_eq!(
                rules.len(),
                PAGE_SIZE,
                "count {count}: a non-final page is full, got {page}"
            );
            assert!(
                pages <= count + 1,
                "count {count}: pagination must terminate, {pages} pages so far"
            );
        }

        assert_eq!(
            seen, expected,
            "count {count}: the pages must together carry every rule once, in name order"
        );
    }
}

#[tokio::test]
async fn a_page_token_survives_a_delete_between_pages() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for index in 0..(PAGE_SIZE + 5) {
        emulator
            .ok("PutConfigRule", &managed_rule(&format!("rule-{index:03}")))
            .await;
    }

    let first = emulator.ok("DescribeConfigRules", &json!({})).await;
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first of two pages carries a NextToken, got {first}"))
        .to_string();

    // Remove a rule from the page already served; the token resumes where it said.
    emulator
        .ok("DeleteConfigRule", &json!({ "ConfigRuleName": "rule-000" }))
        .await;

    let second = emulator
        .ok("DescribeConfigRules", &json!({ "NextToken": token }))
        .await;
    let names: Vec<&str> = second["ConfigRules"]
        .as_array()
        .unwrap_or_else(|| panic!("ConfigRules must be a list, got {second}"))
        .iter()
        .filter_map(|rule| rule["ConfigRuleName"].as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "rule-025", "rule-026", "rule-027", "rule-028", "rule-029"
        ],
        "case \"delete between pages\": the token must resume at the name it encoded, \
         got {second}"
    );
}

#[tokio::test]
async fn filters_select_rules_by_evaluation_mode() {
    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": "detective-only",
                    "Source": { "Owner": "AWS", "SourceIdentifier": "IAM_PASSWORD_POLICY" },
                }
            }),
        )
        .await;
    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": "proactive-only",
                    "Source": { "Owner": "AWS", "SourceIdentifier": "IAM_PASSWORD_POLICY" },
                    "EvaluationModes": [{ "Mode": "PROACTIVE" }],
                }
            }),
        )
        .await;

    // Both published `EvaluationMode` values, from
    // `smithy_query(query="shape", shape="EvaluationMode")`: DETECTIVE, PROACTIVE.
    for (mode, expected) in [("DETECTIVE", "detective-only"), ("PROACTIVE", "proactive-only")] {
        let filtered = emulator
            .ok(
                "DescribeConfigRules",
                &json!({ "Filters": { "EvaluationMode": mode } }),
            )
            .await;
        let names: Vec<&str> = filtered["ConfigRules"]
            .as_array()
            .unwrap_or_else(|| panic!("ConfigRules must be a list, got {filtered}"))
            .iter()
            .filter_map(|rule| rule["ConfigRuleName"].as_str())
            .collect();
        assert_eq!(
            names,
            vec![expected],
            "case {mode:?}: expected only {expected}, got {filtered}"
        );
    }
}

/// One refusal case: the calls that must succeed first, then the call that must fail.
struct Refusal {
    case: &'static str,
    prelude: Vec<(&'static str, Value)>,
    operation: &'static str,
    body: Value,
    expected: &'static str,
}

#[tokio::test]
async fn each_modelled_refusal_is_answered_with_its_own_error() {
    let recorder = || {
        (
            "PutConfigurationRecorder",
            recorder_request(json!({ "roleARN": ROLE_ARN })),
        )
    };
    let channel = || {
        (
            "PutDeliveryChannel",
            json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
        )
    };
    let start = || {
        (
            "StartConfigurationRecorder",
            json!({ "ConfigurationRecorderName": "default" }),
        )
    };

    let cases = vec![
        Refusal {
            case: "a recorder without a roleARN",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({ "name": "default" })),
            expected: "InvalidRoleException",
        },
        Refusal {
            case: "a recorder with a blank roleARN",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({ "name": "default", "roleARN": "   " })),
            expected: "InvalidRoleException",
        },
        Refusal {
            case: "a recorder with a blank name",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({ "name": "  ", "roleARN": ROLE_ARN })),
            expected: "InvalidConfigurationRecorderNameException",
        },
        Refusal {
            case: "a recorder named with the service-linked prefix",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({
                "name": "AWSConfigurationRecorderForSecurityHub",
                "roleARN": ROLE_ARN,
            })),
            expected: "InvalidConfigurationRecorderNameException",
        },
        Refusal {
            case: "allSupported together with an explicit resource-type list",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({
                "roleARN": ROLE_ARN,
                "recordingGroup": {
                    "allSupported": true,
                    "resourceTypes": ["AWS::S3::Bucket"],
                },
            })),
            expected: "InvalidRecordingGroupException",
        },
        Refusal {
            case: "allSupported together with the exclusion strategy",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({
                "roleARN": ROLE_ARN,
                "recordingGroup": {
                    "allSupported": true,
                    "recordingStrategy": { "useOnly": EXCLUSION_STRATEGY },
                },
            })),
            expected: "InvalidRecordingGroupException",
        },
        Refusal {
            case: "a recording group in which every parameter is null, false or empty",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({
                "roleARN": ROLE_ARN,
                "recordingGroup": { "allSupported": false },
            })),
            expected: "InvalidRecordingGroupException",
        },
        Refusal {
            case: "a recorder for a service principal",
            prelude: vec![],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({
                "roleARN": ROLE_ARN,
                "servicePrincipal": "securityhub.amazonaws.com",
            })),
            expected: "UnmodifiableEntityException",
        },
        Refusal {
            case: "a second, differently named recorder",
            prelude: vec![recorder()],
            operation: "PutConfigurationRecorder",
            body: recorder_request(json!({ "name": "second", "roleARN": ROLE_ARN })),
            expected: "MaxNumberOfConfigurationRecordersExceededException",
        },
        Refusal {
            case: "describing more than one recorder at a time",
            prelude: vec![recorder()],
            operation: "DescribeConfigurationRecorders",
            body: json!({ "ConfigurationRecorderNames": ["default", "second"] }),
            expected: "ValidationException",
        },
        Refusal {
            case: "describing a recorder that does not exist",
            prelude: vec![],
            operation: "DescribeConfigurationRecorders",
            body: json!({ "ConfigurationRecorderNames": ["absent"] }),
            expected: "NoSuchConfigurationRecorderException",
        },
        Refusal {
            case: "describing the status of a recorder that does not exist",
            prelude: vec![],
            operation: "DescribeConfigurationRecorderStatus",
            body: json!({ "ConfigurationRecorderNames": ["absent"] }),
            expected: "NoSuchConfigurationRecorderException",
        },
        Refusal {
            case: "describing a recorder by an ARN that names none",
            prelude: vec![recorder()],
            operation: "DescribeConfigurationRecorders",
            body: json!({
                "Arn": "arn:aws:config:us-east-1:000000000000:configuration-recorder/other/vyzjmv9fhzv5rc1g",
            }),
            expected: "NoSuchConfigurationRecorderException",
        },
        Refusal {
            case: "deleting a recorder that does not exist",
            prelude: vec![],
            operation: "DeleteConfigurationRecorder",
            body: json!({ "ConfigurationRecorderName": "absent" }),
            expected: "NoSuchConfigurationRecorderException",
        },
        Refusal {
            case: "a delivery channel before any recorder exists",
            prelude: vec![],
            operation: "PutDeliveryChannel",
            body: json!({ "DeliveryChannel": { "s3BucketName": "config-bucket" } }),
            expected: "NoAvailableConfigurationRecorderException",
        },
        Refusal {
            case: "a delivery channel with a blank name",
            prelude: vec![recorder()],
            operation: "PutDeliveryChannel",
            body: json!({ "DeliveryChannel": { "name": "  ", "s3BucketName": "b" } }),
            expected: "InvalidDeliveryChannelNameException",
        },
        Refusal {
            case: "a second, differently named delivery channel",
            prelude: vec![recorder(), channel()],
            operation: "PutDeliveryChannel",
            body: json!({ "DeliveryChannel": { "name": "second", "s3BucketName": "b" } }),
            expected: "MaxNumberOfDeliveryChannelsExceededException",
        },
        Refusal {
            case: "describing a delivery channel that does not exist",
            prelude: vec![recorder(), channel()],
            operation: "DescribeDeliveryChannels",
            body: json!({ "DeliveryChannelNames": ["absent"] }),
            expected: "NoSuchDeliveryChannelException",
        },
        Refusal {
            case: "deleting a delivery channel that does not exist",
            prelude: vec![recorder(), channel()],
            operation: "DeleteDeliveryChannel",
            body: json!({ "DeliveryChannelName": "absent" }),
            expected: "NoSuchDeliveryChannelException",
        },
        Refusal {
            case: "deleting the delivery channel while the recorder is running",
            prelude: vec![recorder(), channel(), start()],
            operation: "DeleteDeliveryChannel",
            body: json!({ "DeliveryChannelName": "default" }),
            expected: "LastDeliveryChannelDeleteFailedException",
        },
        Refusal {
            case: "starting a recorder that does not exist",
            prelude: vec![recorder(), channel()],
            operation: "StartConfigurationRecorder",
            body: json!({ "ConfigurationRecorderName": "absent" }),
            expected: "NoSuchConfigurationRecorderException",
        },
        Refusal {
            case: "starting a recorder before any delivery channel exists",
            prelude: vec![recorder()],
            operation: "StartConfigurationRecorder",
            body: json!({ "ConfigurationRecorderName": "default" }),
            expected: "NoAvailableDeliveryChannelException",
        },
        Refusal {
            case: "stopping a recorder that does not exist",
            prelude: vec![recorder(), channel()],
            operation: "StopConfigurationRecorder",
            body: json!({ "ConfigurationRecorderName": "absent" }),
            expected: "NoSuchConfigurationRecorderException",
        },
        Refusal {
            case: "a rule before any recorder exists",
            prelude: vec![],
            operation: "PutConfigRule",
            body: managed_rule("a-rule"),
            expected: "NoAvailableConfigurationRecorderException",
        },
        Refusal {
            case: "a rule without a name",
            prelude: vec![recorder(), channel()],
            operation: "PutConfigRule",
            body: json!({
                "ConfigRule": {
                    "Source": { "Owner": "AWS", "SourceIdentifier": "IAM_PASSWORD_POLICY" },
                }
            }),
            expected: "InvalidParameterValueException",
        },
        Refusal {
            case: "a managed rule without a source identifier",
            prelude: vec![recorder(), channel()],
            operation: "PutConfigRule",
            body: json!({
                "ConfigRule": { "ConfigRuleName": "no-identifier", "Source": { "Owner": "AWS" } }
            }),
            expected: "InvalidParameterValueException",
        },
        Refusal {
            case: "a custom policy rule without its policy details",
            prelude: vec![recorder(), channel()],
            operation: "PutConfigRule",
            body: json!({
                "ConfigRule": {
                    "ConfigRuleName": "no-policy",
                    "Source": { "Owner": "CUSTOM_POLICY" },
                }
            }),
            expected: "InvalidParameterValueException",
        },
        Refusal {
            case: "describing a rule that does not exist",
            prelude: vec![recorder(), channel()],
            operation: "DescribeConfigRules",
            body: json!({ "ConfigRuleNames": ["absent"] }),
            expected: "NoSuchConfigRuleException",
        },
        Refusal {
            case: "a NextToken this service never issued",
            prelude: vec![recorder(), channel()],
            operation: "DescribeConfigRules",
            body: json!({ "NextToken": "not a token!!" }),
            expected: "InvalidNextTokenException",
        },
        Refusal {
            case: "deleting a rule that does not exist",
            prelude: vec![recorder(), channel()],
            operation: "DeleteConfigRule",
            body: json!({ "ConfigRuleName": "absent" }),
            expected: "NoSuchConfigRuleException",
        },
    ];

    for case in cases {
        let emulator = Emulator::new();
        for (operation, body) in &case.prelude {
            emulator.ok(operation, body).await;
        }
        let (status, code) = emulator.refused(case.operation, &case.body).await;
        assert_eq!(
            (status, code.as_str()),
            (400, case.expected),
            "case {:?}: expected HTTP 400 {}, got {status} {code}",
            case.case,
            case.expected,
        );
    }
}

#[tokio::test]
async fn the_rule_store_is_capped_at_this_regions_quota() {
    // The Config rules quota for one account in one Region; a create past it is
    // refused, while an update to a rule that already exists still succeeds.
    const QUOTA: usize = 150;

    let emulator = Emulator::new();
    with_recorder_and_channel(&emulator).await;
    for index in 0..QUOTA {
        emulator
            .ok("PutConfigRule", &managed_rule(&format!("rule-{index:03}")))
            .await;
    }

    let (status, code) = emulator
        .refused("PutConfigRule", &managed_rule("one-too-many"))
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "MaxNumberOfConfigRulesExceededException"),
        "case \"one past the quota\": expected HTTP 400 \
         MaxNumberOfConfigRulesExceededException, got {status} {code}"
    );

    emulator
        .ok(
            "PutConfigRule",
            &json!({
                "ConfigRule": {
                    "ConfigRuleName": "rule-000",
                    "Description": "updated at the quota",
                    "Source": { "Owner": "AWS", "SourceIdentifier": "IAM_PASSWORD_POLICY" },
                }
            }),
        )
        .await;
    let described = emulator
        .ok("DescribeConfigRules", &json!({ "ConfigRuleNames": ["rule-000"] }))
        .await;
    assert_eq!(
        described["ConfigRules"][0]["Description"], "updated at the quota",
        "case \"update at the quota\": an existing rule must still be updatable, \
         got {described}"
    );
}

#[tokio::test]
async fn an_unimplemented_operation_still_answers_not_implemented() {
    // The slice implemented here is the setup-and-rule loop; everything else is still
    // scaffolded and must say so rather than answer an empty success. Any operation
    // whose handler still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker does — when
    // the one named below is implemented, point this at another marked handler
    // (`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' src/ops/`).
    let emulator = Emulator::new();
    let (status, code) = emulator
        .refused(
            "DeleteOrganizationConformancePack",
            &json!({ "OrganizationConformancePackName": "pack-000" }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (501, "NotImplemented"),
        "case \"out of slice\": expected HTTP 501 NotImplemented, got {status} {code}"
    );
}
