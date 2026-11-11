// SPDX-License-Identifier: CC0-1.0

//! The ADDRESS an endpoint publishes, and what this emulator answers there.
//!
//! RULE: `domainName` is "the domain name assigned to the endpoint" — the address the
//! endpoint is SERVED at, which a client reads off the endpoint and issues its HTTP
//! request to — so it is an address this emulator answers on, and the endpoint's own
//! requests are served there: a request for an endpoint that is not serving traffic is a
//! `404`, an `IamAuth` endpoint ("requests ... must be signed with Signature Version 4")
//! refuses a request presenting no SigV4 credential with Lambda's `403 Forbidden`
//! document, and an `ApplicationManaged` endpoint ("your application handles
//! authorization") refuses nothing of its own.
//!
//! The published address is read off the wire (`CreateWebFunction` and
//! `GetWebFunctionEndpoint`), and the decision the core's listener hands every request at
//! that address is driven directly
//! (`state::endpoint_plane::serve_request`) — the one decision that listener serves. The
//! rows span the `authType` values, how a client may present a credential, and the
//! endpoint's lifecycle state.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_lambdaweb_lambdaweb::state::endpoint_plane::serve_request;
use service_com_amazonaws_lambdaweb_lambdaweb::state::{service_state, PROVISIONING_DELAY};

use super::web_function_lifecycle::{build_config, role_arn, Emulator};

/// The region every request here signs with.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header, as a signed client sends one.
const SIGNED: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/lambda/\
                      aws4_request, SignedHeaders=host, Signature=deadbeef";

/// How long a test waits for an endpoint's clock-owed `Pending -> Active` transition: it
/// serves a revision, so it is ready one span after that revision is, plus slack.
fn endpoint_settled() -> Duration {
    PROVISIONING_DELAY + PROVISIONING_DELAY + Duration::from_millis(400)
}

