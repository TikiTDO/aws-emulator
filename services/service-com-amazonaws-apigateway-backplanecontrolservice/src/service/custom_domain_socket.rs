// SPDX-License-Identifier: CC0-1.0

//! The SOCKET a REGIONAL custom domain name is reached on: the address the record
//! publishes as its `regionalDomainName`, the TLS termination it performs there, the mutual
//! TLS it enforces, and the hand-off of every request it accepts to the stage a base path
//! mapping routes to ([`super::invoke`]).
//!
//! `DomainName$regionalDomainName` is "the domain name associated with the regional
//! endpoint for this custom domain name… returned by API Gateway when you create a regional
//! endpoint" — the address a CLIENT then points DNS at and dials — so it has to be an
//! address something answers on. A name under API Gateway's own zone resolves to nothing on
//! the machine this emulator runs on, so a record that published one published an address no
//! client could reach. Each regional record therefore gets a listener of the EMULATOR's own
//! ([`crate::state::open_custom_domain_endpoint`]) and publishes the `host:port` it answers
//! at.
//!
//! # What this address serves, and under which certificate
//!
//! The first byte of a connection says what the client opened with: a TLS record (the
//! `handshake` content type, [`::service_common::in_band_tls::TLS_RECORD_FIRST_BYTE`]) or a
//! request in the clear — so ONE address serves both the `https://` a client of a custom
//! domain dials and a request sent in the clear by a client that cannot verify a
//! self-signed certificate. A request in the clear presents no certificate at all, so a
//! record that enforces mutual TLS refuses it the way it refuses an untrusted certificate:
//! that half of the address is served only for a record that enforces none.
//!
//! A TLS connection is terminated HERE rather than by the core's own TLS server for the two
//! things the core's server cannot do: present the certificate Certificate Manager holds for
//! the domain's `regionalCertificateArn` (its own server certificate is the emulator's, and
//! it takes no other — [`::emulator_server::TlsUpgrader`]), and ASK the client for a
//! certificate, which is the whole of the `mutualTlsAuthentication` contract. The key
//! material is read from the ONE shared directory the certificate's owner writes
//! ([`::service_common::acm_certificate_key_material`]) and never leaves this process: it is
//! held in memory, used to answer a handshake, and named in no log.
//!
//! WHICH certificate is presented follows the name the `ClientHello` asks for (SNI), so the
//! material covering the custom domain is the one presented and a client that asks for no
//! name is presented the first certificate the record names.
//!
//! Two connections are handed to the core's own TLS server instead, under the emulator's own
//! certificate: one to a domain name that enforces no mutual TLS and whose certificate this
//! emulator holds no key material for (a certificate Certificate Manager ISSUED rather than
//! one a caller imported into it, so no private key for it exists anywhere here), and one
//! that arrives before this emulator has a TLS server prepared. Nothing is invented for
//! them.
//!
//! A domain name that DOES enforce mutual TLS and whose certificate no material is held for
//! is terminated here under a certificate minted for that domain name — one key pair and one
//! self-signed leaf per socket, so every handshake on the address presents the same
//! certificate. It is not presented as Certificate Manager's: it exists because a handshake
//! needs a server certificate before a client certificate can be asked for at all, and
//! without it the whole truststore contract would be unobservable on an address that reports
//! itself as enforcing it.
//!
//! # The mutual-TLS contract
//!
//! `mutualTlsAuthentication` is "the mutual TLS authentication configuration for a custom
//! domain name. If specified, API Gateway performs two-way authentication between the client
//! and the server. Clients must present a trusted certificate to access your API." The three
//! outcomes that follows from are served here:
//!
//! * a client that presents NO certificate over TLS is refused at the TLS layer — the client
//!   certificate is mandatory, so the handshake ends in a `certificate_required` alert and no
//!   HTTP response is ever written, and a client that sends its request in the clear, which
//!   presents none either, is refused as the untrusted certificate below is;
//! * a client whose certificate the truststore does not trust completes the handshake and is
//!   refused the HTTP way, `403` with the `Forbidden` body API Gateway words such a refusal
//!   with: the trust decision is about the certificate, not about the transport, which is
//!   why AWS answers it in HTTP at all;
//! * a client whose certificate chains to a CA in the truststore is served the stage its
//!   base path mapping names.
//!
//! The truststore is the S3 object `truststoreUri` addresses, at the `truststoreVersion` the
//! record names, read per connection through the emulator's own Amazon S3
//! ([`::service_common::s3_object_read`]) so a truststore a caller re-staged takes effect on
//! the next connection. The CA certificates in it are the trust anchors a presented chain is
//! validated against, by the TLS library's own path validation — so trust is a property of
//! the certificate the client presented, never of anything this emulator stored.
//!
//! # What answers the request
//!
//! Nothing about the ANSWER is decided here: the api and the stage are the ones the base
//! path mapping names, and every method, authorizer and integration of that stage stays in
//! [`super::invoke`], which serves the same request however it was addressed. A request this
//! socket accepts is handed to that plane over the in-process transport this emulator reaches
//! itself on, addressed as the reserved data-plane path of the api the mapping names.
//!
//! Every status, header and byte is computed from what a create or an update persisted, and
//! this module writes NOTHING: serving traffic moves no status, consumes no quota and bumps
//! no counter.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ::bytes::Bytes;
use ::emulator_server::{listener_caps, ListenerConn, NativeHandler, TlsUpgrader};
use ::http_body_util::{BodyExt as _, Full, Limited};
use ::service_common::acm_certificate_key_material::{
    CertificateKeyMaterial, CertificateKeyMaterialDirectory,
};
use ::service_common::in_band_tls::{Prefixed, TLS_RECORD_FIRST_BYTE};
use ::smithy_server_runtime::EmbeddedEmulator;
use ::tokio::io::AsyncReadExt as _;
use ::tokio::sync::{OwnedSemaphorePermit, Semaphore};
use aws_smithy_runtime_api::client::http::HttpConnector;
use aws_smithy_runtime_api::http::Request as RtRequest;
use aws_smithy_types::body::SdkBody;

