// SPDX-License-Identifier: CC0-1.0

//! What one `Authorizer` of an api makes of a SIMULATED request — the decision
//! `TestInvokeAuthorizer` reports.
//!
//! "Simulate the execution of an Authorizer in your RestApi with headers, parameters, and an
//! incoming request body" (`TestInvokeAuthorizer`), so the authorizer is EXECUTED here, by
//! the same steps a request to a deployed stage executes it by:
//!
//! 1. the identity source is read off the simulated request
//!    ([`super::identity_source`], the one reader of that declaration): a source the request
//!    does not carry is a request that carried no identity, so the authorizer is not invoked
//!    at all and the call is refused as one that authorized nobody;
//! 2. a `COGNITO_USER_POOLS` authorizer VERIFIES the presented identity token against the
//!    user pools its `providerARNs` name ([`super::cognito_user_pools`]), and the claims of
//!    an accepted token are what it published;
//! 3. a `TOKEN` or `REQUEST` authorizer IS a Lambda function, so the decision is that
//!    function's return value, obtained by invoking it ([`super::custom_authorizer`]) with
//!    the event its own type documents — and the policy it returned decides the method the
//!    simulation addresses, exactly as a deployed stage's decision is taken.
//!
//! One module, one set of steps: a simulated execution and a request to a deployed stage
//! cannot answer one stored authorizer differently, because neither the identity reading,
//! the invocation, the output reading nor the policy decision is written twice.
//!
//! # What a simulation does NOT do
//!
//! It reaches no stage, so nothing HOLDS what the authorizer returned: a simulation names no
//! cache key ([`custom_authorizer::Pending::cache_key`]), which is what makes the authorizer
//! asked afresh for every simulated request and leaves every stage's own held results exactly
//! as they were. A caller testing an authorizer is testing the authorizer, not filling a
//! stage's cache — and a reporting call that primed one would change what a later request to
//! that stage is answered with.
//!
//! Nothing here is invented: every outcome is the authorizer's own answer about this request
//! or a refusal of it, each carrying the gateway response a client would have been answered
//! through.
//!
//! # A decision and a refusal are two different answers
//!
//! An authorizer that RAN and returned a well-formed authorizer output has DECIDED, whichever
//! way its policy decided: the document is the authorizer's result, not a failure of it. So
//! [`Simulation::decided`] records that a decision exists, separately from the refusal a
//! request the decision does not admit would have been answered through — the caller reports
//! the first as the authorizer having succeeded and the second as what a client would have
//! received.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::smithy_server_runtime::OperationContext;

use crate::state::{AuthorizerEntry, RestApiEntry};

use super::custom_authorizer::{self, Judged, Obtained};
use super::{
    access_denied, cognito_refusal, cognito_user_pools, identity_source, lambda_proxy,
    GatewayRefusal, MethodRequest, UNAUTHORIZED,
};

/// What simulating one authorizer produced.
///
/// Both halves are reported: WHETHER the call was authorized (the gateway response a refused
/// one would have been answered through, `None` for one the authorizer allowed) and what the
/// authorizer itself returned — which is reported for a refusal as well as for an allow,
/// because a developer testing an authorizer that DENIES the call is testing the very
/// document that denied it.
#[derive(Debug, Default)]
pub(crate) struct Simulation {
    /// The gateway response a client would have been answered through, or `None` when the
    /// authorizer authorized the call.
    pub refusal: Option<GatewayRefusal>,
    /// Whether the authorizer PRODUCED A DECISION about this request: its function ran and
    /// returned an authorizer output, or the identity token it evaluates was evaluated and
    /// accepted. `false` for every call that earned no decision at all — an identity source
    /// the request did not carry, an authorizer that could not be asked, one that did not
    /// answer or answered with something that is not an authorizer output, and a presented
    /// token the authorizer did not accept.
    pub decided: bool,
    /// The `principalId` the authorizer named, when its function returned one.
    pub principal_id: Option<String>,
    /// The policy document it returned, verbatim.
    pub policy: Option<String>,
    /// The `context` map it published, which is what an integration would read as
    /// `$context.authorizer.{key}`.
    pub context: BTreeMap<String, String>,
    /// The claims of an identity token a `COGNITO_USER_POOLS` authorizer ACCEPTED, or `None`
    /// for every authorizer that evaluates no such token — a `TOKEN` or `REQUEST` authorizer,
    /// whose verdict is a function's return value, publishes no claims at all rather than an
    /// empty set of them.
    pub claims: Option<BTreeMap<String, String>>,
    /// What the simulation did, one sentence per step, in the order it took them: for the
    /// execution log the caller reports. States the configuration or the authorizer's own
    /// signal, and never a caller credential.
    pub steps: Vec<String>,
}

