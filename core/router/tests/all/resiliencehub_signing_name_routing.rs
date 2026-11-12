// SPDX-License-Identifier: CC0-1.0

//! Cross-service routing for the SigV4 signing name `resiliencehub`, shared by the two
//! emulated Resilience Hub endpoints.
//!
//! The router resolves a request's routing key from the SigV4 credential scope first, so
//! every service that signs under one name sees the same key: a service that claims on the
//! signing name ALONE claims its peer's traffic too, the router finds two claimants and
//! answers `500 AmbiguousService` for BOTH. That refusal is rendered in an awsJson 1.1
//! envelope whatever the target protocol is, so a restJson1 client cannot even parse it
//! into a modelled error shape.
//!
//! Two services share `resiliencehub`, and AWS separates them by API version in the URI
//! path (an emulator cannot separate them by endpoint host: a client configured with one
//! local endpoint sends the same `Host` for both):
//!
//! * `com.amazonaws.resiliencehub#AwsResilienceHub` (sdkId `resiliencehub`, restJson1),
//!   whose operations are bound under their own literal first segments (`/list-apps`,
//!   `/create-resiliency-policy`, …).
//! * `com.amazonaws.resiliencehubv2#NGRHServiceCore` (sdkId `resiliencehubv2`,
//!   restJson1), whose 73 operations are ALL bound under `/v2/…`.
//!
//! This suite pins that: with both mounted, each one's path space reaches it and answers
//! with that service's OWN response — never a router-level refusal.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked: the router here runs with request authentication off.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Both services that share the `resiliencehub` signing name, mounted on one engine,
/// exactly as `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router
}

/// One request to dispatch.
struct Call<'a> {
    method: &'a str,
    uri: &'a str,
    headers: &'a [(&'a str, &'a str)],
    body: &'a str,
}

/// What a dispatched request answered: the status, the response media type, the wire error
/// code (`None` on a success) and the body.
struct Answer {
    status: u16,
    content_type: String,
    code: Option<String>,
    body: String,
}

