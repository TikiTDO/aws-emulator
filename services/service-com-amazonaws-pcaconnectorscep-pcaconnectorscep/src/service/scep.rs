// SPDX-License-Identifier: CC0-1.0

//! The SCEP data plane a connector publishes: the wire `Connector$Endpoint` advertises.
//!
//! # Not a modelled operation
//!
//! Every operation this service models is control plane: it creates, reads, tags and
//! deletes connectors and their challenge passwords. What a SCEP-enabled device or a mobile
//! device management system then does with the URL `GetConnector`/`ListConnectors` hand back
//! is the SCEP protocol itself (RFC 8894) — a plain `GET <Endpoint>?operation=<name>`. No
//! part of that is a modelled operation and none of it carries an `X-Amz-Target`: it is
//! addressed by host, path and query, so nothing in the generated routing table can name it
//! and it is reached through the two seams [`crate::service::EmulatedService`] has for a
//! request the model's routing key cannot name ([`claims_parts`], [`ScepDataPlane`]).
//!
//! A SCEP client holds no AWS credential — it is a device enrolling for a certificate — so
//! this plane declares no auth scheme and serves the request however it arrived. It still
//! claims only an address this service PUBLISHED, and answers only out of records the
//! control plane persisted.
//!
//! # What is served
//!
//! * `GET <Endpoint>?operation=GetCACert` — the certificate of the private CA the connector
//!   links, as the single DER X.509 certificate RFC 8894 answers that request with
//!   ([`CA_CERTIFICATE_CONTENT_TYPE`]). The certificate is read from Amazon Web Services
//!   Private CA itself, through its own `GetCertificateAuthorityCertificate`, at the instant
//!   the request is served — so a CA whose certificate changed, or which no longer exists,
//!   is not answered out of a copy this service took earlier.
//!
//! Any other `operation` is answered [`BAD_REQUEST_STATUS`]: this plane implements none of
//! SCEP's optional capabilities and issues no certificate through `PKIOperation`, and an
//! error is what a SCEP client reads as "this CA does not support that" — a `200` there
//! would stand for an enrolment nothing performed. Each is its own gap, to be implemented
//! against its own recorded behaviour.
//!
//! An address that names no connector of this service is `404`, and a connector that is not
//! `ACTIVE` does not serve its data plane yet, so it is [`NOT_SERVING_STATUS`]: the address
//! exists and the plane behind it is not up.
//!
//! # Where it is addressed
//!
//! Both addressings `Connector$Endpoint` is announced on
//! ([`crate::state::announced_connector_endpoint`]) are claimed, so the address this service
//! handed the client is the address the answer comes back from: the reserved path on the
//! emulator's OWN endpoint, and the published SCEP host for a client that resolves AWS's own
//! hostnames.
//!
//! Serving writes NOTHING beyond the transitions the clock already owed
//! (`ComAmazonawsPcaconnectorscepPcaconnectorscepState::settle`): fetching a CA certificate
//! moves no status, consumes no quota and bumps no counter.

use std::sync::Arc;
use std::time::SystemTime;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, BoxFuture, ErasedOperation, OperationContext, Service};

use crate::state::{self, AddressedConnector};

/// The media type a single DER X.509 certificate is served as, which is what RFC 8894
/// answers `GetCACert` with when the CA has one certificate.
const CA_CERTIFICATE_CONTENT_TYPE: &str = "application/x-x509-ca-cert";

/// The media type this plane states a refusal in: a refusal on this wire carries no SCEP
/// message, only a sentence for whoever is reading the response by hand.
const TEXT_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// The `operation` query parameter naming the request for the CA's own certificate. The
/// names on this wire are spelled exactly as RFC 8894 spells them, so they are compared
/// exactly.
const GET_CA_CERT: &str = "GetCACert";

/// The query parameter that names which SCEP operation a request performs.
const OPERATION_PARAMETER: &str = "operation";

/// What an address naming no connector of this service is answered with: the URL names no
/// resource here.
const NOT_FOUND_STATUS: u16 = 404;

