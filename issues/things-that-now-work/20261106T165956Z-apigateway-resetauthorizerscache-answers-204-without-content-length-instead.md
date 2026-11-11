Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-http-api-cors-accesslog-cache-cleanup
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-http-api-cors-accesslog-cache-cleanup apigatewayv2-reset-authorizers-cache
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|ResetAuthorizersCache|2xx->2xx|$headers.content-length
Issue-Key: apigateway|ResetAuthorizersCache|2xx->2xx|status
Issue-Commit: 73f8eaecfe0448dfa40697a75965cd99973ceee3

# apigateway ResetAuthorizersCache answers 204 without Content-Length instead of AWS's 200

> _Note: this issue was identified against the code base at commit `73f8eaecfe0448dfa40697a75965cd99973ceee3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResetAuthorizersCache

## Description
Cause 1 of 1 — ResetAuthorizersCache, as served under the `apigateway` service token, uses the wrong success framing.
AWS: Smithy `com.amazonaws.apigatewayv2#ResetAuthorizersCache` is `DELETE /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers` with output `Unit` and modelled errors NotFoundException (404) and TooManyRequestsException (429). The model's `@http` code is 204, but the recording of the real service answers a successful cache flush with HTTP **200** and a `Content-Length` response header present (the response has no modelled members, so the header's value is simply the byte length of the body the service writes). The recorded status is the requirement.
Emulator: answers **204** and emits no `Content-Length` header.
Why: the operation returns no members, so the status line is the only signal a caller has that the authorizer cache was flushed; a client (or an SDK response handler, or an HTTP intermediary that treats a 204 as "no entity, no entity headers") that expects the recorded 200-with-framed-body outcome observes a different result from AWS on the success path, and cannot distinguish the emulator's framing-less reply from a no-op.
Fix: the ResetAuthorizersCache handler/serializer for the API Gateway v2 surface served under the `apigateway` token — emit 200 with the body framed by Content-Length rather than a bare 204.
Traces: apigatewayv2-http-api-cors-accesslog-cache-cleanup#task#8, apigatewayv2-reset-authorizers-cache#task#5.

Not owned by any open issue: `20261029T220031Z-apigateway-204-responses-omit-the-content-length-0.md` owns the missing `Content-Length: 0` on operations whose recorded status really is 204 (DeleteApiMapping, DeleteAuthorizer, DeleteCorsConfiguration, DeleteDeployment, DeleteAccessLogSettings, DeleteIntegration) and `20261031T154120Z-apigateway-content-length-0-missing-on-204-deletes-for.md` covers the same family; neither carries this fixture step, and neither addresses an operation whose status itself is wrong (200 expected, 204 served).

## Evidence
Replay of trace set simparity-wire-aws-v1 (106 fixtures, 899 steps, score 75.5%). One cluster with service=apigateway, no owning issue and no blocking stub:

- ResetAuthorizersCache — fixture apigatewayv2-http-api-cors-accesslog-cache-cleanup, step task#8, expected_status 200 / actual_status 204. detail: `status: expected 200, got 204` and `$headers.content-length: missing (expected '<shaped-not-asserted>', got None)`. affected_count 2; the cluster's other member is apigatewayv2-reset-authorizers-cache#task#5, which shows the identical 200 -> 204 divergence.

Smithy consulted (services/service-com-amazonaws-apigatewayv2-apigatewayv2/smithy-model/com-amazonaws-apigatewayv2-apigatewayv2.json): ResetAuthorizersCache — input ApiId/StageName both @httpLabel @required, output Unit, errors NotFoundException (404) / TooManyRequestsException (429), http { method: DELETE, uri: /v2/apis/{ApiId}/stages/{StageName}/cache/authorizers, code: 204 }. The recorded response status is 200, so the modelled 204 is not what the service emits.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture apigatewayv2-http-api-cors-accesslog-cache-cleanup and inspect step task#8 — `DELETE /v2/apis/{apiId}/stages/{stageName}/cache/authorizers` returns 204 with no Content-Length header.
3. Replay fixture apigatewayv2-reset-authorizers-cache step task#5 for the same divergence on a dedicated fixture.
4. Expected: HTTP 200 with a Content-Length header framing the (member-less) body.
