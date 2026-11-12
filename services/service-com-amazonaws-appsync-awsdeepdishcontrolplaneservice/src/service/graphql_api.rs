// SPDX-License-Identifier: CC0-1.0

//! A GraphQL API's REAL-TIME plane: what ANSWERS at the endpoint `CreateGraphqlApi`
//! publishes as `GraphqlApi$uris`'s `REALTIME` entry.
//!
//! That value is not a label. "The URIs." (`GraphqlApi$uris`) are the addresses the API is
//! served at, and the real-time one is where a subscriber completes an RFC 6455 opening
//! handshake and then exchanges frames: `connection_init`/`connection_ack`, a `start` that
//! begins a subscription on a field of the API's schema, the `stop` that ends one, and the
//! `data` frames a subscription receives. So it is a SOCKET this emulator answers on
//! ([`realtime`]), opened when the API is created and closed when it is deleted, and the
//! `host:port` it landed on is what the API publishes — because the whole purpose of the
//! value is that a client dials it, and no client on this machine resolves an AWS hostname
//! (`crate::state::api_dns`).
//!
//! It cannot be served through the emulator's front door for the same reason an Event API's
//! endpoints cannot ([`super::event_api`] states it in full): the seams a service is given
//! take one buffered request and return one buffered response, so nothing there can switch
//! a connection to another protocol and carry frames over it. The core lends a service a
//! socket for exactly this — `engine.listeners().tcp_plaintext(handler)` — binds it on the
//! operator-chosen interface, assigns the port, caps the connections and severs them when
//! the handle is dropped. The socket is handed over PLAINTEXT and the first byte says what
//! the client opened with ([`super::event_api::negotiated`]), so one announced address
//! serves both the `wss://` a client of AWS dials and the `ws://` a client that does not
//! verify a self-signed certificate dials, and this service never touches a key or a
//! certificate.
//!
//! What a connection on it OBSERVES about the API is this service's own records, read under
//! the one shared store lock: the API, its authentication configuration, its API keys and
//! the subscription fields its schema declares.

use ::std::sync::Arc;

use crate::ops::types::AdditionalAuthenticationProvider;
use crate::state::ComAmazonawsAppsyncAwsdeepdishcontrolplaneserviceState as ServiceStore;

pub mod metrics;
pub mod realtime;

/// Everything one of these sockets needs to answer from: the service's own store, the API
/// the socket belongs to (the socket itself says which, so no host has to name it), and
/// where that API's metrics go.
#[derive(Clone)]
pub struct PlaneContext {
    /// The shared store, so every connection reads the records the control plane writes.
    pub state: ::engine::ServiceState<ServiceStore>,
    /// The `apiId` this socket serves.
    pub api_id: String,
    /// The publisher of this API's `AWS/AppSync` series.
    pub metrics: metrics::GraphqlApiMetrics,
}

// By hand, because a store handle has no rendering of its own: what is diagnosable about a
// socket's context is which API it serves and where its metrics go.
impl ::std::fmt::Debug for PlaneContext {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PlaneContext")
            .field("api_id", &self.api_id)
            .field("metrics", &self.metrics)
            .finish()
    }
}