/// Create a function with one endpoint of `auth_type` through the shared harness, and
/// report the `domainName` the create published for it.
async fn create_with_endpoint(emulator: &Emulator, name: &str, auth_type: &str) -> String {
    let created = emulator
        .send(
            "PUT",
            "/web-functions",
            REGION,
            Some(json!({
                "functionName": name,
                "revisionConfig": {
                    "buildConfig": build_config("nodejs24.x", "demo-bucket", "app.zip", None),
                    "serviceConfig": { "executionRoleArn": role_arn(name) },
                },
                "endpointConfig": {
                    "endpointName": "ep",
                    "endpointType": "HomeRegion",
                    "authType": auth_type,
                },
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "CreateWebFunction {name}: expected 202, got {} {:?}",
        created.status, created.body
    );
    created
        .field("endpoint")
        .and_then(|endpoint| endpoint.get("domainName"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One request to a published endpoint, answered by the decision the core's listener
/// serves at that address — read out of the ONE store the emulator's services share.
fn request_endpoint(
    emulator: &Emulator,
    function_name: &str,
    endpoint_name: &str,
    uri: &str,
    headers: &[(&str, &str)],
) -> ::http::Response<Vec<u8>> {
    let mut builder = ::http::Request::builder().method("GET").uri(uri);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(Vec::new()).expect("a well-formed request");
    let state = service_state(&**emulator.engine()).expect("this service's store");
    serve_request(&state, function_name, endpoint_name, &request)
}

/// One row of the request table: the case, the request URI, the headers it carries, and
/// the status the published address answers it with.
type RequestRow<'a> = (&'a str, &'a str, &'a [(&'a str, &'a str)], u16);

/// The body of an answer from a published address, as text.
fn body_text(response: &::http::Response<Vec<u8>>) -> String {
    String::from_utf8_lossy(response.body()).to_string()
}

/// RULE: the published `domainName` is an address this emulator answers on — a `host:port`
/// whose host is the loopback interface the emulator's listeners bind — and it is the same
/// value every reader of the endpoint reports. Two endpoints never publish one address.
#[tokio::test]
async fn a_published_domain_name_is_an_address_the_emulator_answers_on() {
    let emulator = Emulator::new();
    // The rows span the `authType` values, since the address is published for both.
    let mut published = Vec::new();
    for (case, auth_type) in [
        ("iam-authorized", "IamAuth"),
        ("application-managed", "ApplicationManaged"),
    ] {
        let name = format!("published-{case}");
        let domain_name = create_with_endpoint(&emulator, &name, auth_type).await;
        let host = service_common::data_plane_addressing::host_only(&domain_name);
        let port = service_common::data_plane_addressing::addressed_port(Some(&domain_name));
        assert!(
            emulator_server::is_loopback_host(host) && port.is_some_and(|port| port > 0),
            "case {case:?}: expected an address this emulator answers on, got {domain_name:?}"
        );
        assert!(
            !service_common::data_plane_addressing::names_a_service(&domain_name),
            "case {case:?}: a published address must not be a host that names an AWS \
             service, got {domain_name:?}"
        );

        // Every reader of the endpoint reports the one stored address.
        let read = emulator
            .send(
                "GET",
                &format!("/web-functions/{name}/endpoints/ep"),
                REGION,
                None,
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetWebFunctionEndpoint expected 200, got {} {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.string("domainName").as_deref(),
            Some(domain_name.as_str()),
            "case {case:?}: the read reports another address, got {:?}",
            read.body
        );
        published.push(domain_name);
    }
    assert_ne!(
        published[0], published[1],
        "two endpoints must not publish one address, got {published:?}"
    );
}

/// RULE: at the published address, an `IamAuth` endpoint refuses a request presenting no
/// SigV4 credential with Lambda's `403 Forbidden` document, and serves one that presents a
/// credential; an `ApplicationManaged` endpoint refuses nothing of its own.
///
/// What an authorized request is answered with is the web function's OWN handler response,
/// and this emulator runs no web function code, so it answers `501` and synthesises
/// nothing in the handler's name.
#[tokio::test]
async fn the_published_address_applies_the_endpoints_auth_type() {
    let emulator = Emulator::new();
    for (case, auth_type) in [
        ("iam-authorized", "IamAuth"),
        ("application-managed", "ApplicationManaged"),
    ] {
        let name = format!("auth-{case}");
        create_with_endpoint(&emulator, &name, auth_type).await;
        // Until the endpoint is serving traffic, nothing is published at its address.
        let pending =
            request_endpoint(&emulator, &name, "ep", "/", &[("authorization", SIGNED)]);
        assert_eq!(
            pending.status().as_u16(),
            404,
            "case {case:?}: an endpoint that is not serving traffic publishes nothing, got \
             {}",
            body_text(&pending)
        );

        tokio::time::sleep(endpoint_settled()).await;

        // (case, the request URI, its headers, the status the address answers)
        let unsigned_status = if auth_type == "IamAuth" { 403 } else { 501 };
        let rows: &[RequestRow<'_>] = &[
            ("a signed GET", "/", &[("authorization", SIGNED)], 501),
            (
                "a presigned GET",
                "/?X-Amz-Credential=AKIDEXAMPLE&X-Amz-Signature=deadbeef",
                &[],
                501,
            ),
            ("an unsigned GET", "/", &[], unsigned_status),
            (
                "an unsigned GET deeper in the path",
                "/items/42?page=2",
                &[],
                unsigned_status,
            ),
            (
                "another authorization scheme",
                "/",
                &[("authorization", "Basic dXNlcjpwYXNz")],
                unsigned_status,
            ),
        ];
        for (request_case, uri, headers, expected) in rows {
            let answered = request_endpoint(&emulator, &name, "ep", uri, headers);
            assert_eq!(
                answered.status().as_u16(),
                *expected,
                "case {case:?}/{request_case:?}: expected {expected}, got {} with {:?}",
                answered.status(),
                body_text(&answered)
            );
            if *expected == 403 {
                assert_eq!(
                    body_text(&answered),
                    "{\"Message\":\"Forbidden\"}",
                    "case {case:?}/{request_case:?}: expected Lambda's Forbidden document"
                );
            }
        }
    }
}

/// RULE: the published address is a socket this emulator ANSWERS on — a client that
/// followed `domainName` and dialled it gets the endpoint's own answer over HTTP.
///
/// Dialled in the clear, which the address serves beside TLS (the scheme is negotiated
/// from the connection's first byte), so the test needs no certificate trust store to
/// prove the socket answers.
#[tokio::test]
async fn a_client_that_dials_the_published_address_is_answered_there() {
    let emulator = Emulator::new();
    let domain_name = create_with_endpoint(&emulator, "dialled-endpoint", "IamAuth").await;
    tokio::time::sleep(endpoint_settled()).await;

    // (case, the request's headers, the status line the address answers with)
    let rows: [(&str, &str, &str); 2] = [
        (
            "an unsigned GET",
            "",
            "HTTP/1.1 403 Forbidden",
        ),
        (
            "a signed GET",
            &format!("authorization: {SIGNED}\r\n"),
            "HTTP/1.1 501 Not Implemented",
        ),
    ];
    for (case, headers, expected) in rows {
        let answered = dial(&domain_name, headers).await;
        assert!(
            answered.starts_with(expected),
            "case {case:?}: expected the address to answer {expected:?}, got {answered:?}"
        );
    }
}

/// One HTTP/1.1 `GET /` to the published address `authority`, carrying `headers`, read
/// back as text.
///
/// The dial names the LOOPBACK interface itself: the address a published endpoint carries
/// is one of this emulator's own listeners (asserted above), so only its port is taken
/// from the published value and the host is this process's own loopback address. Nothing
/// outside this machine is reachable from here.
async fn dial(authority: &str, headers: &str) -> String {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let host = service_common::data_plane_addressing::host_only(authority);
    assert!(
        emulator_server::is_loopback_host(host),
        "the published address {authority} must be one of this emulator's own listeners"
    );
    let port = service_common::data_plane_addressing::addressed_port(Some(authority))
        .and_then(|port| u16::try_from(port).ok())
        .unwrap_or_else(|| panic!("the published address {authority} must name a port"));
    let mut socket = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
        .await
        .unwrap_or_else(|error| panic!("the published address {authority} must accept a connection: {error}"));
    let request = format!("GET / HTTP/1.1\r\nhost: {authority}\r\n{headers}connection: close\r\n\r\n");
    socket
        .write_all(request.as_bytes())
        .await
        .expect("the request is written");
    let mut read = Vec::new();
    socket
        .read_to_end(&mut read)
        .await
        .expect("the answer is read");
    String::from_utf8_lossy(&read).to_string()
}

/// RULE: the address belongs to the endpoint — once the endpoint is gone, nothing is
/// published there, and the listener it answered on is released.
#[tokio::test]
async fn a_deleted_endpoints_address_publishes_nothing() {
    let emulator = Emulator::new();
    let name = "address-lifetime";
    create_with_endpoint(&emulator, name, "ApplicationManaged").await;
    tokio::time::sleep(endpoint_settled()).await;
    let serving = request_endpoint(&emulator, name, "ep", "/", &[]);
    assert_eq!(
        serving.status().as_u16(),
        501,
        "an active endpoint answers at its address, got {}",
        body_text(&serving)
    );

    // The delete of the function takes its endpoints with it, so the address stops
    // answering and the listener it answered on is released by that delete.
    let deleted = emulator
        .send("DELETE", &format!("/web-functions/{name}"), REGION, None)
        .await;
    assert!(
        (200..300).contains(&deleted.status),
        "DeleteWebFunction expected a 2xx, got {} {:?}",
        deleted.status,
        deleted.body
    );
    let gone = request_endpoint(&emulator, name, "ep", "/", &[("authorization", SIGNED)]);
    assert_eq!(
        gone.status().as_u16(),
        404,
        "a deleted endpoint publishes nothing at its address, got {}",
        body_text(&gone)
    );
    let state = service_state(&**emulator.engine()).expect("this service's store");
    assert!(
        state.lock().endpoint_planes.is_empty(),
        "the listener of a deleted endpoint must be released"
    );
}
