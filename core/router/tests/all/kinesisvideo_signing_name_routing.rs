// SPDX-License-Identifier: CC0-1.0

//! The five Kinesis Video services share one SigV4 signing name — each request must
//! still be claimed by exactly one of them in the assembled router.
//!
//! The control plane (`Kinesis Video`), Media, Archived Media, Signaling Channels and
//! WebRTC Storage all declare `aws.auth#sigv4 name = "kinesisvideo"` and the
//! `kinesisvideo` endpoint prefix, so the routing key alone matches five services and
//! the router rejects a request several services claim with `500 AmbiguousService` —
//! which makes ALL FIVE unreachable. What separates them is their modelled path
//! spaces, which are DISJOINT at the first URI segment, so each crate's `can_handle`
//! adds the framework's path conjunct — `emulator_server::can_handle_based_on_route_key_and_path`
//! over its own generated `ROUTE_PATH_SEGMENTS`, matched by exact segment equality —
//! to the routing-key test. This suite pins that split in the production router: the
//! endpoint host does not matter, the modelled path space does.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for
//! the same reason `boot_serve.rs` does.

use aws_emulator_router::{build_router, InMemoryEngine};
use std::sync::Arc;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// A SigV4 credential scope naming the `kinesisvideo` signing name, which is what
/// all five services' requests carry.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/kinesisvideo/aws4_request, \
     SignedHeaders=host, Signature=0123456789abcdef";

/// What must happen to a request every `kinesisvideo`-signed service could claim on
/// its signing name alone.
enum Expect {
    /// Answered by the service whose model binds the path: the status plus a body
    /// fragment only THAT crate produces is what names the service that served it.
    Answered { status: u16, contains: &'static str },
    /// Bound by NO candidate's modelled path space, so the front door has nothing to
    /// dispatch to and refuses it as `UnknownService` — the one refusal the path
    /// conjunct is allowed to produce, and deliberately NOT `AmbiguousService`.
    NoServiceBindsThePath,
}

/// One routing case.
struct Case {
    name: &'static str,
    host: &'static str,
    path: &'static str,
    body: &'static str,
    expect: Expect,
}

const CASES: &[Case] = &[
    Case {
        name: "control-plane CreateStream on the kinesisvideo endpoint host",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/createStream",
        body: r#"{"StreamName":"routing-probe"}"#,
        expect: Expect::Answered {
            status: 200,
            contains: "StreamARN",
        },
    },
    Case {
        name: "control-plane ListTagsForStream for a stream nobody created",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/listTagsForStream",
        body: r#"{"StreamName":"absent-stream"}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
    Case {
        name: "control-plane TagStream for a stream nobody created, on a loopback endpoint",
        host: "127.0.0.1:9297",
        path: "/tagStream",
        body: r#"{"StreamName":"absent-stream","Tags":{"env":"prod"}}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
    Case {
        name: "control-plane UntagStream for a stream nobody created",
        host: "127.0.0.1:9297",
        path: "/untagStream",
        body: r#"{"StreamName":"absent-stream","TagKeyList":["env"]}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
    Case {
        name: "control-plane TagResource with a malformed resourceARN",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/TagResource",
        body: r#"{"ResourceARN":"not-an-arn","Tags":[{"Key":"env","Value":"prod"}]}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "InvalidArgumentException",
        },
    },
    Case {
        // The media service refuses a request naming the stream BOTH ways, in wording
        // only its `GetMedia` produces; the control plane binds no `/getMedia` at all.
        name: "Media GetMedia reaches the media service, not the control plane",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/getMedia",
        body: r#"{"StreamName":"routing-probe","StreamARN":"arn:aws:kinesisvideo:us-east-1:000000000000:stream/routing-probe/1","StartSelector":{"StartSelectorType":"NOW"}}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "Only one of StreamName and StreamARN",
        },
    },
    Case {
        // The archived-media reader words the one-identifier rule its own way: the
        // control plane says "Either StreamName or StreamARN must be specified." and
        // the media service "Only one of StreamName and StreamARN", so this sentence
        // names the service that served it. (Its missing-stream wording no longer
        // does: the reader and the control plane word a 404 for an absent stream the
        // same way the real services do.)
        name: "Archived Media GetHLSStreamingSessionURL reaches the reader service",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/getHLSStreamingSessionURL",
        body: r#"{"StreamName":"absent-stream","StreamARN":"arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent-stream/1"}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "One of StreamName or StreamARN must be provided, but not both.",
        },
    },
    Case {
        // The archived-media reader refuses a request naming the stream BOTH ways in
        // wording only it produces: the control plane words the same refusal "Either
        // StreamName or StreamARN must be specified." / "refer to different streams",
        // and the media service "Only one of StreamName and StreamARN".
        name: "Archived Media ListFragments reaches the reader service",
        host: "127.0.0.1:9297",
        path: "/listFragments",
        body: r#"{"StreamName":"absent-stream","StreamARN":"arn:aws:kinesisvideo:us-east-1:000000000000:stream/absent-stream/1"}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "One of StreamName or StreamARN must be provided, but not both.",
        },
    },
    Case {
        // The signaling service serves this route from its own channel registry, which
        // this router holds no channel in: the 404 wording naming the signaling channel
        // is what only that crate produces (the control plane binds none of `/v1`).
        name: "Signaling Channels GetIceServerConfig reaches the signaling service",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/v1/get-ice-server-config",
        body: r#"{"ChannelARN":"arn:aws:kinesisvideo:us-east-1:000000000000:channel/probe/1"}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "probe channel does not exist !",
        },
    },
    Case {
        // The WebRTC storage service refuses an ARN that names a stream rather than a
        // signaling channel, in wording only its `JoinStorageSession` produces.
        name: "WebRTC Storage JoinStorageSession reaches the routing service",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/joinStorageSession",
        body: r#"{"channelArn":"arn:aws:kinesisvideo:us-east-1:000000000000:stream/probe/1"}"#,
        expect: Expect::Answered {
            status: 400,
            contains: "channelArn",
        },
    },
    Case {
        name: "a kinesisvideo path no service binds is refused, not claimed ambiguously",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/notAnOperation",
        body: "{}",
        expect: Expect::NoServiceBindsThePath,
    },
    Case {
        // The claim rule is the first path SEGMENT, so this is still the control
        // plane's path space; its route matcher then reads the trailing slash as the
        // same route, and exactly one service answered.
        name: "a control-plane path with a trailing slash",
        host: "kinesisvideo.us-east-1.amazonaws.com",
        path: "/listTagsForStream/",
        body: r#"{"StreamName":"absent-stream"}"#,
        expect: Expect::Answered {
            status: 404,
            contains: "ResourceNotFoundException",
        },
    },
];

