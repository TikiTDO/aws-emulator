// SPDX-License-Identifier: CC0-1.0

//! Whether every destination the actions of a receipt rule name is one Amazon SES may really
//! reach — the ONE pre-flight every operation that STORES a receipt rule runs before it
//! persists anything.
//!
//! A receipt rule's actions address resources of OTHER services, and each of those services
//! has to let Amazon SES act on the account's behalf: `LambdaAction` documents it ("To enable
//! Amazon SES to call your Amazon Web Services Lambda function or to publish to an Amazon SNS
//! topic of another account, Amazon SES must have permission to access those resources"), and
//! `S3Action` states the same for its bucket and its encryption key. So a rule naming a
//! destination Amazon SES could never reach is refused when it is written, rather than being
//! blessed at write time and then silently dropping every message the rule matches — and the
//! refusal is the one the addressed member carries:
//!
//! * an `S3Action` whose bucket or key withholds the write: `InvalidS3Configuration`
//!   ([`super::s3_action_authorization`], which answers that half);
//! * a `LambdaAction` whose function cannot be invoked: `InvalidLambdaFunction`;
//! * any action's `TopicArn` naming a topic that cannot be published to: `InvalidSnsTopic`.
//!
//! Each question is answered by the service that OWNS the answer, reached the way the
//! framework documents a handler reaching another emulated service
//! ([`smithy_server_runtime::OperationContext::callback`]): the AWS SDK over the emulator's
//! own in-process transport, so nothing here opens a socket or leaves the host. Every call
//! made is a READ, so a refused rule leaves each destination exactly as the caller left it.
//!
//! Two readings keep a refusal GROUNDED in what the owning service stated:
//!
//! * an answer that is not that service stating the destination's own facts — an emulator
//!   with no such service mounted beside this one, a transport that carried nothing, any
//!   failure to ask — establishes nothing, so it refuses nothing;
//! * a destination's own policy refuses the access only where the document SAYS so
//!   ([`withholds`]): an explicit deny, or a document that states nothing about the access at
//!   all. A statement that is about the access and hinges on a condition key this emulator
//!   carries no value for leaves the grant undecided, and an undecided grant is not evidence
//!   that the real service would refuse the call.
//!
//! The ONE reading, so `CreateReceiptRule` and `UpdateReceiptRule` hold a stored rule's
//! destinations to the same requirements instead of each keeping its own copy of them.

use ::service_common::iam_authorization::{self as iam, Decision};

/// The service principal Amazon SES acts under when it carries out a receipt rule's actions,
/// and therefore the principal a destination's resource policy names to grant it.
pub const SES_SERVICE_PRINCIPAL: &str = "ses.amazonaws.com";

/// The condition key naming the ACCOUNT whose resource a service principal is acting on
/// behalf of, which is how a grant to such a principal is scoped to one account.
pub const SOURCE_ACCOUNT_KEY: &str = "aws:SourceAccount";

/// The condition key naming the RESOURCE a service principal is acting on behalf of, which
/// for this pre-flight is the receipt rule being stored ([`receipt_rule_arn`]).
pub const SOURCE_ARN_KEY: &str = "aws:SourceArn";

/// The IAM action Amazon SES takes on the function a `LambdaAction` names: the action calls
/// the function ("this action calls an Amazon Web Services Lambda function", `LambdaAction`),
/// and an invocation "requires permission for the `lambda:InvokeFunction` action".
pub const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The IAM action Amazon SES takes on the topic an action's `TopicArn` names: it PUBLISHES a
/// notification to it (`SNSAction` "publishes a notification to Amazon Simple Notification
/// Service").
pub const PUBLISH_ACTION: &str = "sns:Publish";

/// The topic attribute carrying its access control policy: "Policy — the JSON serialization
/// of the topic's access control policy" (`GetTopicAttributes`).
const TOPIC_POLICY_ATTRIBUTE: &str = "Policy";

/// How many DISTINCT destinations one stored rule is authorized against.
///
/// An EMULATOR ceiling, not an AWS quota: `ReceiptRule$Actions` is a list a caller writes, and
/// each destination in it costs this pre-flight a read of another service, so the work one
/// request can drive is bounded here. A receipt rule's action list is short — it is the
/// ordered set of things Amazon SES does with one received message — so no rule a client
/// really writes reaches it. A destination past the ceiling is left unjudged, which is the
/// narrower answer: nothing is refused on an answer nobody asked for.
pub const MAX_AUTHORIZED_DESTINATIONS: usize = 20;

