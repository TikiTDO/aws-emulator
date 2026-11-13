// SPDX-License-Identifier: CC0-1.0

//! The FIRST-CLASS SERVICE ACTION an `AWS_PROXY` route names: an `IntegrationSubtype`
//! integration, performed.
//!
//! "Specifies the AWS service action to invoke" (`Integration$IntegrationSubtype`,
//! "Supported only for HTTP API AWS_PROXY integrations"). Such a route has no function
//! behind it and no backend to forward to: the gateway itself calls that action, with the
//! parameters the integration's `RequestParameters` map out of the request
//! ([`parameters`]), under the role its `CredentialsArn` names ([`credentials`]) — which is
//! the whole point of the subtype, a client publishing to another service with nothing in
//! between.
//!
//! Three parts, each held in exactly one place:
//!
//! 1. **which action** — the subtype, matched against the actions this emulator can perform
//!    ([`Action`]). The call is made THROUGH THE EMULATOR this service runs within, over the
//!    in-process transport `OperationContext::callback` hands a handler, so the action is
//!    served by the very service this emulator mounts: nothing is invented, no connection is
//!    opened, no name is resolved, and no part of the request is a destination — the
//!    endpoint each client is addressed to is built from the api's own Region alone.
//! 2. **with which parameters** — [`parameters::resolve`], the one reading of
//!    `RequestParameters` as the mapping it is. A parameter the integration maps and this
//!    emulator cannot carry into the action is NOT dropped: the request is answered as an
//!    integration that produced no response, because a call missing what the integration
//!    mapped onto it is not the call the api was configured to make. A parameter the action
//!    REQUIRES whose mapping expression THIS REQUEST carries no value for is the request's
//!    own fault rather than the gateway's, so it is refused as a bad request and no action is
//!    performed for it.
//! 3. **as whom** — [`credentials::integration_identity`], a session of the integration's
//!    role. The service serving the action then authorizes it against the policies in force
//!    on that role, exactly as it authorizes any other signed request, so a role that does
//!    not hold the action's permission performs nothing and the route answers that service's
//!    own refusal rather than reporting a publication nobody made.
//!
//! What the CALLER is answered: the action's own answer, as the document that service
//! returned AND IN THE FORM THE INTERFACE THE ACTION WAS PERFORMED OVER STATES IT IN
//! ([`Performed`]) — a `200` carrying it, since the action is what the route exists to perform
//! and its answer is the only thing the gateway has to report. Which form that is belongs to
//! each action's own module, from what is recorded for its subtype, because it is a fact
//! about the api API Gateway performs the action over and not about this gateway: for
//! [`eventbridge`] and [`stepfunctions`] it is the JSON document that service's protocol
//! answers with, and for
//! [`sqs`] the `{Action}Response` envelope the queue api's query interface answers with. An
//! action the service serving it
//! ANSWERED a refusal for is that refusal, RELAYED: that service's own status, the error
//! shape it named and the text it stated, which is what tells a client a missing permission
//! from a malformed parameter. An action that reached no service at all, or whose parameters
//! do not hold together, is the "integration produced no response" class [`super::dispatch`]
//! owns; one the REQUEST carried no required parameter for is that module's bad-request
//! class; and one that had not answered within the integration's own `TimeoutInMillis` is its
//! timeout class — the reason is logged, never told to the caller, which is what API Gateway
//! does.
//!
//! The subtypes performed here are the ones whose service this emulator mounts and whose
//! parameters are the action's own: [`eventbridge`], [`sqs`] and [`stepfunctions`]. Any other
//! subtype is reported as an integration this emulator cannot perform, which is the answer it
//! already had — never a success for a call nothing made.
//!
//! ## The Region an action is called in
//!
//! One parameter belongs to no action's own request shape and may be mapped onto EVERY
//! first-class subtype ([`REGION_PARAMETER`]): the Region the gateway sends the call to. An
//! integration that maps none calls the action in the api's OWN Region, which is the Region
//! the request was addressed in, so the mapping only ever moves a call this emulator would
//! otherwise have made at home.

use std::time::Duration;

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use serde_json::{Map, Value};
use smithy_server_runtime::OperationContext;

use super::super::data_plane::UNPLACEABLE_STATUS;
use super::dispatch::Dispatch;
use crate::state::Integration;

