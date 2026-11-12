// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS a REGIONAL custom domain name is reached at, and the socket that answers
//! there.
//!
//! `DomainName$regionalDomainName` is "the domain name associated with the regional
//! endpoint for this custom domain name. You set up this association by adding a DNS record
//! that points the custom domain name to this regional domain name" — so it is an address a
//! CLIENT then dials, not a label it reads. A name under API Gateway's own zone resolves to
//! nothing on the machine this emulator runs on, so a custom domain name that published one
//! published an address no client could reach, and everything the front door of a custom
//! domain exists for — the TLS handshake under the certificate the domain names, the mutual
//! TLS its truststore enforces, and the stage a base path mapping routes to — was
//! unobservable.
//!
//! So each regional custom domain name gets a socket of the EMULATOR's own, opened through
//! the core (`engine.listeners().tcp_plaintext(..)`, which owns the socket, the bind
//! interface, the port range and the connection caps), and publishes the `host:port` that
//! socket answers on. A client dials exactly what it was told.
//!
//! Only a REGIONAL record gets one: an edge-optimized domain name is served by the
//! CloudFront distribution API Gateway puts in front of it (`distributionDomainName`) and a
//! private one is reached through the VPC endpoints its endpoint configuration names, so
//! neither publishes this member at all.
//!
//! The socket is the ADDRESS, not the data plane: what a request it accepts is answered BY
//! — the api and stage its base path mapping names, that stage's methods, authorizers and
//! integrations — stays in [`crate::service::invoke`], which serves the same request however
//! it was addressed. What the socket itself owns is the TRANSPORT: the TLS termination and
//! the mutual-TLS contract ([`crate::service::custom_domain_socket`]).
//!
//! The socket's life is the domain name's: the handle is held on the store under the
//! record's own key, and dropping it (a delete, or a migration to another endpoint type)
//! closes the port and severs every connection in flight on it.

use ::smithy_server_runtime::EmbeddedEmulator;

/// Whether `record` is served by the Region's API Gateway endpoint — the one kind of custom
/// domain name that publishes a `regionalDomainName` and so the one that gets an address of
/// its own.
///
/// The endpoint type is the record's own `endpointConfiguration.types` entry: `REGIONAL` is
/// documented for "a regional API and its custom domain name", and the `EDGE` and `PRIVATE`
/// records publish the CloudFront distribution's names and nothing at all respectively (see
/// [`super::DomainNameEntry`]).
#[must_use]
pub fn is_regional_domain_name(record: &super::DomainNameEntry) -> bool {
    record
        .endpoint_configuration
        .as_ref()
        .and_then(|configuration| configuration.types.as_ref())
        .is_some_and(|types| {
            types
                .iter()
                .any(|kind| kind == super::REGIONAL_ENDPOINT_TYPE)
        })
        && record.domain_name_id.is_none()
}

/// One regional custom domain name's own address: where it answers, and the handle that
/// keeps it open.
///
/// Dropping it closes the port, so the record is held for exactly as long as the domain
/// name it belongs to is served by a regional endpoint.
pub struct CustomDomainEndpoint {
    /// The `host:port` a client dials, which is what the record reports as its
    /// `regionalDomainName`.
    authority: String,
    /// The listener itself. Never read: it is held because dropping it closes the port.
    _listener: ::emulator_server::ResourceListener,
}

impl ::std::fmt::Debug for CustomDomainEndpoint {
    /// Everything about this record that is not a live socket: the address it answers on.
    /// `ResourceListener` is not `Debug`.
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("CustomDomainEndpoint")
            .field("authority", &self.authority)
            .finish()
    }
}

impl CustomDomainEndpoint {
    /// The `host:port` a client reaches this custom domain name at — its
    /// `regionalDomainName`.
    #[must_use]
    pub fn authority(&self) -> &str {
        &self.authority
    }
}

/// Open the socket one regional custom domain name is served on, and report the address it
/// answers at — or `None` when this engine vends no listeners, in which case the caller
/// falls back to the name API Gateway's own zone would carry.
///
/// `domain_name_key` is the key the record is stored under
/// ([`super::domain_name_key`]), which is what the socket reads the record it serves by:
/// the key of a domain name never changes (no patch replaces `/domainName`), so the socket
/// always serves the record a client dialed the address of.
///
/// `callback` is the handle into the emulator this request is running within
/// (`ctx.callback()`), which is how a request the socket accepts reaches the stage its base
/// path mapping names, and how the truststore object is read: the socket is not a second
/// emulator, it is a second ADDRESS of this one.
pub async fn open_custom_domain_endpoint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: EmbeddedEmulator,
    domain_name_key: &str,
    region: &str,
) -> Option<CustomDomainEndpoint> {
    let shared = (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()?;
    let listeners = shared.listeners();
    let handler = crate::service::custom_domain_socket::CustomDomainSocket::new(
        domain_name_key.to_string(),
        shared.state::<super::ComAmazonawsApigatewayBackplanecontrolserviceState>(),
        certificate_key_material_directory(engine),
        callback,
        region.to_string(),
        // The core's own TLS server, for a TLS connection whose domain name holds no
        // certificate material and enforces no mutual TLS. The certificate Certificate
        // Manager holds is served by the handler itself, which is the only way that
        // certificate can be the one presented.
        listeners.tls_upgrader().ok(),
    );
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one
    // built with no IO driver, which is what a host that drives this emulator in process
    // may hand a request — leaves this create without an address of its own instead of
    // taking the request down with it: the join answers `Err`, this answers `None`, and the
    // caller falls back to the name API Gateway's own zone would carry.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await.ok() }).await;
    let listener = opened.ok().flatten()?;
    Some(CustomDomainEndpoint {
        authority: ::service_common::data_plane_addressing::listener_authority(&listener),
        _listener: listener,
    })
}

/// The SHARED certificate key-material directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// What a custom domain name PRESENTS is the certificate Certificate Manager holds for its
/// `regionalCertificateArn` — that service's resource, in that service's store, which this
/// one cannot see. The material a handshake needs (the leaf, its chain and the private key
/// that matches it) is held in ONE directory under one shared type
/// ([`::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory`]),
/// written by the service that holds the certificate and read here. The engine keys state by
/// TYPE, so a copy of its own in this service's state would be invisible to the writer.
///
/// `None` on an engine carrying no directory.
pub fn certificate_key_material_directory<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::acm_certificate_key_material::CertificateKeyMaterialDirectory,
            >,
        )
}
