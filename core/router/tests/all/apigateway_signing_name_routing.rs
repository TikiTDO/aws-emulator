// SPDX-License-Identifier: CC0-1.0

//! Amazon API Gateway and Amazon API Gateway V2 share one SigV4 signing name —
//! each must still be claimed by exactly one service in the assembled router.
//!
//! Both services declare sigv4 name `apigateway` and endpoint prefix `apigateway` (the
//! control plane is `sdkId` `API Gateway`, the V2 endpoint `ApiGatewayV2`), and the router
//! resolves a request's routing key from the SigV4 credential scope FIRST, so both see the
//! same key. A service that claims on the signing name alone therefore claims its peer's
//! traffic too, the router finds two claimants and answers `500 AmbiguousService` — a code
//! that exists in no AWS model — which made BOTH services unreachable: every API Gateway
//! call answered a retryable server fault instead of its modelled 2xx or 4xx.
//!
//! Their modelled URI path spaces are disjoint — every V2 operation is bound under
//! `/v2/…`, the control plane's under `/restapis`, `/apikeys`, `/account`, `/domainnames`,
//! `/usageplans`, `/vpclinks`, `/clientcertificates`, `/sdktypes`, `/tags`, … — so each
//! crate's `can_handle` adds the framework's path-segment conjunct (the rule
//! `emulator_server::can_handle_based_on_route_key_and_path` implements) over its own
//! generated `ROUTE_PATH_SEGMENTS`. This suite pins that split in the production router,
//! across both services' own paths, both routing tiers (credential scope and
//! `X-Amz-Target`) and either endpoint host.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the
//! same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `apigateway` signing name, which is what both
/// services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/apigateway/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// What must happen to a request both services could claim on their signing name.
enum Expect {
    /// Served by the service the case names, with this status, and a body containing this
    /// marker. The marker is a string ONLY that service can produce — an output member its
    /// peer's model does not carry — so it identifies which crate answered.
    Serves {
        who: &'static str,
        status: u16,
        marker: &'static str,
    },
    /// Claimed by exactly one of the two: neither `AmbiguousService` (both claimed) nor
    /// `UnknownService` (neither did). All that a request to a still-scaffolded operation
    /// guarantees — such an operation answers the scaffold's `501 NotImplemented`, which
    /// is its own issue, not a routing failure.
    ClaimedByExactlyOne,
}

/// One routing case: the request, and what the router must do with it.
struct Case {
    name: &'static str,
    host: &'static str,
    method: &'static str,
    path_and_query: &'static str,
    /// Whether to sign for `apigateway`. A case with `false` carries no credential scope,
    /// so the router falls through to its next routing tier.
    signed: bool,
    /// `X-Amz-Target`: neither service speaks an RPC protocol, so this only ever exercises
    /// the second routing tier.
    target: Option<&'static str>,
    body: &'static str,
    expect: Expect,
}