/// Open the socket `api_id`'s real-time endpoint is served on and remember it, answering
/// the `host:port` it landed on — the authority that API's `REALTIME` endpoint publishes.
///
/// The core owns the socket, the interface, the port and the connection cap; this hands it
/// a handler and stores the returned handle in the service's own state, which is what keeps
/// the port open for as long as the API exists. `Err` when this service was mounted on an
/// engine with no listener surface, or when the core could not bind: the caller then answers
/// that rather than publishing an address nothing serves.
pub async fn open_for_api<Eng: Send + Sync + 'static>(
    engine: &Eng,
    api_id: &str,
) -> Result<String, String> {
    let shared = (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?;
    let state = crate::state::service_state(engine)
        .ok_or_else(|| "this emulator's engine does not carry the appsync store".to_string())?;
    let listeners = shared.listeners();
    let context = PlaneContext {
        state: state.clone(),
        api_id: api_id.to_string(),
        metrics: metrics::GraphqlApiMetrics::new(
            super::event_api::metrics::cloudwatch_state(engine),
            api_id,
        ),
    };
    // The core's own TLS server, for a client that dialled `wss://`. A core that cannot
    // prepare it leaves this `None`, and such a client is then dropped at the first byte
    // rather than answered in the clear.
    let upgrader = listeners.tls_upgrader().ok();
    let listener = listeners
        .tcp_plaintext(realtime::RealtimeSocket::new(context, upgrader))
        .await
        .map_err(|error| format!("the API's real-time endpoint could not be opened: {error}"))?;
    let authority = state
        .lock()
        .graphql_data_plane
        .remember_listener(api_id, Arc::new(listener));
    ::tracing::info!(
        %api_id,
        realtime = %authority,
        "opened a GraphQL API's real-time socket"
    );
    Ok(authority)
}

/// Whether the material presented authorizes a real-time connection to `api_id`, by the
/// rule AppSync applies to a call on a GraphQL API:
///
/// * material has to be PRESENTED: AppSync admits no anonymous connection, so a handshake
///   carrying none at all is unauthorized whatever the API configured;
/// * the material's provider has to be one the API set up — its `authenticationType`
///   ("The authentication type", the one every call may use) or one of its
///   `additionalAuthenticationProviders` ("A list of additional authentication providers
///   for the GraphqlApi API"). So an API configured for `API_KEY` alone refuses a token,
///   and one configured without `API_KEY` refuses a key;
/// * an API KEY has to name a key of this API that has not expired. This is the one
///   provider this service can decide itself, because it is the service that MINTED the key
///   (`CreateApiKey`), and an expired key authorizes nothing;
/// * any other material is of a provider this service does not hold the secret of — a
///   signature, a Cognito or OIDC token, a Lambda authorizer's answer. This emulator
///   verifies request signatures at its front door and a service never checks one itself,
///   and this socket is not that door, so such material is taken as PRESENTED once its
///   provider is one the API configured.
///
/// The same reading the Event API's plane applies to its own `authProviders`
/// ([`super::event_api::authorized`]), over the members a `GraphqlApi` carries them in.
pub fn authorized(
    store: &ServiceStore,
    api_id: &str,
    auth: &super::event_api::PresentedAuth,
) -> bool {
    if !auth.is_present() {
        return false;
    }
    let Some(api) = store.graphql_apis.get(api_id) else {
        return false;
    };
    let configured = |auth_type: &str| {
        api.authentication_type == auth_type
            || api
                .additional_authentication_providers
                .iter()
                .any(|provider| provider_type(provider) == Some(auth_type))
    };
    if let Some(presented) = auth.api_key.as_deref() {
        return configured(API_KEY_AUTH_TYPE)
            && store
                .api_keys_of(api_id)
                .and_then(|keys| keys.get(presented))
                .is_some_and(|key| key.expires > crate::state::unix_seconds());
    }
    // Material of every provider but `API_KEY`: authorized where the API configured any of
    // those providers. A provider that NAMES no authentication type sets none up, so it
    // configures nothing — the same reading [`provider_type`] gives the key branch above.
    api.authentication_type != API_KEY_AUTH_TYPE
        || api
            .additional_authentication_providers
            .iter()
            .any(|provider| matches!(provider_type(provider), Some(named) if named != API_KEY_AUTH_TYPE))
}

/// The authentication type one additional provider sets up, or `None` for a provider that
/// names none.
fn provider_type(provider: &AdditionalAuthenticationProvider) -> Option<&str> {
    provider.authentication_type.as_deref()
}

/// The authorization TYPE an API key belongs to, as `AuthenticationType` spells it.
const API_KEY_AUTH_TYPE: &str = "API_KEY";