#[test]
fn every_kinesisvideo_path_is_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("kinesisvideo-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let router = build_router(engine);

                for case in CASES {
                    let request = http::Request::builder()
                        .method("POST")
                        .uri(format!("https://{}{}", case.host, case.path))
                        .header("host", case.host)
                        .header("authorization", AUTHORIZATION)
                        .header("content-type", "application/json")
                        .body(bytes::Bytes::from_static(case.body.as_bytes()))
                        .expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let body = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !body.contains("AmbiguousService"),
                        "case {}: several services claimed one request: {status} {body}",
                        case.name
                    );
                    if !matches!(case.expect, Expect::NoServiceBindsThePath) {
                        assert!(
                            !body.contains("UnknownService"),
                            "case {}: no service claimed the request: {status} {body}",
                            case.name
                        );
                    }
                    match case.expect {
                        Expect::Answered { status: want, contains } => {
                            assert_eq!(
                                status, want,
                                "case {}: the service whose model binds this path must answer \
                                 {want}, got {status} {body}",
                                case.name
                            );
                            assert!(
                                body.contains(contains),
                                "case {}: expected {contains:?} in the body, got {status} {body}",
                                case.name
                            );
                        }
                        Expect::NoServiceBindsThePath => {
                            assert!(
                                body.contains("UnknownService"),
                                "case {}: a path no model binds must be refused as \
                                 UnknownService, got {status} {body}",
                                case.name
                            );
                        }
                    }
                }
            });
        })
        .expect("spawn the routing-check thread")
        .join()
        .expect("the routing check must pass");
}

