// SPDX-License-Identifier: CC0-1.0

//! Serves the streaming session a `CreateStreamingURL` reserved, at the signed URL it
//! handed out for it.
//!
//! A streaming URL is a RESERVATION, not a formatted string: `CreateStreamingURL`
//! dedicates one [`crate::state::SessionRecord`] for the `(stack, fleet, user)` identity it
//! is asked about and signs a URL addressing that identity
//! ([`crate::state::streaming_url_path`]). Opening that URL is how a client reaches the
//! session, so this module serves the two addresses a client walks:
//!
//! * a `GET` of the signed URL, which resolves the reservation and REDIRECTS (HTTP 302) to
//!   the session's own address — the address carries the session's id and the
//!   `fleet/<fleet name>` reference of the fleet the session was allocated against, so what
//!   a client lands on names the fleet that serves it
//!   ([`crate::state::streaming_session_endpoint_path`]);
//! * a `GET` of that session address, which reports the stored session — the whole
//!   address identifies it, so one that carries no fleet reference, or one naming another
//!   fleet, is answered as no session at all.
//!
//! The model binds no operation to either path — a streaming session is not an API call,
//! it is what the API's URL points at — so both are claimed through
//! [`smithy_server_runtime::Service::override_operation`] rather than through the generated
//! routing table.
//!
//! The two differ in who may reach them, which is why each is served by its own route
//! object ([`ErasedOperation::auth_schemes`] is asked of the route, not of the request):
//! the signed URL is served only to a PRE-SIGNED request, whose signature and expiry
//! window the framework verifies before this route runs, so a session's URL is the only
//! way to resolve it; the session address the redirect lands on is reached by the browser
//! that followed the redirect, which signs nothing.
//!
//! Resolving a URL changes no session: the session was dedicated when the URL was issued,
//! and the only thing these routes materialise is the expiry the clock had already made
//! due — a session whose URL stopped verifying is expired whether or not anybody opened
//! it, so a resolution and a `DescribeSessions` agree about what is there.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

/// The method a streaming session is reached on.
const SESSION_METHOD: &str = "GET";

/// The media type the session's own address reports the session in.
const SESSION_MEDIA_TYPE: &str = "application/json";

/// The media type of the one answer these routes give that is not a session.
const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// What a URL that resolves to no session is answered with: a streaming URL is opened by a
/// browser rather than by an AWS SDK, so the answer is the status itself with a one-line
/// reason, and no API error envelope.
const NO_SESSION: &str = "no streaming session at this location\n";

/// The Smithy auth scheme a SigV4-signed request presents, which a streaming URL is
/// query-signed with.
const SIGV4_SCHEME: &str = "aws.auth#sigv4";

/// Which of the two addresses a route serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serves {
    /// The signed URL `CreateStreamingURL` handed out, resolved to the session's address.
    IssuedUrl,
    /// The session's own address, which the redirect lands on.
    SessionEndpoint,
}

/// The address one request addresses, read out of its path.
#[derive(Debug, PartialEq, Eq)]
enum Addressed {
    /// The signed URL of the session dedicated for this identity.
    IssuedUrl {
        stack_name: String,
        fleet_name: String,
        user_id: String,
    },
    /// One session's own address, by the id the service minted for it.
    SessionEndpoint { session_id: String },
}

/// Serves one of the two streaming-session addresses.
///
/// Owns the shared engine, as an operation does, because what it serves is the session
/// record `CreateStreamingURL` wrote: nothing is held beside that record, so a resolution
/// reports exactly the session the control plane reports.
pub struct StreamingSessionRoute<Eng> {
    engine: Arc<Eng>,
    serves: Serves,
}