/// Why one destination of a receipt rule cannot be stored, keyed by the modelled refusal the
/// addressed member carries. Each carries the value the REQUEST named, which is what the
/// refusal's own member reports, and the sentence Amazon SES words that fault with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DestinationRefusal {
    /// `InvalidS3ConfigurationException`: the bucket or the key of an `S3Action`.
    S3 {
        /// `InvalidS3ConfigurationException$Bucket`.
        bucket: String,
        /// The refusal's message.
        message: String,
    },
    /// `InvalidLambdaFunctionException`: the function a `LambdaAction` names.
    LambdaFunction {
        /// `InvalidLambdaFunctionException$FunctionArn`.
        function_arn: String,
        /// The refusal's message.
        message: String,
    },
    /// `InvalidSnsTopicException`: the topic an action's `TopicArn` names.
    SnsTopic {
        /// `InvalidSnsTopicException$Topic`.
        topic: String,
        /// The refusal's message.
        message: String,
    },
}

/// How Amazon SES words the refusal of a `LambdaAction` whose function it cannot invoke: the
/// fixed sentence plus the `FunctionArn` the request carried.
#[must_use]
pub fn could_not_invoke_message(function_arn: &str) -> String {
    format!("Could not invoke Lambda function: {function_arn}")
}

/// How Amazon SES words the refusal of an action whose Amazon SNS topic it cannot publish to:
/// the fixed sentence plus the `TopicArn` the request carried.
#[must_use]
pub fn could_not_publish_message(topic_arn: &str) -> String {
    format!("Could not publish to SNS topic: {topic_arn}")
}

/// The ARN of the receipt rule `rule_name` of the rule set `rule_set_name`, which is the
/// value `aws:SourceArn` carries in a grant written for one receipt rule.
#[must_use]
pub fn receipt_rule_arn(region: &str, rule_set_name: &str, rule_name: &str) -> String {
    format!(
        "arn:aws:ses:{region}:{}:receipt-rule-set/{rule_set_name}:receipt-rule/{rule_name}",
        ::engine::EMULATOR_ACCOUNT_ID
    )
}

/// The condition context the request Amazon SES makes on behalf of the receipt rule
/// `rule_arn` carries: the account it acts for and the rule itself, which are the two keys a
/// grant written for one receipt rule is scoped by.
#[must_use]
pub fn rule_scoped_context(rule_arn: &str) -> [(String, String); 2] {
    [
        (
            SOURCE_ACCOUNT_KEY.to_string(),
            ::engine::EMULATOR_ACCOUNT_ID.to_string(),
        ),
        (SOURCE_ARN_KEY.to_string(), rule_arn.to_string()),
    ]
}

/// Whether `policy` WITHHOLDS the access `request` names — the one reading a refusal of a
/// destination may be grounded in (see the module docs).
///
/// An explicit deny withholds it, and so does a document that states nothing about the access
/// at all: "ensure that you add a statement to your … policy to give Amazon SES permission",
/// so a policy silent about this principal grants it nothing. A document whose only statement
/// about the access could not be DECIDED — it hinges on a condition key this emulator carries
/// no value for — withholds nothing: that is this evaluation lacking a value, not the owning
/// service refusing the call.
#[must_use]
pub fn withholds(policy: &str, request: &iam::Request<'_>) -> bool {
    let outcome = iam::resource_outcome_prepared(&iam::PreparedDocument::read(policy), request);
    match outcome.decision {
        Decision::Allowed => false,
        Decision::Denied => true,
        Decision::NotStated => outcome.required_context_keys.is_empty(),
    }
}

