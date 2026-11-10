Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers
Issue-Key: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# apigatewayv2__iam__lambda__logs: a WebSocket $connect handshake never reaches the integration, so route-level request-parameter mapping and the $connect event shape are both ungradeable (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRoute

## Description
Cause 1 of 1: a signed/plain RFC 6455 handshake to a deployed WEBSOCKET stage never completes, so neither the integration's RequestParameters header mapping nor the $connect event body can be observed.
AWS: a handshake to the stage endpoint is answered 101 Switching Protocols and the $connect integration is invoked; the integration's RequestParameters mapping 'integration.request.header.<dst>' = 'route.request.header.<src>' delivers the client's handshake header to the backend, and the $connect Lambda event carries requestContext with connectionId, domainName, stage, eventType CONNECT, messageDirection IN and the handshake headers. The identical route-level RequestParameters declaration on a non-$connect route is refused 400 BadRequestException 'Request parameters are only supported for the $connect route in WEBSOCKET APIs' (Smithy apigatewayv2 CreateRoute errors: BadRequestException @httpError 400, NotFoundException 404, TooManyRequestsException 429, ConflictException 409).
Emulator: both checks produce no verdict at all — 'Error: Check timed out after 60.0s'. The handshake never yields a logged backend event and the non-$connect refusal is never reached.
Why: a client cannot establish a WebSocket connection, so no $connect data mapping, event shape or route-parameter restriction can be relied on; a caller also never sees the modelled refusal for a parameter declared off $connect.
Fix: the apigatewayv2 execute-api WEBSOCKET handshake path (upgrade + $connect dispatch with RequestParameters mapping) and CreateRoute's route-level request-parameter validation. The open issue 20261109T143515Z-apigatewayv2-iam-lambda-logs-websocket-connect-handshake-to-a-deployed-stage-answers.md describes a 500-answered handshake for this family but does not carry these two checks' current replay keys (they now hang rather than answer 500), so it does not cover them.
Traces: apigatewayv2__iam__lambda__logs.websocket-header-mapping-connect-only, apigatewayv2__iam__lambda__logs.ws-connect-event-carries-direction-domain-stage-and-handshake-headers.

## Evidence
Both checks end with exactly 'Error: Check timed out after 60.0s'. Expected: 101 Switching Protocols on the handshake, the backend logging an event whose headers contain the mapped destination header, and CreateRoute on a non-$connect route refused 400 BadRequestException with the message 'Request parameters are only supported for the $connect route in WEBSOCKET APIs' (fixed text; no variable parts). Actual: nothing is served and nothing is refused inside the cap.

## Steps to Reproduce
Create a WEBSOCKET API with a $connect route declaring 'route.request.header.<src>' and an integration mapping it to 'integration.request.header.<dst>', deploy a stage, then open a handshake carrying that header and read the backend's logged event; then CreateRoute a non-$connect route with the same RequestParameters and inspect the refusal.
