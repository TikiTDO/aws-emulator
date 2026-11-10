// SPDX-License-Identifier: CC0-1.0

//! The address a connector PUBLISHES, and the SCEP data plane that answers at it.
//!
//! `Connector$Endpoint` is "the connector's HTTPS public SCEP URL" (smithy_query shape
//! Connector): a value whose whole purpose is that the caller DIALS it — a SCEP-enabled
//! device or an MDM system is configured with it and fetches the CA certificate from it.
//!
//! The RULE these cases assert, over a table rather than one recorded request: a URL this
//! service publishes for a caller to fetch addresses an endpoint THIS EMULATOR serves — the
//! emulator's own endpoint the request reached, with the connector in the path under the
//! reserved data-plane namespace, and the service's published SCEP host only for a caller
//! that resolves AWS's own hostnames — and a `GET <Endpoint>?operation=GetCACert` against
//! an ACTIVE connector answers 200 with the DER certificate of the private CA that
//! connector names, as Amazon Web Services Private CA itself holds it.
//!
//! So this suite mounts Amazon Web Services Private CA beside this service on ONE engine —
//! what the assembled emulator does — and activates its CA through that service's own
//! documented procedure, so the certificate the SCEP plane is checked against is the one
//! that service answers for rather than anything this suite seeded.
//!
//! Rows differ along what the rule generalises over: the shape of the address the create
//! request reached (an IPv4 literal with a port, a loopback name with another port, a
//! bracketed IPv6 literal, and one of AWS's own hostnames), the Region, whether the create
//! named a VPC endpoint, which of the two published addressings the SCEP request is sent
//! to, the connector's status, and the SCEP operation named.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use x509_cert::der::{Decode, DecodePem};

use service_com_amazonaws_pcaconnectorscep_pcaconnectorscep as scep;

use crate::harness::{self, label};

/// The account id every generated emulator presents, which both services mint ARNs in.
const ACCOUNT: &str = "000000000000";

/// The template that produces a ROOT CA's own certificate — step 2 of the documented
/// procedure for creating a root private CA.
const ROOT_CA_TEMPLATE: &str = "arn:aws:acm-pca:::template/RootCACertificate/V1";

/// The media type a single DER X.509 certificate is served as, which is what RFC 8894
/// answers `GetCACert` with for a CA holding one certificate.
const CA_CERTIFICATE_CONTENT_TYPE: &str = "application/x-x509-ca-cert";

/// Long enough that a transition either service owed is due, so the next call settles it.
fn wait_for_the_clock() {
    // A test may wait for a due instant; a handler may not. `tokio`'s timer is not in this
    // workspace's feature set, so the current-thread test runtime parks instead.
    std::thread::sleep(
        service_com_amazonaws_acmpca_acmprivateca::state::SETTLE_AFTER
            + Duration::from_millis(250),
    );
}

/// What one call answered: the HTTP status, the response headers and the raw body.
struct Answer {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    json: Value,
}

impl Answer {
    /// One member of the parsed JSON body, as a string.
    fn text(&self, member: &str, case: &str) -> String {
        self.json
            .get(member)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: no string {member:?} in {:?}", self.json))
            .to_string()
    }
}

