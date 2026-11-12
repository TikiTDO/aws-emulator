Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.apigw-concurrent-invocations-through-one-stage-keep-per-request-identity apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage apigateway.apigw-iam-auth-unsigned-request-rejected-at-stage apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage apigateway.apigw-invoke-lambda-proxy apigateway.apigw-response-override-status-requires-numeric-value apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it apigateway.apigw-stage-variable-selects-a-different-backend-per-stage-from-one-deployment apigateway.apigw-strict-passthrough-rejects-an-unmapped-content-type-with-415 apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body apigateway.authorizer-ttl-caches-result-within-window apigateway.authorizer-ttl-zero-disables-caching apigateway.canary-deployment-traffic-shift-and-promotion apigateway.flush-caches-bypass-ttl apigateway.flush-stage-authorizers-cache-bypasses-ttl apigateway.flush-stage-authorizers-cache-forces-reinvoke apigateway.greedy-proxy-and-path-parameter-routing apigateway.request-validator-rejects-body-and-params-at-stage apigateway.deploy-rest-api-cfn-behaves apigateway.flush-stage-cache-bypasses-ttl apigateway.flush-stage-cache-clears-responses apigateway.apigw-request-parameter-required-rejection-happens-before-the-integration apigateway.test-invoke-method-executes-configured-integration apigateway.test-invoke-method-executes-integration-server-side apigateway.mock-selection-pattern-selects-status-and-headers apigateway.put-integration-response-selection-pattern-server-evaluated apigateway.apigw-gateway-response-customization-is-served-for-an-unmatched-route apigateway.gateway-response-default-4xx-customized-at-stage apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings apigateway.apigw-header-parameter-required-rejected-case-insensitive-match apigateway.apigw-payload-over-the-ten-megabyte-limit-rejected-with-413 apigateway.apigw-response-compression-gzips-above-the-minimum-and-not-below apigateway.apigw-response-template-selected-by-accept-header apigateway.e2e-request-validator-model-enforced-on-invoke apigateway.test-invoke-method-bad-request
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.apigw-concurrent-invocations-through-one-stage-keep-per-request-identity|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed|_poll_http#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it|_poll_http#26|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-iam-auth-unsigned-request-rejected-at-stage|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-invoke-lambda-proxy|run#37|Timeout|time.sleep(3)
Issue-Key: apigateway.apigw-response-override-status-requires-numeric-value|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it|_poll_body#30|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-stage-variable-selects-a-different-backend-per-stage-from-one-deployment|_poll_http#23|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-strict-passthrough-rejects-an-unmapped-content-type-with-415|_poll_http#25|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body|_poll_http#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.authorizer-ttl-caches-result-within-window|_http_get_retry#21|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.authorizer-ttl-zero-disables-caching|_wait_http_ok#33|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.canary-deployment-traffic-shift-and-promotion|_poll_http#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.flush-caches-bypass-ttl|_get_json_with_retries#13|Timeout|time.sleep(3)
Issue-Key: apigateway.flush-stage-authorizers-cache-bypasses-ttl|_get_json_with_retries#19|Timeout|time.sleep(3)
Issue-Key: apigateway.flush-stage-authorizers-cache-forces-reinvoke|_http_get_retry#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.greedy-proxy-and-path-parameter-routing|_poll_http#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.request-validator-rejects-body-and-params-at-stage|_poll_http#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.deploy-rest-api-cfn-behaves|_poll_http#7|Timeout|time.sleep(interval_s)
Issue-Key: apigateway.flush-stage-cache-bypasses-ttl|run#21|AssertionError|raise _never_ready(str(exc), is_real=ctx.is_real_aws) from exc
Issue-Key: apigateway.flush-stage-cache-clears-responses|_http_get_retry#20|AssertionError|raise AssertionError(f"endpoint never returned 200 within {timeout_s}s; last={last!r}")
Issue-Key: apigateway.apigw-request-parameter-required-rejection-happens-before-the-integration|run#23|AssertionError|assert good_status == 200, (
Issue-Key: apigateway.test-invoke-method-executes-configured-integration|run#9|AssertionError|assert resp_alice["status"] == 200, f"expected status 200, got {resp_alice.get('status')!r}"
Issue-Key: apigateway.test-invoke-method-executes-integration-server-side|run#13|AssertionError|assert resp_alice["status"] == 200, f"expected status 200, got {resp_alice!r}"
Issue-Key: apigateway.mock-selection-pattern-selects-status-and-headers|run#20|AssertionError|assert status == 500, (
Issue-Key: apigateway.put-integration-response-selection-pattern-server-evaluated|run#16|AssertionError|assert err_resp["status"] == 500, f"expected 500, got {err_resp['status']}"
Issue-Key: apigateway.apigw-gateway-response-customization-is-served-for-an-unmatched-route|run#23|AssertionError|assert unknown_headers.get("x-sim2real-gwr") == marker, (
Issue-Key: apigateway.gateway-response-default-4xx-customized-at-stage|run#17|AssertionError|assert headers.get("x-sim-gw") == "default4xx", (
Issue-Key: apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings|run#23|AssertionError|assert headers.get("x-amzn-errortype", "").split(":", 1)[0] == "MissingAuthenticationTokenException", (
Issue-Key: apigateway.apigw-header-parameter-required-rejected-case-insensitive-match|run#17|AssertionError|assert text == '{"message": "Missing required request parameters: [X-Required]"}', (
Issue-Key: apigateway.apigw-payload-over-the-ten-megabyte-limit-rejected-with-413|run#24|AssertionError|assert over_status == 413, (
Issue-Key: apigateway.apigw-response-compression-gzips-above-the-minimum-and-not-below|run#21|AssertionError|assert big_headers.get("content-encoding") == "gzip", (
Issue-Key: apigateway.apigw-response-template-selected-by-accept-header|run#14|AssertionError|assert headers.get("content-type", "").split(";")[0] == "application/json", (
Issue-Key: apigateway.e2e-request-validator-model-enforced-on-invoke|run#17|AssertionError|assert invalid_invoke["status"] == 400, (
Issue-Key: apigateway.test-invoke-method-bad-request|run#16|AssertionError|assert resp["status"] == 400, (
Issue-Commit: c8d80ea15f1454d07f388eb2b932b3a5430349f4

