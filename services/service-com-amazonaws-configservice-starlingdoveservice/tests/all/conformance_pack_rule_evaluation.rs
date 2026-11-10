// SPDX-License-Identifier: CC0-1.0

//! What a DEPLOYED conformance pack's rules report, when the rule the pack deployed is one
//! whose subject is the ACCOUNT rather than a recorded resource.
//!
//! The rule under test: **a Config Managed rule this emulator can evaluate is evaluated by
//! the pass Config owes it as soon as it exists — including a rule a conformance pack's
//! deployment created — and a rule whose subject is the account is placed from the
//! ACCOUNT'S OWN configuration, read from the service that owns it. So the pack's
//! compliance surface reports one evaluation result per such rule, against the account
//! pseudo-resource `AWS::::Account`, and the pack's own compliance summary is the roll-up
//! of exactly those results.**
//!
//! `IAM_PASSWORD_POLICY` is the managed rule the rows exercise: it "checks if the account
//! password policy for users meets the specified requirements indicated in the parameters",
//! so what it places the account at follows from the password policy IAM holds for the
//! account and from the requirements the rule's own `InputParameters` state — never from
//! the request that read the report.
//!
//! Config and IAM are mounted on ONE engine, because the account's password policy is
//! IAM's to report: the rule is evaluated from what IAM answers over the emulator's own
//! in-process callback, which is why a suite that mounts Config alone sees such a rule stay
//! unevaluated.
//!
//! The rows differ along what the rule generalises over: whether the account has a password
//! policy at all, which requirement the rule's parameters state (a character class, a
//! minimum length, a reuse count, an expiry age, several at once, none at all), and whether
//! the policy in force meets it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_configservice_starlingdoveservice::state::conformance_pack_rule_name;
use service_com_amazonaws_configservice_starlingdoveservice::EmulatedService;
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request below is signed for.
pub(crate) const REGION: &str = "us-east-1";

/// The resource type Config evaluates an account-wide rule against — the account
/// pseudo-resource type, from `com.amazonaws.configservice#ResourceType`'s account entry,
/// which `ExternalEvaluation$ComplianceResourceType` names as the one such a verdict
/// carries ("Config accepts `AWS::::Account` resource type").
const ACCOUNT_RESOURCE_TYPE: &str = "AWS::::Account";

/// `ComplianceType` values this suite reads verdicts as.
const COMPLIANT: &str = "COMPLIANT";
/// See [`COMPLIANT`].
const NON_COMPLIANT: &str = "NON_COMPLIANT";

/// The managed rule the rows exercise, a `Source$SourceIdentifier` value from the Config
/// managed-rule list.
const IAM_PASSWORD_POLICY: &str = "IAM_PASSWORD_POLICY";

/// The resource type a conformance pack template declares a Config rule as, which is what
/// CloudFormation requires of the resource.
const CONFIG_RULE_TYPE: &str = "AWS::Config::ConfigRule";

/// How long a poll waits for the deployment and the evaluation pass the clock owes. Each
/// falls due a fraction of a second after the call that makes it due, so this is many
/// passes' worth of slack and not a span the behaviour depends on.
pub(crate) const CONVERGENCE_BUDGET: Duration = Duration::from_secs(20);

/// How long a poll waits between two reads.
pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Config and IAM over one shared engine.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    callback: EmbeddedEmulator,
}

