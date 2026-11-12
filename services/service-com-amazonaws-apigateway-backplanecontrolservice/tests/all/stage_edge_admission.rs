// SPDX-License-Identifier: CC0-1.0

//! What the `execute-api` EDGE does to a request and to the response it answers with,
//! independently of which integration serves it: the payload it ADMITS, and the content
//! coding it serves the answer under.
//!
//! Both are the endpoint's own rules rather than a method's, so every case here drives the
//! real wire path — an HTTP request through the multi-service router onto the api's own
//! `{rest-api-id}.execute-api.{region}.amazonaws.com` host — and asserts the rule over a
//! table of inputs:
//!
//! * a request whose payload is over the documented, non-adjustable REST API payload limit
//!   is refused `413` with the integration never performed, over a table of body sizes
//!   spanning the limit (well under, one byte under, exactly at it, one byte over, well
//!   over);
//! * a response at least `minimumCompressionSize` bytes long is served under the content
//!   coding the client accepted, and the bytes it carries really are that coding's, over a
//!   table spanning the threshold, the `Accept-Encoding` value and an api that enables no
//!   compression at all.

use std::collections::HashMap;
use std::io::Read;

use bytes::Bytes;
use serde_json::{json, Value};

use crate::execute_api_data_plane::{
    deploy, integration_response, method_response, put_integration, put_method,
};
use crate::rest_api_lifecycle::{resource_under, string_member, Gateway};

/// The region the invoke host is addressed in; the host's region label is not matched
/// against a list, so one region drives the data plane.
const REGION: &str = "us-east-1";

/// The stage every case deploys to.
const STAGE: &str = "edge";

/// The documented payload limit of a request to a REST API: the "Payload size" quota, which
/// API Gateway lists among the quotas that cannot be increased. Written here rather than
/// read off the crate so the test states the published number independently of the code.
const PAYLOAD_LIMIT: usize = 10_485_760;

/// One answered invoke: the status, every response header, and the body as BYTES — bytes
/// because a compressed body is not text.
pub(crate) struct Answered {
    pub(crate) status: u16,
    headers: HashMap<String, String>,
    pub(crate) body: Vec<u8>,
}

impl Answered {
    /// One response header, lowercased as HTTP names compare.
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }

    /// The body as text, for the cases whose answer is a JSON document.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Issue one unsigned request to `api`'s invoke endpoint at `path`, carrying `headers` and