/// Two more things a `kinesisvideo`-signed answer has to get right once the path
/// conjunct above has routed it to the one service that owns the path.
///
/// 1. THE ENVELOPE. All five services declare `restJson1`, for which AWS answers
///    `Content-Type: application/json` and names the modelled error in the
///    `x-amzn-errortype` header. The router's own refusal for a request several
///    services claim is rendered as `awsJson1.1`
///    (`application/x-amz-json-1.1`, `__type` only), so while every Kinesis Video
///    request was refused as `500 AmbiguousService` no caller ever saw the service's
///    own envelope. Asserted over a success, a handler's modelled 4xx, a boundary
///    refusal and a scaffolded operation's 501 — four different renderers, one
///    envelope.
/// 2. THE STORED RECORD. A route that reaches its owner has to answer from state, so
///    the stream a `CreateStream` minted is read back BY THE `StreamARN` IT RETURNED,
///    through `DescribeStream` and through `ListStreams`.
///
/// Both run against the production `build_router`, on the same sized thread and with
/// the same credential scope as the suite above.
mod stream_surface {
    use super::{build_router, InMemoryEngine, AUTHORIZATION, BOOT_STACK_SIZE};
    use serde_json::Value;
    use std::sync::Arc;

    /// One answer from the assembled router.
    struct Answer {
        status: u16,
        /// The `restJson1` error code of the body (`__type`), namespace stripped; empty
        /// on a success, which carries none.
        code: String,
        content_type: String,
        error_type: String,
        body: Value,
    }