/// Dispatch `call` signed for the shared `resiliencehub` key.
///
/// The error code is read from the body, which is what distinguishes the two router-level
/// failures from a service's own answer: the router answers `UnknownService` (404) when
/// nobody claims a request and `AmbiguousService` (500) when two do, so a row naming a
/// service's own code or a success can only pass when exactly one service claimed it.
async fn dispatch(router: &MultiServiceRouter, call: &Call<'_>) -> Answer {
    let mut builder = http::Request::builder()
        .method(call.method)
        .uri(call.uri)
        .header("host", "emulator.local")
        .header("authorization", authorization("resiliencehub"));
    for (name, value) in call.headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(Bytes::from(call.body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let body = String::from_utf8_lossy(response.body()).to_string();
    let code = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|json| {
            json.get("__type")
                .and_then(serde_json::Value::as_str)
                .map(|value| {
                    // restJson1 may qualify the shape name (`namespace#Shape`); the code a
                    // client branches on is the shape name itself.
                    value
                        .rsplit(['#', ':'])
                        .next()
                        .unwrap_or(value)
                        .to_string()
                })
        });
    Answer {
        status,
        content_type,
        code,
        body,
    }
}

#[tokio::test]
async fn each_resiliencehub_service_serves_its_own_path_space() {
    let router = build_router();

    // (case, call, expected status, expected error code, expected media type).
    //
    // The rows span both members of the pair and, within each member, differ along what
    // separates the two services: the first URI path segment. Each expectation is the
    // answer of the ONE service that owns that path — a success or that service's own
    // modelled error — plus the last row, a path NEITHER model publishes, which must be
    // the router's `UnknownService` and not the ambiguity refusal.
    let rows: [(&str, Call<'_>, u16, Option<&str>, &str); 5] = [
        (
            // V2 owns `/v2/…`: `ListTestTemplates` reports its service-owned catalog, so
            // it answers 200 with no prior state. This is the request the issue reports as
            // `500 AmbiguousService` in an awsJson envelope.
            "v2 ListTestTemplates",
            Call {
                method: "GET",
                uri: "https://emulator.local/v2/list-test-templates",
                headers: &[],
                body: "",
            },
            200,
            None,
            "application/json",
        ),
        (
            // Still V2, a different `/v2/…` route: a well-formed ARN naming no stored
            // service is V2's own modelled 404, which the V1 service could not produce
            // (it publishes no `/v2/get-service` route at all).
            "v2 GetService for an ARN nothing holds",
            Call {
                method: "GET",
                uri: "https://emulator.local/v2/get-service?serviceArn=arn%3Aaws%3A\
                      resiliencehub%3Aus-east-1%3A000000000000%3Aservice%2Fnot-a-service",
                headers: &[],
                body: "",
            },
            404,
            Some("ResourceNotFoundException"),
            "application/json",
        ),
        (
            // V1 owns its own literal segments: `CreateResiliencyPolicy` is implemented
            // there, so a fresh policy name succeeds. V2 publishes no such path.
            "v1 CreateResiliencyPolicy",
            Call {
                method: "POST",
                uri: "https://emulator.local/create-resiliency-policy",
                headers: &[("content-type", "application/json")],
                body: "{\"policyName\":\"routing-check-policy\",\"tier\":\"Important\",\
                       \"policy\":{\"Software\":{\"rtoInSecs\":3600,\"rpoInSecs\":3600},\
                       \"Hardware\":{\"rtoInSecs\":3600,\"rpoInSecs\":3600},\
                       \"AZ\":{\"rtoInSecs\":3600,\"rpoInSecs\":3600}}}",
            },
            200,
            None,
            "application/json",
        ),
        (
            // …and a V1 GET on another of its segments: `ListApps` answers its own empty
            // listing on a fresh engine.
            "v1 ListApps",
            Call {
                method: "GET",
                uri: "https://emulator.local/list-apps",
                headers: &[],
                body: "",
            },
            200,
            None,
            "application/json",
        ),
        (
            // A path space NEITHER model publishes. Narrowing each claim to its own path
            // space means nobody claims this, which is the router's 404 `UnknownService` —
            // never the 500 `AmbiguousService` both services used to answer.
            "a path neither model publishes",
            Call {
                method: "GET",
                uri: "https://emulator.local/v3/list-test-templates",
                headers: &[],
                body: "",
            },
            404,
            Some("UnknownService"),
            "application/x-amz-json-1.1",
        ),
    ];

    for (case, call, expected_status, expected_code, expected_content_type) in rows {
        let answer = dispatch(&router, &call).await;
        assert_eq!(
            (answer.status, answer.code.as_deref()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code:?} from the one \
             service that owns {}, got {} {:?} {:?}",
            call.uri,
            answer.status,
            answer.code,
            answer.body
        );
        assert_ne!(
            answer.code.as_deref(),
            Some("AmbiguousService"),
            "case {case:?}: two services claimed {}: {:?}",
            call.uri,
            answer.body
        );
        assert_eq!(
            answer.content_type, expected_content_type,
            "case {case:?}: expected the media type {expected_content_type:?} for {}, got \
             {:?} with body {:?}",
            call.uri, answer.content_type, answer.body
        );
    }
}

#[tokio::test]
async fn a_v2_service_created_through_the_router_reads_back_through_it() {
    let router = build_router();

    // The shared signing name must not cost the V2 service its own state: a write and the
    // read of it both have to reach the SAME service, so the resource created through the
    // router is readable through it.
    let create = Call {
        method: "POST",
        uri: "https://emulator.local/v2/create-service",
        headers: &[("content-type", "application/json")],
        body: "{\"name\":\"routing-check-service\",\"regions\":[\"us-east-1\"],\
               \"permissionModel\":{\"invokerRoleName\":\"ResilienceHubRole\"}}",
    };
    let created = dispatch(&router, &create).await;
    assert_eq!(
        created.status, 200,
        "creating a V2 service: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    let arn = serde_json::from_str::<serde_json::Value>(&created.body)
        .ok()
        .and_then(|json| {
            json["service"]["serviceArn"]
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| panic!("no service.serviceArn in {:?}", created.body));

    let read = Call {
        method: "GET",
        uri: &format!("https://emulator.local/v2/get-service?serviceArn={arn}"),
        headers: &[],
        body: "",
    };
    let answer = dispatch(&router, &read).await;
    assert_eq!(
        (answer.status, answer.code.as_deref()),
        (200, None),
        "reading {arn} back: expected HTTP 200, got {} {:?} {:?}",
        answer.status,
        answer.code,
        answer.body
    );
    assert!(
        answer.body.contains("routing-check-service"),
        "reading {arn} back: expected the created service, got {:?}",
        answer.body
    );
}
