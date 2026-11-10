// SPDX-License-Identifier: CC0-1.0

//! The Amazon Web Services Lambda and Amazon SNS DESTINATIONS of a receipt rule's actions,
//! authorized before the rule is stored — end to end over the wire, with Lambda and Amazon
//! SNS mounted beside this service on ONE engine exactly as the assembled router mounts them.
//!
//! `LambdaAction` documents the requirement: "To enable Amazon SES to call your Amazon Web
//! Services Lambda function or to publish to an Amazon SNS topic of another account, Amazon
//! SES must have permission to access those resources." So the rule this suite pins is: a
//! stored action must name a destination Amazon SES may really reach — a function whose own
//! resource policy grants the service principal `lambda:InvokeFunction` for THIS receipt rule,
//! and a topic Amazon SNS really holds — and an action that does not is refused with the
//! refusal its own member carries (`InvalidLambdaFunction` / `InvalidSnsTopic`, HTTP 400) with
//! NOTHING persisted and, on an update, the stored rule left at its last-good state.
//!
//! The same pre-flight runs on `CreateReceiptRule` and on `UpdateReceiptRule`, so every row
//! below is driven through BOTH.
//!
//! Every function, permission and topic here is made through Lambda's and Amazon SNS's OWN
//! operations over the in-process callback, so what the pre-flight reads is what those
//! services really hold. The table spans what the rule generalises over: which destination
//! kind the action addresses, which member of the action names the topic, whether the
//! destination exists, and what its policy says (nothing, a grant to this principal, a grant
//! scoped to another receipt rule, a grant to another principal).

use crate::harness::{call, element, error_envelope, field, Field, REGION};
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

/// The rule set every rule below is added to.
const RULE_SET: &str = "destination-rules";

/// The IAM action Amazon SES takes on the function a `LambdaAction` names.
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The service principal Amazon SES acts under when it carries out a receipt rule's actions.
const SES_PRINCIPAL: &str = "ses.amazonaws.com";

/// A well-formed execution role ARN for every function below: an execution role is normally
/// provisioned outside the client that creates the function, and Lambda serves a role it
/// holds no record of.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/ses-receipt";

/// The account this emulator serves, which owns every function and topic here.
fn account() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

/// A router carrying this service plus the two services the action pre-flight resolves its
/// destinations against — Lambda (the function and its resource policy) and Amazon SNS (the
/// topic and its access policy) — all over ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; the awsQuery calls
    // below sign nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