impl<Eng: Send + Sync + 'static> StreamingSessionRoute<Eng> {
    /// Build the route from the shared engine, as the service's operations are built.
    pub fn new(engine: Arc<Eng>, serves: Serves) -> Self {
        Self { engine, serves }
    }

    /// Whether this route serves `req`, which is what the service's `can_handle` and
    /// `override_operation` seams both ask.
    pub fn claims(&self, method: &str, uri: &str) -> bool {
        addressed(method, uri).is_some_and(|addressed| match addressed {
            Addressed::IssuedUrl { .. } => self.serves == Serves::IssuedUrl,
            Addressed::SessionEndpoint { .. } => self.serves == Serves::SessionEndpoint,
        })
    }

    /// Serve one request to this address.
    fn serve(&self, req: &RtRequest) -> RtResponse {
        let Some(addressed) = addressed(req.method(), req.uri()) else {
            return answer(404, PLAIN_MEDIA_TYPE, NO_SESSION.as_bytes().to_vec());
        };
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return answer(500, PLAIN_MEDIA_TYPE, NO_SESSION.as_bytes().to_vec());
        };
        let mut state = state.lock();
        // The one thing this materialises is the expiry the clock had already made due,
        // which was owed whether or not anybody opened a URL.
        state.settle_session_states();

        match addressed {
            Addressed::IssuedUrl {
                stack_name,
                fleet_name,
                user_id,
            } => {
                let key = (stack_name, fleet_name, user_id);
                match state.sessions.get(&key) {
                    // A session nobody can stream from has no address to lead to, so an
                    // expired reservation is answered exactly as an absent one.
                    Some(session)
                        if session.state != crate::state::SESSION_STATE_EXPIRED =>
                    {
                        redirect(&crate::state::streaming_session_endpoint_path(
                            &session.id,
                            &session.fleet_name,
                        ))
                    }
                    _ => answer(404, PLAIN_MEDIA_TYPE, NO_SESSION.as_bytes().to_vec()),
                }
            }
            Addressed::SessionEndpoint { session_id } => {
                // A session's address is the WHOLE address the redirect handed out: its
                // id and the reference naming the fleet the session was allocated
                // against. An address that carries no reference, or one naming another
                // fleet, is not an address of this session, so nothing is served for it.
                let reference =
                    query_field(req.uri(), crate::state::STREAMING_SESSION_REFERENCE_FIELD);
                let session = state
                    .sessions
                    .values()
                    .find(|session| session.id == session_id)
                    .filter(|session| {
                        session.state != crate::state::SESSION_STATE_EXPIRED
                            && reference.as_deref() == Some(&fleet_reference(&session.fleet_name))
                    });
                match session {
                    Some(session) => answer(
                        200,
                        SESSION_MEDIA_TYPE,
                        reported_session(session).into_bytes(),
                    ),
                    None => answer(404, PLAIN_MEDIA_TYPE, NO_SESSION.as_bytes().to_vec()),
                }
            }
        }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for StreamingSessionRoute<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        _ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        let answer = self.serve(req);
        Box::pin(async move { answer })
    }

    /// Who may reach this address: the signed URL is served only to a request carrying the
    /// SigV4 query authentication the issuer put on it, which the framework verifies before
    /// this route runs; the session address the redirect lands on is fetched by a browser
    /// that signs nothing, so it is served however it arrives — like every other address a
    /// streaming client is handed.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        match self.serves {
            Serves::IssuedUrl => Some(&[SIGV4_SCHEME]),
            Serves::SessionEndpoint => Some(&[]),
        }
    }
}

/// The `fleet/<fleet name>` reference a session's address names the fleet it was allocated
/// against by, DECODED — the form [`crate::state::streaming_session_endpoint_path`] writes
/// percent-encoded into the URL.
fn fleet_reference(fleet_name: &str) -> String {
    format!(
        "{}/{fleet_name}",
        crate::state::FLEET_ARN_RESOURCE_TYPE
    )
}

/// The session the address serves, as the members `DescribeSessions` reports it by: read
/// from the stored record, so the two reads cannot disagree about one session.
fn reported_session(session: &crate::state::SessionRecord) -> String {
    ::serde_json::json!({
        "Id": session.id,
        "UserId": session.user_id,
        "StackName": session.stack_name,
        "FleetName": session.fleet_name,
        "State": session.state,
        "Reference": fleet_reference(&session.fleet_name),
    })
    .to_string()
}