/// The provider name the credential of an integration's role is reported under by the SDK's
/// own configuration. It names where the credential came from and carries no secret.
const INTEGRATION_ROLE_PROVIDER: &str = "ApiGatewayIntegrationCredentials";

/// The media types the action's answer is carried to the caller as — one per wire form an
/// api this emulator performs an action over states an answer in: the document an AWS JSON
/// protocol answers with, and the envelope a QUERY interface answers with — and the status a
/// performed action answers with.
const JSON_CONTENT_TYPE: &str = "application/json";
const XML_CONTENT_TYPE: &str = "text/xml";
const PERFORMED_STATUS: u16 = 200;

/// The members the gateway relays a refused action's answer in: the error SHAPE the service
/// named, under the discriminator a JSON error envelope names a shape in, and its DETAIL
/// TEXT, under the member this endpoint reports a relayed service error's text in.
const SHAPE_MEMBER: &str = "__type";
const DETAIL_MEMBER: &str = "Message";

/// The parameter every first-class subtype may map and no action's own request shape declares:
/// the Region API Gateway sends the call to. An integration mapping none has the action called
/// in the api's own Region (see the module docs).
const REGION_PARAMETER: &str = "Region";

/// Build the client ONE in-process hop is made with: the AWS SDK for the service the action
/// belongs to, driven over the emulator's own transport.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream — each of those would need a sleep implementation this client is not given
/// — and the deadline is the integration's own `TimeoutInMillis`, applied by the caller
/// below. The endpoint URL is addressing for that transport and not a host this process
/// connects to, and it is built from the Region alone.
///
/// The credential is the integration's role session when one could be minted, and otherwise
/// the one the callback hands this handler ([`credentials`]). Neither is reported or logged,
/// and the CALLER's own credential never travels to the action.
macro_rules! action_client {
    ($sdk:ident, $callback:expr, $endpoint:expr, $region:expr, $identity:expr) => {{
        let configured = ::$sdk::Config::builder()
            .behavior_version(::$sdk::config::BehaviorVersion::latest())
            .http_client($callback.http_client())
            .region(::$sdk::config::Region::new($region.to_string()))
            .endpoint_url(format!("https://{}.{}.amazonaws.com", $endpoint, $region))
            .retry_config(::$sdk::config::retry::RetryConfig::disabled())
            .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
            .identity_cache(::$sdk::config::IdentityCache::no_cache())
            .stalled_stream_protection(
                ::$sdk::config::StalledStreamProtectionConfig::disabled(),
            );
        let configured = match $identity {
            Some(identity) => configured.credentials_provider(::$sdk::config::Credentials::new(
                identity.access_key_id.clone(),
                identity.secret_access_key.clone(),
                identity.session_token.clone(),
                None,
                $crate::service::invoke::service_action::INTEGRATION_ROLE_PROVIDER,
            )),
            None => configured.credentials_provider($callback.aws_credentials_provider()),
        };
        ::$sdk::Client::from_conf(configured.build())
    }};
}

pub mod credentials;
mod eventbridge;
pub mod parameters;
mod sqs;
mod stepfunctions;

use credentials::SigningIdentity;
use parameters::Mapped;

/// Why the action produced no answer of its own to report.
#[derive(Debug, Clone)]
pub enum Unperformed {
    /// The call was made and had not answered when the integration's timeout elapsed.
    TimedOut(Duration),
    /// The service serving the action ANSWERED, refusing the call: a permission the
    /// integration's role does not hold, a parameter that service will not take, a resource
    /// it does not hold. The call WAS served, so what it answered is the answer the gateway
    /// has to report, and it is relayed as that service stated it.
    Answered(ServiceAnswer),
    /// The action was not performed at all: a subtype this emulator cannot perform, a
    /// mapping the action cannot be called with, or a call that reached no such service.
    NotPerformed(String),
    /// The REQUEST carried no value for the mapping expression the integration maps onto a
    /// parameter the action REQUIRES, so there was nothing to call the action with. The
    /// mapping is the api's own configuration and the request is what failed to satisfy it,
    /// which is why this is a different class from an integration this gateway cannot
    /// perform at all.
    Unmappable(String),
}

impl Unperformed {
    /// One sentence for the emulator's own log, never for the caller.
    pub fn reason(&self) -> String {
        match self {
            Unperformed::TimedOut(after) => format!(
                "the action had not answered {} ms after it was called",
                after.as_millis()
            ),
            Unperformed::Answered(answered) => answered.reason(),
            Unperformed::NotPerformed(detail) | Unperformed::Unmappable(detail) => detail.clone(),
        }
    }
}

