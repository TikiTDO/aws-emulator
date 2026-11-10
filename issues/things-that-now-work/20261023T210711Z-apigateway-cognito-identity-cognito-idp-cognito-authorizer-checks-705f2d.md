Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-expired-token-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-valid-token-positive apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-wrong-user-pool-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-expired-token-negative|_wait_api_ready#9|AssertionError|raise AssertionError(f"API never became ready (last response={last})")
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative|_wait_api_ready#7|AssertionError|raise AssertionError(f"API never became ready (last response={last})")
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-valid-token-positive|_wait_api_ready#7|AssertionError|raise AssertionError(f"API never became ready (last response={last})")
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-wrong-user-pool-negative|_wait_api_ready#11|AssertionError|raise AssertionError(f"API never became ready (last response={last})")
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative|_poll_invoke#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive|_poll_invoke#17|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__cognito-identity__cognito-idp: Cognito-authorizer checks never reach a deployed stage — 4 get 404 UnknownService and 2 hang to the 60 s cap (2 causes)

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Two root causes, both in the API Gateway data plane. Open issue 20261023T170254Z-apigateway-cognito-identity-cognito-idp-cognito-authorizer-checks.md covers the TestInvokeAuthorizer/CreateAuthorizer 501 stubs in this service and does not carry the six replay keys below.

Cause 1 of 2: the deployed stage answers 404 UnknownService, so the API never becomes reachable.
AWS: a REST API deployed to a stage with a COGNITO_USER_POOLS authorizer answers at its invoke URL — 200 with the integration's response for a valid ID token, 401 for a missing/invalid/expired token, 403 for a token from a user pool that is not the authorizer's provider.
Emulator: every request to the stage URL returns HTTP 404 with body `{"__type":"UnknownService","message":"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}`. The checks poll for readiness and give up reporting "API never became ready". The request is being handled by the emulator's generic AWS-API front door, not the API Gateway data plane, so the error code `UnknownService` — which API Gateway never emits — is returned for the positive and the negative legs alike.
Why: a client cannot call an API it just deployed, and because valid, invalid, expired and wrong-pool tokens all produce the identical 404, no Cognito authorization decision can be distinguished.
Fix: the emulator's front-door request routing — bind the execute-api stage endpoint to the API Gateway data plane.
Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-expired-token-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-invalid-token-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-valid-token-positive, apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-wrong-user-pool-negative.

Cause 2 of 2: two of the same scenarios hang instead of answering.
AWS: as above, every request to a deployed stage produces an HTTP response.
Emulator: no response arrives at all; both checks exhaust the harness budget and are reported as "Check timed out after 60.0s".
Why: the caller stalls rather than failing, so even the negative Cognito paths (an audience that belongs to a second user pool, an app-client-id that must be validated) cannot be observed.
Fix: the execute-api data-plane request path — it must always produce a response.
Traces: apigateway__cognito-identity__cognito-idp.apigw-cognito-authorizer-multiple-user-pools-audience-negative, apigateway__cognito-identity__cognito-idp.apigw-cognito-user-pool-client-app-client-id-validation-positive.

## Evidence
Cause 1: "AssertionError: API never became ready (last response=(404, '{\"__type\":\"UnknownService\",\"message\":\"could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)\"}'))" — four checks.
Cause 2: detail is exactly "Error: Check timed out after 60.0s" — two checks.

## Steps to Reproduce
1. Create a Cognito user pool + app client, create a REST API with a COGNITO_USER_POOLS authorizer on a method, deploy to a stage.
2. GET the stage invoke URL with an Authorization header carrying a pool-issued ID token.
3. Observe HTTP 404 with the UnknownService body (cause 1) or no response within 60 s (cause 2).
4. Or replay the check ids in trace_ids.
