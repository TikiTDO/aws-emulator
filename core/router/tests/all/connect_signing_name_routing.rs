// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the `connect` SigV4 signing name.
//!
//! Amazon Connect and Amazon Connect Contact Lens SHARE one signing name (`connect`)
//! while serving two different endpoints (`connect` and `contact-lens`). The router
//! resolves a request's routing key from the SigV4 credential scope first, so both
//! services see the key `connect`: a service that claims on the signing name alone
//! claims the other's traffic too, and the router answers `500 AmbiguousService` for
//! BOTH. Each one therefore also requires the request's first URI path segment to be
//! one its own model declares.
//!
//! This suite pins that: with both services mounted, each one's path space reaches it —
//! whether the operation behind it is implemented (it answers from its own store) or
//! still scaffolded (it answers the scaffold's `NotImplemented`).

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names the signing name both
/// services share. The signature is never checked: the router here runs with request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/connect/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_connect_amazonconnectservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_connectcontactlens_amazonconnectcontactlens::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router
}

async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &'static str,
) -> (u16, Option<String>, serde_json::Value) {
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "connect.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(Bytes::from_static(body.as_bytes()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload =
        serde_json::from_slice::<serde_json::Value>(response.body()).unwrap_or(serde_json::Value::Null);
    // The error code, read from the body's `__type` discriminator: `404` alone cannot
    // tell the router's own `UnknownService` (nobody claimed the request) from a
    // service's own modeled `ResourceNotFoundException`.
    let error_code = payload
        .get("__type")
        .or_else(|| payload.get("Code"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    (status, error_code, payload)
}

/// What a row expects of the service that claimed it — either its own modeled error
/// code, or, for an implemented operation, a member only that operation's output
/// carries.
enum Expect {
    Error(&'static str),
    Member(&'static str),
}

#[tokio::test]
async fn each_service_sharing_the_connect_signing_name_serves_its_own_path_space() {
    let router = build_router();

    // (case, method, uri, body, expected status, what the claiming service answered)
    //
    // Contact Lens owns `/realtime-contact-analysis`: its one operation is implemented,
    // and with no analysis session seeded it answers its own modeled
    // `ResourceNotFoundException`. Amazon Connect owns `/instance` (and the rest of its
    // model's path space): `ListInstances` and `DescribeInstance` are implemented, so
    // they answer from Connect's own store, while an operation still carrying the
    // scaffold header (`DescribeInstanceAttribute`) answers the scaffold's
    // `NotImplemented`. Asserting WHAT the response carries, not just the status, is
    // what excludes the two routing failures: the router answers `UnknownService` (404)
    // when nobody claims a request and `AmbiguousService` (500) when both do, so a row
    // naming a service's own error — or a member only its output has — can only pass
    // when exactly one service claimed it.
    let rows: [(&str, &str, &str, &'static str, u16, Expect); 4] = [
        (
            "contact lens analysis segments",
            "POST",
            "/realtime-contact-analysis/analysis-segments",
            // The `InstanceId` is GUID-shaped because that is what an Amazon Connect
            // instance is identified by: Contact Lens resolves the identifier to an
            // instance before it looks anything up, and one of another form is refused at
            // its front door rather than by the session lookup this row is about.
            r#"{"InstanceId":"4cd1aa0d-1f2e-4b3c-8d4e-5f60718293a4","ContactId":"contact-1"}"#,
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            "amazon connect instance list",
            "GET",
            "/instance",
            "",
            200,
            Expect::Member("InstanceSummaryList"),
        ),
        (
            "amazon connect instance describe",
            "GET",
            "/instance/inst-1",
            "",
            404,
            Expect::Error("ResourceNotFoundException"),
        ),
        (
            "amazon connect instance attribute describe",
            "GET",
            "/instance/inst-1/attribute/INBOUND_CALLS",
            "",
            501,
            Expect::Error("NotImplemented"),
        ),
    ];

    for (case, method, uri, body, expected_status, expected) in rows {
        let (status, error_code, payload) = dispatch(&router, method, uri, body).await;
        match expected {
            Expect::Error(expected_code) => assert_eq!(
                (status, error_code.as_deref()),
                (expected_status, Some(expected_code)),
                "case {case:?}: expected HTTP {expected_status} {expected_code} from the one \
                 service that owns {uri}, got {status} {error_code:?}"
            ),
            Expect::Member(member) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected HTTP {expected_status} from the one service that \
                     owns {uri}, got {status} {payload}"
                );
                assert!(
                    payload.get(member).is_some(),
                    "case {case:?}: expected the claiming service's {member} in the response, \
                     got {payload}"
                );
            }
        }
    }
}