/// The answer the service serving the action stated, as the gateway relays it to the caller.
///
/// A first-class integration is the caller reaching that service THROUGH the api, so the
/// answer the caller gets for a call the service refused is that service's own: its status,
/// the error SHAPE it named and the detail text it stated — which is what tells a client
/// which refusal its integration earned, a missing permission from a malformed parameter.
/// The gateway relays those in its own error document ([`SHAPE_MEMBER`], [`DETAIL_MEMBER`]);
/// nothing of the text is this gateway's wording, and nothing is added to it.
#[derive(Debug, Clone)]
pub struct ServiceAnswer {
    /// The action that was refused, for the emulator's own log.
    action: String,
    /// The status that service answered with.
    status: u16,
    /// The error shape it named, as the relayed document's discriminator.
    shape: Option<String>,
    /// The detail text it stated.
    detail: Option<String>,
}

impl ServiceAnswer {
    /// One sentence about that answer for the emulator's own log.
    fn reason(&self) -> String {
        format!(
            "{} was refused with {} {}: {}",
            self.action,
            self.status,
            self.shape.as_deref().unwrap_or("an unnamed error"),
            self.detail.as_deref().unwrap_or_default()
        )
    }

    /// That answer, as the response the caller receives.
    pub fn relayed(&self) -> RtResponse {
        let mut document = Map::new();
        if let Some(shape) = &self.shape {
            document.insert(SHAPE_MEMBER.to_string(), Value::String(shape.clone()));
        }
        if let Some(detail) = &self.detail {
            document.insert(DETAIL_MEMBER.to_string(), Value::String(detail.clone()));
        }
        let body = Value::Object(document).to_string().into_bytes();
        let length = body.len();
        let mut response = RtResponse::new(
            StatusCode::try_from(self.status).unwrap_or(
                StatusCode::try_from(UNPLACEABLE_STATUS).expect("500 is a valid status code"),
            ),
            SdkBody::from(body),
        );
        let headers = response.headers_mut();
        let _ = headers.try_insert("content-type", JSON_CONTENT_TYPE);
        let _ = headers.try_insert("content-length", length.to_string());
        response
    }
}

/// The answer a performed action produced, as the api it was performed over states one on
/// the wire.
///
/// A first-class integration is the caller reaching that service THROUGH the api, so what the
/// caller receives is the document the action answered in the form the interface it was
/// performed over states one in — the document an AWS JSON protocol answers with, or the
/// `{Action}Response` envelope a QUERY interface answers with — and not a re-encoding of it
/// into a shape or media type of this gateway's own: a client of such a route parses the
/// answer it would have been given calling that action itself, which is where it reads the id
/// of what it just did. Which of the two an action is relayed in is that action's own module's
/// to state, from what is recorded for its subtype.
#[derive(Debug, Clone)]
pub struct Performed {
    /// The document, exactly as that api states one.
    body: Vec<u8>,
    /// The media type it carries it under.
    content_type: &'static str,
}

impl Performed {
    /// The answer of an action performed over an api that states one as a JSON document.
    fn json(document: &Value) -> Self {
        Self {
            body: document.to_string().into_bytes(),
            content_type: JSON_CONTENT_TYPE,
        }
    }

    /// The answer of an action performed over a QUERY interface, which states one as the
    /// XML `{Action}Response` envelope that interface wraps an action's result in.
    fn query_envelope(envelope: String) -> Self {
        Self {
            body: envelope.into_bytes(),
            content_type: XML_CONTENT_TYPE,
        }
    }

    /// That answer, as the response the caller receives.
    fn response(self) -> RtResponse {
        let length = self.body.len();
        let mut response = RtResponse::new(
            StatusCode::try_from(PERFORMED_STATUS).unwrap_or(
                StatusCode::try_from(UNPLACEABLE_STATUS).expect("500 is a valid status code"),
            ),
            SdkBody::from(self.body),
        );
        let headers = response.headers_mut();
        let _ = headers.try_insert("content-type", self.content_type);
        let _ = headers.try_insert("content-length", length.to_string());
        response
    }
}

