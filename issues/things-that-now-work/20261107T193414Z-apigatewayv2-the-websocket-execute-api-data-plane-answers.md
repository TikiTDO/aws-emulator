Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.websocket-broadcast-to-multiple-connections apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client
Issue-Key: apigatewayv2.websocket-broadcast-to-multiple-connections|_poll_until#20|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect|_poll_until#18|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client|_poll_until#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: c3a6f169c346304d6b4c388bf8378b1c143b9d70

# apigatewayv2: the WEBSOCKET execute-api data plane answers a correct RFC 6455 handshake with 404 UnknownService, so no connection, route response or @connections delivery is reachable (1 cause, 3 checks)

> _Note: this issue was identified against the code base at commit `c3a6f169c346304d6b4c388bf8378b1c143b9d70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
ONE net-new root cause on the apigatewayv2 service token, from the 3 apigatewayv2 failures the replay marks neither ALREADY FILED nor OWNED. It must be closed before this issue is submitted.

=== Cause 1 of 1: a deployed WEBSOCKET stage's invoke URL does not resolve to any service in the emulator's front door — a correct RFC 6455 upgrade is answered HTTP 404 with x-amzn-errortype: UnknownService (or nothing at all) ===

AWS: recorded behavior only for the handshake — apigatewayv2's Smithy model describes the control plane (CreateApi ProtocolType=WEBSOCKET, CreateRoute, CreateIntegration, CreateDeployment, CreateStage) and not the socket wire. Recorded: once a WEBSOCKET API has a deployment and a stage, a GET to that stage's wss invoke URL carrying `Connection: Upgrade`, `Upgrade: websocket`, `Sec-WebSocket-Version: 13` and `Sec-WebSocket-Key: <nonce>` is answered `HTTP/1.1 101 Switching Protocols` with `Upgrade: websocket`, `Connection: Upgrade` and `Sec-WebSocket-Accept: <base64(sha1(nonce + 258EAFA5-E914-47DA-95CA-C5AB0DC85B11))>`. The gateway then mints a connectionId, invokes `$connect`, dispatches each client frame by RouteSelectionExpression, and returns a two-way route's integration response to the client ONLY when a route response exists for it; `@connections` PostToConnection on the minted id delivers to that socket, and after the client disconnects the same id answers 410 GoneException.

Emulator: the upgrade request never reaches any WEBSOCKET data plane. The front door answers the handshake with `HTTP/1.1 404 Not Found`, `content-type: application/x-amz-json-1.1`, `x-amzn-errortype: UnknownService` — i.e. the request is routed as an unrecognised AWS JSON service call rather than as an execute-api data-plane upgrade — and in the third check the handshake simply produces no response for the whole 150 s budget. With no 101 there is no connectionId, so broadcast-to-many, deliver-then-410-after-disconnect, and the two-way-route/route-response contract are all ungradeable.

Why: a client that has provisioned a WEBSOCKET API exactly as AWS documents cannot connect to it at all, and the 404 UnknownService it gets back says the API/stage does not exist rather than that the feature is missing — so a WebSocket application written against AWS fails on the emulator at its very first call, and every downstream contract (route selection, route responses, @connections fan-out, GoneException after disconnect) is unobservable.

Fix: the emulator's front-door HTTP router (the dispatcher that maps an inbound request to a service, including the execute-api data-plane path /_aws/execute-api/{apiId}/{stage}) must recognise the WebSocket upgrade, complete it with 101 and the computed Sec-WebSocket-Accept, and hand the hijacked connection to an apigatewayv2 WEBSOCKET data plane that mints the connectionId, runs $connect/$disconnect and the route-selection dispatch, honours route responses, and registers the connection with the apigatewaymanagementapi connection store.

Related (does NOT cover these three checks): 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md, 20261107T125043Z-apigatewayv2-a-deployed-websocket-stage-never-completes-the--r2.md and 20261107T125951Z-apigatewayv2-7-net-new-root-causes-where-the.md describe the same missing upgrade for OTHER check ids; the replay reports no owner for the three listed here.

Traces: apigatewayv2.websocket-broadcast-to-multiple-connections, apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect, apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client.

## Evidence
Suite: simparity-behavioral-aws-v1. Totals: 0/47 passed (0.0%); apigatewayv2 0/3 (0%), 3 failed. All three are marked neither ALREADY FILED nor OWNED.

[mismatch] apigatewayv2.websocket-broadcast-to-multiple-connections — AssertionError: a 101 WebSocket upgrade at wss://<host>/_aws/execute-api/<apiId>/<stage> not met within 120.0s. Last result: {'status': 404, 'head': b'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\n...'}
[mismatch] apigatewayv2.websocket-post-to-connection-delivers-then-gone-after-disconnect — AssertionError: a 101 WebSocket upgrade at wss://<host>/_aws/execute-api/<apiId>/<stage> not met within 120.0s. Last result: {'status': 404, 'head': b'HTTP/1.1 404 Not Found\r\ncontent-type: application/x-amz-json-1.1\r\nx-amzn-errortype: UnknownService\r\ncontent-length: 149\r\n...'}
[mismatch] apigatewayv2.websocket-two-way-route-needs-a-route-response-to-reach-the-client — AssertionError: a 101 WebSocket upgrade against stage dev not met within 150.0s. Last result: None

Expected (real AWS): HTTP/1.1 101 Switching Protocols, with headers Upgrade: websocket, Connection: Upgrade and Sec-WebSocket-Accept: base64(sha1(Sec-WebSocket-Key + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11')); then a server-minted connectionId addressable over @connections, PostToConnection delivering the frame to that socket, and 410 GoneException (empty message) for the same id after the client disconnects.
Actual (emulator): HTTP 404 Not Found with x-amzn-errortype: UnknownService for the upgrade request (two checks), or no response at all within 150 s (one check). The host and port in the URLs above are per-run values and are not part of the requirement; the path shape /_aws/execute-api/{apiId}/{stage} and the 404/UnknownService answer are.

Control-plane setup succeeds in all three checks (CreateApi ProtocolType=WEBSOCKET, CreateIntegration, CreateRoute, CreateDeployment, CreateStage all return 2xx), so the divergence is purely in the data plane: a provisioned, deployed stage is unroutable.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator, and replay the three apigatewayv2 check ids in trace_ids from simparity-behavioral-aws-v1.
2. CreateApi with ProtocolType=WEBSOCKET and RouteSelectionExpression=$request.body.action; add an integration and the $connect/$default routes; CreateDeployment; CreateStage.
3. Open a socket to the stage's execute-api invoke URL (path /_aws/execute-api/{apiId}/{stage}) and send a correct RFC 6455 handshake: GET with Host, Connection: Upgrade, Upgrade: websocket, Sec-WebSocket-Version: 13, Sec-WebSocket-Key: <base64 of 16 random bytes>.
4. Observe 'HTTP/1.1 404 Not Found' with content-type: application/x-amz-json-1.1 and x-amzn-errortype: UnknownService instead of 'HTTP/1.1 101 Switching Protocols' with a correct Sec-WebSocket-Accept; repeating for 120 s never changes the answer.
5. Because no connection is minted, apigatewaymanagementapi PostToConnection cannot be exercised against a live id, a two-way route's response cannot be observed, and no id can transition to 410 GoneException after a disconnect.
6. After fixing, re-replay exactly the three check ids in trace_ids: the handshake must answer 101, each client must learn a distinct connectionId, PostToConnection must deliver to the right socket only, a two-way route must reach the client only when a route response exists, and the id must answer 410 GoneException with an empty message after the client disconnects.
