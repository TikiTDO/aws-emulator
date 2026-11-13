Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.product-page-lifecycle apigatewayv2.product-page-lifecycle-crud
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.product-page-lifecycle|run#19|AssertionError|assert err == ("NotFoundException", "ProductPage not found"), (
Issue-Key: apigatewayv2.product-page-lifecycle-crud|run#18|AssertionError|assert err == ("NotFoundException", "ProductPage not found"), (
Issue-Commit: d2ebcd8c12cc6501ce5e652d8f3a2ce865548576

# ProductPage NotFoundException carries the api-plane "Invalid ProductPage identifier specified" sentence instead of "ProductPage not found"

> _Note: this issue was identified against the code base at commit `d2ebcd8c12cc6501ce5e652d8f3a2ce865548576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetProductPage

## Description
Cause 1 of 1: a ProductPage identifier that resolves to nothing is refused with the wrong NotFoundException message. Failing checks: apigatewayv2.product-page-lifecycle, apigatewayv2.product-page-lifecycle-crud.

AWS: GetProductPage (Smithy: GET /v2/portalproducts/{PortalProductId}/productpages/{ProductPageId}, errors AccessDeniedException 403, BadRequestException 400, NotFoundException 404 — "The resource specified in the request was not found. See the message field for more information.", TooManyRequestsException 429) refuses a ProductPageId that names no page under the addressed portal product with HTTP 404, error code NotFoundException and the message `ProductPage not found`. UpdateProductPage (PATCH on the same URI, same modelled error set) refuses the same way. The model does not spell the message text, so the wording is recorded behavior only: the recording shows the exact, fully fixed sentence `ProductPage not found` — there are no variable parts, it embeds neither the ProductPageId, nor the PortalProductId, nor the account. This matches the form every other portal-plane resource of this API uses for an unresolved identifier, and the emulator's own state module already encodes that form for its siblings (`PORTAL_NOT_FOUND_MESSAGE` = "Portal not found", `PRODUCT_REST_ENDPOINT_PAGE_NOT_FOUND_MESSAGE` = "ProductRestEndpointPage not found"); only ProductPage is spelled with the older api-level sentence.

Emulator: the status (404) and the code (NotFoundException) are right, but the message is `Invalid ProductPage identifier specified` — the api-plane `Invalid {resource} identifier specified` template applied to the ProductPage kind. Replay of both checks shows the error pair ('NotFoundException', 'Invalid ProductPage identifier specified') where ('NotFoundException', 'ProductPage not found') is expected, both for GetProductPage on a deleted page and (same code path, same helper) for UpdateProductPage on a deleted page id.

Why: a client that reads the message field to tell apart the resource-not-found conditions of the portal plane — and any assertion, log scraper or user-facing surface that reproduces AWS's sentence — sees text AWS never emits for this resource, while the sibling portal resources in the same emulator emit the correct form. The result is an inconsistent error vocabulary across one API: callers cannot rely on the documented "see the message field for more information" contract.

Fix: the ProductPage not-found refusal in the GetProductPage and UpdateProductPage handlers, and the shared message constant they draw from in the service's state module. ProductPage belongs with the portal-plane resources that name the resource type alone (`ProductPage not found`), not with the api-level `Invalid {resource} identifier specified` kinds; DeleteProductPage and any other operation addressing a ProductPageId must report the same sentence so the wording cannot drift between readers and writers of one page.

Traces: apigatewayv2.product-page-lifecycle (assertion at the GetProductPage-after-DeleteProductPage step), apigatewayv2.product-page-lifecycle-crud (same step; its subsequent UpdateProductPage-on-deleted-page assertion is on the same helper). Code sites: services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/get_product_page.rs lines 181-191, services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/ops/update_product_page.rs lines 169-179, services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state.rs lines 2003-2005 (invalid_identifier_message) and line 2053 (PRODUCT_PAGE_RESOURCE), with the correct sibling form at lines 2047 and 3468.

## Evidence
Expected (AWS, recorded): HTTP 404, Error.Code = "NotFoundException", Error.Message = "ProductPage not found" (fixed text in full; no request- or state-derived parts).
Actual (emulator): HTTP 404, Error.Code = "NotFoundException", Error.Message = "Invalid ProductPage identifier specified".

Replay output:
- apigatewayv2.product-page-lifecycle: AssertionError: expected NotFoundException reading a deleted product page, got ('NotFoundException', 'Invalid ProductPage identifier specified')
- apigatewayv2.product-page-lifecycle-crud: AssertionError: get_product_page after delete returned ('NotFoundException', 'Invalid ProductPage identifier specified'), expected NotFoundException

The code is correct and the status is correct; only the message text diverges. The emulator builds it from the api-plane template `format!("Invalid {resource} identifier specified")` (state.rs invalid_identifier_message) with PRODUCT_PAGE_RESOURCE = "ProductPage", whereas the portal-plane siblings in the same file use the `"<Resource> not found"` form ("Portal not found", "ProductRestEndpointPage not found").

## Steps to Reproduce
1. CreatePortalProduct(DisplayName=<any>) -> PortalProductId.
2. CreateProductPage(PortalProductId, DisplayContent={Title, Body}) -> ProductPageId.
3. DeleteProductPage(PortalProductId, ProductPageId) -> 204.
4. GetProductPage(PortalProductId, ProductPageId): observe HTTP 404 NotFoundException with Message "Invalid ProductPage identifier specified"; AWS reports "ProductPage not found".
5. UpdateProductPage(PortalProductId, ProductPageId, DisplayContent={...}) on the same deleted id: same wrong message from the same helper.
6. Equivalently, call GetProductPage with a ProductPageId that was never minted, or under an unknown PortalProductId.