/// An emulator holding this service AND Amazon Web Services Private CA over one shared
/// engine, reached at whichever address a case addresses it on.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signature verification is the emulator's own concern and is exercised elsewhere;
        // these cases are about this service's behaviour.
        router.set_request_authentication(false);
        router.register_service(scep::EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(Arc::clone(&engine)),
        );
        Self { router, engine }
    }

    /// One request this emulator dispatches, exactly as it arrived.
    async fn dispatch(&self, request: http::Request<bytes::Bytes>) -> Answer {
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response.into_body().to_vec();
        let json = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap_or(Value::Null)
        };
        Answer {
            status,
            content_type,
            body,
            json,
        }
    }

    /// One SIGNED control-plane call of this service, addressed at `authority` — the address
    /// the client reached this emulator on, which is what decides the address a published URL
    /// names.
    async fn connector_call(
        &self,
        authority: &str,
        region: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> Answer {
        let payload = match body {
            Some(value) => bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")),
            None => bytes::Bytes::new(),
        };
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{authority}{path}"))
            .header("host", authority)
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/pca-connector-scep/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .header("content-type", "application/json")
            .body(payload)
            .expect("build request");
        self.dispatch(request).await
    }

    /// One SIGNED private-CA call over that service's awsJson1_1 wire.
    async fn private_ca_call(&self, region: &str, operation: &str, body: &Value) -> Answer {
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://acm-pca.{region}.amazonaws.com/"))
            .header("host", format!("acm-pca.{region}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("ACMPrivateCA.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{region}/acm-pca/aws4_request, \
                     SignedHeaders=host, Signature=abc"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize body"),
            ))
            .expect("build request");
        self.dispatch(request).await
    }

    /// One private-CA call that must answer 200.
    async fn private_ca_ok(
        &self,
        case: &str,
        region: &str,
        operation: &str,
        body: &Value,
    ) -> Answer {
        let answer = self.private_ca_call(region, operation, body).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected {operation} to answer 200, got {} / {:?}",
            answer.status, answer.json
        );
        answer
    }

    /// One UNSIGNED fetch of a published URL, as a SCEP client sends it: the URL is dialled
    /// as it was published, with the `Host` its authority names and no AWS credential at all.
    async fn fetch(&self, url: &str) -> Answer {
        let authority = url
            .split_once("://")
            .map(|(_scheme, rest)| rest.split(['/', '?']).next().unwrap_or(rest))
            .expect("a published URL carries a scheme");
        let request = http::Request::builder()
            .method("GET")
            .uri(url)
            .header("host", authority)
            .body(bytes::Bytes::new())
            .expect("build request");
        self.dispatch(request).await
    }

    /// A root private CA in `region`, driven all the way to ACTIVE through that service's own
    /// documented procedure, answering its ARN and the certificate it was activated with.
    async fn active_private_ca(&self, case: &str, region: &str, common_name: &str) -> (String, String) {
        let created = self
            .private_ca_ok(
                case,
                region,
                "CreateCertificateAuthority",
                &json!({
                    "CertificateAuthorityConfiguration": {
                        "KeyAlgorithm": "RSA_2048",
                        "SigningAlgorithm": "SHA256WITHRSA",
                        "Subject": {"CommonName": common_name},
                    },
                    "CertificateAuthorityType": "ROOT",
                }),
            )
            .await;
        let ca_arn = created.text("CertificateAuthorityArn", case);
        wait_for_the_clock();

        let csr = self
            .private_ca_ok(
                case,
                region,
                "GetCertificateAuthorityCsr",
                &json!({"CertificateAuthorityArn": ca_arn}),
            )
            .await
            .text("Csr", case);
        let issued = self
            .private_ca_ok(
                case,
                region,
                "IssueCertificate",
                &json!({
                    "CertificateAuthorityArn": ca_arn,
                    "Csr": aws_smithy_types::base64::encode(csr.as_bytes()),
                    "SigningAlgorithm": "SHA256WITHRSA",
                    "TemplateArn": ROOT_CA_TEMPLATE,
                    "Validity": {"Type": "YEARS", "Value": 10},
                }),
            )
            .await;
        let certificate_arn = issued.text("CertificateArn", case);
        wait_for_the_clock();

        let certificate = self
            .private_ca_ok(
                case,
                region,
                "GetCertificate",
                &json!({
                    "CertificateAuthorityArn": ca_arn,
                    "CertificateArn": certificate_arn,
                }),
            )
            .await
            .text("Certificate", case);
        self.private_ca_ok(
            case,
            region,
            "ImportCertificateAuthorityCertificate",
            &json!({
                "CertificateAuthorityArn": ca_arn,
                "Certificate": aws_smithy_types::base64::encode(certificate.as_bytes()),
            }),
        )
        .await;

        // The certificate the CA now HOLDS, read through its own front door: that read is
        // what the SCEP plane answers from, so it is what the plane is compared against.
        let held = self
            .private_ca_ok(
                case,
                region,
                "GetCertificateAuthorityCertificate",
                &json!({"CertificateAuthorityArn": ca_arn}),
            )
            .await
            .text("Certificate", case);
        (ca_arn, held)
    }

    /// Create a connector through a request addressed at `authority`, settle it, and answer
    /// its ARN.
    async fn active_connector(
        &self,
        case: &str,
        authority: &str,
        region: &str,
        request: Value,
    ) -> String {
        let created = self
            .connector_call(authority, region, "POST", "/connectors", Some(request))
            .await;
        assert_eq!(
            created.status, 202,
            "case {case:?}: expected 202 from CreateConnector, got {} / {:?}",
            created.status, created.json
        );
        let arn = created.text("ConnectorArn", case);
        harness::release_pending(&self.engine, &arn);
        arn
    }

    /// The `Endpoint` `GetConnector` reports for `arn`, read at `authority`.
    async fn reported_endpoint(&self, case: &str, authority: &str, region: &str, arn: &str) -> String {
        let read = self
            .connector_call(
                authority,
                region,
                "GET",
                &format!("/connectors/{}", label(arn)),
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetConnector, got {} / {:?}",
            read.status, read.json
        );
        read.json["Connector"]["Endpoint"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no string Endpoint in {:?}", read.json))
            .to_string()
    }

    /// The `Endpoint` `ListConnectors` reports for `arn`, read at `authority`.
    async fn listed_endpoint(&self, case: &str, authority: &str, region: &str, arn: &str) -> String {
        let page = self
            .connector_call(authority, region, "GET", "/connectors", None)
            .await;
        assert_eq!(
            page.status, 200,
            "case {case:?}: expected 200 from ListConnectors, got {} / {:?}",
            page.status, page.json
        );
        let summaries = page.json["Connectors"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no Connectors array in {:?}", page.json))
            .clone();
        let summary = summaries
            .iter()
            .find(|entry| entry["Arn"] == json!(arn))
            .unwrap_or_else(|| panic!("case {case:?}: no ConnectorSummary for {arn:?}"));
        summary["Endpoint"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no string Endpoint in {summary:?}"))
            .to_string()
    }
}

/// The connector id inside a connector ARN.
fn connector_id_of(arn: &str) -> &str {
    arn.rsplit_once('/')
        .unwrap_or_else(|| panic!("expected a connector ARN ending `connector/<uuid>`, got {arn:?}"))
        .1
}

/// A CA ARN of `region` naming no CA this emulator holds, in the form
/// `CertificateAuthorityArn`'s own pattern requires — for a case that needs a connector
/// without needing a private CA behind it.
fn absent_ca_arn(region: &str) -> String {
    format!("arn:aws:acm-pca:{region}:{ACCOUNT}:certificate-authority/11111111-2222-3333-4444-555555555555")
}

/// The subject and serial number of a DER X.509 certificate, as the two facts that identify
/// it whatever framing it arrived in.
fn identity_of_der(der: &[u8], case: &str) -> (String, String) {
    let certificate = x509_cert::Certificate::from_der(der)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a DER X.509 certificate: {error}"));
    (
        certificate.tbs_certificate.subject.to_string(),
        hex::encode(certificate.tbs_certificate.serial_number.as_bytes()),
    )
}

/// The same two facts out of a PEM certificate.
fn identity_of_pem(text: &str, case: &str) -> (String, String) {
    let certificate = x509_cert::Certificate::from_pem(text)
        .unwrap_or_else(|error| panic!("case {case:?}: expected a PEM X.509 certificate: {error}"));
    (
        certificate.tbs_certificate.subject.to_string(),
        hex::encode(certificate.tbs_certificate.serial_number.as_bytes()),
    )
}

/// One row of the announcement table: the address the create request reached, and whether
/// that address is one of the emulator's own.
#[derive(Debug)]
struct AddressRow {
    name: &'static str,
    authority: &'static str,
    region: &'static str,
    vpc_endpoint_id: Option<&'static str>,
    emulators_own: bool,
}

/// The table: every shape the emulator's own endpoint takes, and one of AWS's own hostnames.
fn address_rows() -> Vec<AddressRow> {
    vec![
        AddressRow {
            name: "an IPv4 loopback literal with a port",
            authority: "127.0.0.1:9297",
            region: "eu-west-1",
            vpc_endpoint_id: None,
            emulators_own: true,
        },
        AddressRow {
            name: "a loopback name with another port",
            authority: "localhost:14100",
            region: "us-west-2",
            vpc_endpoint_id: None,
            emulators_own: true,
        },
        AddressRow {
            name: "a bracketed IPv6 loopback literal",
            authority: "[::1]:9297",
            region: "ap-southeast-2",
            vpc_endpoint_id: Some("vpce-0123abcd"),
            emulators_own: true,
        },
        AddressRow {
            name: "the emulator's own endpoint, connector restricted to a VPC endpoint",
            authority: "127.0.0.1:35059",
            region: "eu-central-1",
            vpc_endpoint_id: Some("vpce-0123abcd456789abc"),
            emulators_own: true,
        },
        AddressRow {
            name: "one of AWS's own hostnames",
            authority: "pca-connector-scep.us-east-2.amazonaws.com",
            region: "us-east-2",
            vpc_endpoint_id: None,
            emulators_own: false,
        },
        AddressRow {
            name: "one of AWS's own hostnames, connector restricted to a VPC endpoint",
            authority: "pca-connector-scep.eu-west-3.amazonaws.com",
            region: "eu-west-3",
            vpc_endpoint_id: Some("vpce-fedcba9887654321f"),
            emulators_own: false,
        },
    ]
}

/// The `Endpoint` a connector publishes is an address the caller can reach: the emulator's
/// own endpoint the request came in on, with the connector in the path, and the published
/// SCEP host only for a caller that resolves AWS's own hostnames. Every reader reports it
/// and it does not move between reads.
#[tokio::test]
async fn the_published_endpoint_is_an_address_of_the_emulator_the_caller_reached() {
    for row in address_rows() {
        let emulator = Emulator::new();
        let mut request = json!({"CertificateAuthorityArn": absent_ca_arn(row.region)});
        if let Some(vpce) = row.vpc_endpoint_id {
            request["VpcEndpointId"] = json!(vpce);
        }
        let arn = emulator
            .active_connector(row.name, row.authority, row.region, request)
            .await;
        let id = connector_id_of(&arn).to_string();
        let endpoint = emulator
            .reported_endpoint(row.name, row.authority, row.region, &arn)
            .await;

        let expected = if row.emulators_own {
            scep::state::local_connector_endpoint(row.authority, row.region, &id)
        } else {
            scep::state::connector_endpoint(row.region, &id, row.vpc_endpoint_id)
        };
        assert_eq!(
            endpoint, expected,
            "case {row:?}: expected the Endpoint {expected:?}, got {endpoint:?}"
        );
        assert!(
            endpoint.starts_with("https://") && endpoint.contains(&id),
            "case {row:?}: the Endpoint must be an HTTPS URL naming the connector {id:?}, got \
             {endpoint:?}"
        );
        if row.emulators_own {
            assert!(
                endpoint.starts_with(&format!("https://{}/", row.authority)),
                "case {row:?}: a caller that reached the emulator's own endpoint must be given \
                 an address on that endpoint, got {endpoint:?}"
            );
            assert!(
                !endpoint.contains("amazonaws.com"),
                "case {row:?}: the published Endpoint must not name a host the emulator does \
                 not serve, got {endpoint:?}"
            );
        }

        // Every reader agrees, and the address does not move between reads.
        let again = emulator
            .reported_endpoint(row.name, row.authority, row.region, &arn)
            .await;
        let listed = emulator
            .listed_endpoint(row.name, row.authority, row.region, &arn)
            .await;
        assert_eq!(
            (again.as_str(), listed.as_str()),
            (endpoint.as_str(), endpoint.as_str()),
            "case {row:?}: every reader must report the one Endpoint {endpoint:?}, got \
             {again:?} and {listed:?}"
        );
    }
}

/// One row of the round-trip table: the Region the CA and the connector live in, the access
/// path the create chose, and which of the two published addressings the SCEP request uses.
#[derive(Debug)]
struct RoundTripRow {
    name: &'static str,
    authority: &'static str,
    region: &'static str,
    common_name: &'static str,
    vpc_endpoint_id: Option<&'static str>,
    /// Whether the SCEP request is sent to the PUBLISHED AWS SCEP host instead of the
    /// address the connector was announced on — the addressing a client that resolves AWS's
    /// own hostnames holds.
    through_published_host: bool,
}

/// The table: two Regions, both access paths, and both published addressings.
fn round_trip_rows() -> Vec<RoundTripRow> {
    vec![
        RoundTripRow {
            name: "a public connector dialled at the address it was announced on",
            authority: "127.0.0.1:9297",
            region: "eu-west-1",
            common_name: "scep-root-one",
            vpc_endpoint_id: None,
            through_published_host: false,
        },
        RoundTripRow {
            name: "a VPC-endpoint connector in another Region, same addressing",
            authority: "localhost:14100",
            region: "us-west-2",
            common_name: "scep-root-two",
            vpc_endpoint_id: Some("vpce-0123abcd"),
            through_published_host: false,
        },
        RoundTripRow {
            name: "a public connector dialled at the SCEP host AWS publishes",
            authority: "127.0.0.1:9297",
            region: "ap-south-1",
            common_name: "scep-root-three",
            vpc_endpoint_id: None,
            through_published_host: true,
        },
    ]
}

/// `GET <Endpoint>?operation=GetCACert` against an ACTIVE connector answers 200 with the DER
/// certificate of the private CA that connector names — the same subject and serial number
/// Amazon Web Services Private CA answers for that CA.
#[tokio::test]
async fn get_ca_cert_answers_the_certificate_of_the_connectors_private_ca() {
    for row in round_trip_rows() {
        let emulator = Emulator::new();
        let (ca_arn, ca_certificate) = emulator
            .active_private_ca(row.name, row.region, row.common_name)
            .await;

        let mut request = json!({"CertificateAuthorityArn": ca_arn});
        if let Some(vpce) = row.vpc_endpoint_id {
            request["VpcEndpointId"] = json!(vpce);
        }
        let arn = emulator
            .active_connector(row.name, row.authority, row.region, request)
            .await;
        let endpoint = emulator
            .reported_endpoint(row.name, row.authority, row.region, &arn)
            .await;
        let dialled = if row.through_published_host {
            scep::state::connector_endpoint(row.region, connector_id_of(&arn), row.vpc_endpoint_id)
        } else {
            endpoint.clone()
        };

        let answer = emulator.fetch(&format!("{dialled}?operation=GetCACert")).await;
        assert_eq!(
            answer.status, 200,
            "case {row:?}: expected 200 from GetCACert at {dialled:?}, got {} / {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type, CA_CERTIFICATE_CONTENT_TYPE,
            "case {row:?}: expected the DER certificate media type, got {:?}",
            answer.content_type
        );
        assert!(
            !answer.body.is_empty(),
            "case {row:?}: expected a non-empty certificate body"
        );
        let served = identity_of_der(&answer.body, row.name);
        let held = identity_of_pem(&ca_certificate, row.name);
        assert_eq!(
            served, held,
            "case {row:?}: expected the certificate of {ca_arn:?} (subject, serial) {held:?}, \
             got {served:?}"
        );
    }
}

/// A connector that is not ACTIVE, an address naming no connector, and a SCEP operation this
/// plane does not serve are each answered a non-2xx — never a certificate.
#[tokio::test]
async fn the_scep_plane_refuses_what_it_cannot_serve() {
    let region = "eu-west-1";
    let authority = "127.0.0.1:9297";
    let emulator = Emulator::new();

    // One connector that never reaches ACTIVE, held mid-creation, and one that does.
    let pending = {
        let created = emulator
            .connector_call(
                authority,
                region,
                "POST",
                "/connectors",
                Some(json!({"CertificateAuthorityArn": absent_ca_arn(region)})),
            )
            .await;
        let arn = created.text("ConnectorArn", "a connector still being created");
        harness::hold_pending(&emulator.engine, &arn);
        arn
    };
    let pending_endpoint = emulator
        .reported_endpoint("a connector still being created", authority, region, &pending)
        .await;

    let active = emulator
        .active_connector(
            "an active connector",
            authority,
            region,
            json!({"CertificateAuthorityArn": absent_ca_arn(region)}),
        )
        .await;
    let active_endpoint = emulator
        .reported_endpoint("an active connector", authority, region, &active)
        .await;
    let absent_endpoint = scep::state::local_connector_endpoint(
        authority,
        region,
        "99999999-8888-7777-6666-555544443333",
    );

    // The rows: what is wrong with the request, the URL it is sent to, and the status that
    // says so. An unserved operation is a bad request; an address that names no connector is
    // not found; a connector whose plane is not up is unavailable; and a connector whose
    // private CA holds no certificate has none to present.
    let rows: &[(&str, String, u16)] = &[
        (
            "an operation this plane does not serve",
            format!("{active_endpoint}?operation=GetCACaps"),
            400,
        ),
        (
            "an enrolment nothing here performs",
            format!("{active_endpoint}?operation=PKIOperation"),
            400,
        ),
        (
            "no operation at all",
            active_endpoint.clone(),
            400,
        ),
        (
            "an address naming no connector",
            format!("{absent_endpoint}?operation=GetCACert"),
            404,
        ),
        (
            "a path on this plane naming no connector",
            format!("https://{authority}/_aws/scep/{region}?operation=GetCACert"),
            404,
        ),
        (
            "a connector whose creation has not settled",
            format!("{pending_endpoint}?operation=GetCACert"),
            503,
        ),
        (
            "a connector whose private CA holds no certificate",
            format!("{active_endpoint}?operation=GetCACert"),
            502,
        ),
    ];

    for (case, url, expected) in rows {
        let answer = emulator.fetch(url).await;
        assert_eq!(
            answer.status, *expected,
            "case {case:?}: expected {expected} from {url:?}, got {} / {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_ne!(
            answer.content_type, CA_CERTIFICATE_CONTENT_TYPE,
            "case {case:?}: a refusal must not be served as a certificate"
        );
    }
}
