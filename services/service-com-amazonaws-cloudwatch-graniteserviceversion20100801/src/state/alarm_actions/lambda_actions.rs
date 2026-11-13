// SPDX-License-Identifier: CC0-1.0

//! The LAMBDA FUNCTION actions an alarm may carry, and what executing one does.
//!
//! "Lambda actions: Invoke the latest version of a Lambda function:
//! arn:aws:lambda:region:account-id:function:function-name Invoke a specific version of a
//! Lambda function: arn:aws:lambda:region:account-id:function:function-name:version-number
//! Invoke a function by using an alias Lambda function:
//! arn:aws:lambda:region:account-id:function:function-name:alias-name"
//! (`PutMetricAlarmInput$AlarmActions`, and the same for `OKActions`,
//! `InsufficientDataActions` and every action list of `PutCompositeAlarm`) — so an alarm whose
//! transition fires one of these ARNs has that FUNCTION invoked, at the version or alias the
//! ARN names, with the transition as its payload
//! ([`super::state_change_events::AlarmStateChange::lambda_payload`]).
//!
//! The invocation is ASYNCHRONOUS (`InvocationType` `Event`): an alarm action is not part of
//! any operation's answer, so nothing waits for what the function returns and the alarm's
//! history records the handover rather than the run.
//!
//! # What authorizes it
//!
//! A function is a resource of another service and CloudWatch invokes it as its own service
//! principal ([`ALARM_ACTION_PRINCIPALS`]) rather than as the function's owner, so the
//! invocation needs a GRANT in the function's own resource-based policy — the document
//! `AddPermission` writes — naming that principal and [`INVOKE_FUNCTION_ACTION`]. That is the
//! whole of what `PutMetricAlarm` says about a Lambda action's permissions: "to perform a
//! PutSubscriptionFilter operation for any destination except a Lambda function, you must also
//! have the iam:PassRole permission" is the sibling rule for a log subscription, and an alarm
//! carries no role for a function either — what admits the invocation is the function's own
//! policy. So a function whose policy grants this service nothing is NOT invoked, and the
//! alarm's history records the authorization failure AWS Lambda answers such a call with; this
//! is deliberately stricter than the reading a topic action applies ([`super::publish_refusal`],
//! where only an explicit deny withholds the publish), because a topic action is a publish the
//! topic's own owner configured while a function grant is one a client has to write.
//!
//! AWS Lambda is reached the way any other service is reached from here: over the AWS SDK
//! driven on the in-process callback ([`smithy_server_runtime::OperationContext::callback`]),
//! in the Region the action's own ARN names.

/// The ARN vendor of a Lambda function action.
const LAMBDA_ARN_VENDOR: &str = "lambda";

/// The ARN resource type of a function, which is what tells a function's ARN from any other
/// ARN of this vendor (a layer, an event source mapping).
const FUNCTION_RESOURCE_TYPE: &str = "function";

/// The IAM action an invocation performs, which is what a grant for this service has to name
/// ("Actions defined by AWS Lambda", IAM Service Authorization Reference).
pub const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// Every spelling of the principal CloudWatch invokes a function alarm action as.
///
/// A client grants the invocation to the principal AWS documents for an alarm action
/// (`lambda.alarms.cloudwatch.amazonaws.com`, the principal the Lambda alarm-action
/// documentation has a client add a permission for); the service's own principal is the other
/// spelling of the same caller, and a policy written with either names this service. Both are
/// passed as the calling principal's spellings, exactly as an assumed role is decided under
/// both its session ARN and its role ARN.
const ALARM_ACTION_PRINCIPALS: [&str; 2] = [
    "lambda.alarms.cloudwatch.amazonaws.com",
    super::CLOUDWATCH_SERVICE_PRINCIPAL,
];

/// The condition key naming the resource a cross-service invocation came FROM — the alarm whose
/// transition fires the action — which is what a grant may scope its statement by.
const SOURCE_ARN_CONDITION_KEY: &str = "aws:SourceArn";

/// The condition key naming the ACCOUNT that alarm belongs to.
const SOURCE_ACCOUNT_CONDITION_KEY: &str = "aws:SourceAccount";

/// The error code AWS Lambda's `GetPolicy` answers when there is no policy to report: the same
/// answer a name no function resolves to earns, and for an invocation those are the same thing
/// — there is no document granting this service anything.
const POLICY_ABSENT_CODE: &str = "ResourceNotFoundException";

/// Whether `arn` names a Lambda FUNCTION, and the ARN itself when it does.
///
/// The partition and the Region are not matched: a function action is one whatever partition it
/// names, and the Region is where the invocation is addressed rather than part of what makes
/// the ARN a function's.
#[must_use]
pub fn of_action_arn(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    (parsed.vendor == LAMBDA_ARN_VENDOR && parsed.resource_type() == FUNCTION_RESOURCE_TYPE)
        .then_some(arn)
}

