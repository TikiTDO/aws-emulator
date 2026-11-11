Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-product-rest-endpoint-page-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-product-rest-endpoint-page-error-paths apigatewayv2-product-rest-endpoint-page-errors apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateProductRestEndpointPage|4xx->2xx|$.displayContent.endpoint
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-3f46c6a5"].vpcLinkStatusMessage
Issue-Related: 20261029T220031Z-apigateway-control-plane-accepts-invalid-input-and-rejects.md
Issue-Commit: c4fcf6952896a9081c9de16adffed799f1736bf9

# apigateway admits an unresolvable restEndpointIdentifier and reworded VPC link status text

> _Note: this issue was identified against the code base at commit `c4fcf6952896a9081c9de16adffed799f1736bf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateProductRestEndpointPage

## Description
Two net-new causes: one missing refusal and one reworded status string. Neither is covered by an existing open issue carrying these replay keys.

Cause 1 of 2: CreateProductRestEndpointPage accepts a restEndpointIdentifier that names no REST API or stage.
AWS: Smithy: CreateProductRestEndpointPage input member RestEndpointIdentifier (target RestEndpointIdentifier, jsonName 'restEndpointIdentifier', required), errors BadRequestException (httpError 400), NotFoundException (404); http POST /v2/portalproducts/{PortalProductId}/productrestendpointpages code 201. When the identifier's restApiId/stage pair does not resolve, AWS refuses with HTTP 400, header x-amzn-errortype 'BadRequestException' and body message 'Invalid restEndpointIdentifier stage or restApiId' — that text is fixed, with no variable parts.
Emulator: answers 201 and creates the page, with no message and no x-amzn-errortype header, echoing the caller's unresolvable reference into displayContent.endpoint (recorded 'https://abcd1234.execute-api.us-west-2.amazonaws.com/prod', a host it does not serve, verdict host_not_own).
Why: the refusal is the whole point of the step — a client validating its portal wiring is told the bad reference is fine, then publishes a product page pointing at an API that does not exist, and no error-type header is available for SDK error mapping.
Fix: CreateProductRestEndpointPage input validation — resolve restEndpointIdentifier against stored REST APIs/stages before admitting the page and raise BadRequestException (400, x-amzn-errortype: BadRequestException, message 'Invalid restEndpointIdentifier stage or restApiId') otherwise.
Traces: apigatewayv2-product-rest-endpoint-page-error-paths#task#2 (also apigatewayv2-product-rest-endpoint-page-errors#task#2).

Cause 2 of 2: GetVpcLinks reports a non-AWS statusMessage for an AVAILABLE VPC link.
AWS: Smithy (apigateway): VpcLink member statusMessage, documented 'A description about the VPC link status'; GetVpcLinks http GET /vpclinks code 200. Recorded behavior: for a VPC link in status AVAILABLE, AWS's statusMessage is the fixed text 'VPC link is ready to route traffic' — no variable parts.
Emulator: returns 'The VPC link is available for use by an integration.'
Why: the status message is the human- and script-readable explanation of the link's state; a client (or a snapshot/diff test) that matches on AWS's text sees no match and cannot tell an AVAILABLE link from an unexpected state.
Fix: the VpcLink state/serializer — the per-status statusMessage table used by GetVpcLinks/GetVpcLink/CreateVpcLink.
Traces: apigatewayv2-vpc-link-errors#task#4.

## Evidence
Replay of simparity-wire-aws-v1, clusters with empty stub_path and empty owned_by:

1) apigateway|CreateProductRestEndpointPage|status=400->201, fixture apigatewayv2-product-rest-endpoint-page-error-paths step task#2 (2 affected steps) — status: expected 400, got 201; $.message: missing (expected 'Invalid restEndpointIdentifier stage or restApiId'); $headers.x-amzn-errortype: missing (expected 'BadRequestException'); $.displayContent.endpoint: host_not_own (expected absent, got 'https://abcd1234.execute-api.us-west-2.amazonaws.com/prod').
2) apigateway|GetVpcLinks|status=200->200, fixture apigatewayv2-vpc-link-errors step task#4 — $.items[name="sim2real-wire-vpclink-3f46c6a5"].vpcLinkStatusMessage: expected 'VPC link is ready to route traffic', got 'The VPC link is available for use by an integration.'

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1.
2. POST /v2/portalproducts/{portalProductId}/productrestendpointpages with a restEndpointIdentifier whose restApiId/stage does not exist: expect 400 + x-amzn-errortype BadRequestException + message 'Invalid restEndpointIdentifier stage or restApiId'; the emulator answers 201 with a created page. Replay apigatewayv2-product-rest-endpoint-page-error-paths#task#2.
3. Create a VPC link, wait for status AVAILABLE, then GET /vpclinks and read the entry's statusMessage — it is emulator prose instead of AWS's 'VPC link is ready to route traffic'. Replay apigatewayv2-vpc-link-errors#task#4.
