Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers
Issue-Key: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigatewayv2: the `$connect` handshake is served on the api's own socket, but the front door still cannot upgrade one, so a client that reaches the SERVICE endpoint gets no 101 (1 cause)

> _Split out of `issues/things-that-now-work/20261110T153046Z-apigatewayv2-iam-lambda-logs-a-websocket-connect-handshake.md`. That report's cause is now SERVED on the address the api announces: the `$connect` route's `AWS_PROXY` integration is RUN with the WebSocket `$connect` event and the status the function answered is what admits the connection (`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/websocket/integration.rs`, `event.rs`), the integration's `RequestParameters` map `integration.request.{location}.{name}` from a request parameter of the call or a quoted static value onto what that function reads (`parameters.rs`), and `CreateRoute`/`UpdateRoute` refuse a route-level `RequestParameters` declaration off `$connect` (`src/state/route_request_parameters.rs`) — driven end to end over a real connection in `tests/all/websocket_connect_backend.rs` and `tests/all/websocket_lambda_routes.rs`. This report carries the two replay keys that still do NOT pass, re-filed here so they are not recorded as working, and what that fix established about them. It is the same cause as `issues/things-that-do-not-work/20261107T120000Z-apigatewayv2-the-front-door-cannot-switch-a-websocket-handshake-to-the-apis-own-socket.md`, which carries three OTHER checks' keys and not these two._

## Operation
WebSocket upgrade (execute-api data plane)

## Description
One cause, and it is a CAPABILITY OF THE CORE rather than a wrong answer from a service.

A WEBSOCKET api is reached, in full, at the address its `ApiEndpoint` announces: the socket the core opened for that api (`engine.listeners().tcp_plaintext`), which answers `101 Switching Protocols`, runs the `$connect` route's Lambda proxy integration with the `$connect` event (`requestContext` carrying `connectionId`, `domainName`, `stage`, `eventType` `CONNECT`, `messageDirection` `IN`, and the handshake's headers after the integration's `RequestParameters` mapping), and then carries frames. Neither of the two checks below dials that address. Each resolves the announced URI onto the ENDPOINT UNDER TEST — the emulator's one front door — and sends the opening handshake there, carrying the api id in the rewritten path (`/_aws/execute-api/{api-id}/{stage}`). That request is claimed and answered from the api's records today, but the front door cannot answer `101`: it serves one BUFFERED request per call, so both checks poll for a live socket until the suite's 60 s cap.

Why it cannot be closed from `services/` as the tree stands — the client reaches the one buffered front door, and nothing in the vendored path can hand a service the connection's byte stream:
* `vendor/smithy-server-runtime/src/service.rs` — the whole `Service` trait. `can_handle(&Parts)`, `override_operation(&RtRequest) -> Option<&dyn ErasedOperation>`, `override_routing_error(&RtRequest) -> Option<RtResponse>`: every seam takes an already-buffered request and returns a buffered response. There is no upgrade seam.
* `vendor/smithy-server-runtime/src/multi_service_router.rs:602-621` — `http_response_from_runtime_api` strips `connection` and `upgrade` from every response, so even a hand-built `101` would reach the client without the fields RFC 6455 §4.1 requires of the server's handshake.
* `vendor/server/src/lib.rs:1313` — the production listener is `axum_server::from_tcp_rustls(...)`: the fallback buffers the body and returns one `http::Response<Bytes>`. The hyper `OnUpgrade` extension survives into `OperationContext::parts()`, but only as `&Parts`, and `OnUpgrade` cannot be taken out of an immutable borrow, so no handler can await the upgrade.
* `vendor/server/src/listeners.rs:548-551` — the core's own HTTP listener refuses a WebSocket upgrade with `501`, so the `http()` listener is closed for this protocol too (the `tcp_plaintext()` one is what the api's socket uses, and that is the path that now works).

Required core change: a seam that lets the CLAIMING service answer `101` and then receive the upgraded connection (an `override_upgrade(&Parts) -> Option<UpgradeHandler>`, or a `Service` hook handed the `OnUpgrade` by value), plus a response path that does not strip a `101`'s hop-by-hop fields. With it, `websocket::serve` would hand the connection to the same `transport::carry_frames` the api's own socket already uses, and these two checks would pass without any further service work. Answering `101` without such a seam would be a fabricated success — the client would see the upgrade and then read EOF, with no socket to carry `$connect` and no session a minted connection id could address.

Alternatively, a client that dials the announced `ApiEndpoint` as given needs nothing: the socket is there today, and both the mapped destination header and the `$connect` event shape are observable over it.

Traces: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only (its non-`$connect` `CreateRoute` refusal is now served; it hangs on the handshake leg), apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers (hang).

## Evidence
Replay of the two checks against the branch that served the parent report (2 of 2 replayed): both end with `Error: Check timed out after 60.0s`, at `_poll_until#10` and `_poll_until#15`, polling for an upgraded socket on the endpoint under test.

The same two behaviours pass over the api's OWN socket, in the owning crate's suite: `tests/all/websocket_connect_backend.rs`'s `an_integrations_request_parameters_map_onto_what_the_connect_function_reads` (twelve rows, each a form the integration's `RequestParameters` take, decided by the function the api names) and `tests/all/websocket_lambda_routes.rs`'s `$connect` lifecycle rows, which assert the event's `eventType`, `routeKey` and `connectionId`. The route-level restriction the first check also asserts passes at the control plane, in `request_parameters_are_declared_only_on_the_connection_requests_route`.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType='WEBSOCKET'); CreateFunction for a `$connect` handler returning `{"statusCode": 200}`; CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<the function>, RequestParameters={'integration.request.header.<dst>': 'route.request.header.<src>'}); CreateRoute(RouteKey='$connect', Target='integrations/<integ>'); CreateStage; CreateDeployment.
3. Open an RFC 6455 upgrade against the `ApiEndpoint` the api announces, AS GIVEN: expect — and observe — `101 Switching Protocols`, and the `$connect` function invoked with the event carrying the mapped destination header.
4. Open the same upgrade against the emulator's own data port, with the path `/_aws/execute-api/<api-id>/<stage>`: expect `101`; observe `501 NotImplemented` naming the announced socket.
5. Re-replay the two check ids in trace_ids.