/// Simulate `authorizer` of `api` for the request `call` describes, judging what it returns
/// against `method_arn` — the ARN of the method the simulated request addresses.
///
/// Made with NO store lock held: asking a `TOKEN` or `REQUEST` authorizer is a CALL to
/// another service of this emulator, so every record this needs is read off the store by the
/// caller and handed over owned.
pub(crate) async fn simulated<Eng: Send + Sync + 'static>(
    ctx: &OperationContext<'_>,
    engine: &Eng,
    api: &RestApiEntry,
    authorizer: &AuthorizerEntry,
    call: &MethodRequest,
    method_arn: &str,
) -> Simulation {
    let mut report = Simulation::default();
    let presented = match identity_source::presented(
        authorizer.identity_source.as_deref(),
        |name| header_of(call, name),
        |name| call.inputs.querystring.get(name).cloned(),
    ) {
        Ok(presented) => presented,
        Err(absent) => {
            report.steps.push(format!(
                "Authorizer identity source {source} is not carried by the simulated request, \
                 so the authorizer was not invoked",
                source = absent.source
            ));
            report.refusal = Some(UNAUTHORIZED);
            return report;
        }
    };

    if authorizer.r#type == crate::ops::create_authorizer::AUTHORIZER_TYPE_COGNITO_USER_POOLS {
        let Some(token) = presented.first else {
            report.steps.push(format!(
                "Authorizer {name} declares no identity source this request could be read \
                 for, so no identity token was presented to it",
                name = authorizer.name
            ));
            report.refusal = Some(UNAUTHORIZED);
            return report;
        };
        // A test invocation addresses no METHOD of the api, and `authorizationScopes` is a
        // method's own member ("used with a COGNITO_USER_POOLS authorizer to authorize the
        // method invocation", `Method$authorizationScopes`), so no scope bounds this
        // simulation — only what the authorizer itself declares does.
        return match cognito_user_pools::evaluate(
            engine,
            authorizer,
            &[],
            &token,
            SystemTime::now(),
        ) {
            cognito_user_pools::Decision::Authorized(authorized) => {
                report.steps.push(format!(
                    "Authorizer {name} accepted the presented identity token",
                    name = authorizer.name
                ));
                report.decided = true;
                report.claims = Some(authorized.claims);
                report
            }
            cognito_user_pools::Decision::Refused(refusal, reason) => {
                report.steps.push(format!(
                    "Authorizer {name} did not accept the presented identity token: {reason}",
                    name = authorizer.name
                ));
                report.refusal = Some(cognito_refusal(refusal));
                report
            }
        };
    }

    // A `CUSTOM` authorizer of either Lambda type. A `TOKEN` authorizer is handed nothing but
    // its token, so a request presenting none authorized nobody; a `REQUEST` authorizer is
    // handed the whole method request, so it is invoked for every simulation.
    let token = authorizer.r#type == crate::ops::create_authorizer::AUTHORIZER_TYPE_TOKEN;
    if token && presented.first.is_none() {
        report.steps.push(format!(
            "Authorizer {name} declares no identity source this request could be read for, so \
             no authorization token was presented to it",
            name = authorizer.name
        ));
        report.refusal = Some(UNAUTHORIZED);
        return report;
    }
    let event = if token {
        custom_authorizer::token_event(presented.first.as_deref().unwrap_or_default(), method_arn)
    } else {
        lambda_proxy::request_authorizer_event(call, method_arn)
    };
    report.steps.push(format!(
        "Starting authorizer: {id} of type {kind} for the simulated request",
        id = authorizer.id,
        kind = authorizer.r#type
    ));
    let pending = custom_authorizer::Pending {
        api_id: api.id.clone(),
        // The stage a simulated authorizer execution's own method ARN names
        // ([`crate::ops::test_invoke_authorizer`] owns that one spelling). Nothing is held
        // under it: `cache_key` is `None`, so this names the stage the simulated request
        // belongs to without touching what any stage of the api holds.
        stage: crate::ops::test_invoke_authorizer::TEST_INVOKE_ARN_STAGE.to_string(),
        region: api.arn_region().to_string(),
        authorizer_name: authorizer.name.clone(),
        authorizer_uri: authorizer.authorizer_uri.clone().unwrap_or_default(),
        authorizer_arn: super::authorizer_arn(api, &authorizer.id),
        event,
        trace_header: call.trace.authorizer_header.clone(),
        // A simulation holds nothing (see the module docs), so no result is looked for and
        // none is stored — which is also what makes the TTL below carry no duration.
        cache_key: None,
        ttl: Duration::ZERO,
    };
    match custom_authorizer::obtained(ctx, &pending).await {
        Obtained::Returned(held) => {
            // The function ran and returned an authorizer output, so a decision EXISTS: what
            // its policy decided about the method is the decision's content, reported beside
            // it rather than instead of it.
            report.decided = true;
            // WHAT it returned, in the log: the principal it named and every value it
            // PUBLISHED, each under the name it published it by. Those values are the
            // authorizer's own output about this request — what an integration would read as
            // `$context.authorizer.{key}` — so the developer who asked for this execution to
            // be simulated reads them back here. Nothing of the REQUEST's identity travels
            // into the log; only what the authorizer answered does.
            report.steps.push(format!(
                "Authorizer {name} returned principalId {principal}, publishing [{published}]",
                name = authorizer.name,
                principal = held.principal_id,
                published = held
                    .context
                    .iter()
                    .map(|(key, value)| format!("{key}={value}"))
                    .collect::<Vec<String>>()
                    .join(", "),
            ));
            report.principal_id = Some(held.principal_id.clone());
            report.policy = Some(held.policy_document.clone());
            report.context = held.context.clone();
            match custom_authorizer::decided(&held, method_arn) {
                Judged::Published(_) => report.steps.push(format!(
                    "Authorizer {name} returned a policy allowing \
                     {action} on {method_arn}",
                    name = authorizer.name,
                    action = super::super::resource_policy::INVOKE_ACTION
                )),
                Judged::Refused(sentence) => {
                    report.steps.push(format!(
                        "Authorizer {name} returned a policy that does not allow \
                         {action} on {method_arn}: {sentence}",
                        name = authorizer.name,
                        action = super::super::resource_policy::INVOKE_ACTION
                    ));
                    report.refusal = Some(access_denied(sentence.to_string()));
                }
            }
        }
        Obtained::Unobtained { refusal, reason } => {
            report
                .steps
                .push(format!("Execution failed due to configuration error: {reason}"));
            report.refusal = Some(refusal);
        }
    }
    report
}

/// The value the simulated request carries for the header `name`, compared
/// case-insensitively because a header name is case-insensitive on the wire and these
/// arrived as the keys of a map.
fn header_of(call: &MethodRequest, name: &str) -> Option<String> {
    call.inputs.header.iter().find_map(|(key, value)| {
        key.eq_ignore_ascii_case(name).then(|| value.clone())
    })
}