/// Invoke the function `arn` names with `change`, and answer the failure it failed with —
/// `None` when AWS Lambda accepted the invocation.
///
/// The function's own resource-based policy is read FIRST and decides the handover: a policy
/// that grants this service nothing withholds the invocation, which is recorded as the
/// authorization failure it is and invokes nothing. A function that answers that read with
/// anything other than the absence of a policy decides nothing here — the `Invoke` that follows
/// reports what such a function answers, which is that service's own answer rather than a
/// verdict read off a policy this never saw.
pub async fn execute(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    arn: &str,
    change: &super::state_change_events::AlarmStateChange,
) -> Option<String> {
    // The Region ends up in the endpoint URL this hop is addressed on, so it is read under the
    // Region grammar every model states for the field rather than as arbitrary text.
    let Some(region) = ::service_common::arn::parse(arn)
        .map(|parsed| parsed.region)
        .and_then(::service_common::arn::region_label)
    else {
        return Some(format!(
            "The action {arn} names no Region in which a Lambda function can be invoked"
        ));
    };
    let client = lambda_client(callback, region);
    if let Some(refusal) = invoke_refusal(&client, arn, change.alarm_arn.as_deref()).await {
        return Some(refusal);
    }
    let payload = change.lambda_payload().to_string();
    client
        .invoke()
        .function_name(arn)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
        .payload(::aws_smithy_types::Blob::new(payload.into_bytes()))
        .send()
        .await
        .err()
        .map(|error| super::action_failure(&error))
}

/// The authorization failure the function `arn` names refuses this alarm action with, or `None`
/// when its own resource-based policy allows it.
///
/// The decision is the ordinary IAM question — may this principal perform this action on this
/// resource? — answered by the shared evaluation of policy documents
/// ([`::service_common::iam_authorization`]) over the document the function itself reports,
/// rather than by a reading of its own. The request carries the keys such a grant scopes its
/// statements by — the alarm's own ARN and the account it belongs to — so a statement
/// conditioned on them is decided rather than left undecided.
async fn invoke_refusal(
    client: &::aws_sdk_lambda::Client,
    arn: &str,
    alarm_arn: Option<&str>,
) -> Option<String> {
    let policy = function_policy(client, arn).await?;
    let principals: Vec<String> = ALARM_ACTION_PRINCIPALS
        .iter()
        .map(|principal| (*principal).to_string())
        .collect();
    let mut context: Vec<(String, String)> = Vec::new();
    if let Some(alarm_arn) = alarm_arn {
        context.push((SOURCE_ARN_CONDITION_KEY.to_string(), alarm_arn.to_string()));
        if let Some(account) = ::service_common::arn::parse(alarm_arn).map(|parsed| parsed.account)
        {
            context.push((
                SOURCE_ACCOUNT_CONDITION_KEY.to_string(),
                account.to_string(),
            ));
        }
    }
    let request = ::service_common::iam_authorization::Request::new(
        INVOKE_FUNCTION_ACTION,
        arn,
        &principals,
    )
    .with_context(&context);
    let allows = policy.as_deref().is_some_and(|policy| {
        ::service_common::iam_authorization::resource_decision(policy, &request)
            == ::service_common::iam_authorization::Decision::Allowed
    });
    if allows {
        return None;
    }
    // AWS Lambda's own refusal of such a call: the authorization sentence naming the principal
    // the invocation would have been made as, the action and the function.
    Some(
        ::service_common::iam_authorization::not_authorized_by_resource_policy(
            ALARM_ACTION_PRINCIPALS[0],
            INVOKE_FUNCTION_ACTION,
            arn,
        ),
    )
}

/// What the function `arn` names answered when its resource-based policy was read:
/// `Some(Some(document))` for a function carrying one, `Some(None)` for one that carries none,
/// and `None` for a function that did not answer the read at all — the three told apart because
/// only the last leaves the decision to the invocation itself.
async fn function_policy(
    client: &::aws_sdk_lambda::Client,
    arn: &str,
) -> Option<Option<String>> {
    let answered = client.get_policy().function_name(arn).send().await;
    let error = match answered {
        Ok(answer) => return Some(answer.policy().map(ToString::to_string)),
        Err(error) => error,
    };
    if ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error)
        == Some(POLICY_ABSENT_CODE)
    {
        return Some(None);
    }
    ::tracing::warn!(
        function = %arn,
        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
        "this alarm action's function could not be read for its resource-based policy, so the \
         invocation answers for it"
    );
    None
}

/// An AWS Lambda client over the in-process callback into this same emulator, addressed in
/// `region`.
fn lambda_client(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_lambda::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which ARNs name a function action: the three forms the model documents, and the ARNs of
    /// other kinds that do not.
    #[test]
    fn a_function_action_is_told_from_every_other_action_kind() {
        for (case, arn, expected) in [
            (
                "the latest version of a function",
                "arn:aws:lambda:us-west-2:000000000000:function:fan-out",
                true,
            ),
            (
                "a specific version of a function",
                "arn:aws:lambda:us-west-2:000000000000:function:fan-out:7",
                true,
            ),
            (
                "a function by alias",
                "arn:aws:lambda:us-west-2:000000000000:function:fan-out:live",
                true,
            ),
            (
                "another partition's function",
                "arn:aws-us-gov:lambda:us-gov-west-1:000000000000:function:fan-out",
                true,
            ),
            (
                "a layer of the same vendor",
                "arn:aws:lambda:us-west-2:000000000000:layer:shared:1",
                false,
            ),
            (
                "an SNS topic",
                "arn:aws:sns:us-west-2:000000000000:alarms",
                false,
            ),
            (
                "an EC2 instance action",
                "arn:aws:automate:us-west-2:ec2:terminate",
                false,
            ),
            ("not an ARN at all", "fan-out", false),
        ] {
            let found = of_action_arn(arn).is_some();
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected} for {arn:?}, got {found}"
            );
        }
    }
}