/// Perform the service action `subtype` names, and answer the caller with what it returned.
pub async fn perform<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    dispatch: &Dispatch,
    integration: &Integration,
    subtype: &str,
    timeout: Duration,
) -> Result<RtResponse, Unperformed> {
    let Some(action) = Action::named(subtype) else {
        return Err(Unperformed::NotPerformed(format!(
            "this emulator performs no integration of the subtype {subtype}"
        )));
    };
    // Reading the request's own parts is CPU-bound and holds no lock: yielding here gives a
    // cancellation or shutdown signal somewhere to land before that work starts, and before
    // the deadline below is armed.
    ::tokio::task::yield_now().await;
    let mapped = parameters::resolve(
        &integration.request_parameters,
        &dispatch.request,
        &dispatch.invocation,
    );
    let carried: Vec<&str> = action
        .parameters()
        .iter()
        .copied()
        .chain(::std::iter::once(REGION_PARAMETER))
        .collect();
    if let Some(beyond) = mapped.beyond(&carried) {
        return Err(Unperformed::NotPerformed(format!(
            "the integration maps the request parameter {beyond}, which this emulator does \
             not carry into {subtype}"
        )));
    }
    if let Some(unresolved) = mapped.unresolved_required(action.required_parameters()) {
        return Err(Unperformed::Unmappable(format!(
            "this request carries no value for the {unresolved} the integration maps onto \
             {subtype}, which that action requires"
        )));
    }
    // The role's session is resolved BEFORE the call and with no store of this service's
    // locked, because minting one takes the token service's and IAM's locks.
    let identity = credentials::integration_identity(
        engine,
        ctx,
        integration.credentials_arn.as_deref(),
    );
    // The Region the action is called in: the one the integration maps, and otherwise the
    // api's own.
    let region = mapped
        .get(REGION_PARAMETER)
        .map_or_else(|| dispatch.request.region.clone(), str::to_string);
    let call = action.invoke(ctx, &region, identity.as_ref(), &mapped);
    let answered = match ::tokio::time::timeout(timeout, call).await {
        Ok(answered) => answered,
        Err(_elapsed) => return Err(Unperformed::TimedOut(timeout)),
    };
    answered.map(Performed::response)
}

/// The service actions this emulator performs for a first-class subtype integration.
#[derive(Debug, Clone, Copy)]
enum Action {
    /// `EventBridge-PutEvents`: publish the mapped event onto the bus it names.
    EventBridgePutEvents,
    /// `SQS-SendMessage`: send the mapped message to the queue it names.
    SqsSendMessage,
    /// `StepFunctions-StartExecution`: start an execution of the state machine it names.
    StepFunctionsStartExecution,
    /// `StepFunctions-StartSyncExecution`: start one synchronously and answer what it came
    /// to.
    StepFunctionsStartSyncExecution,
}

impl Action {
    /// The action `subtype` names, or `None` for a subtype this emulator cannot perform.
    ///
    /// Matched exactly: an `IntegrationSubtype` is the action's own name, so a value spelled
    /// any other way names no action.
    fn named(subtype: &str) -> Option<Self> {
        match subtype {
            eventbridge::PUT_EVENTS_SUBTYPE => Some(Self::EventBridgePutEvents),
            sqs::SEND_MESSAGE_SUBTYPE => Some(Self::SqsSendMessage),
            stepfunctions::START_EXECUTION_SUBTYPE => Some(Self::StepFunctionsStartExecution),
            stepfunctions::START_SYNC_EXECUTION_SUBTYPE => {
                Some(Self::StepFunctionsStartSyncExecution)
            }
            _ => None,
        }
    }