impl Emulator {
    /// Config and IAM mounted on one engine, as `core/router/src/lib.rs` mounts them.
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        let callback = EmbeddedEmulator::from_router(router.clone());
        Self { router, callback }
    }

    /// One Config call (`awsJson1_1`), answering the HTTP status and the parsed body.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/config/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=deadbeef"
        );
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("config.{REGION}.amazonaws.com"))
            .header("authorization", authorization)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("StarlingDoveService.{operation}"))
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

    /// A Config call that must answer 200.
    pub(crate) async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "{operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// An IAM client over the in-process callback, the way the recorder's own reads of IAM
    /// are made.
    pub(crate) fn iam(&self) -> aws_sdk_iam::Client {
        service_common::iam_client::iam_client_through(&self.callback, REGION)
    }

    /// Create the configuration recorder's own role in IAM, trusting the Config service
    /// principal, and answer its ARN.
    ///
    /// With IAM mounted beside Config, a delivery channel is taken only while IAM shows
    /// Config can assume the recorder's role, so every row that puts a recorder needs this
    /// role to exist.
    pub(crate) async fn config_role(&self) -> String {
        self.iam()
            .create_role()
            .role_name("config-role")
            .assume_role_policy_document(
                json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": { "Service": "config.amazonaws.com" },
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            )
            .send()
            .await
            .expect("creating the recorder's role must succeed")
            .role
            .expect("CreateRole answers the role")
            .arn
    }

    /// Put a configuration recorder recording — with the global resource types in scope,
    /// which is what a recorder has to ask for to record IAM's — and the delivery channel a
    /// recorder needs.
    ///
    /// The recorder's own role is created in IAM first ([`Self::config_role`]).
    pub(crate) async fn recording(&self) {
        let role = self.config_role().await;
        self.ok(
            "PutConfigurationRecorder",
            &json!({
                "ConfigurationRecorder": {
                    "name": "the-recorder",
                    "roleARN": role,
                    "recordingGroup": {
                        "allSupported": true,
                        "includeGlobalResourceTypes": true,
                    },
                }
            }),
        )
        .await;
        self.ok(
            "PutDeliveryChannel",
            &json!({ "DeliveryChannel": { "s3BucketName": "config-delivery" } }),
        )
        .await;
        self.ok(
            "StartConfigurationRecorder",
            &json!({ "ConfigurationRecorderName": "the-recorder" }),
        )
        .await;
    }
}

/// The password policy a row puts in force, as the members
/// `UpdateAccountPasswordPolicy` takes.
#[derive(Debug, Clone, Copy, Default)]
struct Policy {
    minimum_password_length: Option<i32>,
    require_symbols: bool,
    require_numbers: bool,
    require_uppercase_characters: bool,
    require_lowercase_characters: bool,
    max_password_age: Option<i32>,
    password_reuse_prevention: Option<i32>,
}

/// A conformance pack template declaring ONE Config rule of the managed rule
/// `source_identifier`, carrying `parameters` as its `InputParameters` when it states any.
fn template(rule_name: &str, source_identifier: &str, parameters: Option<&Value>) -> String {
    let mut body = format!(
        "Resources:\n  Rule0:\n    Type: {CONFIG_RULE_TYPE}\n    Properties:\n      \
         ConfigRuleName: {rule_name}\n      Source:\n        Owner: AWS\n        \
         SourceIdentifier: {source_identifier}\n"
    );
    if let Some(parameters) = parameters {
        body.push_str(&format!("      InputParameters: '{parameters}'\n"));
    }
    body
}