/// The address `uri` addresses on a `GET`, or `None` when the request is not one of the
/// two these routes serve.
///
/// The path is read the way the signer wrote it — its segments percent-decoded — so a
/// stack, fleet or user id that had to be encoded in the URL names the same session here
/// as it does in the record. The signed URL is claimed only for a PRE-SIGNED request
/// ([`is_presigned`]): nothing which is not a URL this service signed is served from that
/// path at all.
fn addressed(method: &str, uri: &str) -> Option<Addressed> {
    if method != SESSION_METHOD {
        return None;
    }
    let segments = decoded_segments(uri);
    let [prefix, rest @ ..] = segments.as_slice() else {
        return None;
    };
    if prefix != crate::state::STREAMING_SESSION_PATH_PREFIX {
        return None;
    }
    match rest {
        [marker, session_id] if marker == crate::state::STREAMING_SESSION_ENDPOINT_SEGMENT => {
            Some(Addressed::SessionEndpoint {
                session_id: session_id.clone(),
            })
        }
        [stack_name, fleet_name, user_id] if is_presigned(uri) => Some(Addressed::IssuedUrl {
            stack_name: stack_name.clone(),
            fleet_name: fleet_name.clone(),
            user_id: user_id.clone(),
        }),
        _ => None,
    }
}

/// One answer of these routes: the bytes, their media type and their length.
///
/// The headers are set through the NON-panicking setter: `Headers::insert` aborts on a
/// value it cannot represent, and nothing a request can reach may panic. Every value here
/// is this module's own, so neither can fail; a header that somehow did is dropped rather
/// than taken down the process with.
fn answer(status: u16, media_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut response = RtResponse::new(session_status(status), SdkBody::from(body));
    let _ = response
        .headers_mut()
        .try_insert("content-type", media_type.to_string());
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// The redirect to a session's own address: `302 Found`, the status a client following a
/// streaming URL is sent on with.
///
/// `location` is a relative reference, so the session is reached on whatever address the
/// client reached this emulator on — it resolves the target against the URL it was handed,
/// and no host this emulator guesses at can be one the client cannot reach.
///
/// A `Location` that cannot be set is not answered as a redirect to nowhere: the address
/// is built from a stored session's own id and fleet name, so this cannot happen, and if
/// it did the caller is told there is nothing there rather than sent to no address.
fn redirect(location: &str) -> RtResponse {
    let mut response = answer(302, PLAIN_MEDIA_TYPE, Vec::new());
    if response
        .headers_mut()
        .try_insert("location", location.to_string())
        .is_err()
    {
        return answer(404, PLAIN_MEDIA_TYPE, NO_SESSION.as_bytes().to_vec());
    }
    response
}

/// A status these routes answer with. Every call site passes a constant, so the conversion
/// cannot fail.
fn session_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("a streaming session answers constant statuses")
}

/// Whether `uri` carries the SigV4 query authentication of a pre-signed request: the
/// credential scope and the signature [`smithy_server_runtime::UrlSigner`] puts on a URL
/// it signs.
///
/// Presence is all this reads, and all it has to: the framework's own request
/// authentication is what VERIFIES such a request before any route runs — the signature
/// over the method, the path and the query, and the `X-Amz-Expires` window the signer
/// wrote, past which the URL is refused. What this adds is that nothing which is not a
/// pre-signed request is served from that path at all.
fn is_presigned(uri: &str) -> bool {
    const PRESIGNED_FIELDS: [&str; 2] = ["X-Amz-Credential", "X-Amz-Signature"];
    PRESIGNED_FIELDS
        .iter()
        .all(|field| query_field(uri, field).is_some())
}

/// The decoded value of one query field of `uri`, or `None` when it carries none.
fn query_field(uri: &str, field: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_, query)| query).unwrap_or("");
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        name.eq_ignore_ascii_case(field)
            .then(|| smithy_server_runtime::percent_decode(value))
    })
}

/// The path segments of `uri`, percent-decoded, empty segments dropped — the same view of
/// a path the signer built the URL from.
fn decoded_segments(uri: &str) -> Vec<String> {
    uri.parse::<http::Uri>()
        .map(|parsed| {
            parsed
                .path()
                .split('/')
                .filter(|segment| !segment.is_empty())
                .map(smithy_server_runtime::percent_decode)
                .collect()
        })
        .unwrap_or_default()
}
