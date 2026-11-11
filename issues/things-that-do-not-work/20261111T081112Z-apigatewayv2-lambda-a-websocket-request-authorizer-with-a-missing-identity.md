Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401
Issue-Key: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401|_ws_handshake#9|Timeout|return status, head, None
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# A WebSocket REQUEST authorizer with a missing identity source never resolves the handshake

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAuthorizer

## Description
Cause 1 of 1: a WebSocket API $connect route behind a REQUEST authorizer whose declared identity source is absent from the handshake never produces a verdict.
AWS: recorded behavior only - when a REQUEST authorizer's IdentitySource is not present on the request, API Gateway does not invoke the authorizer and refuses the $connect handshake with HTTP 401 (Unauthorized); the WebSocket upgrade does not complete.
Emulator: the step never reaches a verdict - the handshake neither upgrades nor is refused, and the check ends `Check timed out after 60.0s`. No status is ever observable.
Why: a client whose client omits the authorizer's identity header gets a hanging connection attempt instead of the documented 401, so it cannot distinguish a misconfigured authorizer from an unreachable endpoint.
Fix: the $connect handshake path of the apigatewayv2 WebSocket front door - the identity-source precondition must short-circuit to 401 before any authorizer invocation, and must always answer the handshake.

Open issues 20261110T194854Z-apigatewayv2-lambda-2-net-new-causes-a-connect.md and 20261110T170829Z-apigatewayv2-lambda-7-net-new-causes-aws-iam.md cover neighbouring WebSocket causes but carry none of this check's replay key.

Traces: apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401.

## Evidence
Expected: the $connect handshake is refused with HTTP 401 without invoking the authorizer.
Actual: Error: Check timed out after 60.0s - the handshake never completes and no status is returned.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. Create a WebSocket API with a $connect route and a REQUEST authorizer whose IdentitySource names a header; deploy to a stage.
3. Open a WebSocket handshake to the stage URL WITHOUT that header and observe the HTTP status - expect 401.
4. Re-replay apigatewayv2__lambda.xsvc-apigatewayv2-2-ws-request-authorizer-missing-identity-source-unauthorized-401.