# API Gateway execute-api data plane: 10 root causes (invocations hang, VTL/selectionPattern unevaluated, gateway-response/compression/payload-limit/validator ignored)

> _Note: this issue was identified against the code base at commit `c8d80ea15f1454d07f388eb2b932b3a5430349f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestInvokeMethod

## Description
This issue bundles 10 NET-NEW root causes in the API Gateway data plane (the execute-api invoke path and TestInvokeMethod). A fixer must close all 10 before submitting.

Cause 1 of 10: a deployed stage invocation never returns.
AWS: an invocation of a deployed stage URL returns an HTTP response promptly (MOCK integrations answer in milliseconds); recorded behavior only for the latency, but a response always arrives.
Emulator: 21 checks that invoke a deployed stage never complete and the harness kills them with 'Check timed out after 60.0s'. No status line is ever produced, so nothing downstream of routing can be asserted.
Why: every client that calls a deployed REST API through the emulator blocks until its own timeout; no caller can distinguish a hang from a slow backend.
Fix: the execute-api invoke handler/router for deployed stages (request dispatch and response completion), not any one operation's validation.
Traces: apigateway.apigw-concurrent-invocations-through-one-stage-keep-per-request-identity, apigateway.apigw-extended-request-id-is-minted-per-api-per-request-not-echoed, apigateway.apigw-http-proxy-passes-cors-header-plain-http-integration-drops-it, apigateway.apigw-iam-auth-unknown-access-key-rejected-at-stage, apigateway.apigw-iam-auth-unsigned-request-rejected-at-stage, apigateway.apigw-iam-auth-wrong-secret-signature-rejected-at-stage, apigateway.apigw-invoke-lambda-proxy, apigateway.apigw-response-override-status-requires-numeric-value, apigateway.apigw-stage-cache-serves-a-stale-body-until-flush-invalidates-it, apigateway.apigw-stage-variable-selects-a-different-backend-per-stage-from-one-deployment, apigateway.apigw-strict-passthrough-rejects-an-unmapped-content-type-with-415, apigateway.apigw-vtl-request-and-response-templates-transform-a-real-body, apigateway.authorizer-ttl-caches-result-within-window, apigateway.authorizer-ttl-zero-disables-caching, apigateway.canary-deployment-traffic-shift-and-promotion, apigateway.flush-caches-bypass-ttl, apigateway.flush-stage-authorizers-cache-bypasses-ttl, apigateway.flush-stage-authorizers-cache-forces-reinvoke, apigateway.greedy-proxy-and-path-parameter-routing, apigateway.request-validator-rejects-body-and-params-at-stage, apigateway.deploy-rest-api-cfn-behaves

Cause 2 of 10: Velocity (VTL) request/response mapping templates are not evaluated; the method answers 500.
AWS: a MOCK or non-proxy integration whose requestTemplates/responseTemplates carry a Velocity template evaluates it and serves the rendered body with HTTP 200.
Emulator: the method answers HTTP 500 with body {"message":"Internal server error"} and the TestInvokeMethod execution log states 'Execution failed due to configuration error: the application/json response template of 42 bytes is a Velocity template, which this emulator does not evaluate: no body is invented for it' followed by 'Answered through this API's API_CONFIGURATION_ERROR gateway response'. In that message the fixed text is the configuration-error line and the API_CONFIGURATION_ERROR line; the byte count and content type come from the configured template.
Why: every REST API that uses mapping templates (the normal way to build a non-proxy integration) is unusable; clients see a 500 where AWS serves the mapped payload.
Fix: the integration/response template evaluation component of the invoke pipeline (shared by execute-api and TestInvokeMethod).
Traces: apigateway.flush-stage-cache-bypasses-ttl, apigateway.flush-stage-cache-clears-responses, apigateway.apigw-request-parameter-required-rejection-happens-before-the-integration, apigateway.test-invoke-method-executes-configured-integration, apigateway.test-invoke-method-executes-integration-server-side

Cause 3 of 10: integration-response selectionPattern is not evaluated server-side.
AWS: the integration response whose selectionPattern matches the backend result is selected, so a 5\d{2} branch surfaces as HTTP 500 on the wire.
Emulator: the default 200 integration response is always used; the request is answered 200 with body '{"route":"internal"}'.
Why: error mapping configured on a method is silently inert, so clients never see the status an API owner configured.
Fix: integration-response selection in the invoke pipeline.
Traces: apigateway.mock-selection-pattern-selects-status-and-headers, apigateway.put-integration-response-selection-pattern-server-evaluated

Cause 4 of 10: GatewayResponse responseParameters are not applied to served gateway responses.
AWS: a customized gateway response (e.g. DEFAULT_4XX) with responseParameters {"gatewayresponse.header.<Name>": "'<value>'"} emits that header on the response it serves.
Emulator: the header is absent (got None) for both an unmatched route and a stage-customized DEFAULT_4XX.
Why: CORS and diagnostic headers configured through gateway responses never reach the client, so browsers and clients that depend on them fail.
Fix: the gateway-response serializer in the invoke pipeline.
Traces: apigateway.apigw-gateway-response-customization-is-served-for-an-unmatched-route, apigateway.gateway-response-default-4xx-customized-at-stage

Cause 5 of 10: the x-amzn-ErrorType response header is missing on gateway error responses.
AWS: a request to a route with no matching method is refused with HTTP 403 and the response header x-amzn-ErrorType: MissingAuthenticationTokenException (the header value is the error class name; it is fixed text per error class).
Emulator: the header is absent (got None).
Why: SDKs and clients classify edge refusals from x-amzn-ErrorType; without it they cannot tell MissingAuthenticationTokenException from any other 403.
Fix: the gateway-response serializer (header emission) in the invoke pipeline.
Traces: apigateway.apigw-cors-preflight-served-from-mock-options-header-mappings

Cause 6 of 10: gateway error bodies are serialized without AWS's space after the JSON colon.
AWS: a missing required request parameter is refused with HTTP 400 and the exact body {"message": "Missing required request parameters: [X-Required]"} — fixed text '{"message": "Missing required request parameters: [' and ']}'; the bracketed list is the declared parameter names from the method's requestParameters.
Emulator: serves {"message":"Missing required request parameters: [X-Required]"} (no space after the colon), so a byte-exact body comparison fails.
Why: clients and proxies that compare or hash the error body see a different payload than AWS produces.
Fix: the gateway-response body serializer.
Traces: apigateway.apigw-header-parameter-required-rejected-case-insensitive-match

Cause 7 of 10: the 10 MB request payload limit is not enforced.
AWS: a request body above the documented, non-adjustable 10485760-byte REST API payload limit is refused at the edge with HTTP 413 before the integration runs.
Emulator: a 10551296-byte body is accepted and the integration answers HTTP 200 with the integration's body.
Why: clients can push payloads AWS rejects, so an application that relies on the limit for back-pressure behaves differently in the emulator.
Fix: request admission/limit check in the invoke pipeline, before integration dispatch.
Traces: apigateway.apigw-payload-over-the-ten-megabyte-limit-rejected-with-413

Cause 8 of 10: stage minimumCompressionSize is ignored; responses are never gzipped.
AWS: with minimumCompressionSize configured, a response body above the threshold is served with Content-Encoding: gzip and a body below it is served uncompressed.
Emulator: a 3941-byte body over a 1024-byte threshold is served with no Content-Encoding header at all (headers were content-type, content-length, x-amzn-requestid, connection, date).
Why: clients that negotiate compression get uncompressed bodies and cannot exercise the compression path.
Fix: response post-processing (content encoding) in the invoke pipeline.
Traces: apigateway.apigw-response-compression-gzips-above-the-minimum-and-not-below

Cause 9 of 10: selecting a response template changes the served Content-Type.
AWS: when no Content-Type response-parameter mapping is declared, the served Content-Type stays application/json even when the xml response template is selected by the Accept header; the template picks the body, not the header.
Emulator: serves Content-Type: application/xml.
Why: clients parse by Content-Type and will route an XML-templated body to an XML parser in the emulator and a JSON parser on AWS.
Fix: method-response header resolution in the invoke pipeline (do not derive Content-Type from the selected template key).
Traces: apigateway.apigw-response-template-selected-by-accept-header

Cause 10 of 10: a RequestValidator's JSON-schema model is not enforced, at the stage or in TestInvokeMethod.
AWS: a body that violates the model attached through the method's requestValidator is refused with HTTP 400 before the integration runs, both on a deployed stage and through TestInvokeMethod.
Emulator: the request is accepted; the stage answers 200 with {"message": "accepted"} and TestInvokeMethod reports status 200.
Why: invalid payloads reach the backend in the emulator and are rejected on AWS, so negative tests pass that must fail.
Fix: request-validation stage of the invoke pipeline (body model validation) and the same path used by TestInvokeMethod.
Traces: apigateway.e2e-request-validator-model-enforced-on-invoke, apigateway.test-invoke-method-bad-request

## Evidence
Representative replay output (verbatim):
- 'Error: Check timed out after 60.0s' for 21 stage-invocation checks (Cause 1).
- 'AssertionError: expected status 200, got 500' and execution log 'Execution failed due to configuration error: the application/json response template of 42 bytes is a Velocity template, which this emulator does not evaluate: no body is invented for it' / 'Answered through this API's API_CONFIGURATION_ERROR gateway response' (Cause 2).
- 'AssertionError: expected the selectionPattern 5\\d{2} branch to surface as a real 500 on the wire, got 200: '{"route":"internal"}'' (Cause 3).
- 'AssertionError: expected the gatewayresponse.header parameter to reach the client as X-Sim2real-Gwr: ..., got None' and 'expected the DEFAULT_4XX responseParameters static header mapping to emit X-Sim-Gw: default4xx, got None' (Cause 4).
- 'AssertionError: x-amzn-ErrorType must be MissingAuthenticationTokenException; got None' (Cause 5).
- 'AssertionError: unexpected rejection body: '{"message":"Missing required request parameters: [X-Required]"}'' against the asserted '{"message": "Missing required request parameters: [X-Required]"}' (Cause 6).
- 'AssertionError: expected 413 for a 10551296-byte body (65536 bytes over the documented, non-adjustable 10485760-byte payload limit), got 200' (Cause 7).
- 'AssertionError: expected Content-Encoding: gzip for a 3941-byte body over a 1024-byte threshold, got None' (Cause 8).
- 'AssertionError: with no Content-Type mapping the served Content-Type stays application/json even when the xml template is selected; got 'application/xml'' (Cause 9).
- 'AssertionError: expected 400 for schema-invalid body, got 200: {"message": "accepted"}' and 'expected a 400 response from TestInvokeMethod for a body violating the attached RequestValidator's JSON-schema model, got 200' (Cause 10).
Smithy: PutIntegration (PUT /restapis/{restApiId}/resources/{resourceId}/methods/{httpMethod}/integration) models requestTemplates/contentHandling/passthroughBehavior; PutMethodResponse models responseParameters/responseModels — the model carries the configuration whose runtime effect is missing.

## Steps to Reproduce
1. Create a REST API, a resource and a method; attach a MOCK or non-proxy integration with a Velocity requestTemplate/responseTemplate, an integration response with selectionPattern 5\\d{2}, a customized DEFAULT_4XX gateway response with a gatewayresponse.header.* parameter, a stage with minimumCompressionSize, and a RequestValidator with a JSON-schema model.
2. Create a deployment and a stage, then invoke the stage URL. Observe that the call never returns (Cause 1).
3. Call TestInvokeMethod on the same method and read 'status' and 'log': status 500 and the API_CONFIGURATION_ERROR / Velocity-not-evaluated lines (Cause 2), 200 instead of the selectionPattern 500 (Cause 3), 200 instead of 400 for a schema-invalid body (Cause 10).
4. Invoke a route with no matching method and inspect the response headers and body (Causes 4, 5, 6).
5. POST a body larger than 10485760 bytes (Cause 7); request a body above minimumCompressionSize (Cause 8); send Accept: application/xml to a method with an xml response template and read Content-Type (Cause 9).
6. Replay the check ids listed under each cause in trace_ids.
