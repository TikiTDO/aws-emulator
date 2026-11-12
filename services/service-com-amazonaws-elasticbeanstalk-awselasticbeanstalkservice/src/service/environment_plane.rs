// SPDX-License-Identifier: CC0-1.0

//! The ENVIRONMENT data plane: how a request a client sends to the address an
//! environment publishes is recognised, and how the application the environment
//! runs answers it.
//!
//! An environment publishes the address its traffic goes to —
//! `EnvironmentDescription$CNAME`, "the URL to the CNAME for this environment",
//! and the `EndpointURL` beside it — and a client READS that address and dials
//! it. That address is the emulator's own endpoint with the environment's CNAME
//! prefix in the path ([`crate::state::environment_cname`]), which names no
//! modelled operation of this service: the front door could resolve no operation
//! for such a request and answered a routing failure, for the one address this
//! service told the caller to use. So this plane claims it
//! ([`claims_parts`], read by `Service::can_handle`) and serves it
//! ([`EnvironmentPlane`], returned by `Service::override_operation`).
//!
//! What it answers is what the DEPLOYED APPLICATION answers: the version the
//! environment reports running is RUN, in process, and the response its own
//! request handler ends is the response — or, for a bundle that starts no server
//! of its own, the file of that bundle the request addresses
//! ([`crate::state::DeployedApplication`]). Nothing is invented: an environment
//! running no application this emulator could start answers as an address with
//! nothing behind it.
//!
//! Serving traffic WRITES NOTHING: no status moves, no quota is consumed and no
//! counter is bumped, so a client dialling the address leaves every later caller
//! seeing exactly what it saw.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};

use crate::state::{
    environment_serving_cname_prefix, service_state, GuestRequest, GuestResponse,
    DATA_PLANE_SEGMENT,
};

/// What this plane answers for an address no environment serves — the prefix of
/// an environment the store does not hold, or one the clock has terminated
/// ("Terminated: Environment is not running").
const NOT_SERVED_STATUS: u16 = 404;

/// What it answers when the environment IS there but runs no application that
/// answered: the status a proxy answers for a request it has no upstream for,
/// which is what an environment whose application this emulator could not start
/// is.
const NO_APPLICATION_STATUS: u16 = 502;

/// The media type an answer this plane renders itself carries.
const PLAIN_TEXT: &str = "text/plain; charset=utf-8";

/// Whether `path` addresses this plane at all: the reserved namespace segment,
/// then this service's own plane segment, then a Region and a CNAME prefix.
///
/// The SOCKET an environment's address is served on
/// ([`crate::state::environment_plane_endpoint`]) reads it before handing a
/// request to the front door, so that port serves environment traffic and
/// nothing else.
pub fn addresses_environment_plane(path: &str) -> bool {
    addressed(path).is_some()
}

/// The CNAME prefix a request addresses, its Region, and the request path under
/// them — or `None` when the request does not address this plane at all.
///
/// The addressing is the one [`crate::state::environment_plane_path`] writes:
/// `/_aws/elasticbeanstalk/<region>/<cname prefix>` and then the application's
/// own request path, so `/_aws/elasticbeanstalk/us-west-2/my-env` addresses `/`
/// of `my-env` in `us-west-2`.
fn addressed(path: &str) -> Option<(&str, &str, &str)> {
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != DATA_PLANE_SEGMENT {
        return None;
    }
    let (region, rest) = first_segment(rest)?;
    let (prefix, path) = first_segment(rest)?;
    (!region.is_empty() && !prefix.is_empty()).then_some((region, prefix, path))
}

/// Whether this plane serves the request `parts` addresses: its path addresses a
/// CNAME prefix some live environment of this store carries.
///
/// The claim is narrowed to an environment this service HOLDS, read from the one
/// shared store, rather than to a path shape alone, so one request keeps one
/// claimant.
pub fn claims_parts<Eng: Send + Sync + 'static>(engine: &Eng, parts: &http::request::Parts) -> bool {
    claims_path(engine, parts.uri.path())
}

/// [`claims_parts`] for the runtime-API request the serving seams are handed,
/// read by the same rule, so the claim and the serving cannot disagree about
/// which requests belong to this plane.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims_path(engine, ::service_common::request_path(req.uri()))
}