/// Deploy `pack` from a template declaring one rule, and answer the name Config gave that
/// rule once the deployment the clock owes has settled.
async fn deploy(
    case: &str,
    emulator: &Emulator,
    pack: &str,
    rule_name: &str,
    parameters: Option<&Value>,
) -> String {
    emulator
        .ok(
            "PutConformancePack",
            &json!({
                "ConformancePackName": pack,
                "TemplateBody": template(rule_name, IAM_PASSWORD_POLICY, parameters),
            }),
        )
        .await;
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let status = emulator
            .ok(
                "DescribeConformancePackStatus",
                &json!({ "ConformancePackNames": [pack] }),
            )
            .await;
        let detail = &status["ConformancePackStatusDetails"][0];
        if detail["ConformancePackState"] == json!("CREATE_COMPLETE") {
            let id = detail["ConformancePackId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected the pack id, got {status}"));
            return conformance_pack_rule_name(rule_name, id);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the pack's deployment to settle, got {status}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Read the pack's compliance details until they report at least one evaluation result, and
/// answer those results.
async fn await_results(case: &str, emulator: &Emulator, pack: &str) -> Vec<Value> {
    let deadline = Instant::now() + CONVERGENCE_BUDGET;
    loop {
        let body = emulator
            .ok(
                "GetConformancePackComplianceDetails",
                &json!({ "ConformancePackName": pack }),
            )
            .await;
        let results = body["ConformancePackRuleEvaluationResults"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !results.is_empty() {
            return results;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the pack's rules to be evaluated, got {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `ConformancePackComplianceStatus` the pack's own summary reports for `pack`.
async fn pack_status(case: &str, emulator: &Emulator, pack: &str) -> String {
    let body = emulator
        .ok(
            "GetConformancePackComplianceSummary",
            &json!({ "ConformancePackNames": [pack] }),
        )
        .await;
    body["ConformancePackComplianceSummaryList"][0]["ConformancePackComplianceStatus"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the pack's compliance, got {body}"))
        .to_string()
}

/// One row: what it is called, the password policy to put in force (none at all for an
/// account that never set one), the requirements the rule states, and the verdict the rule
/// has to place the account at.
type Case = (&'static str, Option<Policy>, Option<Value>, &'static str);

/// The rows. Each parameter name is one `IAM_PASSWORD_POLICY` takes, as the managed rule's
/// own documentation spells it.
fn cases() -> Vec<Case> {
    let strong = Policy {
        minimum_password_length: Some(14),
        require_symbols: true,
        require_numbers: true,
        require_uppercase_characters: true,
        require_lowercase_characters: true,
        max_password_age: Some(30),
        password_reuse_prevention: Some(24),
    };
    vec![
        (
            "no password policy at all, and no requirement stated",
            None,
            None,
            NON_COMPLIANT,
        ),
        (
            "no password policy at all, with a requirement stated",
            None,
            Some(json!({ "MinimumPasswordLength": "14" })),
            NON_COMPLIANT,
        ),
        (
            "a policy in force, and no requirement stated",
            Some(Policy::default()),
            None,
            COMPLIANT,
        ),
        (
            "a minimum length the policy meets exactly",
            Some(Policy {
                minimum_password_length: Some(14),
                ..Policy::default()
            }),
            Some(json!({ "MinimumPasswordLength": "14" })),
            COMPLIANT,
        ),
        (
            "a minimum length the policy falls short of",
            Some(Policy {
                minimum_password_length: Some(8),
                ..Policy::default()
            }),
            Some(json!({ "MinimumPasswordLength": "14" })),
            NON_COMPLIANT,
        ),
        (
            "a character class the policy requires",
            Some(Policy {
                require_symbols: true,
                ..Policy::default()
            }),
            Some(json!({ "RequireSymbols": "true" })),
            COMPLIANT,
        ),
        (
            "a character class the policy does not require",
            Some(Policy {
                require_numbers: true,
                ..Policy::default()
            }),
            Some(json!({ "RequireSymbols": "true" })),
            NON_COMPLIANT,
        ),
        (
            "a character class the rule explicitly does not demand",
            Some(Policy::default()),
            Some(json!({ "RequireUppercaseCharacters": "false" })),
            COMPLIANT,
        ),
        (
            "a reuse count the policy prevents more of",
            Some(Policy {
                password_reuse_prevention: Some(24),
                ..Policy::default()
            }),
            Some(json!({ "PasswordReusePrevention": "5" })),
            COMPLIANT,
        ),
        (
            "a reuse count the policy prevents fewer of",
            Some(Policy {
                password_reuse_prevention: Some(3),
                ..Policy::default()
            }),
            Some(json!({ "PasswordReusePrevention": "5" })),
            NON_COMPLIANT,
        ),
        (
            "an expiry age the policy expires sooner than",
            Some(Policy {
                max_password_age: Some(30),
                ..Policy::default()
            }),
            Some(json!({ "MaxPasswordAge": "90" })),
            COMPLIANT,
        ),
        (
            "an expiry age under a policy whose passwords never expire",
            Some(Policy::default()),
            Some(json!({ "MaxPasswordAge": "90" })),
            NON_COMPLIANT,
        ),
        (
            "every requirement at once, all of them met",
            Some(strong),
            Some(json!({
                "RequireUppercaseCharacters": "true",
                "RequireLowercaseCharacters": "true",
                "RequireSymbols": "true",
                "RequireNumbers": "true",
                "MinimumPasswordLength": "14",
                "PasswordReusePrevention": "24",
                "MaxPasswordAge": "90",
            })),
            COMPLIANT,
        ),
        (
            "every requirement at once, one of them unmet",
            Some(Policy {
                minimum_password_length: Some(10),
                ..strong
            }),
            Some(json!({
                "RequireUppercaseCharacters": "true",
                "RequireLowercaseCharacters": "true",
                "RequireSymbols": "true",
                "RequireNumbers": "true",
                "MinimumPasswordLength": "14",
                "PasswordReusePrevention": "24",
                "MaxPasswordAge": "90",
            })),
            NON_COMPLIANT,
        ),
    ]
}

#[tokio::test]
async fn a_packs_account_wide_rule_is_evaluated_from_the_accounts_own_configuration() {
    for (case, policy, parameters, expected) in cases() {
        let emulator = Emulator::new();
        if let Some(policy) = policy {
            emulator
                .iam()
                .update_account_password_policy()
                .set_minimum_password_length(policy.minimum_password_length)
                .require_symbols(policy.require_symbols)
                .require_numbers(policy.require_numbers)
                .require_uppercase_characters(policy.require_uppercase_characters)
                .require_lowercase_characters(policy.require_lowercase_characters)
                .set_max_password_age(policy.max_password_age)
                .set_password_reuse_prevention(policy.password_reuse_prevention)
                .send()
                .await
                .unwrap_or_else(|error| {
                    panic!("case {case:?}: setting the password policy must succeed, got {error:?}")
                });
        }
        emulator.recording().await;

        let pack = "password-policy-pack";
        let rule = deploy(case, &emulator, pack, "account-password-policy", parameters.as_ref()).await;
        let results = await_results(case, &emulator, pack).await;

        // One rule, one subject: the ACCOUNT. So the pack reports exactly one result, and
        // it is about the account pseudo-resource rather than any recorded resource.
        assert_eq!(
            results.len(),
            1,
            "case {case:?}: expected one evaluation result, got {results:?}"
        );
        let identifier = &results[0]["EvaluationResultIdentifier"]["EvaluationResultQualifier"];
        assert_eq!(
            identifier["ConfigRuleName"].as_str(),
            Some(rule.as_str()),
            "case {case:?}: the result names the rule the pack deployed, got {:?}",
            results[0]
        );
        assert_eq!(
            identifier["ResourceType"].as_str(),
            Some(ACCOUNT_RESOURCE_TYPE),
            "case {case:?}: the result is about the account, got {:?}",
            results[0]
        );
        assert_eq!(
            identifier["ResourceId"].as_str(),
            Some(engine::emulator_account_id()),
            "case {case:?}: the result names this account, got {:?}",
            results[0]
        );
        assert_eq!(
            results[0]["ComplianceType"].as_str(),
            Some(expected),
            "case {case:?}: expected the account to be placed at {expected}, got {:?}",
            results[0]
        );

        // The pack's own summary is the roll-up of exactly those results, so it cannot
        // disagree with them.
        let status = pack_status(case, &emulator, pack).await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected the pack's compliance to be {expected}, got {status}"
        );

        // The same verdict is what the RULE's own evaluation-details read reports: one
        // record, two reports of it.
        let own = emulator
            .ok(
                "GetComplianceDetailsByConfigRule",
                &json!({ "ConfigRuleName": rule }),
            )
            .await;
        assert_eq!(
            own["EvaluationResults"][0]["ComplianceType"].as_str(),
            Some(expected),
            "case {case:?}: expected the rule's own details to report {expected}, got {own}"
        );
    }
}

#[tokio::test]
async fn a_managed_rule_this_emulator_cannot_evaluate_reports_no_verdict() {
    // The counter-case to the table above: a managed rule whose implementation this
    // emulator does not have records nothing at all rather than a synthesised verdict, so
    // the pack it belongs to has no compliance to report.
    let emulator = Emulator::new();
    emulator.recording().await;
    let pack = "unimplemented-rule-pack";
    emulator
        .ok(
            "PutConformancePack",
            &json!({
                "ConformancePackName": pack,
                "TemplateBody": template("made-up-rule", "A_RULE_THIS_EMULATOR_HAS_NO_CHECK_FOR", None),
            }),
        )
        .await;
    tokio::time::sleep(CONVERGENCE_BUDGET / 4).await;
    let body = emulator
        .ok(
            "GetConformancePackComplianceDetails",
            &json!({ "ConformancePackName": pack }),
        )
        .await;
    let results = body["ConformancePackRuleEvaluationResults"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        results.is_empty(),
        "case \"a rule with no implementation\": expected no evaluation result, got {results:?}"
    );
}