    async fn call(
        router: &smithy_server_runtime::MultiServiceRouter,
        path: &str,
        body: &str,
    ) -> Answer {
        let host = "kinesisvideo.us-east-1.amazonaws.com";
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(body.to_owned()))
            .expect("the request is well formed");
        let response = router.dispatch(request).await;
        let status = response.status().as_u16();
        let header = |name: &str| {
            response
                .headers()
                .get(name)
                .and_then(|value| value.to_str().ok())
                .unwrap_or_default()
                .to_string()
        };
        let content_type = header("content-type");
        let error_type = header("x-amzn-errortype");
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        let code = parsed
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        Answer {
            status,
            code,
            content_type,
            error_type,
            body: parsed,
        }
    }

    #[test]
    fn kinesisvideo_answers_carry_the_restjson1_envelope() {
        on_a_sized_thread("kinesisvideo-envelope", || {
            Box::pin(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let router = build_router(engine);
                // (case, path, body, whether the answer is an error)
                let rows: [(&str, &str, &str, bool); 4] = [
                    (
                        "a success",
                        "/createStream",
                        r#"{"StreamName":"envelope-probe"}"#,
                        false,
                    ),
                    (
                        "a modelled 404 from a handler",
                        "/describeStream",
                        r#"{"StreamName":"absent-stream"}"#,
                        true,
                    ),
                    (
                        "a boundary refusal",
                        "/describeStream",
                        r#"{"StreamARN":"not-an-arn"}"#,
                        true,
                    ),
                    (
                        "a scaffolded operation's 501",
                        "/deleteEdgeConfiguration",
                        r#"{"StreamName":"envelope-probe"}"#,
                        true,
                    ),
                ];

                for (case, path, body, is_error) in rows {
                    let answer = call(&router, path, body).await;
                    assert_ne!(
                        answer.code, "AmbiguousService",
                        "case {case:?} ({path}): several services claimed the request, \
                         got HTTP {} ({})",
                        answer.status, answer.body
                    );
                    assert_eq!(
                        answer.content_type, "application/json",
                        "case {case:?} ({path}): a restJson1 service answers \
                         `application/json`, got {:?} with HTTP {} ({})",
                        answer.content_type, answer.status, answer.body
                    );
                    if is_error {
                        assert!(
                            !answer.error_type.is_empty()
                                && answer.error_type == answer.code,
                            "case {case:?} ({path}): a restJson1 error names its code in \
                             `x-amzn-errortype`, got {:?} beside the body code {:?} ({})",
                            answer.error_type,
                            answer.code,
                            answer.body
                        );
                    }
                }
            })
        });
    }

    /// Run one async check on a thread sized like `boot_serve.rs`'s: assembling the
    /// production router costs stack, for the same reason that suite gives.
    fn on_a_sized_thread(
        name: &str,
        check: fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
    ) {
        let name = name.to_string();
        std::thread::Builder::new()
            .name(name)
            .stack_size(BOOT_STACK_SIZE)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .build()
                    .expect("build a current-thread runtime");
                runtime.block_on(check());
            })
            .expect("spawn the check thread")
            .join()
            .expect("the check must pass");
    }

    #[test]
    fn a_stream_created_through_the_shared_endpoint_reads_back() {
        on_a_sized_thread("kinesisvideo-round-trip", || {
            Box::pin(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let router = build_router(engine);
                let created = call(
                    &router,
                    "/createStream",
                    r#"{"StreamName":"kv-round-trip","DataRetentionInHours":2}"#,
                )
                .await;
                assert_eq!(
                    (created.status, created.code.as_str()),
                    (200, ""),
                    "case \"CreateStream\": expected HTTP 200, got {} {:?} ({})",
                    created.status,
                    created.code,
                    created.body
                );
                let arn = created.body["StreamARN"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();

                // The modelled `ResourceARN` form
                // `arn:<partition>:kinesisvideo:<region>:<account>:<type>/<name>/<digits>`,
                // read off the `@pattern` on `DescribeStreamInput$StreamARN`
                // (`smithy_query(query="operation", operation="DescribeStream")`).
                // Checked structurally here and then PROVED against the model by feeding
                // it back in through that pattern-carrying member below: a value outside
                // the pattern is refused at the boundary.
                let fields: Vec<&str> = arn.split(':').collect();
                assert_eq!(
                    fields.len(),
                    6,
                    "case \"CreateStream\": a StreamARN has six `:`-separated fields, \
                     got {arn:?}"
                );
                assert_eq!(
                    (fields[0], fields[2], fields[3]),
                    ("arn", "kinesisvideo", "us-east-1"),
                    "case \"CreateStream\": expected an arn of the kinesisvideo namespace \
                     in the request's region, got {arn:?}"
                );
                let resource: Vec<&str> = fields[5].split('/').collect();
                assert_eq!(
                    resource.len(),
                    3,
                    "case \"CreateStream\": a StreamARN resource is \
                     `<type>/<name>/<digits>`, got {arn:?}"
                );
                assert_eq!(
                    (resource[0], resource[1]),
                    ("stream", "kv-round-trip"),
                    "case \"CreateStream\": expected the created stream's own name in its \
                     ARN, got {arn:?}"
                );
                assert!(
                    !resource[2].is_empty()
                        && resource[2].bytes().all(|byte| byte.is_ascii_digit()),
                    "case \"CreateStream\": a StreamARN ends in a numeric segment, \
                     got {arn:?}"
                );

                let fetched = call(
                    &router,
                    "/describeStream",
                    &format!(r#"{{"StreamARN":"{arn}"}}"#),
                )
                .await;
                assert_eq!(
                    (fetched.status, fetched.code.as_str()),
                    (200, ""),
                    "case \"DescribeStream by the returned StreamARN\": expected HTTP 200, \
                     got {} {:?} ({})",
                    fetched.status,
                    fetched.code,
                    fetched.body
                );
                let info = &fetched.body["StreamInfo"];
                assert_eq!(
                    info["StreamARN"].as_str(),
                    Some(arn.as_str()),
                    "case \"DescribeStream by the returned StreamARN\": the stream read \
                     back must be the stream created, got {info}"
                );
                assert_eq!(
                    info["DataRetentionInHours"].as_i64(),
                    Some(2),
                    "case \"DescribeStream by the returned StreamARN\": the retention the \
                     create asked for must read back, got {info}"
                );

                let listed = call(&router, "/listStreams", "{}").await;
                assert_eq!(
                    (listed.status, listed.code.as_str()),
                    (200, ""),
                    "case \"ListStreams\": expected HTTP 200, got {} {:?} ({})",
                    listed.status,
                    listed.code,
                    listed.body
                );
                let arns: Vec<&str> = listed.body["StreamInfoList"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|entry| entry["StreamARN"].as_str())
                            .collect()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    arns,
                    vec![arn.as_str()],
                    "case \"ListStreams\": expected exactly the created stream, got {}",
                    listed.body
                );
            })
        });
    }
}
