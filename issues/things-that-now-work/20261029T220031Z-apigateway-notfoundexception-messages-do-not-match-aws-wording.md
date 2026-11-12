Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-integration-response-errors
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-integration-response-errors apigatewayv2-portal-product-error-paths apigatewayv2-product-page-error-paths apigatewayv2-portal-product-page-errors apigatewayv2-product-rest-endpoint-page-errors apigatewayv2-route-response-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|GetIntegrationResponse|4xx->4xx|$.message
Issue-Key: apigateway|GetPortalProduct|4xx->4xx|$.message
Issue-Key: apigateway|UpdatePortalProduct|4xx->4xx|$.message
Issue-Key: apigateway|CreateProductPage|4xx->4xx|$.message
Issue-Key: apigateway|GetProductPage|4xx->4xx|$.message
Issue-Key: apigateway|DeleteProductRestEndpointPage|4xx->4xx|$.message
Issue-Key: apigateway|GetRouteResponse|4xx->4xx|$.message
Issue-Key: apigateway|UpdateRouteResponse|4xx->4xx|$.message
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# apigateway NotFoundException messages do not match AWS wording

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetIntegrationResponse

## Description
Eight lookup refusals return the right status (404) but an emulator-authored message instead of AWS's. The message is part of the error contract, so each cause below gives AWS's template with its fixed text and the parts interpolated from the request or state.

Cause 1 of 8: GetIntegrationResponse with an unknown integration-response id.
AWS: 404, message `IntegrationResponse with ID <integrationResponseId> is not present in integration with ID <integrationId>` - fixed text `IntegrationResponse with ID ` / ` is not present in integration with ID `, both ids from the request path.
Emulator: `Invalid IntegrationResponse identifier specified` - no ids, different wording.
Why: the message is the only place AWS tells the caller WHICH id in WHICH integration was not found; a client surfacing or parsing it loses that.
Fix: the GetIntegrationResponse not-found message.
Traces: apigatewayv2-integration-response-errors#task#5.

Cause 2 of 8: GetPortalProduct with an unknown product id.
AWS: 404, message `Product not found in account <accountId> with id <portalProductId>` - fixed text `Product not found in account ` / ` with id `, account id from the caller's credentials and product id from the request path.
Emulator: `Invalid PortalProduct identifier specified`.
Why/Fix: the GetPortalProduct not-found message.
Traces: apigatewayv2-portal-product-error-paths#task#2.

Cause 3 of 8: UpdatePortalProduct with an unknown product id.
AWS: 404, message `Product not found in account <accountId> with id <portalProductId>`.
Emulator: `Invalid PortalProduct identifier specified`.
Why/Fix: the UpdatePortalProduct not-found message.
Traces: apigatewayv2-portal-product-error-paths#task#3.

Cause 4 of 8: CreateProductPage under an unknown product id.
AWS: 404, message `Product not found in account <accountId> with id <portalProductId>` - the parent product is named, not the page.
Emulator: `Invalid PortalProduct identifier specified`.
Why/Fix: the CreateProductPage parent-resolution message.
Traces: apigatewayv2-portal-product-error-paths#task#4.

Cause 5 of 8: GetProductPage with an unknown page id.
AWS: 404, message `ProductPage not found` - the whole sentence is fixed text, with no id interpolated.
Emulator: `Invalid ProductPage identifier specified`.
Why/Fix: the GetProductPage not-found message.
Traces: apigatewayv2-portal-product-error-paths#task#6.

Cause 6 of 8: DeleteProductRestEndpointPage under an unknown product id.
AWS: 404, message `Product not found in account <accountId> with id <portalProductId>` - AWS resolves and names the PARENT product that does not exist.
Emulator: `ProductRestEndpointPage not found` - it reports the child as missing instead of the parent.
Why: the caller is told the wrong resource is missing and retries the wrong fix.
Fix: the DeleteProductRestEndpointPage parent-resolution order and message.
Traces: apigatewayv2-product-rest-endpoint-page-errors#task#4.

Cause 7 of 8: GetRouteResponse with an unknown route-response id.
AWS: 404, message `Invalid route response specified` - the whole sentence is fixed text (note the lower-case `route response`).
Emulator: `Invalid RouteResponse identifier specified`.
Why/Fix: the GetRouteResponse not-found message.
Traces: apigatewayv2-route-response-errors#task#5.

Cause 8 of 8: UpdateRouteResponse with an unknown route-response id.
AWS: 404, message `Invalid route response specified`.
Emulator: `Invalid RouteResponse identifier specified`.
Why/Fix: the UpdateRouteResponse not-found message.
Traces: apigatewayv2-route-response-errors#task#6.

## Evidence
Replay verdict `mismatch` on all eight clusters with expected_status == actual_status == 404 and `$.message: value_mismatch (expected '<AWS text>', got '<emulator text>')`, e.g. expected `IntegrationResponse with ID nope-1 is not present in integration with ID <integrationId>` vs got `Invalid IntegrationResponse identifier specified`; expected `Product not found in account <accountId> with id <productId>` vs got `Invalid PortalProduct identifier specified`; expected `ProductPage not found` vs got `Invalid ProductPage identifier specified`; expected `Invalid route response specified` vs got `Invalid RouteResponse identifier specified`.

## Steps to Reproduce
Replay the corpus. Cause 1: create an integration, then GET /v2/apis/{apiId}/integrations/{integrationId}/integrationresponses/nope-1 (fixture apigatewayv2-integration-response-errors step task#5) and read the `message` field of the 404 body. Repeat per listed fixture#step.