/// What a request this plane cannot serve is answered with — an `operation` it does not
/// implement, or none at all.
const BAD_REQUEST_STATUS: u16 = 400;

/// What a connector whose data plane is not up yet is answered with: the connector exists
/// and is not `ACTIVE`, so its SCEP endpoint does not serve.
const NOT_SERVING_STATUS: u16 = 503;

/// What a connector whose private CA cannot be read is answered with: the plane reached
/// Amazon Web Services Private CA and has no certificate to present.
const CA_UNAVAILABLE_STATUS: u16 = 502;

/// How large a PEM body this plane will decode a certificate out of.
///
/// `CertificateBody` is bounded by Amazon Web Services Private CA's own model at 32 KiB; the
/// ceiling here is that bound with room for the PEM framing, so the work one SCEP request
/// costs is bounded whatever the answer carries.
const MAX_CA_CERTIFICATE_BYTES: usize = 64 * 1024;

/// The PEM label of an X.509 certificate (RFC 7468 section 5), which is the only block this
/// plane takes a certificate out of.
const PEM_LABEL_CERTIFICATE: &str = "CERTIFICATE";

/// One connector's SCEP data plane, as the one [`ErasedOperation`] the service holds for it.
///
/// Owns the shared engine exactly as a generated operation does, so every answer is computed
/// from the SAME per-service store the control plane's handlers write: a connector's plane
/// serves only while that connector's own record says it is `ACTIVE`, and one
/// `DeleteConnector` removed serves nothing.
pub struct ScepDataPlane<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> ScepDataPlane<Eng> {
    /// Build the plane from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ScepDataPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { self.serve(req, ctx).await })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // A SCEP-enabled device configured with a connector's `Endpoint` presents no AWS
        // credential and holds none to sign with: SCEP is the device's own protocol, not an
        // AWS API. This plane therefore declares no scheme and serves the request however it
        // arrived — it is not a modelled operation, so no model states one for it, and it
        // claims only an address this service published.
        Some(&[])
    }
}

