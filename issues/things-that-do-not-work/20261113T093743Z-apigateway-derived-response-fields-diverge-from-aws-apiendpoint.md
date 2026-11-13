Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-api-mapping-negative-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-api-mapping-negative-paths apigatewayv2-api-tagging-lifecycle apigatewayv2-reset-authorizers-cache apigatewayv2-route-request-parameter-lifecycle apigatewayv2-route-settings-lifecycle apigatewayv2-routing-rule-error-paths apigatewayv2-stage-access-log-settings-lifecycle apigatewayv2-stage-deployment-workflow apigatewayv2-tag-resource-workflow apigatewayv2-import-export-reimport-api apigatewayv2-import-export-reimport-api-round-trip apigatewayv2-vpc-link-errors
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigateway|CreateApi|2xx->2xx|$.apiEndpoint
Issue-Key: apigateway|ImportApi|2xx->2xx|$.apiEndpoint
Issue-Key: apigateway|ReimportApi|2xx->2xx|$.apiEndpoint
Issue-Key: apigateway|GetVpcLinks|2xx->2xx|$.items[name="sim2real-wire-vpclink-6cde7d74"].vpcLinkStatusMessage
Issue-Commit: 522a89e9a46f5432e9084a06c26f1bc29a02dbde

# apigateway derived response fields diverge from AWS: apiEndpoint shape and VPC link status message

> _Note: this issue was identified against the code base at commit `522a89e9a46f5432e9084a06c26f1bc29a02dbde`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Four net-new wire divergences on the apigateway (API Gateway v2 control plane) surface. All four are server-DERIVED response members whose value the emulator computes itself, and in each case the computed value does not have the form AWS returns. 4 causes, all in this one service.

Cause 1 of 4: CreateApi publishes an apiEndpoint that is not an execute-api URI.
AWS: Smithy: Api.ApiEndpoint (jsonName apiEndpoint, target __string) is documented as "The URI of the API, of the form {api-id}.execute-api.{region}.amazonaws.com. The stage name is typically appended to this URI to form a complete path to a deployed API stage." Api.DisableExecuteApiEndpoint repeats the same template: "clients can invoke your API with the default https://{api_id}.execute-api.{region}.amazonaws.com". So the requirement is the TEMPLATE: scheme https, a host whose first label is the api id returned in the same response, then the literal labels execute-api.<region>.amazonaws.com, and no path. The api id and the region are values taken from the response/request; the literal segments are the fixed part.
Emulator: CreateApi returns 201 with the right body otherwise, but apiEndpoint is a loopback URL of the emulator's own data listener with the api id demoted into a path segment, e.g. 'https://127.0.0.1:40705/_aws/execute-api/<api-id>'. The fixture asserts only the SHAPE of apiEndpoint (expected '<shaped-not-asserted>') and the emulator's value fails that shape check; the api id itself matches, so this is not a per-run-value difference but a different URI form.
Why: every client and SDK-adjacent tool derives the invoke URL, the api id and the region by parsing this field against the documented template (host-first-label = api id, third label = region). A loopback URL with the id in the path makes that parse fail or yield the wrong api id/region, and anything that stores the endpoint for later invocation records an address that is not reproducible in another environment.
Fix: the shared Api response projection used by the apigatewayv2 control-plane handlers (the apiEndpoint derivation), not the individual handlers' validation. Note the deliberate tension with the already-open issues '20261110T024230Z-apigatewayv2-lambda-apigatewayv2-createapi-getapi-publish-apiendpoint-as-https-api.md' and the '20261109T154749Z-apigatewayv2-*' family, which report the opposite complaint (an execute-api host the emulator does not serve). Neither of those covers this divergence, and the wire requirement is the documented template: the field must carry the AWS form, so the data plane has to accept execute-api host-style requests (or be routed to) rather than the field being rewritten to a loopback URL.
Traces: apigatewayv2-api-mapping-negative-paths#setup#1 (representative; cluster also covers apigatewayv2-api-tagging-lifecycle#setup#1, apigatewayv2-reset-authorizers-cache#setup#1, apigatewayv2-route-request-parameter-lifecycle#setup#1, apigatewayv2-route-settings-lifecycle#task#1, apigatewayv2-routing-rule-error-paths#setup#1, apigatewayv2-stage-access-log-settings-lifecycle#setup#1, apigatewayv2-stage-deployment-workflow#task#1, apigatewayv2-tag-resource-workflow#setup#1).

Cause 2 of 4: ImportApi publishes the same non-AWS apiEndpoint form.
AWS: same Smithy member, Api.ApiEndpoint, on the ImportApi output: 'https://<api-id>.execute-api.<region>.amazonaws.com' where <api-id> is the apiId minted by the import and <region> the request's region.
Emulator: ImportApi answers 200 but apiEndpoint is 'https://127.0.0.1:<port>/_aws/execute-api/<api-id>'; the replay classifies it resource_not_identified because the api the endpoint names cannot be recovered from the URI in the documented position.
Why: an import-then-invoke workflow (import an OpenAPI document, then call the imported API) reads the endpoint straight out of the import response; with the id moved into the path it addresses the wrong thing or nothing.
Fix: same apiEndpoint derivation, applied on the import path of the Api projection.
Traces: apigatewayv2-import-export-reimport-api#task#1.