/// The rows differ along what the rule generalises over: which service owns the first path
/// segment, the segments BOTH models bind under different prefixes (`domainnames`,
/// `tags`), the routing tier the key comes from, the endpoint host, read versus write, and
/// near-miss spellings.
const CASES: &[Case] = &[
    Case {
        // The issue's own call: `CreateRestApi`, reported as a retried
        // `500 AmbiguousService`. `rootResourceId` is a control-plane output member the
        // V2 model does not carry.
        name: "control plane CreateRestApi on the apigateway host",
        host: "apigateway.us-east-1.amazonaws.com",
        method: "POST",
        path_and_query: "/restapis",
        signed: true,
        target: None,
        body: r#"{"name":"routing-check-api"}"#,
        expect: Expect::Serves {
            who: "the API Gateway control plane",
            status: 201,
            marker: "rootResourceId",
        },
    },
    Case {
        // The same call on a loopback endpoint: the host does not decide, the modelled
        // path space does.
        name: "control plane CreateRestApi on a loopback endpoint",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/restapis",
        signed: true,
        target: None,
        body: r#"{"name":"routing-check-api-loopback"}"#,
        expect: Expect::Serves {
            who: "the API Gateway control plane",
            status: 201,
            marker: "rootResourceId",
        },
    },
    Case {
        // A control-plane READ, and the collection form of the same path.
        name: "control plane GetRestApis",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/restapis",
        signed: true,
        target: None,
        body: "",
        expect: Expect::Serves {
            who: "the API Gateway control plane",
            status: 200,
            marker: "rootResourceId",
        },
    },
    Case {
        // The V2 endpoint's own `CreateApi`: `protocolType` is a V2 output member the
        // control-plane model does not carry.
        name: "V2 CreateApi on the apigateway host",
        host: "apigateway.us-east-1.amazonaws.com",
        method: "POST",
        path_and_query: "/v2/apis",
        signed: true,
        target: None,
        body: r#"{"name":"routing-check-v2","protocolType":"HTTP"}"#,
        expect: Expect::Serves {
            who: "API Gateway V2",
            status: 201,
            marker: "protocolType",
        },
    },
    Case {
        name: "V2 GetApis on a loopback endpoint",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/v2/apis",
        signed: true,
        target: None,
        body: "",
        expect: Expect::Serves {
            who: "API Gateway V2",
            status: 200,
            marker: "items",
        },
    },
    Case {
        // An UNSIGNED V2 request: with no credential scope the router falls through to its
        // next tier, so this row pins only that the claim does not become ambiguous.
        name: "V2 GetApis with no credential scope",
        host: "apigateway.us-east-1.amazonaws.com",
        method: "GET",
        path_and_query: "/v2/apis",
        signed: false,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // `domainnames` is a segment BOTH models bind. Unprefixed it is the control
        // plane's; under `/v2/` it is the V2 endpoint's. Only the FIRST segment decides,
        // so neither row may be ambiguous.
        name: "control plane GetDomainNames",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/domainnames",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "V2 GetDomainNames",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/v2/domainnames",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // …and so is `tags`.
        name: "control plane GetTags",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/tags/arn%3Aaws%3Aapigateway%3Aus-east-1%3A%3A%2Frestapis%2Fabc",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "V2 GetTags",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/v2/tags/arn%3Aaws%3Aapigateway%3Aus-east-1%3A%3A%2Fapis%2Fa1",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // One row per remaining family of the control plane's own first segments.
        name: "control plane CreateApiKey",
        host: "127.0.0.1:9297",
        method: "POST",
        path_and_query: "/apikeys",
        signed: true,
        target: None,
        body: r#"{"name":"routing-check-key","enabled":true}"#,
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "control plane GetAccount",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/account",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "control plane GetUsagePlans",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/usageplans",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "control plane GetVpcLinks",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/vpclinks",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "control plane GetClientCertificates",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/clientcertificates",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "control plane GetSdkTypes",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/sdktypes",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        // …and per remaining family of the V2 endpoint's.
        name: "V2 GetVpcLinks",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/v2/vpclinks",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "V2 ListPortals",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/v2/portals",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
    Case {
        name: "V2 ListPortalProducts",
        host: "127.0.0.1:9297",
        method: "GET",
        path_and_query: "/v2/portalproducts",
        signed: true,
        target: None,
        body: "",
        expect: Expect::ClaimedByExactlyOne,
    },
];

#[test]
fn api_gateway_and_api_gateway_v2_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("apigateway-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                // The IO and TIME drivers a service's background work needs; see the
                // sibling `apigateway_private_vpce_invoke` suite.
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check:
                // these cases are about which service claims a request.
                router.set_request_authentication(false);

                for case in CASES {
                    let mut builder = http::Request::builder()
                        .method(case.method)
                        .uri(format!("https://{}{}", case.host, case.path_and_query))
                        .header("host", case.host)
                        .header("content-type", "application/json");
                    if case.signed {
                        builder = builder.header("authorization", AUTHORIZATION);
                    }
                    if let Some(target) = case.target {
                        builder = builder.header("x-amz-target", target);
                    }
                    let request = builder
                        .body(bytes::Bytes::from_static(case.body.as_bytes()))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    assert!(
                        !body.contains("UnknownService"),
                        "case {:?}: no service claimed the request: expected exactly one \
                         claimant, got {status} {body}",
                        case.name
                    );
                    match &case.expect {
                        Expect::Serves {
                            who,
                            status: expected_status,
                            marker,
                        } => {
                            assert_eq!(
                                status, *expected_status,
                                "case {:?}: expected HTTP {expected_status} from {who}, got \
                                 {status} {body}",
                                case.name
                            );
                            assert!(
                                body.contains(*marker),
                                "case {:?}: expected a body from {who} carrying {marker:?}, \
                                 got {status} {body}",
                                case.name
                            );
                        }
                        Expect::ClaimedByExactlyOne => {}
                    }
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}
