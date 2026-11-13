Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers
Issue-Key: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers|_ws_attempt#15|Timeout|return status, head, body.decode("utf-8", "replace")
Issue-Commit: 455edd8297574e8c9cd5843518c18f9edb691c55

# apigatewayv2 WebSocket: the endpoint under test never answers 101 Switching Protocols, so the $connect event shape (eventType/messageDirection/domainName/stage/handshake headers) is never observable

> _Note: this issue was identified against the code base at commit `455edd8297574e8c9cd5843518c18f9edb691c55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
WebSocket upgrade ($connect, execute-api data plane)

## Description
This issue carries 1 root cause, in the front door that serves the execute-api data plane for a WEBSOCKET api.

Cause 1 of 1: an RFC 6455 opening handshake sent to the endpoint under test for a deployed WEBSOCKET api is never upgraded, so the check polls for a live socket until the suite's 60 s cap and the $connect event it must read is never produced.

AWS: recorded behavior only (the Smithy model for com.amazonaws.apigatewayv2 covers the control plane; the upgrade itself is HTTP, not a modeled operation). AWS answers the handshake on the address the api announces in ApiEndpoint with `HTTP/1.1 101 Switching Protocols` carrying the `Upgrade: websocket`, `Connection: Upgrade` and `Sec-WebSocket-Accept` fields RFC 6455 §4.2.2 requires, then runs the $connect route's AWS_PROXY integration with the WebSocket $connect event — `requestContext` carrying `connectionId`, `domainName`, `stage`, `eventType` `CONNECT`, `messageDirection` `IN`, and the handshake's headers (after the integration's RequestParameters mapping) — and admits or refuses the connection per the status that function returns. Variable parts of that event (connection id, domain name, stage name, header values) come from the request and from the api's own records; the member names and the CONNECT/IN values are the requirement.

Emulator: the handshake is answered from the api's records but never upgraded on the endpoint the check reaches; the replay ends `Error: Check timed out after 60.0s` while polling for the upgraded socket. No 101, no $connect invocation, no connection id a later PostToConnection could address.

Why: a client relying on AWS's behavior opens a WebSocket against the api's endpoint and expects a carried connection plus a $connect invocation. Against the emulator the connect hangs and then fails, so no WebSocket workload — handshake authorization, header mapping onto the integration, $connect/$disconnect bookkeeping, server-to-client sends — can be exercised at all.

Fix: the core front door / execute-api data plane path, not a service handler: the request path that claims the api's execute-api traffic needs an upgrade seam (a hook that lets the claiming service answer 101 and then receive the upgraded byte stream) and a response path that does not strip a 101's hop-by-hop fields, after which the api's existing WebSocket serving code can carry the frames.

Traces: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers (timeout at the handshake poll). SAME CAUSE as the open report 20261111T090000Z-apigatewayv2-iam-lambda-logs-the-front-door-cannot-upgrade-the-connect-handshake.md (which also names 20261107T120000Z-apigatewayv2-the-front-door-cannot-switch-a-websocket-handshake-to-the-apis-own-socket.md); this check is re-filed because the suite report does NOT mark it owned — that report's recorded replay key for this check (`_poll_until#15|Timeout`) no longer matches the failure as replayed, so nothing currently carries this check's key. Close it together with that report if the single upgrade seam fixes both.

## Evidence
Failing check: [mismatch] apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers

Replay detail (end): `Error: Check timed out after 60.0s`

The check (its source is carried in the suite report) creates a WEBSOCKET api, a $connect handler, an AWS_PROXY integration and a $connect route, deploys a stage, then opens an RFC 6455 handshake and polls for the upgraded socket to read the $connect event's requestContext (eventType CONNECT, messageDirection IN, domainName, stage) and the handshake headers.

Expected (AWS): 101 Switching Protocols with Upgrade/Connection/Sec-WebSocket-Accept, then the $connect integration invoked with that event.
Actual (emulator): no upgrade on the endpoint reached; the check polls until the 60 s cap and reports a timeout. The sibling check apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only fails the same way and is owned by 20261111T090000Z-apigatewayv2-iam-lambda-logs-the-front-door-cannot-upgrade-the-connect-handshake.md.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateApi(ProtocolType='WEBSOCKET'); CreateFunction for a $connect handler returning {'statusCode': 200}; CreateIntegration(IntegrationType='AWS_PROXY', IntegrationUri=<function>); CreateRoute(RouteKey='$connect', Target='integrations/<id>'); CreateStage; CreateDeployment.
3. Send an RFC 6455 opening handshake for that api's deployed stage to the endpoint under test (the address the suite's clients are pointed at, with the api id in the request target).
4. Expect 101 Switching Protocols and the $connect integration invoked with requestContext.eventType='CONNECT', messageDirection='IN', the api's domainName and stage, plus the handshake headers; observe no upgrade and a hang until the 60 s cap.
5. Re-replay the check id in trace_ids.