/// The refusal the FIRST destination of `actions` that Amazon SES could not reach earns, and
/// `None` when every one of them is reachable — or when nothing establishes otherwise.
///
/// `rule_set_name` and `rule_name` name the rule being stored, which is what a grant in a
/// destination's policy is scoped to; `region` is the Region the request is signed for, which
/// is where a destination whose own ARN names no Region is asked of.
///
/// The actions are walked in the order the rule carries them, and each DISTINCT destination is
/// asked about once (at most [`MAX_AUTHORIZED_DESTINATIONS`] of them).
pub async fn unauthorized_destination(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    rule_set_name: &str,
    rule_name: &str,
    actions: &[crate::ops::types::ReceiptAction],
) -> Option<DestinationRefusal> {
    let rule_arn = receipt_rule_arn(region, rule_set_name, rule_name);
    let mut asked: Vec<String> = Vec::new();
    for action in actions {
        if let Some(s3_action) = &action.s3_action {
            let destination = format!(
                "s3\u{1}{}\u{1}{}\u{1}{}",
                s3_action.bucket_name,
                s3_action.object_key_prefix.as_deref().unwrap_or_default(),
                s3_action.kms_key_arn.as_deref().unwrap_or_default()
            );
            if first_ask(&mut asked, destination) {
                if let Some(refusal) = super::s3_action_authorization::unauthorized_s3_destination(
                    ctx, region, s3_action, &rule_arn,
                )
                .await
                {
                    return Some(DestinationRefusal::S3 {
                        bucket: refusal.bucket,
                        message: refusal.message,
                    });
                }
            }
        }
        if let Some(lambda_action) = &action.lambda_action {
            let function_arn = lambda_action.function_arn.as_str();
            if first_ask(&mut asked, format!("lambda\u{1}{function_arn}")) {
                if let Some(message) = function_refusal(ctx, region, function_arn, &rule_arn).await
                {
                    return Some(DestinationRefusal::LambdaFunction {
                        function_arn: function_arn.to_string(),
                        message,
                    });
                }
            }
        }
        for (_, topic_arn) in super::topic_arn_members(action) {
            if first_ask(&mut asked, format!("sns\u{1}{topic_arn}")) {
                if let Some(message) = topic_refusal(ctx, region, topic_arn, &rule_arn).await {
                    return Some(DestinationRefusal::SnsTopic {
                        topic: topic_arn.to_string(),
                        message,
                    });
                }
            }
        }
    }
    None
}

/// Whether `destination` is one this request still has an ask to spend on, noting it as asked
/// about when it is: a destination already asked about, and one past the ceiling
/// ([`MAX_AUTHORIZED_DESTINATIONS`]), is not asked about again.
fn first_ask(asked: &mut Vec<String>, destination: String) -> bool {
    if asked.contains(&destination) || asked.len() >= MAX_AUTHORIZED_DESTINATIONS {
        return false;
    }
    asked.push(destination);
    true
}

/// Why Amazon SES could not invoke the function `function_arn` names for the rule `rule_arn`,
/// and `None` when it could — or when nothing establishes otherwise.
///
/// The decision is the FUNCTION's own resource-based policy, read through Lambda's `GetPolicy`
/// — the same document `AddPermission` writes — and evaluated for the service principal the
/// invocation is made as, scoped to the receipt rule being stored. A function Lambda holds no
/// policy for, and one it does not hold at all, grants Amazon SES nothing: Lambda answers both
/// with the one error it states about a resource it cannot show
/// ([`::aws_sdk_lambda::operation::get_policy::GetPolicyError::ResourceNotFoundException`]),
/// and either way nothing permits the invocation. Any OTHER answer — a fault, a throttle, a
/// read this emulator's Lambda does not serve — states nothing about the function, so it
/// refuses nothing.
///
/// `region` is the Region the REQUEST is signed for, which is where the rule being stored
/// lives: no text of a request chooses which endpoint is asked.
async fn function_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    function_arn: &str,
    rule_arn: &str,
) -> Option<String> {
    let answered = lambda_client(ctx, region)
        .get_policy()
        .function_name(function_arn)
        .send()
        .await;
    let policy = match answered {
        Ok(answer) => answer.policy().unwrap_or_default().to_string(),
        Err(failure) => {
            let stated = matches!(
                failure.as_service_error(),
                Some(
                    ::aws_sdk_lambda::operation::get_policy::GetPolicyError::
                        ResourceNotFoundException(_)
                )
            );
            if !stated {
                unanswered("the receipt rule's Lambda function", function_arn, &failure);
                return None;
            }
            // Lambda stated it can show no such policy: it holds no such function, or the
            // function holds none. Nothing grants Amazon SES the invocation in either case.
            return Some(could_not_invoke_message(function_arn));
        }
    };
    let principals = [SES_SERVICE_PRINCIPAL.to_string()];
    let context = rule_scoped_context(rule_arn);
    let request = iam::Request::new(INVOKE_FUNCTION_ACTION, function_arn, &principals)
        .with_context(&context)
        .with_published_keys(&[SOURCE_ACCOUNT_KEY, SOURCE_ARN_KEY]);
    withholds(&policy, &request).then(|| could_not_invoke_message(function_arn))
}