/// A Lambda client over this emulator's own in-process callback.
fn lambda(router: &MultiServiceRouter) -> aws_sdk_lambda::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_lambda::Config::builder()
        .behavior_version(aws_sdk_lambda::config::BehaviorVersion::latest())
        .region(aws_sdk_lambda::config::Region::new(REGION))
        .endpoint_url(format!("https://lambda.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    aws_sdk_lambda::Client::from_conf(config)
}

/// An Amazon SNS client over the same callback.
fn sns(router: &MultiServiceRouter) -> aws_sdk_sns::Client {
    let callback = smithy_server_runtime::EmbeddedEmulator::from_router(router.clone());
    let config = aws_sdk_sns::Config::builder()
        .behavior_version(aws_sdk_sns::config::BehaviorVersion::latest())
        .region(aws_sdk_sns::config::Region::new(REGION))
        .endpoint_url(format!("https://sns.{REGION}.amazonaws.com"))
        .credentials_provider(callback.aws_credentials_provider())
        .http_client(callback.http_client())
        .retry_config(aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    aws_sdk_sns::Client::from_conf(config)
}

/// The ARN of the receipt rule `rule_name` of [`RULE_SET`], which is the value a grant written
/// for one receipt rule scopes itself to (`aws:SourceArn`).
fn rule_arn(rule_name: &str) -> String {
    format!(
        "arn:aws:ses:{REGION}:{}:receipt-rule-set/{RULE_SET}:receipt-rule/{rule_name}",
        account()
    )
}

/// `CreateReceiptRuleSet`.
async fn create_rule_set(router: &MultiServiceRouter) {
    let (status, body) = call(
        router,
        "CreateReceiptRuleSet",
        &[field("RuleSetName", RULE_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "creating the rule set: expected 200, got {status} with {body}"
    );
}

/// Create the function `name` with no resource policy, and answer its ARN.
async fn create_function(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let code = aws_sdk_lambda::types::FunctionCode::builder()
        .zip_file(aws_smithy_types::Blob::new(b"receipt-handler".to_vec()))
        .build();
    let created = lambda(router)
        .create_function()
        .function_name(name)
        .role(ROLE)
        .runtime(aws_sdk_lambda::types::Runtime::Python312)
        .handler("index.handler")
        .code(code)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: creating {name}: {error}"));
    created
        .function_arn()
        .unwrap_or_else(|| panic!("case {case:?}: the created function has an ARN"))
        .to_string()
}

/// Add one statement to `name`'s resource-based policy, as `AddPermission` writes it.
async fn add_permission(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    principal: &str,
    source_arn: Option<&str>,
) {
    let mut request = lambda(router)
        .add_permission()
        .function_name(name)
        .statement_id("ses-receipt-invoke")
        .action(INVOKE_ACTION)
        .principal(principal);
    if let Some(source_arn) = source_arn {
        request = request.source_arn(source_arn);
    }
    request
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: granting {principal} on {name}: {error}"));
}

/// Create the topic `name`, and answer the ARN Amazon SNS resolved it to.
async fn create_topic(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let created = sns(router)
        .create_topic()
        .name(name)
        .send()
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: creating the topic {name}: {error}"));
    created
        .topic_arn()
        .unwrap_or_else(|| panic!("case {case:?}: the created topic has an ARN"))
        .to_string()
}

/// What the row's `LambdaAction` function looks like in Lambda.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FunctionSetup {
    /// No function at all: the ARN names nothing Lambda holds.
    Absent,
    /// The function exists and carries no resource policy.
    NoPolicy,
    /// Its policy grants Amazon SES the invocation for the rule being stored.
    GrantsThisRule,
    /// Its policy grants Amazon SES the invocation with no `SourceArn` at all.
    GrantsUnscoped,
    /// Its policy grants Amazon SES the invocation for a DIFFERENT receipt rule.
    GrantsAnotherRule,
    /// Its policy grants the invocation to another service principal.
    GrantsAnotherPrincipal,
}

/// Which member of the action names the Amazon SNS topic, and whether that topic exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopicSetup {
    /// `SNSAction.TopicArn`, naming a topic Amazon SNS holds.
    SnsActionHeld,
    /// `SNSAction.TopicArn`, naming a well-formed topic ARN Amazon SNS holds nothing under.
    SnsActionAbsent,
    /// `BounceAction.TopicArn` — the notification topic of another action — naming a topic
    /// Amazon SNS holds.
    BounceActionHeld,
    /// The same member, naming a topic Amazon SNS holds nothing under.
    BounceActionAbsent,
}

/// The `Rule.*` form fields of a rule named `rule_name` carrying `action` as its only action.
fn rule_fields(rule_name: &str, action: &[Field]) -> Vec<Field> {
    let mut params = vec![
        field("Rule.Name", rule_name),
        field("Rule.Enabled", "true"),
        field("Rule.ScanEnabled", "false"),
    ];
    params.extend(action.iter().cloned());
    params
}

/// `CreateReceiptRule` carrying that rule.
async fn create_rule(
    router: &MultiServiceRouter,
    rule_name: &str,
    action: &[Field],
) -> (u16, String) {
    let mut params = vec![field("RuleSetName", RULE_SET)];
    params.extend(rule_fields(rule_name, action));
    call(router, "CreateReceiptRule", &params).await
}

/// `UpdateReceiptRule` carrying that rule.
async fn update_rule(
    router: &MultiServiceRouter,
    rule_name: &str,
    action: &[Field],
) -> (u16, String) {
    let mut params = vec![field("RuleSetName", RULE_SET)];
    params.extend(rule_fields(rule_name, action));
    call(router, "UpdateReceiptRule", &params).await
}

/// The `<Rule>` element `DescribeReceiptRule` reports for `rule_name`, or `None` when the set
/// holds no rule of that name.
async fn described_rule(router: &MultiServiceRouter, rule_name: &str) -> Option<String> {
    let (status, body) = call(
        router,
        "DescribeReceiptRule",
        &[field("RuleSetName", RULE_SET), field("RuleName", rule_name)],
    )
    .await;
    (status == 200)
        .then(|| element(&body, "Rule"))
        .flatten()
}

/// Assert that the refusal `body` carries `code` and `message`, at HTTP 400.
fn assert_refused(case: &str, status: u16, body: &str, code: &str, message: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {body}"
    );
    let envelope = error_envelope(body);
    assert_eq!(
        envelope,
        (Some(code.to_string()), Some(message.to_string())),
        "case {case:?}: expected ({code:?}, {message:?}), got {envelope:?}"
    );
}

/// A rule is stored only when the function its `LambdaAction` names is one Amazon SES may
/// invoke for THAT rule, on the create and on the update alike, and a refused call leaves the
/// set exactly as it was.
#[tokio::test]
async fn a_lambda_action_is_authorized_against_the_function_policy_before_the_rule_is_stored() {
    // `Some(sentence)` is the refusal, `{function}` standing for the ARN the request named.
    let rows = [
        (
            "a function whose policy grants this rule the invocation",
            "lambda-granted",
            FunctionSetup::GrantsThisRule,
            None,
        ),
        (
            "a function whose policy grants the invocation unscoped",
            "lambda-unscoped",
            FunctionSetup::GrantsUnscoped,
            None,
        ),
        (
            "a function carrying no resource policy at all",
            "lambda-unpolicied",
            FunctionSetup::NoPolicy,
            Some("Could not invoke Lambda function: {function}"),
        ),
        (
            "a grant scoped to another receipt rule",
            "lambda-other-rule",
            FunctionSetup::GrantsAnotherRule,
            Some("Could not invoke Lambda function: {function}"),
        ),
        (
            "a grant to another service principal",
            "lambda-other-principal",
            FunctionSetup::GrantsAnotherPrincipal,
            Some("Could not invoke Lambda function: {function}"),
        ),
        (
            "a well-formed function ARN Lambda holds no function under",
            "lambda-absent",
            FunctionSetup::Absent,
            Some("Could not invoke Lambda function: {function}"),
        ),
    ];

    let router = emulator();
    create_rule_set(&router).await;
    // The rule every UPDATE row replaces: an action with no destination of its own, so the
    // last-good state a refused update must leave behind is one this suite can read back.
    let good_action = vec![
        field("Rule.Actions.member.1.AddHeaderAction.HeaderName", "X-Probe"),
        field("Rule.Actions.member.1.AddHeaderAction.HeaderValue", "kept"),
    ];

    for (case, name, setup, refused_with) in rows {
        let function = format!("{name}-function");
        let function_arn = match setup {
            FunctionSetup::Absent => {
                format!("arn:aws:lambda:{REGION}:{}:function:{function}", account())
            }
            _ => create_function(&router, case, &function).await,
        };
        match setup {
            FunctionSetup::Absent | FunctionSetup::NoPolicy => {}
            FunctionSetup::GrantsThisRule => {
                add_permission(
                    &router,
                    case,
                    &function,
                    SES_PRINCIPAL,
                    Some(&rule_arn(name)),
                )
                .await;
            }
            FunctionSetup::GrantsUnscoped => {
                add_permission(&router, case, &function, SES_PRINCIPAL, None).await;
            }
            FunctionSetup::GrantsAnotherRule => {
                add_permission(
                    &router,
                    case,
                    &function,
                    SES_PRINCIPAL,
                    Some(&rule_arn("some-other-rule")),
                )
                .await;
            }
            FunctionSetup::GrantsAnotherPrincipal => {
                add_permission(
                    &router,
                    case,
                    &function,
                    "sns.amazonaws.com",
                    Some(&rule_arn(name)),
                )
                .await;
            }
        }
        let action = vec![
            field(
                "Rule.Actions.member.1.LambdaAction.FunctionArn",
                &function_arn,
            ),
            field("Rule.Actions.member.1.LambdaAction.InvocationType", "Event"),
        ];

        // The CREATE: stored, or refused with nothing persisted.
        let (status, body) = create_rule(&router, name, &action).await;
        match refused_with {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?} (create): expected the rule to be stored, got {status} with \
                     {body}"
                );
                let reported = described_rule(&router, name).await.unwrap_or_default();
                assert_eq!(
                    element(&reported, "FunctionArn").as_deref(),
                    Some(function_arn.as_str()),
                    "case {case:?}: expected the stored rule to carry {function_arn:?}, got \
                     {reported}"
                );
            }
            Some(sentence) => {
                assert_refused(
                    &format!("{case} (create)"),
                    status,
                    &body,
                    "InvalidLambdaFunction",
                    &sentence.replace("{function}", &function_arn),
                );
                let reported = described_rule(&router, name).await;
                assert_eq!(
                    reported, None,
                    "case {case:?}: expected the refused rule not to be stored, got {reported:?}"
                );
                // The UPDATE of a rule that IS stored must refuse the same way and leave the
                // stored rule at its last-good state.
                let (status, body) = create_rule(&router, name, &good_action).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: seeding the last-good rule: expected 200, got {status} with \
                     {body}"
                );
            }
        }

        // The UPDATE: the same pre-flight, over a rule the set already holds.
        let before = described_rule(&router, name).await;
        let (status, body) = update_rule(&router, name, &action).await;
        match refused_with {
            None => assert_eq!(
                status, 200,
                "case {case:?} (update): expected the update to be stored, got {status} with \
                 {body}"
            ),
            Some(sentence) => {
                assert_refused(
                    &format!("{case} (update)"),
                    status,
                    &body,
                    "InvalidLambdaFunction",
                    &sentence.replace("{function}", &function_arn),
                );
                let after = described_rule(&router, name).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: expected the refused update to leave the stored rule \
                     untouched, got {after:?}"
                );
            }
        }
    }
}

