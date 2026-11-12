// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a signaling channel's WebSocket data plane is reached at.
//!
//! A channel's peers do not exchange one request and one response: a master completes an
//! RFC 6455 opening handshake against the endpoint `GetSignalingChannelEndpoint` announces
//! for the `WSS` protocol, a viewer does the same, and the two then exchange FRAMES over
//! those sockets. That needs a socket of its own, which the core owns
//! ([`::emulator_server::Listeners::tcp_plaintext`], bound on the emulator's own interface
//! — loopback unless the operator says otherwise — with the core owning the port, the
//! connection caps and their lifetime), and this module holds the handle its lifetime
//! hangs off.
//!
//! ONE listener serves every channel of the store, as one AWS regional signaling endpoint
//! serves every channel of a region: the handshake names its channel in the
//! `X-Amz-ChannelARN` query parameter the SDKs put there, so nothing about the address has
//! to. It is opened on the first `WSS` endpoint resolution and kept for the life of the
//! store, so the announced address never moves under a client.
//!
//! The announced value is `wss://host:port`, and that scheme is served: the connection is
//! handed over plaintext and its FIRST byte says what the client opened with — a TLS
//! record goes to the core's own TLS server and everything after it is read decrypted,
//! anything else is read as it stands ([`::service_common::in_band_tls`]). So the address
//! serves both the `wss://` a client of AWS dials and the `ws://` a client that does not
//! verify a self-signed certificate dials, and this crate never touches a key or a
//! certificate.

/// The URI scheme the announced endpoint carries: "if you specify WSS as the protocol,
/// this API produces a secure websocket endpoint" (`GetSignalingChannelEndpoint`).
pub const WSS_SCHEME: &str = "wss";

/// The socket every channel's WebSocket data plane is served on, and the address it landed
/// on.
///
/// The store OWNS the listener: dropping it closes the port and severs every connection on
/// it, so it must outlive every endpoint handed out — which is the whole life of the
/// store.
pub struct SignalingEndpoint {
    /// `host:port`, the authority the announced `wss://` URL carries.
    authority: String,
    /// Held only to keep the port open; the core owns the socket and its lifetime.
    _listener: ::emulator_server::ResourceListener,
}

/// Written by hand rather than derived: a [`::emulator_server::ResourceListener`] is a
/// live socket and carries no `Debug`. What is diagnosable about the endpoint is the
/// address it announced.
impl std::fmt::Debug for SignalingEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SignalingEndpoint")
            .field("authority", &self.authority)
            .finish()
    }
}

impl SignalingEndpoint {
    /// The `host:port` this endpoint is dialled at.
    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }

    /// The whole value announced for the `WSS` protocol.
    #[must_use]
    pub fn announced_url(&self) -> String {
        format!("{WSS_SCHEME}://{}", self.authority)
    }
}

/// The `wss://host:port` every channel of this store's signaling data plane is reached at,
/// opening the socket on first use.
///
/// Idempotent: the first listener opened is kept, so the value is the same for every
/// channel and for as long as the store lives — a client can key a session on it. `None`
/// when this service was mounted on an engine with no listener surface or no store, or
/// when the core could not bind: the caller then answers with a modelled error rather than
/// announcing an address nothing serves.
pub async fn ensure_wss_endpoint<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<String> {
    let state = super::service_state(engine)?;
    // The guard is taken and dropped before the await below: a lock held across an await
    // would make this future non-`Send`.
    if let Some(announced) = state
        .lock()
        .wss_endpoint
        .as_ref()
        .map(SignalingEndpoint::announced_url)
    {
        return Some(announced);
    }

    let listeners = (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()?
        .listeners();
    let listener = listeners
        .tcp_plaintext(crate::service::wss::ChannelSocket::new(
            state.clone(),
            // The core's own TLS server, for a client that dialled `wss://`. A core that
            // cannot prepare one leaves this `None`, and such a client is then dropped at
            // its first byte rather than answered in the clear.
            listeners.tls_upgrader().ok(),
        ))
        .await
        .ok()?;
    let authority = ::service_common::data_plane_addressing::listener_authority(&listener);

    let mut store = state.lock();
    // Another call may have opened one while this was binding; keep the first, so an
    // announced endpoint never changes under a client. Dropping the loser closes its port.
    if let Some(held) = store.wss_endpoint.as_ref() {
        return Some(held.announced_url());
    }
    let endpoint = SignalingEndpoint {
        authority,
        _listener: listener,
    };
    let announced = endpoint.announced_url();
    store.wss_endpoint = Some(endpoint);
    ::tracing::info!(%announced, "opened the signaling channels' WebSocket data plane");
    Some(announced)
}