/// The one rule both of those read.
fn claims_path<Eng: Send + Sync + 'static>(engine: &Eng, path: &str) -> bool {
    let Some((region, prefix, _)) = addressed(path) else {
        return false;
    };
    let Some(state) = service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    // A termination the clock has already completed is what makes an address stop
    // being served, and it was due whether or not anybody dialled.
    store.materialize_due_transitions();
    environment_serving_cname_prefix(&store, region, prefix).is_some()
}

/// The data plane as the ROUTER serves it: the operation
/// `Service::override_operation` hands back for a request addressed to an
/// environment this service holds.
///
/// An environment's own traffic is no modelled operation — no routing key names
/// it — so it is mounted the way an emulated data plane is: claimed by
/// `Service::can_handle` ([`claims_parts`]) and served here. It declares NO auth
/// scheme, because a client of the address an environment publishes carries no
/// AWS credential and has none to sign with.
pub struct EnvironmentPlane<Eng> {
    /// The ONE shared engine this service's operations read and write, so a
    /// request served here sees exactly what the control plane persisted.
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> EnvironmentPlane<Eng> {
    /// Mount the plane over the shared engine.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }

    /// Whether this plane serves `req`, by the one rule the claim uses.
    pub fn claims(&self, req: &RtRequest) -> bool {
        claims_request(&*self.engine, req)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for EnvironmentPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { serve(&*self.engine, req).await })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // A client that sends traffic to the address an environment published
        // presents no AWS credential: this plane serves the request however it
        // arrived.
        Some(&[])
    }
}

/// Serve one request the environment's own address received.
async fn serve<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> RtResponse {
    let path = ::service_common::request_path(req.uri());
    let Some((region, prefix, application_path)) = addressed(path) else {
        return rendered(NOT_SERVED_STATUS, PLAIN_TEXT, Vec::new());
    };
    // The query string belongs to the request the application reads off
    // `req.url`, the way Node.js hands it over.
    let application_path = match req.uri().split_once('?') {
        Some((_, query)) => format!("{application_path}?{query}"),
        None => application_path.to_string(),
    };
    let request = GuestRequest {
        method: req.method().to_string(),
        path: application_path,
        headers: req
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect(),
    };
    // The deployment is read out of the store and the store released: the
    // application then RUNS with no guard held.
    let deployed = {
        let Some(state) = service_state(engine) else {
            return rendered(NO_APPLICATION_STATUS, PLAIN_TEXT, Vec::new());
        };
        let mut store = state.lock();
        store.materialize_due_transitions();
        let Some(environment) = environment_serving_cname_prefix(&store, region, prefix) else {
            return rendered(NOT_SERVED_STATUS, PLAIN_TEXT, Vec::new());
        };
        let environment_id = environment.environment_id.clone();
        store.deployed_application(&environment_id)
    };
    let Some(deployed) = deployed else {
        return rendered(NO_APPLICATION_STATUS, PLAIN_TEXT, Vec::new());
    };
    let answered = deployed.serve(&request).await;
    match answered {
        Some(response) => served(&response),
        // The application ran and answered nothing at this address, and its
        // bundle carries no file for it either.
        None => rendered(NOT_SERVED_STATUS, PLAIN_TEXT, Vec::new()),
    }
}

/// The response the application ended, as the client reads it: its status, every
/// header it set, and its body under the length it has.
fn served(response: &GuestResponse) -> RtResponse {
    let mut answer = RtResponse::new(status(response.status), SdkBody::from(response.body.clone()));
    for (name, value) in &response.headers {
        // Only a header a client can actually read: a name or a value HTTP
        // cannot carry is left out rather than ending the response.
        if ::http::HeaderName::from_bytes(name.as_bytes()).is_ok()
            && ::http::HeaderValue::from_str(value).is_ok()
        {
            answer.headers_mut().insert(name.clone(), value.clone());
        }
    }
    answer
        .headers_mut()
        .insert("content-length", response.body.len().to_string());
    answer
}

/// One answer this plane renders itself, for an address nothing is served at.
fn rendered(code: u16, media_type: &str, body: Vec<u8>) -> RtResponse {
    let mut answer = RtResponse::new(status(code), SdkBody::from(body));
    answer.headers_mut().insert("content-type", media_type.to_string());
    answer
}

/// A status this plane answers with. Every call site passes a constant or a status
/// the engine already held as a `u16`, so an out-of-range value falls back to the
/// one a proxy with no upstream answers.
fn status(code: u16) -> StatusCode {
    StatusCode::try_from(code)
        .unwrap_or_else(|_| StatusCode::try_from(NO_APPLICATION_STATUS).expect("a valid status"))
}