// The two `rustls` modules whose names warn that implementing their traits REPLACES a
// verification the library would otherwise do, imported under names that say which side of
// the handshake each belongs to. What is implemented from them here verifies a CLIENT
// certificate's presence and then its chain against the record's truststore
// ([`trust_verdict`]); no verification of a peer is turned off, and nothing here verifies a
// SERVER certificate at all, because this code IS the server.
use ::rustls::client::{danger as handshake_signatures};
use ::rustls::server::{danger as client_certificates};

use crate::state::{
    BasePathMappingEntry, ComAmazonawsApigatewayBackplanecontrolserviceState as ApiGatewayState,
    DomainNameEntry,
};

/// The host the forwarded leg addresses this service's own data plane at: the endpoint that
/// names THIS service, so the emulator's front door resolves the request to the plane it is
/// meant for.
const SERVICE_ENDPOINT: &str = "apigateway.amazonaws.com";

/// The most truststore bytes this socket reads into the process per connection.
///
/// CHOSEN by this emulator rather than read off a published quota, and the same ceiling the
/// sibling trust-store reader applies: a CA bundle is on the order of a kilobyte per
/// certificate, and the read is RANGED to this, so a larger object is read as a truststore
/// nothing could be trusted from rather than buffered whole.
const MAX_TRUSTSTORE_BYTES: usize = 1_048_576;

/// How many request BODIES this socket reads at once across every custom domain name's
/// address: the core's own HTTP in-flight cap, one permit per request, taken before a byte of
/// a body is read and released when the answer is written.
///
/// ONE semaphore for the whole service, because a caller chooses how many custom domain names
/// exist and each one would otherwise raise the peak again.
fn in_flight() -> Arc<Semaphore> {
    static IN_FLIGHT: OnceLock<Arc<Semaphore>> = OnceLock::new();
    Arc::clone(IN_FLIGHT.get_or_init(|| Arc::new(Semaphore::new(listener_caps::MAX_HTTP_INFLIGHT))))
}

/// The headers the forwarded leg does not copy from the client's request.
///
/// `Host` is this socket's own address, which the forwarded request replaces with the
/// endpoint that names this service (the plane resolves the api from the reserved path
/// instead), and the two framing headers describe a body this leg re-frames itself.
const HEADERS_NOT_FORWARDED: [&str; 3] = ["host", "content-length", "transfer-encoding"];

/// How long one request may take, from its first header byte to the answer: the core's own
/// HTTP request deadline, so this address bounds a request exactly as the core's HTTP
/// listeners do.
fn request_deadline() -> Duration {
    Duration::from_secs(listener_caps::HTTP_REQUEST_DEADLINE_SECS)
}

/// How long a TLS handshake on this address may take, matching the deadline the core puts on
/// a handshake its own TLS server runs: a client that completes TCP and then stalls the
/// handshake cannot hold the connection permit the core issued for it.
const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(10);

/// How many bytes of one request this address reads: the core's per-connection ceiling.
fn body_ceiling() -> usize {
    usize::try_from(listener_caps::MAX_BYTES_PER_CONN).unwrap_or(usize::MAX)
}

/// Serves ONE regional custom domain name's socket: it terminates TLS under the certificate
/// the record names, enforces the record's mutual-TLS truststore, and hands every request it
/// accepts to the stage the record's base path mappings name.
///
/// Cheap to clone — every field is a handle — which is what the core's `'static`
/// per-connection handler contract asks for.
#[derive(Clone)]
pub struct CustomDomainSocket {
    /// The key the custom domain name this socket serves is stored under
    /// ([`crate::state::domain_name_key`]). A record this store no longer holds, or one an
    /// update moved to another endpoint type, is served nothing.
    domain_key: String,
    /// The ONE shared store the control plane writes, so a connection is served from exactly
    /// the record a create or an update persisted.
    state: ::engine::ServiceState<ApiGatewayState>,
    /// The shared directory the key material of an imported Certificate Manager certificate
    /// lives in, and `None` on an engine that carries none.
    key_material: Option<::engine::ServiceState<CertificateKeyMaterialDirectory>>,
    /// The handle into the emulator this socket belongs to: how a request it accepts reaches
    /// the data plane, and how the truststore object is read. The socket is not a second
    /// emulator, it is a second ADDRESS of this one.
    callback: EmbeddedEmulator,
    /// The Region the record was created in, which is the Region the truststore object is
    /// read from.
    region: String,
    /// The core's own TLS server, for a TLS connection this socket presents no certificate of
    /// the account's on. `None` when this emulator has none prepared, in which case such a
    /// connection is closed rather than served in the clear.
    upgrader: Option<TlsUpgrader>,
    /// The certificate this socket terminates a mutual-TLS connection under when Certificate
    /// Manager holds no key material for the one the record names; minted once, on the first
    /// such connection, so every handshake on this address presents the same certificate.
    minted: Arc<OnceLock<Option<Arc<CertificateKeyMaterial>>>>,
}

