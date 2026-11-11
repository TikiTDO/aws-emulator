Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-mapping-negative-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-mapping-negative-paths apigatewayv2-api-tagging-lifecycle apigatewayv2-reset-authorizers-cache apigatewayv2-route-request-parameter-lifecycle apigatewayv2-route-settings-lifecycle apigatewayv2-routing-rule-error-paths apigatewayv2-stage-access-log-settings-lifecycle apigatewayv2-stage-deployment-workflow apigatewayv2-tag-resource-workflow apigatewayv2-import-export-reimport-api apigatewayv2-import-export-reimport-api-round-trip apigatewayv2-product-rest-endpoint-page-lifecycle
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateApi|2xx->2xx|$.apiEndpoint
Issue-Key: apigateway|ImportApi|2xx->2xx|$.apiEndpoint
Issue-Key: apigateway|ReimportApi|2xx->2xx|$.apiEndpoint
Issue-Key: apigateway|GetProductRestEndpointPage|2xx->2xx|$.displayContent.endpoint
Issue-Key: apigateway|UpdateProductRestEndpointPage|2xx->2xx|$.displayContent.endpoint
Issue-Related: 20261108T194509Z-apigateway-status-message-fields-carry-emulator-authored-text.md
Issue-Commit: c4fcf6952896a9081c9de16adffed799f1736bf9

# apigateway publishes API and portal-page endpoints at addresses that are neither AWS-shaped nor served

