// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the CALLER perform the invocation it asks for — the decision every
//! route of this data plane takes before the endpoint the request addresses is resolved.
//!
//! An endpoint is invoked under some application's own credentials — a function's
//! execution role, an instance profile — and AWS decides a signed call against that
//! principal's identity-based policies at the front door, ahead of the service looking at
//! what the request named. So a caller whose policies grant none of this service's actions
//! is told it is not authorized whatever endpoint it addressed, which is what makes a
//! MISSING GRANT distinguishable from an endpoint the account does not host: the first is
//! `AccessDeniedException` with HTTP 403, the second this service's modelled
//! `ValidationError` with HTTP 400. The two outcomes are therefore ordered as well as
//! distinct — authorization first, endpoint existence second — so a refused call tells an
//! unauthorized caller nothing about what the account holds.
//!
//! `AccessDeniedException` is not a shape this model declares (`InvokeEndpoint` models
//! `InternalDependencyException`, `InternalFailure`, `ModelError`,
//! `ModelNotReadyException`, `ServiceUnavailable` and `ValidationError`, and the service's
//! `common_errors` add none of it — `smithy_query(query="operation",
//! operation="InvokeEndpoint")`), precisely because the refusal belongs to the
//! authorization layer rather than to the operation; it travels on each route's error
//! channel through [`crate::scaffolded::Scaffolded::access_denied`].
//!
//! # Where each piece lives
//!
//! Nothing of the decision is written here. The SageMaker CONTROL PLANE — the service that
//! owns every endpoint this data plane invokes, and whose crate this one already depends on
//! — holds the one gate for the `sagemaker` action namespace
//! (`service_com_amazonaws_sagemaker_sagemaker::service::authorization`): who the request's
//! access key id speaks for, the identity-based documents and permissions boundary the IAM
//! store holds for that principal, and the shared evaluation of those documents
//! (`service_common::iam_authorization`) together with the sentence a denial carries. Both
//! SageMaker services therefore answer the same question the same way, and a policy written
//! for the control plane's actions decides this data plane's too.
//!
//! What IS this module's own is the pair of things only this service knows: that the action
//! is `sagemaker:<OperationName>` for the operation the request reached, and that the
//! resource is the ENDPOINT it addressed ([`endpoint_resource`]).
//!
//! # The resource
//!
//! The resource is the endpoint ARN, as the control plane spells one
//! (`arn:aws:sagemaker:<region>:<account>:endpoint/<name>`, the form
//! `CreateEndpoint$EndpointArn`'s own pattern declares), in the Region the caller signed
//! for. It is built from the name the REQUEST carried — the `@httpLabel` in its URI —
//! never from a resolved record, so the decision is the same whether or not that name
//! names anything.

/// The ARN segment an endpoint is addressed by, from the modelled `EndpointArn` pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:endpoint/`): the same segment the
/// control plane's `CreateEndpoint` mints an endpoint's ARN with.
const ENDPOINT_ARN_KIND: &str = "endpoint";

/// The ARN of the endpoint named `endpoint_name` in `region`, built through the SageMaker
/// control plane's own ARN builder so this ARN and the one `DescribeEndpoint` reports are
/// one spelling.
#[must_use]
pub fn endpoint_resource(region: &str, endpoint_name: &str) -> String {
    ::service_com_amazonaws_sagemaker_sagemaker::state::resource_arn(
        region,
        ENDPOINT_ARN_KIND,
        endpoint_name,
    )
}

/// The ARN of the endpoint ONE request addresses: [`endpoint_resource`] in the Region the
/// caller signed for.
///
/// The Region comes from the caller's own SigV4 credential scope, read through the control
/// plane's `request_region` — the one reading of it the two SageMaker services share, so a
/// scope naming something that is not a Region cannot reshape the ARN's colon-delimited
/// fields.
#[must_use]
pub fn endpoint_addressed(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    endpoint_name: &str,
) -> String {
    endpoint_resource(
        &::service_com_amazonaws_sagemaker_sagemaker::state::request_region(ctx),
        endpoint_name,
    )
}

/// `Some(message)` when IAM does not allow `sagemaker:<operation>` on the endpoint the
/// request addressed — the sentence the 403 carries — and `None` when it does.
///
/// Call it FIRST in a route, before this service's log or the control plane's endpoint
/// store is touched: a request IAM refuses never reached the service on AWS, so it leaves
/// no invocation record behind either.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    endpoint_name: &str,
) -> Option<String> {
    let resource = endpoint_addressed(ctx, endpoint_name);
    ::service_com_amazonaws_sagemaker_sagemaker::service::authorization::authorize(
        engine, ctx, operation, &resource,
    )
    .err()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the resource a decision is taken against is the endpoint ARN of the name the
    /// request carried, in the Region it was signed for — the spelling the modelled
    /// `EndpointArn` pattern declares.
    #[test]
    fn the_resource_is_the_endpoint_arn_of_the_name_the_request_carried() {
        let account = ::engine::emulator_account_id();
        // (case, the Region signed for, the endpoint name the request addressed)
        let rows = [
            ("a deployed endpoint", "us-east-1", "live"),
            ("another Region", "eu-west-3", "live"),
            ("a name no endpoint carries", "us-east-1", "never-created"),
            ("a hyphenated name", "ap-southeast-4", "ep-with-hyphens-1"),
        ];
        for (case, region, endpoint) in rows {
            let actual = endpoint_resource(region, endpoint);
            assert_eq!(
                actual,
                format!("arn:aws:sagemaker:{region}:{account}:endpoint/{endpoint}"),
                "case {case:?}: expected the endpoint ARN, got {actual:?}"
            );
        }
    }

    /// RULE: the action asked about is the one this service's own operations are published
    /// under, so a statement scoped to SageMaker's actions decides every route of this data
    /// plane and nothing outside the service.
    #[test]
    fn a_grant_scoped_to_one_endpoint_covers_that_endpoints_invocations() {
        use ::service_common::iam_authorization::{self as iam, Decision};
        use ::service_com_amazonaws_sagemaker_sagemaker::service::authorization::ACTION_PREFIX;

        let granted = endpoint_resource("us-east-1", "live");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"{ACTION_PREFIX}:InvokeEndpoint","Resource":"{granted}"}}]}}"#
        );
        // (case, the operation the request reached, the endpoint ARN it addressed, allowed)
        let rows: &[(&str, &str, String, bool)] = &[
            (
                "the granted action on the granted endpoint",
                "InvokeEndpoint",
                granted.clone(),
                true,
            ),
            (
                "another route of the same data plane",
                "InvokeEndpointAsync",
                granted.clone(),
                false,
            ),
            (
                "the streamed route",
                "InvokeEndpointWithResponseStream",
                granted.clone(),
                false,
            ),
            (
                "the granted action on another endpoint",
                "InvokeEndpoint",
                endpoint_resource("us-east-1", "other"),
                false,
            ),
            (
                "the granted action in another Region",
                "InvokeEndpoint",
                endpoint_resource("eu-west-3", "live"),
                false,
            ),
        ];
        for (case, operation, resource, expected) in rows {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let said = iam::identity_decision(
                ::std::slice::from_ref(&document),
                &iam::Request::new(&action, resource, &[]),
            );
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *expected,
                "case {case:?}: expected allowed == {expected} for {action} on {resource}, got \
                 {actual} ({said:?})"
            );
        }
    }
}
