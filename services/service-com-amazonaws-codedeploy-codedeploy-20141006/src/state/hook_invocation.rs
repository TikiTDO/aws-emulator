// SPDX-License-Identifier: CC0-1.0

//! The INVOCATION of a deployment's lifecycle event hooks: the one hop this
//! service makes to AWS Lambda for a validation function.
//!
//! A hook of a Lambda or Amazon ECS deployment is a Lambda function CodeDeploy
//! invokes with the lifecycle event — "Pass this ID to a Lambda function that
//! validates a deployment lifecycle event" of the deployment id, and "the
//! execution ID of a deployment's lifecycle hook" of the id the event carries
//! (`smithy_query(query="doc",shape="PutLifecycleEventHookExecutionStatusInput",member="deploymentId")`)
//! — and the function reports its result back through
//! `PutLifecycleEventHookExecutionStatus` with that pair. So the event is those
//! two members, and nothing else of the run is this service's: the RESULT is the
//! function's own report, never synthesised here.
//!
//! The invocation is made through the in-process callback, over AWS Lambda's own
//! modelled `Invoke`, so the function that runs is the one `CreateFunction`
//! created and the report comes back through this service's own front door. It
//! runs off the response path, in a task of its own, because the call that created
//! the deployment answered long before — the deployment is IN FLIGHT while its
//! hook runs, which is exactly the window a client polls, and a validation
//! function may take as long as Lambda lets it.
//!
//! Nothing about the RUN is this service's: the function is invoked exactly as a
//! client's own `Invoke` of it would be, so what the run may reach, how long it
//! may take and what it may allocate are the ones AWS Lambda already applies to
//! every invocation it serves — this module grants no authority of its own, holds
//! no credential of its own (it signs with the callback's), reads no host
//! resource and starts no process. The one bound it does choose is how long THIS
//! service waits ([`HOOK_RUN_BUDGET`]), so a function that never finishes cannot
//! hold a deployment open for ever. Which function is invoked is the one the
//! deployment's own stored revision declared when it was created, never a value
//! taken off the request being served.
//!
//! Which hook is due, and what a reported result does, is
//! [`super::lifecycle_hook`]'s; this module only runs the function and records
//! what came of the run.

use std::sync::Arc;
use std::time::Duration;

use ::smithy_server_runtime::EmbeddedEmulator;

use super::lifecycle_hook::{self, Dispatch};
use super::{now_ms, with_state};

/// The member of the lifecycle event naming the deployment being validated.
const EVENT_DEPLOYMENT_ID: &str = "DeploymentId";

/// The member naming the hook execution the function reports its result under.
const EVENT_HOOK_EXECUTION_ID: &str = "LifecycleEventHookExecutionId";

/// How long this service waits for one validation function to run.
///
/// The RUN is bounded by Lambda — the function's own configured timeout — so this
/// is the bound on the WAIT: a function still running when it elapses is not
/// waited for any longer, and the lifecycle event it was validating has no result,
/// which fails the deployment rather than leaving it in flight for ever. It is
/// this emulator's own figure, above the longest run Lambda will give a function
/// in this process and far below a span a client would call stuck.
const HOOK_RUN_BUDGET: Duration = Duration::from_secs(60);

/// AWS Lambda's ARN namespace, for reading the Region out of a hook the AppSpec
/// names by ARN.
const LAMBDA_ARN_NAMESPACE: &str = "lambda";

/// The Region one hook's invocation is addressed to: the one the function's own
/// ARN names, when the AppSpec names it that way.
///
/// A Lambda function is a regional resource, and the full-ARN form is the only one
/// that names a Region, so a hook named by ARN is invoked through that Region's
/// endpoint and one named by name through this service's default. The transport is
/// in process either way: the label selects the endpoint the client is configured
/// for, and the invocation is served by the Lambda this emulator mounts.
fn invocation_region(function_name: &str) -> &str {
    ::service_common::arn::region_of(function_name, LAMBDA_ARN_NAMESPACE)
        .unwrap_or(super::DEFAULT_REGION)
}