> _Note: this issue was identified against the code base at commit `c4fcf6952896a9081c9de16adffed799f1736bf9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Five net-new causes, all in how this service renders the URL members of its own resources. The replayed statuses are correct; the address values are not. Related open issues describe the same class from the behavioral side (20261109T152222Z-apigateway-apigatewayv2-an-http-api-s-apiendpoint-is.md, 20261110T030315Z-apigateway-api-gateway-getexport-advertises-an-execute-api-host.md); neither carries any of the replay keys below, so none of these causes is owned.

Cause 1 of 5: CreateApi.apiEndpoint is a loopback URL instead of the execute-api endpoint.
AWS: Smithy: CreateApi output member ApiEndpoint (jsonName 'apiEndpoint', target __string), http POST /v2/apis code 201. AWS populates it with the API's execute-api endpoint, i.e. the template 'https://<apiId>.execute-api.<region>.amazonaws.com' — the variable parts are the apiId returned in the same response and the request's region; there is no port and no path component.
Emulator: returns 'https://<loopback-host>:<port>/_aws/execute-api/<apiId>' — a host:port plus a '/_aws/execute-api/' path prefix. The replay recorded 'https://127.0.0.1:39033/_aws/execute-api/mp67t4dbd4' for apiId mp67t4dbd4 (host, port and id are per-run values; the shape is the defect).
Why: a client stores apiEndpoint and derives its invoke URL from it (SDK samples, CDK outputs, CFN Outputs). A value with a port and an extra path prefix does not parse as an execute-api endpoint: the apiId cannot be recovered from the host label, and any code appending '/<stage>/<route>' builds a URL the service does not route.
Fix: the CreateApi handler's response projection — the apiEndpoint derivation in the Api resource serializer (and the address the emulator's execute-api front door answers on, so the published endpoint is also the one it serves).
Traces: apigatewayv2-api-mapping-negative-paths#setup#1 (cluster of 9 steps, same key across apigatewayv2-api-tagging-lifecycle, apigatewayv2-reset-authorizers-cache, apigatewayv2-route-request-parameter-lifecycle, apigatewayv2-route-settings-lifecycle, apigatewayv2-routing-rule-error-paths, apigatewayv2-stage-access-log-settings-lifecycle, apigatewayv2-stage-deployment-workflow, apigatewayv2-tag-resource-workflow).

Cause 2 of 5: ImportApi.apiEndpoint does not identify the imported API.
AWS: Smithy: ImportApi output member ApiEndpoint (jsonName 'apiEndpoint'), http PUT /v2/apis code 201. Same template as Cause 1, with the apiId minted by the import.
Emulator: same loopback host:port + '/_aws/execute-api/<id>' form; the replay verdict is resource_not_identified against the expected 'https://<importedApiId>.execute-api.<region>.amazonaws.com'.
Why: an import-then-invoke workflow (the common OpenAPI bootstrap path) cannot resolve the API it just created from the response it was handed.
Fix: the ImportApi handler's response projection, sharing the apiEndpoint derivation with CreateApi.
Traces: apigatewayv2-import-export-reimport-api#task#1.

Cause 3 of 5: ReimportApi.apiEndpoint likewise.
AWS: Smithy: ReimportApi returns the same Api representation; apiEndpoint stays 'https://<apiId>.execute-api.<region>.amazonaws.com' for the existing apiId across a reimport.
Emulator: returns the loopback host:port form (recorded: 'https://127.0.0.1:33767/_aws/execute-api/mp67t4dbd4' where AWS returned the execute-api host for the same apiId).
Why: a reimport round-trip that compares the before/after representation sees a changed, unusable endpoint, and callers re-deriving the invoke URL after a reimport get a non-routable one.
Fix: the ReimportApi handler's response projection (same derivation).
Traces: apigatewayv2-import-export-reimport-api-round-trip#task#5.

Cause 4 of 5: GetProductRestEndpointPage advertises an endpoint the emulator does not serve, and reports a status exception for a valid rest endpoint.
AWS: Smithy: GetProductRestEndpointPage output members DisplayContent (EndpointDisplayContentResponse, jsonName 'displayContent') and StatusException (jsonName 'statusException'), http GET /v2/portalproducts/{PortalProductId}/productrestendpointpages/{ProductRestEndpointPageId} code 200. displayContent.endpoint is the invoke URL of the referenced REST API stage; statusException reports why the page could not be resolved and is absent/benign for a page whose restEndpointIdentifier does resolve.
Emulator: the page's displayContent.endpoint is emitted as an execute-api host that this emulator does not answer on (verdict host_not_own for 'https://<apiId>.execute-api.<region>.amazonaws.com/<stage>'), and statusException.message carries 'Invalid restEndpointIdentifier stage or restApiId' for a page created successfully (201) over an existing stage in the same fixture.
Why: the portal page is the thing a consumer clicks; it is published pointing at an address nothing serves, and the page simultaneously reports its own reference as invalid, so a client cannot distinguish a genuinely broken page from a working one.
Fix: the ProductRestEndpointPage state/serializer — resolve restEndpointIdentifier against stored REST APIs and stages instead of failing it, and derive displayContent.endpoint from the address the emulator's execute-api front door actually serves.
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#7 (also task#9).

Cause 5 of 5: UpdateProductRestEndpointPage republishes the same unserved endpoint.
AWS: Smithy: UpdateProductRestEndpointPage output member DisplayContent (EndpointDisplayContentResponse), http PATCH .../productrestendpointpages/{ProductRestEndpointPageId} code 200.
Emulator: the updated representation's displayContent.endpoint is again an execute-api URL the emulator does not serve (verdict host_not_own).
Why: an update that is supposed to refresh the page leaves the consumer-visible endpoint unreachable.
Fix: the UpdateProductRestEndpointPage handler's response projection, sharing the endpoint derivation with Cause 4.
Traces: apigatewayv2-product-rest-endpoint-page-lifecycle#task#8.

## Evidence
Replay of simparity-wire-aws-v1 (score 78.0%, 72 divergence clusters), clusters with empty stub_path and empty owned_by:

1) apigateway|CreateApi|status=201->201 — $.apiEndpoint: value_mismatch (expected execute-api endpoint shape, got 'https://127.0.0.1:39033/_aws/execute-api/mp67t4dbd4'); fixture apigatewayv2-api-mapping-negative-paths step setup#1, 9 affected steps.
2) apigateway|ImportApi|status=200->200 — $.apiEndpoint: resource_not_identified (expected 'https://{{imported_api_id}}.execute-api.us-west-2.amazonaws.com', got 'https://127.0.0.1:42553/_aws/execute-api/mp67t4dbd4'); fixture apigatewayv2-import-export-reimport-api step task#1.
3) apigateway|ReimportApi|status=200->200 — $.apiEndpoint: value_mismatch (expected 'https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com', got 'https://127.0.0.1:33767/_aws/execute-api/mp67t4dbd4'); fixture apigatewayv2-import-export-reimport-api-round-trip step task#5.
4) apigateway|GetProductRestEndpointPage|status=200->200 — $.statusException.message: value_mismatch (got 'Invalid restEndpointIdentifier stage or restApiId'); $.displayContent.endpoint: host_not_own ('https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com/sim2real-probe-apigatewayv2-prepstg-0c1247d8'); fixture apigatewayv2-product-rest-endpoint-page-lifecycle steps task#7, task#9.
5) apigateway|UpdateProductRestEndpointPage|status=200->200 — $.displayContent.endpoint: host_not_own (same value); fixture apigatewayv2-product-rest-endpoint-page-lifecycle step task#8.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. CreateApi (POST /v2/apis, protocolType HTTP) and read apiEndpoint — it is 'https://<host>:<port>/_aws/execute-api/<apiId>' rather than 'https://<apiId>.execute-api.<region>.amazonaws.com'. Replay apigatewayv2-api-mapping-negative-paths#setup#1.
3. ImportApi (PUT /v2/apis with an OpenAPI body) and read apiEndpoint — same form, and the apiId is not the host label. Replay apigatewayv2-import-export-reimport-api#task#1.
4. ReimportApi (PUT /v2/apis/{apiId}) and read apiEndpoint. Replay apigatewayv2-import-export-reimport-api-round-trip#task#5.
5. Create a portal product REST endpoint page over an existing REST API stage, then GET it: statusException.message says 'Invalid restEndpointIdentifier stage or restApiId' and displayContent.endpoint is an execute-api host the emulator does not answer. PATCH it and observe the same endpoint. Replay apigatewayv2-product-rest-endpoint-page-lifecycle#task#7 and #task#8.