impl CustomDomainSocket {
    /// The socket handler for one custom domain name, over the shared engine's own stores.
    pub fn new(
        domain_key: String,
        state: ::engine::ServiceState<ApiGatewayState>,
        key_material: Option<::engine::ServiceState<CertificateKeyMaterialDirectory>>,
        callback: EmbeddedEmulator,
        region: String,
        upgrader: Option<TlsUpgrader>,
    ) -> Self {
        Self {
            domain_key,
            state,
            key_material,
            callback,
            region,
            upgrader,
            minted: Arc::new(OnceLock::new()),
        }
    }
}

impl NativeHandler for CustomDomainSocket {
    /// One call per accepted connection. The handles are cloned into the connection's own
    /// future, which is what the core's `'static` handler contract asks for.
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let socket = self.clone();
        Box::pin(async move { socket.serve_connection(conn).await })
    }
}

/// What the mutual-TLS configuration of a custom domain name made of the client on one
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClientVerdict {
    /// The record enforces no mutual TLS, so no client certificate was asked for.
    NotEnforced,
    /// The client presented a certificate that chains to a CA of the record's truststore.
    Trusted,
    /// The client presented NO certificate the record's truststore trusts: one the truststore
    /// does not trust, none at all on a connection in the clear, and every certificate where
    /// no CA could be read out of the truststore.
    Untrusted,
}

/// What this socket serves a connection with: the record's own configuration, read once per
/// connection under the one store the control plane writes.
struct ConnectionPlan {
    /// The custom domain name itself, which is the name a certificate is selected for when
    /// the client asks for none.
    domain_name: String,
    /// The certificates the record names, in the order a selection considers them: its
    /// regional certificate first, then the certificate member an edge-optimized or private
    /// endpoint would be served under, which a migrated record may still carry.
    certificate_arns: Vec<String>,
    /// Where the mutual-TLS truststore is, for a record that enforces mutual TLS: the
    /// `s3://bucket/key` of `truststoreUri` and the `truststoreVersion` beside it.
    truststore: Option<TruststoreLocation>,
}

/// The address of a mutual-TLS truststore object, as `mutualTlsAuthentication` names it.
#[derive(Debug, Clone)]
struct TruststoreLocation {
    bucket: String,
    key: String,
    version: Option<String>,
}

impl CustomDomainSocket {
    /// Serve one accepted connection: negotiate what the client opened with, then speak
    /// HTTP/1.1 on it until the client closes it.
    async fn serve_connection(&self, conn: ListenerConn) {
        let Some(plan) = self.plan() else {
            return;
        };
        let Some((conn, verdict)) = self.accepted(conn, &plan).await else {
            return;
        };
        let io = ::hyper_util::rt::TokioIo::new(conn);
        let callback = self.callback.clone();
        let state = self.state.clone();
        let domain_key = self.domain_key.clone();
        let service = ::hyper::service::service_fn(move |request: ::http::Request<_>| {
            let callback = callback.clone();
            let state = state.clone();
            let domain_key = domain_key.clone();
            async move {
                // A panic anywhere behind this address — in the dispatch machinery or in the
                // plane the forwarded leg reaches — is CONTAINED here and answered as the
                // protocol-shaped internal failure, exactly as the endpoint a control call
                // arrives on answers the same request: one client cannot take the
                // connection, the listener or the process down with it.
                let facts = ::emulator_server::RequestFacts::from_request(&request);
                Ok::<_, ::std::convert::Infallible>(
                    ::emulator_server::contain_panics(
                        facts,
                        answer(callback, state, domain_key, verdict, request),
                    )
                    .await,
                )
            }
        });
        if let Err(error) = ::hyper::server::conn::http1::Builder::new()
            .timer(::hyper_util::rt::TokioTimer::new())
            .header_read_timeout(request_deadline())
            .serve_connection(io, service)
            .await
        {
            // A connection the client dropped, or a request this emulator could not frame,
            // ends here and takes nothing else with it: the next connection is served
            // normally.
            ::tracing::debug!(
                %error,
                "a connection to a custom domain name's own address ended before its response"
            );
        }
    }

    /// What the record this socket serves holds now, or `None` when there is nothing to
    /// serve on the connection: a domain name this store no longer holds, and one an update
    /// moved to an endpoint type that is not reached at this address.
    ///
    /// Read under the store's lock, which is released before anything else happens: a
    /// handshake, a truststore read and a forwarded request never hold this service's store.
    fn plan(&self) -> Option<ConnectionPlan> {
        let store = self.state.lock();
        let record = store.domain_names.get(&self.domain_key)?;
        if !crate::state::is_regional_domain_name(record) {
            return None;
        }
        let certificate_arns = record
            .regional_certificate_arn
            .iter()
            .chain(record.certificate_arn.iter())
            .cloned()
            .collect();
        Some(ConnectionPlan {
            domain_name: record.domain_name.clone(),
            certificate_arns,
            truststore: truststore_location(record),
        })
    }

