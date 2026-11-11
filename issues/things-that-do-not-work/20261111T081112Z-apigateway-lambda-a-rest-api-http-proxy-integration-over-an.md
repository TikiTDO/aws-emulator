Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden
Issue-Key: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden|run#9|AssertionError|assert status == 403, f"stage returned {status!r} {body!r}; expected the function URL's 403"
Issue-Commit: f4372f081be041f1c06808452ec2531466fafcb7

# A REST API HTTP_PROXY integration over an AWS_IAM function URL answers 500 instead of passing the URL's 403 through

> _Note: this issue was identified against the code base at commit `f4372f081be041f1c06808452ec2531466fafcb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
Cause 1 of 1: an HTTP_PROXY integration whose URI is a Lambda function URL with AuthType=AWS_IAM swallows the function URL's refusal and answers with API Gateway's own 500.
AWS: recorded behavior only - an HTTP_PROXY integration forwards the request to the configured endpoint and passes the endpoint's response through unchanged. API Gateway does not sign the forwarded request, so an AWS_IAM function URL refuses it with HTTP 403, and the stage returns that 403 with the function URL's body. API Gateway only substitutes its own `{"message": "Internal server error"}` with HTTP 500 when the integration itself fails (unreachable endpoint, malformed response).
Emulator: the stage answers HTTP 500 with body `{"message":"Internal server error"}` - the integration result is replaced by API Gateway's generic failure instead of the endpoint's 403 being proxied. The fixed text here is API Gateway's `{"message": "Internal server error"}`; the response that should appear is whatever the function URL returned (status 403, with the function URL's AccessDenied body).
Why: a client cannot tell an authorization refusal at the proxied endpoint from a broken integration, so every non-2xx from an HTTP_PROXY backend looks like a gateway fault and the client retries or escalates wrongly.
Fix: the HTTP_PROXY integration response path of the execute-api data plane - the endpoint's status, headers and body must be passed through, and the 500 substitution reserved for an integration failure.

Traces: apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden.

## Evidence
Expected: stage returns 403 (the function URL's refusal, passed through by the HTTP_PROXY integration).
Actual: AssertionError: stage returned 500 '{"message":"Internal server error"}'; expected the function URL's 403.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite.
2. CreateFunction, then CreateFunctionUrlConfig with AuthType=AWS_IAM.
3. Create a REST API with a method whose integration type is HTTP_PROXY and whose uri is that function URL; deploy it to a stage.
4. Send an unsigned request to the stage URL and read the status and body - expect the function URL's 403, not 500 '{"message":"Internal server error"}'.
5. Re-replay apigateway__lambda.xsvc-apigateway-3-http-proxy-iam-function-url-unsigned-forward-forbidden.