impl<Eng: Send + Sync + 'static> ScepDataPlane<Eng> {
    /// Whether this plane serves `req`, by the one rule the claim uses ([`claims_at`]).
    pub fn claims(&self, req: &RtRequest) -> bool {
        claims_at(req.uri(), addressed_host_of_request(req).as_deref())
    }

    /// Serve one SCEP request to a published connector endpoint.
    ///
    /// The address names the connector and the `operation` parameter names the interaction —
    /// which is how a SCEP server is addressed; there is no operation name on this wire.
    async fn serve(&self, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        let Some(addressed) = addressed_connector(req) else {
            return refused(NOT_FOUND_STATUS, "This URL names no SCEP connector.");
        };
        let connector_arn = addressed.connector_arn();

        // What the connector record says, read under the lock and carried out of it, so the
        // private CA is read with no lock held.
        let serving = {
            let Some(store) = state::service_state(&*self.engine) else {
                return refused(
                    CA_UNAVAILABLE_STATUS,
                    "The Connector for SCEP connector store is unavailable.",
                );
            };
            let mut store = store.lock();
            // Serving a connector's SCEP plane means serving it from the connector's own
            // record, up to date with the clock: the `CREATING` -> `ACTIVE` transition the
            // create owed it was due at this instant whether or not anybody called, which is
            // what makes a client that waited for activation able to enrol.
            store.settle(SystemTime::now());
            let Some(connector) = store.connector(&addressed.region, &connector_arn) else {
                return refused(NOT_FOUND_STATUS, "This URL names no SCEP connector.");
            };
            connector
                .is_active()
                .then(|| connector.certificate_authority_arn.clone())
        };
        let Some(certificate_authority_arn) = serving else {
            return refused(
                NOT_SERVING_STATUS,
                "This SCEP connector is not active, so its endpoint does not serve yet.",
            );
        };

        match operation_of(req.uri()) {
            Some(GET_CA_CERT) => {
                self.get_ca_cert(ctx, &addressed.region, &certificate_authority_arn)
                    .await
            }
            // Every other SCEP operation — `GetCACaps`, `PKIOperation` and anything else a
            // client names — is one this plane does not implement. A SCEP client reads the
            // error as the capability being absent, and nothing here issues a certificate.
            _ => refused(
                BAD_REQUEST_STATUS,
                "This SCEP endpoint serves only the GetCACert operation.",
            ),
        }
    }

    /// `GetCACert`: the certificate of the private CA this connector links, as the DER
    /// certificate RFC 8894 answers with.
    ///
    /// The certificate comes from Amazon Web Services Private CA's OWN read
    /// (`GetCertificateAuthorityCertificate`) rather than from anything this service stored,
    /// so what a device enrols against is the certificate that service holds right now.
    async fn get_ca_cert(
        &self,
        ctx: &OperationContext<'_>,
        region: &str,
        certificate_authority_arn: &str,
    ) -> RtResponse {
        let answer = private_ca_client(ctx, region)
            .get_certificate_authority_certificate()
            .certificate_authority_arn(certificate_authority_arn)
            .send()
            .await;
        let certificate = match answer {
            Ok(answer) => answer.certificate,
            Err(error) => {
                ::tracing::warn!(
                    certificate_authority = %certificate_authority_arn,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "the connector's private CA did not answer with its certificate"
                );
                return refused(
                    CA_UNAVAILABLE_STATUS,
                    "The private certificate authority this connector links holds no \
                     certificate to present.",
                );
            }
        };
        let Some(der) = certificate
            .as_deref()
            .filter(|pem| pem.len() <= MAX_CA_CERTIFICATE_BYTES)
            .and_then(certificate_der)
        else {
            return refused(
                CA_UNAVAILABLE_STATUS,
                "The private certificate authority this connector links holds no certificate \
                 to present.",
            );
        };
        answered(CA_CERTIFICATE_CONTENT_TYPE, der)
    }
}

/// The DER of the single X.509 certificate a PEM body frames, or `None` when the body is not
/// one well-formed certificate block.
///
/// The framing is read by the `pem` crate rather than by anything written here: this decodes
/// a body on a request path, where a hand-rolled reader is where unbounded allocation and
/// malformed-input defects live.
fn certificate_der(body: &str) -> Option<Vec<u8>> {
    let block = ::pem::parse(body).ok()?;
    (block.tag() == PEM_LABEL_CERTIFICATE && !block.contents().is_empty())
        .then(|| block.contents().to_vec())
}