    /// Every parameter of this action an integration may map, which is the set its own
    /// request shape declares.
    fn parameters(self) -> &'static [&'static str] {
        match self {
            Self::EventBridgePutEvents => eventbridge::PUT_EVENTS_PARAMETERS,
            Self::SqsSendMessage => sqs::SEND_MESSAGE_PARAMETERS,
            Self::StepFunctionsStartExecution => stepfunctions::START_EXECUTION_PARAMETERS,
            Self::StepFunctionsStartSyncExecution => {
                stepfunctions::START_SYNC_EXECUTION_PARAMETERS
            }
        }
    }

    /// Every parameter of this action a call must carry a value for, which is the set its own
    /// request shape marks REQUIRED.
    fn required_parameters(self) -> &'static [&'static str] {
        match self {
            Self::EventBridgePutEvents => eventbridge::PUT_EVENTS_REQUIRED_PARAMETERS,
            Self::SqsSendMessage => sqs::SEND_MESSAGE_REQUIRED_PARAMETERS,
            Self::StepFunctionsStartExecution => {
                stepfunctions::START_EXECUTION_REQUIRED_PARAMETERS
            }
            Self::StepFunctionsStartSyncExecution => {
                stepfunctions::START_SYNC_EXECUTION_REQUIRED_PARAMETERS
            }
        }
    }

    /// Call the action, and answer the document the service returned, in the form the
    /// interface it was performed over states one in.
    async fn invoke(
        self,
        ctx: &OperationContext<'_>,
        region: &str,
        identity: Option<&SigningIdentity>,
        mapped: &Mapped,
    ) -> Result<Performed, Unperformed> {
        match self {
            Self::EventBridgePutEvents => {
                eventbridge::put_events(ctx, region, identity, mapped).await
            }
            Self::SqsSendMessage => sqs::send_message(ctx, region, identity, mapped).await,
            Self::StepFunctionsStartExecution => {
                stepfunctions::start_execution(ctx, region, identity, mapped).await
            }
            Self::StepFunctionsStartSyncExecution => {
                stepfunctions::start_sync_execution(ctx, region, identity, mapped).await
            }
        }
    }
}

/// Why one call produced no answer of its own, told apart the way every other in-process hop
/// of this service tells it apart: a call that reached no such service in this emulator is
/// reported as that ([`::service_common::in_process_refusal`]) rather than as a backend
/// answer — mapping it as one would report a status for a call nothing served — while a
/// refusal the SERVICE itself stated is that refusal, relayed to the caller
/// ([`ServiceAnswer`]).
fn unperformed<Error>(
    action: &str,
    error: &::aws_smithy_runtime_api::client::result::SdkError<
        Error,
        ::aws_smithy_runtime_api::http::Response,
    >,
) -> Unperformed
where
    Error: ::aws_smithy_types::error::metadata::ProvideErrorMetadata,
{
    use ::aws_smithy_types::error::metadata::ProvideErrorMetadata as _;

    let Some(answered) = error.raw_response() else {
        return Unperformed::NotPerformed(format!(
            "{action} produced no answer: {}",
            error.code().unwrap_or("an unnamed error")
        ));
    };
    let body = answered
        .body()
        .bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    if ::service_common::in_process_refusal::is_transport_refusal(&body) {
        return Unperformed::NotPerformed(format!(
            "the {action} call reached no such service in this emulator, so no answer exists \
             to report"
        ));
    }
    Unperformed::Answered(ServiceAnswer {
        action: action.to_string(),
        status: answered.status().as_u16(),
        // The error SHAPE that service named and the TEXT it stated, read off the answer it
        // sent: `ProvideErrorMetadata` is the one reading of either, so a relayed refusal
        // carries what the service itself said and nothing of this gateway's.
        shape: error.code().map(str::to_string),
        detail: error.message().map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a subtype names the action it spells, and nothing else — and every action
    /// names the parameters its own request shape declares.
    #[test]
    fn a_subtype_names_the_action_it_spells() {
        // Rows differ along the SPELLING: each performed subtype, a case-folded spelling of
        // one, a subtype of a service this emulator does not perform the action of, and an
        // empty value.
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "the EventBridge publication",
                eventbridge::PUT_EVENTS_SUBTYPE,
                Some("Detail"),
            ),
            ("the SQS send", sqs::SEND_MESSAGE_SUBTYPE, Some("MessageBody")),
            (
                "the Step Functions start",
                stepfunctions::START_EXECUTION_SUBTYPE,
                Some("StateMachineArn"),
            ),
            (
                "the synchronous Step Functions start",
                stepfunctions::START_SYNC_EXECUTION_SUBTYPE,
                Some("StateMachineArn"),
            ),
            ("a case-folded spelling", "eventbridge-putevents", None),
            ("an action this emulator does not perform", "SQS-PurgeQueue", None),
            ("nothing at all", "", None),
        ];
        for (case, subtype, expected) in rows {
            let action = Action::named(subtype);
            let carries = action.map(|action| action.parameters());
            assert_eq!(
                carries.is_some(),
                expected.is_some(),
                "case {case:?}: expected an action={}, got {action:?}",
                expected.is_some()
            );
            if let (Some(parameters), Some(expected)) = (carries, expected) {
                assert!(
                    parameters.contains(expected),
                    "case {case:?}: expected {expected:?} among {parameters:?}"
                );
            }
        }
    }
}