/// Why Amazon SES could not publish to the topic `topic_arn` names for the rule `rule_arn`,
/// and `None` when it could — or when nothing establishes otherwise.
///
/// The topic has to BE there, which is Amazon SNS's own answer (`GetTopicAttributes`, whose
/// `NotFound` and `InvalidParameter` are the two it states about a topic it cannot show), and a
/// topic of ANOTHER account has to grant the service principal the publish: "to enable Amazon
/// SES to … publish to an Amazon SNS topic of another account, Amazon SES must have permission
/// to access those resources" (`S3Action`), so a topic of the account the rule belongs to
/// needs no statement of its own. Any OTHER answer states nothing about the topic and refuses
/// nothing.
///
/// `region` is the Region the REQUEST is signed for, as it is for the function above.
async fn topic_refusal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    topic_arn: &str,
    rule_arn: &str,
) -> Option<String> {
    let answered = sns_client(ctx, region)
        .get_topic_attributes()
        .topic_arn(topic_arn)
        .send()
        .await;
    let attributes = match answered {
        Ok(attributes) => attributes,
        Err(failure) => {
            let stated = matches!(
                failure.as_service_error(),
                Some(
                    ::aws_sdk_sns::operation::get_topic_attributes::GetTopicAttributesError::
                        NotFoundException(_)
                        | ::aws_sdk_sns::operation::get_topic_attributes::
                            GetTopicAttributesError::InvalidParameterException(_)
                )
            );
            if !stated {
                unanswered("the receipt rule's Amazon SNS topic", topic_arn, &failure);
                return None;
            }
            // Amazon SNS stated it holds no topic the value names: there is nowhere to
            // publish the notification to.
            return Some(could_not_publish_message(topic_arn));
        }
    };
    if owned_by_this_account(topic_arn) {
        return None;
    }
    let policy = attributes
        .attributes()
        .and_then(|held| held.get(TOPIC_POLICY_ATTRIBUTE))
        .cloned()
        .unwrap_or_default();
    let principals = [SES_SERVICE_PRINCIPAL.to_string()];
    let context = rule_scoped_context(rule_arn);
    let request = iam::Request::new(PUBLISH_ACTION, topic_arn, &principals)
        .with_context(&context)
        .with_published_keys(&[SOURCE_ACCOUNT_KEY, SOURCE_ARN_KEY]);
    withholds(&policy, &request).then(|| could_not_publish_message(topic_arn))
}

/// Whether `arn` names a resource of the account this emulator serves, which is the account a
/// receipt rule belongs to. An ARN carrying no account field names no other account.
fn owned_by_this_account(arn: &str) -> bool {
    ::service_common::arn::parse(arn).is_none_or(|parsed| {
        parsed.account.is_empty() || parsed.account == ::engine::EMULATOR_ACCOUNT_ID
    })
}

/// Note that a destination's own service did not state its facts, and so that the destination
/// is judged against nothing. Worth a line to an operator, because a deployment with no such
/// service to ask is one where this half of the pre-flight does not run; the caller's own
/// value is rendered in its escaped `Debug` form, so an identifier carrying a newline cannot
/// forge a log line.
fn unanswered<E, R>(
    destination: &str,
    named: &str,
    failure: &::aws_smithy_runtime_api::client::result::SdkError<E, R>,
) where
    E: ::std::error::Error + 'static,
    R: ::std::fmt::Debug,
{
    ::tracing::warn!(
        destination,
        named = ?named,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(failure),
        "the receipt rule's destination could not be read, so it is not judged"
    );
}

/// Lambda's client for `region`, over the in-process callback into this same emulator.
///
/// The transport is in-process, so this disables what a network client needs and this one
/// cannot have: retries, timeouts, stalled-stream protection and identity caching each need a
/// sleep implementation the callback transport is not given, and there is nothing to retry
/// behind it anyway. Nothing here opens a socket or leaves the host — `ctx.callback()` is the
/// framework's in-process channel into this same emulator's services — and the only
/// caller-supplied value that reaches the client's authority is the signing Region, which is a
/// Region LABEL by the time it gets here ([`crate::state::region_from_request`]), so no text of
/// a request can steer the address.
fn lambda_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

/// Amazon SNS's client for `region`, built exactly as [`lambda_client`] is and for the same
/// reasons.
fn sns_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_sns::Client {
    let config = ::aws_sdk_sns::Config::builder()
        .behavior_version(::aws_sdk_sns::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_sns::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://sns.{region}.amazonaws.com"))
        .retry_config(::aws_sdk_sns::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_sns::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_sns::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_sns::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_sns::Client::from_conf(config)
}
