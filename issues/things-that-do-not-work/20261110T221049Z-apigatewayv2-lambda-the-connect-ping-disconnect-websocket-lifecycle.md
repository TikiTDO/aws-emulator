Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas|_ws_handshake#15|Timeout|return status, head, None
Issue-Commit: 8c5ca24b773ca2762f91097a2e2db632761ff691

# apigatewayv2__lambda: the $connect/ping/$disconnect WebSocket lifecycle reaches no verdict — the session never answers inside the harness budget and the check is killed at 60s

> _Note: this issue was identified against the code base at commit `8c5ca24b773ca2762f91097a2e2db632761ff691`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
ONE net-new root cause on the apigatewayv2__lambda service token, taken from the single failure of this replay that the report marks neither ALREADY FILED nor OWNED.

=== Cause 1 of 1: a WebSocket session dispatches nothing a client can observe — the watcher's two-way route round trip, the $connect invocation and the $disconnect invocation all fail to arrive, so the check consumes the whole 60 s harness budget without a verdict ===

AWS: the handshake and the data frames are not modelled operations — recorded behavior only. The wiring is modelled: CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action'), CreateRoute for '$connect' and '$disconnect' (one-way) and for '$default' and 'ping' (two-way, with a RouteResponse), each with its OWN AWS_PROXY Lambda integration, CreateDeployment + CreateStage, and lambda:InvokeFunction granted to apigateway.amazonaws.com. Recorded behavior: (a) opening the socket invokes the $connect route's own Lambda with requestContext.routeKey '$connect', requestContext.eventType 'CONNECT', the freshly minted requestContext.connectionId and the handshake's queryStringParameters; (b) a frame whose body selects the 'ping' route invokes the ping route's own Lambda under the SAME connectionId, and because the route is two-way the handler's body travels back down the same socket; (c) a clean close invokes the $disconnect route's own Lambda with routeKey '$disconnect', eventType 'DISCONNECT' and that same connectionId; (d) each connection gets its own connectionId, and only the lifecycle functions push to a watcher connection via ApiGatewayManagementApi PostToConnection on https://{domainName}/{stage}. Every one of these arrives in seconds.

Emulator: the replay reports `Error: Check timed out after 60.0s` — no verdict at all. The check's very first step is a watcher socket opened on the deployed stage followed by a two-way round trip of {"action":"ping"} with a 45 s timeout; that round trip is what consumes the budget, so the emulator's WebSocket session path produces neither the selection-expression route dispatch nor the reply frame, and therefore none of the $connect/$disconnect lifecycle invocations are ever observable. AWS always answers a frame on a two-way route and always fires the two lifecycle routes. Overlapping open issue 20261110T194854Z-apigatewayv2-lambda-2-net-new-causes-a-connect.md lists this check id in its Issue-Trace-Ids (its recorded replay key for it is `_poll_until#15|Timeout`), but this replay does NOT mark this failure as OWNED by it — the failure key differs — so it is filed here; a fixer should close the two together, and 20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md is the related upgrade-path issue.

Why: WebSocket applications key their entire connection bookkeeping on these three events — register the connection on $connect, route frames by the selection expression, clean up on $disconnect. With no reply and no lifecycle invocation a client hangs instead of receiving either data or a terminal status, no connection state is ever written or reaped, and nothing built on an API Gateway WebSocket API can be exercised against the emulator.

Fix: the apigatewayv2 WEBSOCKET execute-api front door and session path — resolve the stage from the handshake, invoke the $connect route integration with a CONNECT requestContext before admitting the socket, dispatch each inbound frame through RouteSelectionExpression to that route's own integration and write a two-way route's response back on the socket, invoke the $disconnect route integration with a DISCONNECT requestContext on close, and make ApiGatewayManagementApi PostToConnection deliver to the addressed connection. Any request that cannot be served must get a terminal HTTP status rather than hanging.

Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas.

## Evidence
Suite: simparity-behavioral-aws-v1. Service heading `apigatewayv2__lambda`: 7/19 (37%), 12 failed; 11 of those are marked ALREADY FILED or OWNED, this is the only survivor.

[mismatch] apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-connect-and-disconnect-routes-fire-their-own-lambdas
  Error: Check timed out after 60.0s

Expected (AWS): the watcher socket's {"action":"ping"} frame is answered on the same socket by the ping route's Lambda with its connectionId; opening a second socket pushes a watcher record with fn='$connect', routeKey='$connect', eventType='CONNECT' and a connectionId distinct from the watcher's; that socket's ping is answered under that same connectionId; closing it pushes a record with fn='$disconnect', routeKey='$disconnect', eventType='DISCONNECT' and the same connectionId; no non-lifecycle function pushes to the watcher.
Actual (emulator): no verdict — the check is killed at the 60 s harness cap while still waiting for the watcher's two-way ping round trip (its own timeout is 45 s), so none of the assertions above is reached. Connection ids, marker names and api ids in the check are per-run values; the requirement is the dispatch and the requestContext fields, not those literals.

## Steps to Reproduce
1. CreateApi(ProtocolType='WEBSOCKET', RouteSelectionExpression='$request.body.action').
2. Create four Lambda functions from one handler differing only in FN_TAG ('$connect', '$disconnect', '$default', 'ping'); the message routes return {statusCode:200, body: json(record)} where record carries requestContext.routeKey/eventType/connectionId and queryStringParameters.marker, and the lifecycle routes PostToConnection that record to a watcher connection.
3. CreateRoute '$connect' and '$disconnect' (one-way) and '$default' and 'ping' (two-way, with CreateRouteResponse '$default'), each with its own AWS_PROXY integration; grant lambda:InvokeFunction to apigateway.amazonaws.com; CreateDeployment + CreateStage.
4. Open wss://{api-id}.execute-api.{region}.amazonaws.com/{stage}?marker=watcher and send {"action":"ping"}. AWS replies on the same socket with the ping Lambda's record including the watcher's connectionId; the emulator returns nothing and the check runs to the 60 s cap here.
5. Point the $connect/$disconnect functions at the watcher connection id, open a second socket with ?marker=<marker>, and assert the watcher receives the CONNECT record (fn='$connect', eventType='CONNECT', new connectionId).
6. Send {"action":"ping"} on the second socket and assert the reply carries that connectionId; close it cleanly and assert the watcher receives the DISCONNECT record (fn='$disconnect', eventType='DISCONNECT', same connectionId) and that no other function pushed to the watcher.
7. After fixing, re-replay the id in trace_ids.
