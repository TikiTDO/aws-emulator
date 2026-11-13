// SPDX-License-Identifier: CC0-1.0

//! A `forward` to an `ip` or `instance` target whose workload RUNS ON THE HOST: how a request
//! a client sent to an Application Load Balancer reaches a container this emulator started.
//!
//! By default no workload behind such a target runs anywhere, and the plane answers the `502`
//! a load balancer gives for a target it cannot connect to ([`super::data_plane`]). With host
//! workloads turned on ([`::service_common::host_workloads::enabled`]), the scheduler that
//! started the workload published where each of its container ports can be dialled
//! ([`::service_common::host_workloads::HostWorkloads`]), and the registration the load
//! balancer chose resolves to that address — by the workload the scheduler registered it for,
//! else by the target id a caller registered.
//!
//! The request is then PROXIED the way an Application Load Balancer forwards it: over HTTP/1.1
//! to the target's port, with the client's method, path, query, headers and body; the
//! hop-by-hop headers dropped; and the `X-Forwarded-Proto` and `X-Forwarded-Port` headers
//! saying which listener the client reached. The target's answer is the client's answer.
//!
//! What a load balancer answers when the target does not:
//!
//! * a port nothing published, or a connection that cannot be made within
//!   [`CONNECT_TIMEOUT`]: `502`, "the load balancer could not establish a connection to the
//!   target";
//! * a response that does not arrive within [`RESPONSE_TIMEOUT`]: `504`, "the target did not
//!   respond before the idle timeout period elapsed";
//! * a response that is not HTTP, or a body larger than [`MAX_RESPONSE_BYTES`]: `502`.

use std::net::SocketAddr;
use std::time::Duration;

use ::service_common::host_workloads::WorkloadKey;
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::rt::TokioIo;

use super::data_plane::{DataPlaneRequest, Served, TARGET_UNREACHABLE_STATUS};

/// How long the load balancer waits for the target to accept the connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the load balancer waits for the target's whole answer: the default idle timeout
/// of an Application Load Balancer.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);
/// The largest body read back from a target.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
/// What a load balancer answers when the target accepted the connection but did not answer in
/// time.
pub const TARGET_TIMEOUT_STATUS: u16 = 504;

/// Headers that describe one connection rather than the request, so they are not carried
/// across the hop (RFC 9110 §7.6.1), plus the framing headers the forwarded request sets for
/// itself.
const NOT_FORWARDED: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "content-length",
    "host",
];

/// What the plane resolved a `forward` to a host workload to.
#[derive(Debug)]
pub(super) struct Forwarded {
    /// Every key the chosen registration can be resolved by, in the order they are tried.
    pub(super) keys: Vec<WorkloadKey>,
    /// The port the target receives traffic on.
    pub(super) port: i32,
    /// The protocol and port of the listener the client reached.
    pub(super) protocol: Option<String>,
    pub(super) listener_port: Option<i32>,
    /// The stickiness pin the forward action mints, sent back with the target's answer.
    pub(super) set_cookie: Option<String>,
}

impl Forwarded {
    /// The address the registration resolves to, by the first key something published.
    pub(super) fn address(
        &self,
        workloads: &::service_common::host_workloads::HostWorkloads,
    ) -> Option<SocketAddr> {
        self.keys
            .iter()
            .find_map(|key| workloads.resolve(key, self.port))
    }
}