    /// Read what the client opened the connection with and hand back the stream the request
    /// is read from, together with what the record's mutual TLS made of the client.
    ///
    /// `None` when there is nothing to serve on the connection: a client that sent nothing, a
    /// TLS handshake this emulator could not complete — which is what a client presenting no
    /// certificate to a domain name that requires one gets — and a TLS connection on an
    /// emulator with no TLS server of its own prepared.
    async fn accepted(
        &self,
        mut conn: ListenerConn,
        plan: &ConnectionPlan,
    ) -> Option<(ListenerConn, ClientVerdict)> {
        let mut first = [0u8; 1];
        match conn.read(&mut first).await {
            Ok(1) => {}
            _ => return None,
        }
        let prefixed: ListenerConn = Box::new(Prefixed::new(first.to_vec(), conn));
        if first[0] != TLS_RECORD_FIRST_BYTE {
            // A request in the clear presents no certificate at all, so a record that
            // enforces mutual TLS has none of its truststore's to trust: such a connection
            // reaches no api of the account. Only a record that enforces none is served the
            // other half of the address.
            let verdict = match plan.truststore.is_some() {
                true => ClientVerdict::Untrusted,
                false => ClientVerdict::NotEnforced,
            };
            return Some((prefixed, verdict));
        }
        // The trust anchors of the record's truststore, read before the handshake: a client
        // certificate is asked for exactly when the record enforces mutual TLS, and what it
        // is validated against is this.
        let anchors = match plan.truststore.as_ref() {
            Some(location) => Some(self.trust_anchors(location).await),
            None => None,
        };
        let certificates = self.material_of(plan, anchors.is_some());
        let Some(config) = tls_server_config(&certificates, anchors.is_some()) else {
            // No certificate to present: the connection is served by the core's own TLS
            // server, under the emulator's own certificate. Such a connection asks for no
            // client certificate, so it is reached only by a record that enforces no mutual
            // TLS: one that does is presented the certificate this socket minted for it (see
            // [`Self::material_of`]), which is what lets a client certificate be asked for.
            let upgrader = self.upgrader.clone()?;
            return match upgrader.accept(prefixed, None).await {
                Ok((upgraded, _negotiated)) => Some((upgraded, ClientVerdict::NotEnforced)),
                Err(error) => {
                    ::tracing::debug!(
                        %error,
                        "a TLS handshake on a custom domain name's own address was not \
                         completed by the core's TLS server"
                    );
                    None
                }
            };
        };
        let acceptor = ::tokio_rustls::TlsAcceptor::from(config);
        let stream = match ::tokio::time::timeout(HANDSHAKE_DEADLINE, acceptor.accept(prefixed))
            .await
        {
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                // A client that presented no certificate where one is required ends here,
                // with no HTTP response ever written — which is what a mutual-TLS front door
                // answers such a client.
                ::tracing::debug!(
                    %error,
                    "a TLS handshake on a custom domain name's own address was not completed"
                );
                return None;
            }
            Err(_) => return None,
        };
        let verdict = match anchors {
            None => ClientVerdict::NotEnforced,
            Some(anchors) => {
                let presented: Vec<::rustls::pki_types::CertificateDer<'static>> = stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .map(|chain| chain.iter().map(|der| der.clone().into_owned()).collect())
                    .unwrap_or_default();
                trust_verdict(&anchors, &presented)
            }
        };
        Some((Box::new(stream) as ListenerConn, verdict))
    }

    /// The CA certificates of the record's truststore: the PEM certificate blocks of the
    /// object `location` addresses, read at the version the record names
    /// (`mutualTlsAuthentication.truststoreVersion`) and at the object's current version when
    /// it names none, through the emulator's own Amazon S3.
    ///
    /// Empty for a truststore nothing could be read from — an object the store holds nothing
    /// for at that address or under that version, one no read is permitted of, one larger than
    /// this socket reads, and one holding no certificate — in which case no client certificate
    /// chains to a trusted CA and every client is refused. The object's own bytes are never
    /// logged. The read is a READ: it changes nothing a later caller sees.
    async fn trust_anchors(
        &self,
        location: &TruststoreLocation,
    ) -> Vec<::rustls::pki_types::CertificateDer<'static>> {
        let read = ::service_common::s3_object_read::read_object(
            &self.callback,
            &self.region,
            &location.bucket,
            &location.key,
            location.version.as_deref(),
            MAX_TRUSTSTORE_BYTES,
        )
        .await;
        match read {
            ::service_common::s3_object_read::ObjectRead::Served(body) => certificates_in(&body),
            _unreadable => {
                ::tracing::debug!(
                    versioned = location.version.is_some(),
                    "a custom domain name's mutual-TLS truststore could not be read, so no \
                     client certificate is trusted"
                );
                Vec::new()
            }
        }
    }

    /// The key material of each certificate the record names, in the order it names them,
    /// skipping every certificate no material is held for.
    ///
    /// When mutual TLS is enforced and no material is held for any of them, the certificate
    /// this socket minted for the domain name is the one presented: a handshake needs a
    /// server certificate before a client certificate can be asked for at all. See this
    /// module's own documentation.
    fn material_of(
        &self,
        plan: &ConnectionPlan,
        mutual_tls: bool,
    ) -> Vec<Arc<CertificateKeyMaterial>> {
        let held: Vec<Arc<CertificateKeyMaterial>> = match self.key_material.as_ref() {
            None => Vec::new(),
            Some(directory) => {
                let directory = directory.lock();
                plan.certificate_arns
                    .iter()
                    .filter_map(|arn| directory.material(arn))
                    .collect()
            }
        };
        if !held.is_empty() || !mutual_tls {
            return held;
        }
        self.minted
            .get_or_init(|| minted_material(&plan.domain_name))
            .clone()
            .into_iter()
            .collect()
    }
}