Cause 3 of 4: ReimportApi publishes the same non-AWS apiEndpoint form.
AWS: same Smithy member; after a reimport the endpoint is unchanged and still 'https://<api-id>.execute-api.<region>.amazonaws.com' for the existing apiId.
Emulator: ReimportApi answers 200 with apiEndpoint 'https://127.0.0.1:<port>/_aws/execute-api/<api-id>' instead of the execute-api URI for that api id.
Why: a reimport round trip that compares the endpoint before and after (or re-resolves it) sees a value that is neither stable across environments nor parseable as an api id plus region.
Fix: same apiEndpoint derivation on the reimport path.
Traces: apigatewayv2-import-export-reimport-api-round-trip#task#5.

Cause 4 of 4: GetVpcLinks reports a non-AWS vpcLinkStatusMessage for an AVAILABLE VPC link.
AWS: Smithy: VpcLink.VpcLinkStatusMessage (jsonName vpcLinkStatusMessage, target StringWithLengthBetween0And1024) is "A message summarizing the cause of the status of the VPC link"; VpcLink.VpcLinkStatus is the enum PENDING|AVAILABLE|DELETING|FAILED|INACTIVE. The model does not fix the text, so this is recorded behavior only: for a VPC link in status AVAILABLE the recording shows the message 'VPC link is ready to route traffic' — the whole string is fixed text, with no values taken from the request or state.
Emulator: GetVpcLinks answers 200 and the listed link carries vpcLinkStatusMessage 'The VPC link is available for use by an integration.' instead of 'VPC link is ready to route traffic'.
Why: callers that surface or assert the status message for an available VPC link (readiness polling, console-style display, golden-output comparisons) see wording AWS never emits.
Fix: the VPC link state's status-message derivation (the text attached when a link settles into AVAILABLE), not the GetVpcLinks serializer.
Traces: apigatewayv2-vpc-link-errors#task#4.

## Evidence
Cause 1 — CreateApi, fixture apigatewayv2-api-mapping-negative-paths step setup#1 (cluster of 9 steps):
  expected status 201, actual 201
  $.apiEndpoint: value_mismatch — expected '<shaped-not-asserted>' (an execute-api URI of the form https://{api-id}.execute-api.{region}.amazonaws.com), got 'https://127.0.0.1:40705/_aws/execute-api/mp67t4dbd4'

Cause 2 — ImportApi, fixture apigatewayv2-import-export-reimport-api step task#1:
  expected status 200, actual 200
  $.apiEndpoint: resource_not_identified — expected 'https://{{imported_api_id}}.execute-api.us-west-2.amazonaws.com', got 'https://127.0.0.1:45099/_aws/execute-api/mp67t4dbd4'

Cause 3 — ReimportApi, fixture apigatewayv2-import-export-reimport-api-round-trip step task#5:
  expected status 200, actual 200
  $.apiEndpoint: value_mismatch — expected 'https://mp67t4dbd4.execute-api.us-west-2.amazonaws.com', got 'https://127.0.0.1:37869/_aws/execute-api/mp67t4dbd4'

Cause 4 — GetVpcLinks, fixture apigatewayv2-vpc-link-errors step task#4:
  expected status 200, actual 200
  $.items[name="..."].vpcLinkStatusMessage: value_mismatch — expected 'VPC link is ready to route traffic', got 'The VPC link is available for use by an integration.'

In causes 1-3 the api id in the emulator's URL matches the one it returned, so the divergence is the URI form (host labels vs path segment), not a per-run id. Hosts/ports are not the requirement here; the documented template is.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1: replay apigatewayv2-api-mapping-negative-paths and read step setup#1 — CreateApi (protocolType HTTP). Inspect $.apiEndpoint in the 201 body: it is 'https://127.0.0.1:<port>/_aws/execute-api/<apiId>' rather than 'https://<apiId>.execute-api.<region>.amazonaws.com'.
3. Cause 2: replay apigatewayv2-import-export-reimport-api step task#1 — ImportApi with an OpenAPI body; inspect $.apiEndpoint of the 200 body.
4. Cause 3: replay apigatewayv2-import-export-reimport-api-round-trip step task#5 — ReimportApi against the api created earlier in the fixture; inspect $.apiEndpoint of the 200 body.
5. Cause 4: replay apigatewayv2-vpc-link-errors step task#4 — CreateVpcLink then GetVpcLinks; find the item by name and read vpcLinkStatusMessage while vpcLinkStatus is AVAILABLE.
6. All four steps must pass before this issue is closed; re-replay the four fixtures named in Issue-Trace/trace_ids.