/// Forward `request` with `body` to the target at `address`, and answer with what it returned.
pub(super) async fn served(
    address: SocketAddr,
    forwarded: &Forwarded,
    request: &DataPlaneRequest,
    body: &[u8],
) -> Served {
    let Ok(Ok(stream)) =
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(address)).await
    else {
        tracing::info!(%address, "the host workload behind the target accepted no connection");
        return Served::status(TARGET_UNREACHABLE_STATUS);
    };
    let Ok((mut sender, connection)) =
        hyper::client::conn::http1::handshake(TokioIo::new(stream)).await
    else {
        return Served::status(TARGET_UNREACHABLE_STATUS);
    };
    // The connection is driven beside the request and ends with it.
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });

    let Some(outbound) = outbound_request(forwarded, request, body) else {
        driver.abort();
        return Served::status(TARGET_UNREACHABLE_STATUS);
    };
    let answered = tokio::time::timeout(RESPONSE_TIMEOUT, async {
        let response = sender.send_request(outbound).await.ok()?;
        let (parts, incoming) = response.into_parts();
        let bytes = Limited::new(incoming, MAX_RESPONSE_BYTES)
            .collect()
            .await
            .ok()?
            .to_bytes();
        Some((parts, bytes))
    })
    .await;
    driver.abort();
    let (parts, bytes) = match answered {
        Err(_) => return Served::status(TARGET_TIMEOUT_STATUS),
        Ok(None) => return Served::status(TARGET_UNREACHABLE_STATUS),
        Ok(Some(answer)) => answer,
    };

    let mut served = Served::status(parts.status.as_u16());
    for (name, value) in &parts.headers {
        let Ok(value) = value.to_str() else { continue };
        match name.as_str() {
            "content-type" => served.content_type = Some(value.to_string()),
            "location" => served.location = Some(value.to_string()),
            held if NOT_FORWARDED.contains(&held) => {}
            held => served.headers.push((held.to_string(), value.to_string())),
        }
    }
    if let Some(field) = forwarded.set_cookie.clone() {
        served.headers.push(("set-cookie".to_string(), field));
    }
    served.body = bytes.to_vec();
    served
}

/// The request as it leaves the load balancer for the target.
fn outbound_request(
    forwarded: &Forwarded,
    request: &DataPlaneRequest,
    body: &[u8],
) -> Option<http::Request<Full<bytes::Bytes>>> {
    let target = if request.query.is_empty() {
        request.path.clone()
    } else {
        format!("{}?{}", request.path, request.query)
    };
    let mut builder = http::Request::builder()
        .method(request.method.as_str())
        .uri(target)
        .header("host", request.host.as_str());
    for (name, value) in &request.headers {
        if NOT_FORWARDED.contains(&name.as_str()) || name.starts_with("x-forwarded-") {
            continue;
        }
        builder = builder.header(name.as_str(), value.as_str());
    }
    if let Some(protocol) = &forwarded.protocol {
        builder = builder.header("x-forwarded-proto", protocol.to_ascii_lowercase());
    }
    if let Some(port) = forwarded.listener_port {
        builder = builder.header("x-forwarded-port", port.to_string());
    }
    builder
        .body(Full::new(bytes::Bytes::copy_from_slice(body)))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn request() -> DataPlaneRequest {
        DataPlaneRequest {
            method: "GET".into(),
            path: "/up".into(),
            query: "a=1".into(),
            host: "app.local".into(),
            headers: vec![
                ("accept".into(), "text/plain".into()),
                ("connection".into(), "close".into()),
            ],
            port: Some(80),
        }
    }

    fn forwarded() -> Forwarded {
        Forwarded {
            keys: vec![WorkloadKey::Target("10.0.1.5".into())],
            port: 3000,
            protocol: Some("HTTP".into()),
            listener_port: Some(80),
            set_cookie: None,
        }
    }

    #[tokio::test]
    async fn the_target_answers_the_client() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut seen = vec![0u8; 4096];
            let read = socket.read(&mut seen).await.unwrap();
            let seen = String::from_utf8_lossy(&seen[..read]).to_string();
            socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 2\r\nx-app: rails\r\n\r\nok")
                .await
                .unwrap();
            seen
        });
        let served = served(address, &forwarded(), &request(), b"").await;
        let seen = server.await.unwrap().to_ascii_lowercase();
        assert_eq!(served.status, 200);
        assert_eq!(served.body, b"ok");
        assert_eq!(served.content_type.as_deref(), Some("text/plain"));
        assert!(served.headers.contains(&("x-app".into(), "rails".into())));
        assert!(seen.starts_with("get /up?a=1 http/1.1"));
        assert!(seen.contains("host: app.local"));
        assert!(seen.contains("x-forwarded-proto: http"));
        assert!(seen.contains("x-forwarded-port: 80"));
        assert!(!seen.contains("connection: close"));
    }

    #[tokio::test]
    async fn a_target_that_accepts_no_connection_is_a_502() {
        // Bind and drop, so the port is very likely closed.
        let address = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let served = served(address, &forwarded(), &request(), b"").await;
        assert_eq!(served.status, TARGET_UNREACHABLE_STATUS);
    }
}