/// `body` — what a client of a published invoke URL sends.
///
/// `pub(crate)` for the sibling `gateway_response_wire_rendering` suite, which asserts what
/// the edge answers a REFUSED request with and needs the same raw reading of the response.
pub(crate) async fn invoke_bytes(
    gateway: &Gateway,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answered {
    let host = format!("{api}.execute-api.{REGION}.amazonaws.com");
    let mut builder = http::Request::builder()
        .method(verb)
        .uri(format!("https://{host}{path}"))
        .header("host", &host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(body)
        .expect("the invoke request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_ascii_lowercase(), value.to_owned()))
        })
        .collect();
    Answered {
        status,
        headers,
        body: response.into_body().to_vec(),
    }
}

/// `CreateRestApi` with `members` beside the name, answering the api id and its root
/// resource id — the one create here, so a case that enables compression and one that does
/// not stand their api up the same way.
async fn create_api_with(gateway: &Gateway, name: &str, members: Value) -> (String, String) {
    let mut body = json!({ "name": name });
    for (member, value) in members.as_object().into_iter().flatten() {
        body[member] = value.clone();
    }
    let answer = gateway.call("POST", "/restapis", Some(body)).await;
    assert_eq!(
        answer.status, 201,
        "case CreateRestApi({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let api = string_member(&answer.body, "id", "CreateRestApi");
    let root = string_member(&answer.body, "rootResourceId", "CreateRestApi");
    (api, root)
}

/// A `POST` method on a child of the root whose `MOCK` integration answers 200 with
/// `served`, which is the one integration type that needs no backend.
async fn mock_route(gateway: &Gateway, api: &str, root: &str, route: &str, served: &str) {
    let resource = resource_under(gateway, api, root, route).await;
    put_method(
        gateway,
        api,
        &resource,
        "POST",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(
        gateway,
        api,
        &resource,
        "POST",
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/json": r#"{"statusCode": 200}"# },
        }),
    )
    .await;
    method_response(gateway, api, &resource, "POST", "200", json!({})).await;
    integration_response(
        gateway,
        api,
        &resource,
        "POST",
        "200",
        json!({ "responseTemplates": { "application/json": served } }),
    )
    .await;
}

#[tokio::test]
async fn a_payload_over_the_documented_limit_is_refused_before_the_integration() {
    let gateway = Gateway::new();
    let (api, root) = create_api_with(&gateway, "payload-limit", json!({})).await;
    let served = r#"{"integration":"ran"}"#;
    mock_route(&gateway, &api, &root, "orders", served).await;
    // A customization of the whole 4xx class, so the case can assert that the refusal the
    // EDGE answers — taken before the api's own records are read — carries none of it.
    let answer = gateway
        .call(
            "PUT",
            &format!("/restapis/{api}/gatewayresponses/DEFAULT_4XX"),
            Some(json!({
                "responseTemplates": { "application/json": r#"{"gw":"customized"}"# },
                "responseParameters": { "gatewayresponse.header.X-Gw": "'customized'" },
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case PutGatewayResponse(DEFAULT_4XX): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    deploy(&gateway, &api, STAGE).await;

    // Rows differ along the one thing the rule generalises over: how large the payload is
    // relative to the published limit — well under it, one byte under, exactly at it, one
    // byte over, and well over.
    let cases: [(&str, usize, u16); 6] = [
        ("an empty payload", 0, 200),
        ("a small payload", 1_024, 200),
        ("one byte under the limit", PAYLOAD_LIMIT - 1, 200),
        ("exactly the limit", PAYLOAD_LIMIT, 200),
        ("one byte over the limit", PAYLOAD_LIMIT + 1, 413),
        ("well over the limit", PAYLOAD_LIMIT + 65_536, 413),
    ];
    for (case, size, expected) in cases {
        let answered = invoke_bytes(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/orders"),
            &[("content-type", "application/json")],
            Bytes::from(vec![b'a'; size]),
        )
        .await;
        assert_eq!(
            answered.status, expected,
            "case {case:?} ({size} bytes against the {PAYLOAD_LIMIT}-byte limit): expected \
             {expected}, got {} with {:?}",
            answered.status,
            answered.text()
        );
        if expected == 413 {
            // Refused at the edge: the integration never ran, so none of its body is in the
            // answer, and the answer is the endpoint's own — stated with a text media type,
            // which the api's own customization of the 4xx class does not reach.
            // The sentence API Gateway documents for the condition, under the media type the
            // endpoint declares for this refusal. Its EXACT bytes on AWS are not available
            // from the recording this issue carries — the replay withholds every recorded
            // value but a wire step's error code — so what is pinned here is the rendering
            // rule (that sentence, as text, with none of the api's records in it), not a
            // byte string read off a capture.
            assert_eq!(
                answered.text(),
                "Request Too Long",
                "case {case:?}: expected the over-sized-payload sentence as text under the \
                 media type the refusal declares, got {:?}",
                answered.text()
            );
            assert_eq!(
                answered.header("x-gw"),
                None,
                "case {case:?}: expected the api's gateway-response customization not to \
                 reach a refusal taken before its records were read, got {:?}",
                answered.header("x-gw")
            );
            // The media type the endpoint states for the refusal it takes at the edge.
            assert_eq!(
                answered.header("content-type"),
                Some("text; charset=utf-8"),
                "case {case:?}: expected the edge refusal's own media type, got {:?}",
                answered.header("content-type")
            );
        } else {
            assert_eq!(
                answered.text(),
                served,
                "case {case:?}: expected the integration's body, got {:?}",
                answered.text()
            );
        }
    }
}

#[tokio::test]
async fn a_response_past_the_minimum_compression_size_is_served_under_the_accepted_coding() {
    // The body the integration answers: long enough to be over the threshold the api sets,
    // and compressible text.
    let large = format!(r#"{{"filler":"{}"}}"#, "compress-me ".repeat(300));
    let small = r#"{"filler":"short"}"#.to_string();
    /// The threshold every compressing api in this suite is created with — inside the
    /// documented `0`..=`10485760` range of `RestApi$minimumCompressionSize`.
    const THRESHOLD: usize = 1_024;

    struct Case {
        name: &'static str,
        /// `minimumCompressionSize`, or `None` for an api that enables no compression.
        threshold: Option<usize>,
        /// Whether the route answers the large body or the small one.
        large_body: bool,
        accept_encoding: Option<&'static str>,
        /// The `Content-Encoding` the answer is expected to carry.
        expected_encoding: Option<&'static str>,
    }

    // Rows differ along what the rule generalises over: the body's size against the
    // threshold, the coding the client accepts, and whether the api enables compression at
    // all.
    let cases = [
        Case {
            name: "a body over the threshold, gzip accepted",
            threshold: Some(THRESHOLD),
            large_body: true,
            accept_encoding: Some("gzip, deflate"),
            expected_encoding: Some("gzip"),
        },
        Case {
            name: "a body over the threshold, only deflate accepted",
            threshold: Some(THRESHOLD),
            large_body: true,
            accept_encoding: Some("deflate"),
            expected_encoding: Some("deflate"),
        },
        Case {
            name: "a body under the threshold is served uncompressed",
            threshold: Some(THRESHOLD),
            large_body: false,
            accept_encoding: Some("gzip, deflate"),
            expected_encoding: None,
        },
        Case {
            name: "a threshold of zero compresses a small body too",
            threshold: Some(0),
            large_body: false,
            accept_encoding: Some("gzip"),
            expected_encoding: Some("gzip"),
        },
        Case {
            name: "identity is the bytes as they are",
            threshold: Some(THRESHOLD),
            large_body: true,
            accept_encoding: Some("identity"),
            expected_encoding: None,
        },
        Case {
            name: "a client that accepts no coding gets none",
            threshold: Some(THRESHOLD),
            large_body: true,
            accept_encoding: None,
            expected_encoding: None,
        },
        Case {
            name: "an api that enables no compression never compresses",
            threshold: None,
            large_body: true,
            accept_encoding: Some("gzip, deflate"),
            expected_encoding: None,
        },
    ];

    for case in cases {
        let gateway = Gateway::new();
        let members = match case.threshold {
            Some(threshold) => json!({ "minimumCompressionSize": threshold }),
            None => json!({}),
        };
        let (api, root) = create_api_with(&gateway, "compression", members).await;
        let served = if case.large_body {
            large.clone()
        } else {
            small.clone()
        };
        mock_route(&gateway, &api, &root, "report", &served).await;
        deploy(&gateway, &api, STAGE).await;

        let mut headers: Vec<(&str, &str)> = vec![("content-type", "application/json")];
        if let Some(accepted) = case.accept_encoding {
            headers.push(("accept-encoding", accepted));
        }
        let answered = invoke_bytes(
            &gateway,
            &api,
            "POST",
            &format!("/{STAGE}/report"),
            &headers,
            Bytes::new(),
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {:?}: expected 200, got {} with {:?}",
            case.name,
            answered.status,
            answered.text()
        );
        assert_eq!(
            answered.header("content-encoding"),
            case.expected_encoding,
            "case {:?}: expected Content-Encoding {:?}, got {:?}",
            case.name,
            case.expected_encoding,
            answered.header("content-encoding")
        );
        // Whatever the coding, the bytes the client receives decode to the body the
        // integration answered: nothing is relabelled.
        let decoded = match case.expected_encoding {
            None => answered.body.clone(),
            Some("gzip") => {
                let mut decoded = Vec::new();
                flate2::read::GzDecoder::new(&answered.body[..])
                    .read_to_end(&mut decoded)
                    .unwrap_or_else(|error| {
                        panic!("case {:?}: the body does not decode as gzip: {error}", case.name)
                    });
                decoded
            }
            Some(_) => {
                let mut decoded = Vec::new();
                flate2::read::ZlibDecoder::new(&answered.body[..])
                    .read_to_end(&mut decoded)
                    .unwrap_or_else(|error| {
                        panic!(
                            "case {:?}: the body does not decode as deflate: {error}",
                            case.name
                        )
                    });
                decoded
            }
        };
        assert_eq!(
            String::from_utf8_lossy(&decoded),
            served,
            "case {:?}: expected the body to decode to the integration's {} bytes, got {} \
             bytes",
            case.name,
            served.len(),
            decoded.len()
        );
        // A compressed answer states the length of what it sent, which is the compressed
        // body rather than the one the integration produced.
        assert_eq!(
            answered.header("content-length").map(str::to_owned),
            Some(answered.body.len().to_string()),
            "case {:?}: expected Content-Length to be the {} bytes sent, got {:?}",
            case.name,
            answered.body.len(),
            answered.header("content-length")
        );
    }
}