/// A rule is stored only when every Amazon SNS topic its actions notify is one Amazon SNS
/// really holds, whichever member names the topic, on the create and on the update alike.
#[tokio::test]
async fn an_action_topic_is_resolved_against_amazon_sns_before_the_rule_is_stored() {
    let rows = [
        (
            "a topic Amazon SNS holds, named by the SNSAction",
            "sns-held",
            TopicSetup::SnsActionHeld,
            None,
        ),
        (
            "a topic ARN Amazon SNS holds nothing under, named by the SNSAction",
            "sns-absent",
            TopicSetup::SnsActionAbsent,
            Some("Could not publish to SNS topic: {topic}"),
        ),
        (
            "a topic Amazon SNS holds, named by another action's notification member",
            "bounce-held",
            TopicSetup::BounceActionHeld,
            None,
        ),
        (
            "a topic ARN Amazon SNS holds nothing under, named by that same member",
            "bounce-absent",
            TopicSetup::BounceActionAbsent,
            Some("Could not publish to SNS topic: {topic}"),
        ),
    ];

    let router = emulator();
    create_rule_set(&router).await;
    let good_action = vec![
        field("Rule.Actions.member.1.AddHeaderAction.HeaderName", "X-Probe"),
        field("Rule.Actions.member.1.AddHeaderAction.HeaderValue", "kept"),
    ];

    for (case, name, setup, refused_with) in rows {
        let topic = format!("{name}-topic");
        let topic_arn = match setup {
            TopicSetup::SnsActionHeld | TopicSetup::BounceActionHeld => {
                create_topic(&router, case, &topic).await
            }
            _ => format!("arn:aws:sns:{REGION}:{}:{topic}", account()),
        };
        let action = match setup {
            TopicSetup::SnsActionHeld | TopicSetup::SnsActionAbsent => vec![field(
                "Rule.Actions.member.1.SNSAction.TopicArn",
                &topic_arn,
            )],
            TopicSetup::BounceActionHeld | TopicSetup::BounceActionAbsent => vec![
                field("Rule.Actions.member.1.BounceAction.TopicArn", &topic_arn),
                field("Rule.Actions.member.1.BounceAction.SmtpReplyCode", "550"),
                field("Rule.Actions.member.1.BounceAction.Message", "Mailbox full"),
                field(
                    "Rule.Actions.member.1.BounceAction.Sender",
                    "bounces@example.com",
                ),
            ],
        };

        let (status, body) = create_rule(&router, name, &action).await;
        match refused_with {
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?} (create): expected the rule to be stored, got {status} with \
                     {body}"
                );
                let reported = described_rule(&router, name).await.unwrap_or_default();
                assert_eq!(
                    element(&reported, "TopicArn").as_deref(),
                    Some(topic_arn.as_str()),
                    "case {case:?}: expected the stored rule to carry {topic_arn:?}, got \
                     {reported}"
                );
            }
            Some(sentence) => {
                assert_refused(
                    &format!("{case} (create)"),
                    status,
                    &body,
                    "InvalidSnsTopic",
                    &sentence.replace("{topic}", &topic_arn),
                );
                let reported = described_rule(&router, name).await;
                assert_eq!(
                    reported, None,
                    "case {case:?}: expected the refused rule not to be stored, got {reported:?}"
                );
                let (status, body) = create_rule(&router, name, &good_action).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: seeding the last-good rule: expected 200, got {status} with \
                     {body}"
                );
            }
        }

        let before = described_rule(&router, name).await;
        let (status, body) = update_rule(&router, name, &action).await;
        match refused_with {
            None => assert_eq!(
                status, 200,
                "case {case:?} (update): expected the update to be stored, got {status} with \
                 {body}"
            ),
            Some(sentence) => {
                assert_refused(
                    &format!("{case} (update)"),
                    status,
                    &body,
                    "InvalidSnsTopic",
                    &sentence.replace("{topic}", &topic_arn),
                );
                let after = described_rule(&router, name).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: expected the refused update to leave the stored rule \
                     untouched, got {after:?}"
                );
            }
        }
    }
}