/// Where the mutual-TLS truststore of `record` is, or `None` for a record that enforces no
/// mutual TLS.
///
/// `truststoreUri` is "an Amazon S3 URL that specifies the truststore for mutual TLS
/// authentication, for example `s3://bucket-name/key-name`", so the bucket is the host of
/// that URL and the key is the path under it; anything that is not such a URL addresses no
/// object and enforces nothing.
fn truststore_location(record: &DomainNameEntry) -> Option<TruststoreLocation> {
    let authentication = record.mutual_tls_authentication.as_ref()?;
    let uri = authentication.truststore_uri.as_deref()?.trim();
    let (bucket, key) = uri.strip_prefix("s3://")?.split_once('/')?;
    if bucket.is_empty() || key.is_empty() {
        return None;
    }
    Some(TruststoreLocation {
        bucket: bucket.to_string(),
        key: key.to_string(),
        version: authentication
            .truststore_version
            .as_ref()
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty()),
    })
}

/// Every PEM certificate block of `body`, as the DER a handshake validates against.
///
/// The PEM framing is read by `rustls-pemfile`, the reader this tree reads a certificate
/// bundle's blocks with, so a block that is not a certificate is no trust anchor.
fn certificates_in(body: &[u8]) -> Vec<::rustls::pki_types::CertificateDer<'static>> {
    let mut reader = ::std::io::BufReader::new(body);
    ::rustls_pemfile::certs(&mut reader)
        .filter_map(Result::ok)
        .collect()
}

/// What the truststore `anchors` make of the chain `presented`: `Trusted` when the leaf
/// chains to one of the anchors by the TLS library's own path validation, and `Untrusted`
/// otherwise — which is every chain when the truststore yielded no anchor at all.
fn trust_verdict(
    anchors: &[::rustls::pki_types::CertificateDer<'static>],
    presented: &[::rustls::pki_types::CertificateDer<'static>],
) -> ClientVerdict {
    let Some((leaf, intermediates)) = presented.split_first() else {
        return ClientVerdict::Untrusted;
    };
    let mut roots = ::rustls::RootCertStore::empty();
    for anchor in anchors {
        let _ = roots.add(anchor.clone());
    }
    let provider = Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
    let Ok(verifier) =
        ::rustls::server::WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider)
            .build()
    else {
        return ClientVerdict::Untrusted;
    };
    match verifier.verify_client_cert(
        leaf,
        intermediates,
        ::rustls::pki_types::UnixTime::now(),
    ) {
        Ok(_verified) => ClientVerdict::Trusted,
        Err(error) => {
            ::tracing::debug!(
                %error,
                "a client certificate presented to a custom domain name does not chain to its \
                 truststore"
            );
            ClientVerdict::Untrusted
        }
    }
}

/// A certificate and key pair minted for `domain_name`, for the handshake a mutual-TLS
/// record needs where Certificate Manager holds no material; `None` when no key pair could
/// be generated at all, in which case the connection is served by the core's TLS server.
fn minted_material(domain_name: &str) -> Option<Arc<CertificateKeyMaterial>> {
    let minted = ::rcgen::generate_simple_self_signed(vec![domain_name.to_string()]).ok()?;
    Some(Arc::new(CertificateKeyMaterial {
        certificate_pem: minted.cert.pem(),
        certificate_chain_pem: None,
        private_key_pem: minted.key_pair.serialize_pem(),
        domain_names: vec![domain_name.to_string()],
    }))
}

/// The TLS server configuration this address answers a handshake with, or `None` when there
/// is no certificate to present — in which case the caller falls back to the core's own TLS
/// server.
///
/// The certificate is chosen per handshake by the name the `ClientHello` asks for
/// ([`SelectedByServerName`]), so one address serves every name the record's certificates
/// cover. `mutual_tls` is what decides whether a CLIENT certificate is asked for, and it is
/// asked for as MANDATORY: "clients must present a trusted certificate to access your API",
/// so a client that presents none is refused at the TLS layer. Whether the certificate it
/// presents is TRUSTED is decided after the handshake, against the truststore's own anchors
/// ([`trust_verdict`]), because AWS answers an untrusted certificate in HTTP.
///
/// The versions negotiated are the library's defaults, TLS 1.2 and TLS 1.3. `securityPolicy`
/// is a MINIMUM — its two values are `TLS_1_0` and `TLS_1_2`
/// (`smithy_query(query="shape", shape="SecurityPolicy")`) — and both of those are satisfied
/// by every version negotiated here, so no record's policy admits a version this address
/// would refuse.
///
/// The provider is named explicitly rather than taken from the process default: this process
/// links more than one, and a configuration that has to work whichever was installed cannot
/// depend on which.
fn tls_server_config(
    certificates: &[Arc<CertificateKeyMaterial>],
    mutual_tls: bool,
) -> Option<Arc<::rustls::ServerConfig>> {
    let resolver = SelectedByServerName::of(certificates)?;
    let provider = Arc::new(::rustls::crypto::aws_lc_rs::default_provider());
    let builder = ::rustls::ServerConfig::builder_with_provider(Arc::clone(&provider))
        .with_safe_default_protocol_versions()
        .ok()?;
    let config = if mutual_tls {
        builder
            .with_client_cert_verifier(Arc::new(RequiresAClientCertificate(provider)))
            .with_cert_resolver(Arc::new(resolver))
    } else {
        builder
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(resolver))
    };
    Some(Arc::new(config))
}

