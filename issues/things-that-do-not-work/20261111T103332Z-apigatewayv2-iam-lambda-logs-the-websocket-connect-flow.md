Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers
Issue-Key: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers|_ws_attempt#15|Timeout|chunk = sock.recv(4096)
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# apigatewayv2__iam__lambda__logs: the WebSocket $connect flow never completes — the check asserting the CONNECT event's direction, domain, stage and handshake headers times out with no response

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Cause 1 of 1: a WebSocket API's $connect route never produces a CONNECT invocation the client can observe, so the whole handshake-driven flow stalls.
AWS: for a deployed WebSocket API (ProtocolType WEBSOCKET) the execute-api endpoint answers the client's HTTP/1.1 Upgrade request with 101 Switching Protocols after the $connect integration returns successfully, and the $connect integration is invoked with a Lambda proxy event whose requestContext carries `connectionId`, `eventType: "CONNECT"`, `messageDirection: "IN"`, `domainName` (the API's execute-api host) and `stage` (the deployed stage name), plus the handshake request's headers in `headers`/`multiValueHeaders`. Those fields are the only way a backend knows which connection, stage and domain to call back on.
Emulator: the check never gets that far. It is reported `[mismatch] ... Error: Check timed out after 60.0s` — the flow makes no observable progress within its budget, so no CONNECT event is ever available to assert against. The sibling check in this same section, `websocket-header-mapping-connect-only`, times out identically and is owned by the open issue 20261111T090000Z-apigatewayv2-iam-lambda-logs-the-front-door-cannot-upgrade-the-connect-handshake.md, which reports that the front door cannot upgrade the $connect handshake at all; the same unserved upgrade is the plain reading of this timeout too. That issue does not carry THIS check id, so this check is not covered by it and the CONNECT event's own contract (direction, domain, stage, handshake headers) is not pinned by anything filed.
Why: no client can use a WebSocket API. The handshake never completes, so no connection id is minted, nothing is delivered to the $connect integration, and a backend that depends on `messageDirection`, `domainName` and `stage` to post back through @connections has nothing to read. A timeout also gives the caller no error to branch on — it is indistinguishable from a hung network.
Fix: the apigatewayv2 execute-api front door's WebSocket path — complete the HTTP Upgrade handshake for a deployed WEBSOCKET API and invoke the $connect route's integration with the full proxy event (requestContext.connectionId/eventType=CONNECT/messageDirection=IN/domainName/stage and the handshake headers), rather than leaving the request unanswered.
Traces: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### apigatewayv2__iam__lambda__logs` section (1/3 passed).

[mismatch] apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers
  Error: Check timed out after 60.0s

The check performs the WebSocket handshake itself (raw socket + ssl + base64 Sec-WebSocket-Key, per its imports) against the deployed WebSocket API's execute-api endpoint and then reads the $connect Lambda event out of CloudWatch Logs; it never reaches any assertion.

Same-section sibling, same symptom, already owned (listed here only as corroboration, not re-filed):
[mismatch] apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only — Error: Check timed out after 60.0s — OWNED by 20261111T090000Z-apigatewayv2-iam-lambda-logs-the-front-door-cannot-upgrade-the-connect-handshake.md

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. CreateApi with ProtocolType=WEBSOCKET, a $connect route with a Lambda proxy integration, and a deployed stage.
3. Open a TCP/TLS connection to the API's execute-api host and send the HTTP/1.1 Upgrade: websocket handshake for /<stage>.
4. AWS answers 101 Switching Protocols and the $connect integration receives an event whose requestContext has eventType 'CONNECT', messageDirection 'IN', the domainName and stage, and the handshake headers. The emulator produces no completed handshake and no CONNECT event within the budget.
5. Replay apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers; it ends 'Check timed out after 60.0s'.