/// The Amazon Web Services Private CA client this plane reads a connector's CA certificate
/// through: the AWS SDK driven over the emulator's own in-process transport, signing in the
/// emulator's name — Connector for SCEP reads the CA it was given with its own service
/// principal, not with the device's credential, and a SCEP device presents none.
///
/// NOTHING here opens a socket or leaves the host: the transport is
/// `ctx.callback().http_client()`, the emulator's in-process channel into its own services,
/// and the endpoint is only the address that channel resolves the private CA by. The
/// credentials are the emulator's own and are used to sign this one call; they are never
/// returned or logged.
fn private_ca_client(
    ctx: &OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_acmpca::Client {
    let config = ::aws_sdk_acmpca::Config::builder()
        .behavior_version(::aws_sdk_acmpca::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_acmpca::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://{PRIVATE_CA_ENDPOINT_PREFIX}.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client is
        // not given. The same goes for caching the identity: the credentials are handed over
        // directly.
        .retry_config(::aws_sdk_acmpca::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_acmpca::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_acmpca::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_acmpca::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_acmpca::Client::from_conf(config)
}

/// The endpoint prefix the in-process channel resolves Amazon Web Services Private CA by,
/// which is also the service segment of every `CertificateAuthorityArn` this service accepts
/// (`^arn:aws(-[a-z]+)*:acm-pca:…`).
const PRIVATE_CA_ENDPOINT_PREFIX: &str = "acm-pca";

/// The SCEP operation a request URI names, or `None` when it names none.
/// The SCEP operation a request URI names, or `None` when it names none.
///
/// Read with the tree's own query-parameter reading (`service_common::query_parameter`),
/// so this plane writes no second rule for how a query is split.
fn operation_of(uri: &str) -> Option<&str> {
    ::service_common::query_parameter(request_query(uri), OPERATION_PARAMETER)
}

/// Which connector `req` addresses, or `None` when it addresses none.
///
/// The two addressings are the ones this service publishes
/// ([`crate::state::announced_connector_endpoint`]): the reserved path on the emulator's own
/// endpoint, read first because it names this plane explicitly whatever the host is, and the
/// published SCEP host a client that resolves AWS's own hostnames holds.
fn addressed_connector(req: &RtRequest) -> Option<AddressedConnector> {
    state::parse_connector_plane_path(::service_common::request_path(req.uri())).or_else(|| {
        addressed_host_of_request(req)
            .as_deref()
            .and_then(state::connector_addressed_by_host)
    })
}

/// Whether the buffered request parts `Service::can_handle` is handed address this plane,
/// read by the same rule [`ScepDataPlane::claims`] applies, so the claim and the answer
/// cannot disagree about which requests this plane serves.
pub fn claims_parts(parts: &http::request::Parts) -> bool {
    claims_at(
        &parts.uri.to_string(),
        addressed_host_of_parts(parts).as_deref(),
    )
}

/// The one reading of that claim: this plane's reserved path, whatever the host (which is how
/// it is addressed on the emulator's own endpoint), else a published SCEP host, whatever the
/// path — a request to either is this service's to answer, including one whose address names
/// no connector it holds, which is answered here rather than by the front door's "unknown
/// service".
fn claims_at(uri: &str, host: Option<&str>) -> bool {
    state::addresses_connector_plane(::service_common::request_path(uri))
        || host
            .and_then(state::connector_addressed_by_host)
            .is_some()
}

/// The host a runtime-API request addressed, by the tree's one reading of that question
/// (`service_common::data_plane_addressing::addressed_host`): the `Host` header, else the
/// authority an absolute-form URI carries.
fn addressed_host_of_request(req: &RtRequest) -> Option<String> {
    ::service_common::data_plane_addressing::addressed_host(req.uri(), req.headers().get("host"))
}

/// The host a buffered request addressed, by that same reading — the `Host` header an HTTP/1
/// request carries, else the authority an HTTP/2 request carries on its URI (`http` parses
/// `:authority` onto the URI rather than exposing it as a header).
fn addressed_host_of_parts(parts: &http::request::Parts) -> Option<String> {
    ::service_common::data_plane_addressing::addressed_host(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|host| host.to_str().ok()),
    )
}

/// The query component of a request URI: everything after the first `?`, which is what
/// `service_common::query_parameter` reads a parameter out of. The tree carries no shared
/// reading of this half, only of the path (`service_common::request_path`).
fn request_query(uri: &str) -> &str {
    match uri.split_once('?') {
        Some((_path, query)) => query,
        None => "",
    }
}

/// One refusal on this wire: the status and the sentence that says why, as fixed text — a
/// refusal here quotes nothing a caller sent.
fn refused(status: u16, message: &str) -> RtResponse {
    response(status, TEXT_CONTENT_TYPE, format!("{message}\n").into_bytes())
}

/// One successful answer on this wire: the media type and the bytes.
fn answered(content_type: &str, body: Vec<u8>) -> RtResponse {
    response(200, content_type, body)
}

/// One response on this wire: the status, the media type, the body and its length, so a
/// client can read it without the connection closing.
fn response(status: u16, content_type: &str, body: Vec<u8>) -> RtResponse {
    let length = body.len();
    let mut answer = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::from(body),
    );
    let headers = answer.headers_mut();
    headers.insert("content-type", content_type.to_string());
    headers.insert("content-length", length.to_string());
    answer
}