/// The client-certificate verifier of a mutual-TLS address: a certificate is MANDATORY and
/// any certificate completes the handshake.
///
/// That split is the contract AWS serves: a client presenting no certificate is refused at
/// the TLS layer with no HTTP response, while one presenting a certificate the truststore
/// does not trust is answered `403` — an HTTP answer, so its handshake completed. The trust
/// decision therefore happens after the handshake, against the truststore's own anchors
/// ([`trust_verdict`]), and this verifier decides only that a certificate was presented at
/// all.
#[derive(Debug)]
struct RequiresAClientCertificate(Arc<::rustls::crypto::CryptoProvider>);

impl client_certificates::ClientCertVerifier for RequiresAClientCertificate {
    fn offer_client_auth(&self) -> bool {
        true
    }

    fn client_auth_mandatory(&self) -> bool {
        true
    }

    /// No CA names are advertised: `advertiseTrustStoreCaNames` is not a member of this
    /// service's model, so a client chooses which certificate to present on its own.
    fn root_hint_subjects(&self) -> &[::rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &::rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[::rustls::pki_types::CertificateDer<'_>],
        _now: ::rustls::pki_types::UnixTime,
    ) -> Result<client_certificates::ClientCertVerified, ::rustls::Error> {
        Ok(client_certificates::ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &::rustls::pki_types::CertificateDer<'_>,
        dss: &::rustls::DigitallySignedStruct,
    ) -> Result<handshake_signatures::HandshakeSignatureValid, ::rustls::Error> {
        ::rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &::rustls::pki_types::CertificateDer<'_>,
        dss: &::rustls::DigitallySignedStruct,
    ) -> Result<handshake_signatures::HandshakeSignatureValid, ::rustls::Error> {
        ::rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<::rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

/// The certificate this address presents, chosen by the name the `ClientHello` asked for:
/// the first of the record's certificates that covers it, and the first certificate for a
/// client that asks for a name none of them covers or for no name at all.
#[derive(Debug)]
struct SelectedByServerName {
    /// Each certificate's names and the handshake key loaded from its material, in the
    /// order the record names them.
    loaded: Vec<(Arc<CertificateKeyMaterial>, Arc<::rustls::sign::CertifiedKey>)>,
}

impl SelectedByServerName {
    /// The resolver over `certificates`, or `None` when none of them yields a key a
    /// handshake can be run with — material a `rustls` provider does not accept is material
    /// this address cannot present.
    fn of(certificates: &[Arc<CertificateKeyMaterial>]) -> Option<Self> {
        let loaded: Vec<(Arc<CertificateKeyMaterial>, Arc<::rustls::sign::CertifiedKey>)> =
            certificates
                .iter()
                .filter_map(|material| {
                    certified_key(material)
                        .map(|key| (Arc::clone(material), Arc::new(key)))
                })
                .collect();
        (!loaded.is_empty()).then_some(Self { loaded })
    }
}

impl ::rustls::server::ResolvesServerCert for SelectedByServerName {
    fn resolve(
        &self,
        client_hello: ::rustls::server::ClientHello<'_>,
    ) -> Option<Arc<::rustls::sign::CertifiedKey>> {
        let asked_for = client_hello.server_name();
        if let Some(asked_for) = asked_for {
            if let Some((_material, key)) = self
                .loaded
                .iter()
                .find(|(material, _key)| material.covers(asked_for))
            {
                return Some(Arc::clone(key));
            }
        }
        self.loaded.first().map(|(_material, key)| Arc::clone(key))
    }
}

/// The handshake key of one certificate's material: its chain and the private key that
/// signs with it, as a `rustls` provider reads them.
///
/// `None` for material a provider does not accept — a key whose algorithm it cannot sign
/// with, a chain it cannot read — which is material this address cannot present.
fn certified_key(material: &CertificateKeyMaterial) -> Option<::rustls::sign::CertifiedKey> {
    let mut chain: Vec<::rustls::pki_types::CertificateDer<'static>> = Vec::new();
    for pem in [
        Some(material.certificate_pem.as_str()),
        material.certificate_chain_pem.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        let mut reader = ::std::io::BufReader::new(pem.as_bytes());
        for certificate in ::rustls_pemfile::certs(&mut reader) {
            chain.push(certificate.ok()?);
        }
    }
    if chain.is_empty() {
        return None;
    }
    let mut reader = ::std::io::BufReader::new(material.private_key_pem.as_bytes());
    let key = ::rustls_pemfile::private_key(&mut reader).ok()??;
    let signing_key = ::rustls::crypto::aws_lc_rs::sign::any_supported_type(&key).ok()?;
    Some(::rustls::sign::CertifiedKey::new(chain, signing_key))
}

/// Answer one request on a custom domain name's own address.
async fn answer(
    callback: EmbeddedEmulator,
    state: ::engine::ServiceState<ApiGatewayState>,
    domain_key: String,
    verdict: ClientVerdict,
    request: ::http::Request<::hyper::body::Incoming>,
) -> ::http::Response<Full<Bytes>> {
    // A client whose certificate the truststore does not trust reaches no api of the
    // account: "clients must present a trusted certificate to access your API".
    if verdict == ClientVerdict::Untrusted {
        return forbidden();
    }
    let (parts, body) = request.into_parts();
    // Which api and stage this path reaches, from the record's base path mappings — read
    // per request, so a mapping created after the connection was opened serves it.
    let Some(target) = invoke_target(&state, &domain_key, parts.uri.path()) else {
        return forbidden();
    };
    // One permit per request IN FLIGHT, taken BEFORE a byte of the body is read: what the
    // bodies being read hold at once is then the in-flight cap times the ceiling below,
    // rather than the connection cap (or the number of custom domain names) times it.
    let Ok(permit) = in_flight().try_acquire_owned() else {
        return generated(SERVICE_UNAVAILABLE, SERVER_BUSY);
    };
    let limited = Limited::new(body, body_ceiling());
    let served = ::tokio::time::timeout(request_deadline(), async move {
        // Held for the whole read and dispatch, and released with it.
        let _permit: OwnedSemaphorePermit = permit;
        let Ok(collected) = limited.collect().await else {
            return generated(PAYLOAD_TOO_LARGE, BODY_TOO_LARGE);
        };
        let Some(forwarded) = forwarded(parts, collected.to_bytes(), &target) else {
            return generated(BAD_GATEWAY, NO_STAGE);
        };
        match callback.http_connector().call(forwarded).await {
            Ok(response) => {
                let status = response.status().as_u16();
                let mut answer = ::http::Response::builder().status(status);
                for (name, value) in response.headers().iter() {
                    answer = answer.header(name, value);
                }
                let body = Bytes::from(
                    response
                        .into_body()
                        .bytes()
                        .map(<[u8]>::to_vec)
                        .unwrap_or_default(),
                );
                answer
                    .body(Full::new(body.clone()))
                    .unwrap_or_else(|_| ::http::Response::new(Full::new(body)))
            }
            // The data plane is this same process, so this is no answer about an api; it is
            // reported the way this address reports an answer it could not obtain.
            Err(_) => generated(BAD_GATEWAY, NO_STAGE),
        }
    })
    .await;
    served.unwrap_or_else(|_| generated(GATEWAY_TIMEOUT, REQUEST_TIMED_OUT))
}

/// The api, the stage and the path one request reaches through the record's base path
/// mappings.
struct InvokeTarget {
    api_id: String,
    /// The stage the mapping names, and `None` for a mapping that names none — where the
    /// stage is the first segment of the path the client sent, as it is on an api's own
    /// invoke endpoint.
    stage: Option<String>,
    /// The path under the base path, without a leading `/`.
    path: String,
}

/// Which api, stage and path the request path `path` reaches on the custom domain name
/// `domain_key`, or `None` when it reaches none — a domain name this store no longer holds,
/// and a path no base path mapping of it answers to.
///
/// "Specify '(none)' if you do not want callers to specify any base path name after the
/// domain name" (`BasePathMapping$basePath`), so the mapping stored under that spelling
/// answers every path no other mapping claims, and the LONGEST base path that is a prefix of
/// the path — on a segment boundary, since a base path is a path segment and not a string
/// prefix — is the one that serves it.
fn invoke_target(
    state: &::engine::ServiceState<ApiGatewayState>,
    domain_key: &str,
    path: &str,
) -> Option<InvokeTarget> {
    let store = state.lock();
    let record = store.domain_names.get(domain_key)?;
    let (mapping, remainder) = matching_mapping(record, path)?;
    Some(InvokeTarget {
        api_id: mapping.rest_api_id.clone(),
        stage: mapping.stage.clone(),
        path: remainder.to_string(),
    })
}

/// The base path mapping of `record` that answers `path`, and the path left under it; see
/// [`invoke_target`].
///
/// A path carrying a DOT SEGMENT (`.` or `..`) answers to no mapping: such a segment is a
/// relative reference, and what a mapping selects is the api and stage the path under its
/// base path is served by — a remainder that could climb back out of that prefix selects
/// something the mapping does not name, so it reaches no api at all.
fn matching_mapping<'r>(
    record: &'r DomainNameEntry,
    path: &'r str,
) -> Option<(&'r BasePathMappingEntry, &'r str)> {
    if carries_a_dot_segment(path) {
        return None;
    }
    let requested = path.trim_start_matches('/');
    let mut best: Option<(&BasePathMappingEntry, &str)> = None;
    for (base_path, mapping) in &record.base_path_mappings {
        if base_path == crate::state::NONE_BASE_PATH {
            continue;
        }
        let base = base_path.trim_matches('/');
        let Some(under) = requested.strip_prefix(base) else {
            continue;
        };
        let remainder = match under.strip_prefix('/') {
            Some(remainder) => remainder,
            // The path IS the base path, with nothing under it.
            None if under.is_empty() => under,
            // A longer segment that merely starts with the base path is a different path.
            None => continue,
        };
        if best.is_none_or(|(held, _)| held.base_path.trim_matches('/').len() < base.len()) {
            best = Some((mapping, remainder));
        }
    }
    best.or_else(|| {
        record
            .base_path_mappings
            .get(crate::state::NONE_BASE_PATH)
            .map(|mapping| (mapping, requested))
    })
}

/// Whether `path` carries a segment that is a relative reference — `.` or `..`, in the
/// percent-encoded spellings a URI may write them in as well, since nothing between a client
/// and this address decodes them.
fn carries_a_dot_segment(path: &str) -> bool {
    path.split('/').any(|segment| {
        let decoded = segment
            .replace("%2e", ".")
            .replace("%2E", ".");
        decoded == "." || decoded == ".."
    })
}

/// The request the data plane is handed for one client request: the same method, body and
/// headers, addressed to this service's own endpoint under the reserved data-plane path of
/// the api the base path mapping names, with the stage that mapping names ahead of the path
/// the client sent.
///
/// `None` when the client's own request does not render as one, which addresses nothing —
/// and when the rendered path does not stay under the reserved prefix of the api the mapping
/// names, which is the one thing this leg must not let a request leave.
fn forwarded(
    parts: ::http::request::Parts,
    body: Bytes,
    target: &InvokeTarget,
) -> Option<RtRequest> {
    let query = parts
        .uri
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let prefix = match target.stage.as_deref() {
        Some(stage) => format!(
            "/{}/{}/{}/{stage}",
            crate::service::invoke::EMULATOR_PATH_NAMESPACE,
            crate::service::invoke::EXECUTE_API_LABEL,
            target.api_id
        ),
        None => format!(
            "/{}/{}/{}",
            crate::service::invoke::EMULATOR_PATH_NAMESPACE,
            crate::service::invoke::EXECUTE_API_LABEL,
            target.api_id
        ),
    };
    let uri = format!("https://{SERVICE_ENDPOINT}{prefix}/{}{query}", target.path);
    let mut builder = ::http::Request::builder()
        .method(parts.method.clone())
        .uri(uri)
        .header(::http::header::HOST, SERVICE_ENDPOINT);
    for (name, value) in parts.headers.iter() {
        if HEADERS_NOT_FORWARDED.contains(&name.as_str()) {
            continue;
        }
        builder = builder.header(name, value);
    }
    let built = builder.body(SdkBody::from(body)).ok()?;
    // What the leg addresses is READ BACK off the rendered request and checked to still name
    // the reserved prefix of the api the mapping named: the path under a base path is the
    // client's own, so this is the assertion that nothing it carried moved the request to
    // another api, another stage or another plane of this process.
    (built.uri().path().starts_with(&prefix)).then_some(())?;
    RtRequest::try_from(built).ok()
}

/// The status API Gateway refuses a request on a custom domain name that reaches no api of
/// the account with, and the one a client whose certificate the truststore does not trust
/// is answered.
const FORBIDDEN: u16 = 403;

/// The body of that refusal: API Gateway answers a request its front door turned away with
/// a one-member JSON document naming the refusal.
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The header API Gateway names the exception class of such a refusal in.
const ERROR_TYPE_HEADER: &str = "x-amzn-ErrorType";

/// The class of that refusal, as the header above carries it.
const FORBIDDEN_ERROR_TYPE: &str = "ForbiddenException";

/// What this address answers a request it could not hand to the data plane at all.
const BAD_GATEWAY: u16 = 502;

/// What it answers a request that arrived while this service was already reading as many
/// request bodies as it reads at once.
const SERVICE_UNAVAILABLE: u16 = 503;

/// What it answers a request that did not finish within one request deadline.
const GATEWAY_TIMEOUT: u16 = 504;

/// What it answers a request carrying more than one connection may.
const PAYLOAD_TOO_LARGE: u16 = 413;

/// The body of an answer for a request shed at the in-flight cap.
const SERVER_BUSY: &str = "This address is serving as many requests as it can. Try again.\n";

/// The body of an answer for a request whose stage this emulator could not reach.
const NO_STAGE: &str = "This address reached no stage of this api.\n";

/// The body of an answer for a request larger than this address accepts.
const BODY_TOO_LARGE: &str = "This request is larger than this address accepts.\n";

/// The body of an answer for a request that outlasted one request deadline.
const REQUEST_TIMED_OUT: &str = "This request took longer than this address allows.\n";

/// The refusal a custom domain name's front door turns a request away with: `403`, the
/// one-member JSON body and the exception class in the error-type header.
fn forbidden() -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(format!("{{\"message\":\"{FORBIDDEN_MESSAGE}\"}}"));
    ::http::Response::builder()
        .status(FORBIDDEN)
        .header(::http::header::CONTENT_TYPE, "application/json")
        .header(ERROR_TYPE_HEADER, FORBIDDEN_ERROR_TYPE)
        .body(Full::new(body.clone()))
        .unwrap_or_else(|_| ::http::Response::new(Full::new(body)))
}

/// One answer of this address's own, for a request the data plane never saw. Plain text, as
/// a front door's own generated answers are.
fn generated(status: u16, message: &str) -> ::http::Response<Full<Bytes>> {
    let body = Bytes::from(message.to_string());
    ::http::Response::builder()
        .status(status)
        .header(::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Full::new(body.clone()))
        // Every argument is this function's own, so the builder cannot fail; answer a bare
        // status rather than panicking if it ever does.
        .unwrap_or_else(|_| {
            let mut response = ::http::Response::new(Full::new(body));
            *response.status_mut() = ::http::StatusCode::BAD_GATEWAY;
            response
        })
}

#[cfg(test)]
mod tests;