/// Dispatch the lifecycle event hook of every deployment that is due one now.
///
/// Called from the clock that drives a deployment in flight
/// ([`super::deployment_clock`]), which is the one place that can invoke anything
/// while nobody is calling this service: the hook of a deployment that started
/// after its caller went back to polling is dispatched there.
///
/// Each claimed hook is run in a task of its own, so one validation function's run
/// holds up neither the clock nor another deployment's hook.
pub fn dispatch_due_hooks<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
) {
    let claimed = with_state(&**engine, |state| {
        lifecycle_hook::claim_due_hooks(state, now_ms())
    })
    .unwrap_or_default();
    for dispatch in claimed {
        let engine = Arc::clone(engine);
        let callback = callback.clone();
        ::tokio::spawn(async move {
            run_hook(&engine, &callback, &dispatch).await;
        });
    }
}

/// Invoke one claimed hook's validation function and record what came of the run.
///
/// The result itself arrives through this service's own
/// `PutLifecycleEventHookExecutionStatus`, made by the function while it runs, so
/// what is recorded HERE is only the case where no result arrived at all: a
/// function that could not be invoked, that raised, or that returned without
/// reporting leaves its lifecycle event with no result, and a deployment never
/// reports success for a validation that did not happen.
async fn run_hook<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &EmbeddedEmulator,
    dispatch: &Dispatch,
) {
    let event = ::serde_json::json!({
        EVENT_DEPLOYMENT_ID: dispatch.deployment_id,
        EVENT_HOOK_EXECUTION_ID: dispatch.lifecycle_event_hook_execution_id,
    });
    let invoked = client(callback, invocation_region(&dispatch.function_name))
        .invoke()
        .function_name(&dispatch.function_name)
        .invocation_type(::aws_sdk_lambda::types::InvocationType::RequestResponse)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            event.to_string().into_bytes(),
        ))
        .send();
    match ::tokio::time::timeout(HOOK_RUN_BUDGET, invoked).await {
        Ok(Ok(output)) => {
            if let Some(function_error) = output.function_error() {
                ::tracing::warn!(
                    deployment = %dispatch.deployment_id,
                    lifecycle_event = %dispatch.lifecycle_event_name,
                    function = %dispatch.function_name,
                    function_error,
                    "the validation function raised, so this lifecycle event has no result"
                );
            }
        }
        Ok(Err(error)) => ::tracing::warn!(
            deployment = %dispatch.deployment_id,
            lifecycle_event = %dispatch.lifecycle_event_name,
            function = %dispatch.function_name,
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            "the validation function could not be invoked, so this lifecycle event has no result"
        ),
        Err(_elapsed) => ::tracing::warn!(
            deployment = %dispatch.deployment_id,
            lifecycle_event = %dispatch.lifecycle_event_name,
            function = %dispatch.function_name,
            "the validation function did not finish inside this service's budget"
        ),
    }
    settle_unreported(engine, dispatch);
}

/// Fail `dispatch`'s deployment when its hook execution reported nothing.
///
/// A function that DID report has already been recorded by the operation it
/// reported through, so this writes nothing for it: the check is on the stored
/// hook execution rather than on what the run answered.
fn settle_unreported<Eng: Send + Sync + 'static>(engine: &Arc<Eng>, dispatch: &Dispatch) {
    let now = now_ms();
    // Through the one entry every writer of this store uses, so the deployment is
    // first brought up to whatever the clock already owed it — a deployment that
    // had meanwhile reached an outcome keeps it.
    with_state(&**engine, |store| {
        let reported = store
            .deployments
            .get(&dispatch.deployment_id)
            .is_some_and(|deployment| {
                deployment.hook_executions.iter().any(|hook| {
                    hook.lifecycle_event_hook_execution_id.as_deref()
                        == Some(dispatch.lifecycle_event_hook_execution_id.as_str())
                        && hook.reported()
                })
            });
        if reported {
            return;
        }
        lifecycle_hook::fail_for_hook(
            store,
            &dispatch.deployment_id,
            lifecycle_hook::no_result_message(
                &dispatch.lifecycle_event_name,
                &dispatch.function_name,
            ),
            now,
        );
    });
}

/// AWS Lambda's client for this hop, over the in-process callback into this same
/// emulator: a validation function is one this emulator holds, so the invocation
/// never leaves the process.
///
/// The transport is in-process, so what a network client needs and this one cannot
/// have is disabled — retries, timeouts, stalled-stream protection and identity
/// caching each need a sleep implementation the callback transport is not given,
/// and the wait is bounded by [`HOOK_RUN_BUDGET`] above.
fn client(callback: &EmbeddedEmulator, region: &str) -> ::aws_sdk_lambda::Client {
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(callback.http_client())
        .credentials_provider(callback.aws_credentials_provider())
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
