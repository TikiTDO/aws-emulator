Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin
Issue-Key: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin|_do_handshake_attempt#30|Timeout|return status, body, sock
Issue-Commit: e88b23ba1

# apigatewayv2: the DisableExecuteApiEndpoint check still reaches no verdict — its re-enabled WEBSOCKET leg retries the front door's `501` upgrade refusal to the 60 s cap (1 cause)

> _Split out of `issues/things-that-now-work/20261110T200558Z-apigatewayv2-iam-lambda-logs-sts-updateapi-disableexecuteapiendpoint-on.md`, whose cause — the `DisableExecuteApiEndpoint` gate answering nothing — is now served: an api that turned its default `execute-api` endpoint off is refused `403` `{"message":"Forbidden"}` on both of this service's data planes (the HTTP invoke, the WebSocket opening handshake at the service endpoint, and the handshake on the api's own socket), and serves again as soon as the member is cleared (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/data_plane.rs`'s `serves_default_endpoint`, driven in `tests/all/disabled_execute_api_endpoint.rs`). This report carries that check's replay key, which still does NOT pass, re-filed here so it is not recorded as working._

## Operation
WebSocket upgrade (execute-api data plane)

## Description
One cause, and it is the CAPABILITY the front door does not have rather than a wrong answer from a service — the same cause `20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md` and `20261107T120000Z-apigatewayv2-the-front-door-cannot-switch-a-websocket-handshake-to-the-apis-own-socket.md` own for their own check ids. This report adds only this check's key.

The check drives `UpdateApi(DisableExecuteApiEndpoint)` through both states and requests both planes after each change. Its refusal legs are now graded: with the member set, the HTTP stage and the WEBSOCKET twin are both answered `403` `{"message":"Forbidden"}` promptly. Its last leg clears the member and expects the WebSocket endpoint to SERVE again — `101 Switching Protocols`. The handshake is made against the endpoint under test, which is the emulator's one front door, and that door cannot answer `101`: it serves one buffered request per call, so the admitted handshake is refused `501` naming the socket the api's `ApiEndpoint` announces. The check's handshake helper treats a `5xx` as transient and retries (the older shape of this report recorded the key `_attempt_handshake#30|Timeout|time.sleep(2)`), so the `501` spins to the suite's 60 s cap and the check reaches no verdict, with the enable/disable transition it asserts left ungradeable.

Why it cannot be closed from `services/` as the tree stands — the client reaches the one buffered front door and nothing in the vendored path hands a service the connection's byte stream:
* `vendor/smithy-server-runtime/src/service.rs` — the whole `Service` trait: `can_handle(&Parts)`, `override_operation`, `override_routing_error(&RtRequest) -> Option<RtResponse>`. Every seam takes an already-buffered request and returns a buffered response; there is no upgrade seam.
* `vendor/smithy-server-runtime/src/multi_service_router.rs:611` — the response path strips `connection` and `upgrade` from every response, so even a hand-built `101` would reach the client without the fields RFC 6455 §4.1 requires of the server's handshake.
* `vendor/server/src/lib.rs:1313` — the production listener is `axum_server::from_tcp_rustls(...).serve(...)`: the fallback buffers the body and returns one buffered response.

Required core change: a seam that lets the CLAIMING service answer `101` and then receive the upgraded connection (an `override_upgrade(&Parts) -> Option<UpgradeHandler>`, or a `Service` hook handed hyper's `OnUpgrade` by value), plus a response path that does not strip a `101`'s hop-by-hop fields. With it, `crate::service::websocket::serve_with_transport` would hand the connection to the same `transport::carry_frames` the api's own socket already uses. Answering `101` without it would be a fabricated success: the client would see the upgrade and then read EOF, with no socket to carry `$connect` and no session a minted connection id could address.

Alternatively, a client that dials the announced `ApiEndpoint` as given needs nothing: the api's own socket completes the handshake today, and refuses it `403` while the default endpoint is off (`tests/all/disabled_execute_api_endpoint.rs::a_disabled_default_endpoint_refuses_the_handshake_on_the_apis_own_socket`).

Traces: apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin.

## Evidence
Replay of the check against the branch that served the gate: 1 of 1 replayed, verdict `failed`, `Check timed out after 60.0s` — unchanged from before the gate, because the leg that spins is the one that needs a `101`. The refusal legs the gate owns are exercised in the owning crate's suite instead, over every addressing of both protocols and in both directions of the transition (`tests/all/disabled_execute_api_endpoint.rs`).

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'), CreateRoute('$default') over an integration, CreateStage, CreateDeployment.
3. `UpdateApi(DisableExecuteApiEndpoint=true)`, then open an RFC 6455 upgrade against the emulator's own data port with the path `/_aws/execute-api/<api-id>/<stage>`: observe `403` `{"message":"Forbidden"}` (served).
4. `UpdateApi(DisableExecuteApiEndpoint=false)` and repeat the upgrade: expect `101 Switching Protocols`; observe `501 NotImplemented` naming the announced socket, which the check retries until its cap.
5. Re-replay apigatewayv2__iam__lambda__logs__sts.disable-execute-api-endpoint-manual-stage-and-websocket-twin.
