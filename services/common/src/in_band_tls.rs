// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of a data-plane connection whose ENCRYPTION is negotiated IN BAND, for
//! every service that opens a core-owned plaintext listener and has to serve both the
//! `https://`/`wss://` a client of AWS dials and the `http://`/`ws://` a client that does
//! not verify a self-signed certificate dials.
//!
//! The core lends a service a socket for a resource with a native protocol
//! ([`::emulator_server::Listeners::tcp_plaintext`]), which hands the handler the
//! connection's FIRST byte. That byte says what the client opened with: a TLS record is
//! handed to the core's own TLS server ([`::emulator_server::TlsUpgrader`], so the service
//! never touches a key or a certificate) and everything after it is read decrypted, while
//! an HTTP method is read as it stands. ONE announced address therefore serves both
//! schemes.
//!
//! Doing that needs the first byte REPLAYED to whoever reads the stream next — the TLS
//! server has to see byte 0, and so does an HTTP or WebSocket parser — which is what
//! [`Prefixed`] is for. The rule is here so the sniff and the replay are written once:
//! a fix to either is a fix for every plane.
//!
//! Services still carrying their own copy of this reading, to be MIGRATED onto it (it
//! postdates them): `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/
//! websocket/transport.rs`. Nothing new inlines it again — call this module.

use ::std::pin::Pin;
use ::std::task::{Context, Poll};

use ::emulator_server::{ListenerConn, TlsUpgrader};
use ::tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};

/// The first byte of a TLS record, which is what a client that dialled `https://` or
/// `wss://` opens with: the `handshake` content type (22) of TLS 1.0 and later. A request
/// made in the clear begins with an HTTP method instead, so one byte tells the two apart.
pub const TLS_RECORD_FIRST_BYTE: u8 = 0x16;

/// The connection with encryption negotiated: handed to the core's TLS server when the
/// client opened with a TLS record, and read as it stands otherwise.
///
/// `upgrader` is the core's TLS server, as the listener's owner obtained it
/// (`engine.listeners().tls_upgrader()`); `None` means this emulator has none prepared, and
/// a client that dialled TLS is then dropped rather than answered in the clear.
///
/// `None` for a client that sent nothing, for a read that failed, and for a TLS handshake
/// this emulator could not complete — in each case there is nothing to serve on the
/// connection.
pub async fn negotiated(
    conn: ListenerConn,
    upgrader: Option<TlsUpgrader>,
) -> Option<ListenerConn> {
    negotiated_scheme(conn, upgrader)
        .await
        .map(|(conn, _encrypted)| conn)
}

/// [`negotiated`], reporting WHICH half of the address the client dialled: `true` when the
/// connection was opened with a TLS record and is now read decrypted, `false` when the
/// client sent its request in the clear.
///
/// One announced address serves both schemes, so a service that tells a client (or an origin
/// behind it) which scheme the client used reads it here — there is nothing else on the
/// connection that says so.
pub async fn negotiated_scheme(
    mut conn: ListenerConn,
    upgrader: Option<TlsUpgrader>,
) -> Option<(ListenerConn, bool)> {
    let mut first = [0u8; 1];
    match conn.read(&mut first).await {
        Ok(1) => {}
        _ => return None,
    }
    let prefixed: ListenerConn = Box::new(Prefixed {
        prefix: first.to_vec(),
        read: 0,
        inner: conn,
    });
    if first[0] != TLS_RECORD_FIRST_BYTE {
        return Some((prefixed, false));
    }
    let upgrader = upgrader?;
    // ALPN `None`: a client of such a plane offers `http/1.1` or nothing, and what follows
    // is HTTP/1.1 either way.
    match upgrader.accept(prefixed, None).await {
        Ok((upgraded, _negotiated)) => Some((upgraded, true)),
        Err(error) => {
            ::tracing::debug!(%error, "a TLS handshake on a data-plane listener was not completed");
            None
        }
    }
}

/// A connection whose first bytes have already been read: they are served from the prefix
/// before the socket itself is read again, so a reader that has to see byte 0 — the core's
/// TLS server, an HTTP parser, a WebSocket handshake — reads the stream as the client sent
/// it.
pub struct Prefixed<S> {
    /// The bytes already taken off the socket, to be served first.
    prefix: Vec<u8>,
    /// How many of them have been served.
    read: usize,
    /// The socket itself.
    inner: S,
}

impl<S> Prefixed<S> {
    /// A stream that serves `prefix` before reading `inner`.
    #[must_use]
    pub fn new(prefix: Vec<u8>, inner: S) -> Self {
        Self {
            prefix,
            read: 0,
            inner,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if self.read < self.prefix.len() {
            let remaining = &self.prefix[self.read..];
            let taken = remaining.len().min(buffer.remaining());
            buffer.put_slice(&remaining[..taken]);
            self.read += taken;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(context, buffer)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(context, buffer)
    }

    fn poll_flush(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(context)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_prefixed_stream_serves_the_bytes_already_read_before_the_socket() {
        // The rows span what the replay generalises over: no prefix at all, one byte (the
        // sniffed one), and a prefix longer than the reader's first read.
        let cases: &[(&str, &[u8], &[u8])] = &[
            ("no prefix", b"", b"GET / HTTP/1.1"),
            ("the one sniffed byte", b"G", b"ET / HTTP/1.1"),
            ("several bytes", b"GET ", b"/ HTTP/1.1"),
        ];
        for (case, prefix, rest) in cases {
            let mut stream = Prefixed::new(prefix.to_vec(), &rest[..]);
            let mut read = Vec::new();
            stream
                .read_to_end(&mut read)
                .await
                .unwrap_or_else(|error| panic!("case {case:?}: the stream must be read: {error}"));
            let mut expected = prefix.to_vec();
            expected.extend_from_slice(rest);
            assert_eq!(
                read, expected,
                "case {case:?}: expected the prefix then the socket, got {read:?}"
            );
        }
    }
}
