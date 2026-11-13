Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-deploy-and-export-api-definition
Issue-Step: task#6
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-deploy-and-export-api-definition
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|GetExport|2xx->2xx|$
Issue-Commit: 64b86d5e14a7a25edd6b553817d01f64764c1ea9

# API Gateway GetExport oas30 omits the top-level components section of the exported document

> _Note: this issue was identified against the code base at commit `64b86d5e14a7a25edd6b553817d01f64764c1ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExport

## Description
One net-new wire divergence cluster (service=apigateway). GetExport of a deployed stage as an OpenAPI 3.0 document answers 200 with a body that is structurally short of the document AWS emits: the emulator's generator never writes the top-level `components` key.

Cause 1 of 1: the exported OAS30 document has no `components` section. AWS: Smithy `com.amazonaws.apigateway#GetExport` (GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType}, code 200, output ExportResponse with contentType/contentDisposition headers and `body` as the httpPayload blob) does not describe the document grammar, so this is recorded behavior only: the recorded export is a YAML OpenAPI 3.0.1 document that always carries a top-level `components` mapping — emitted as `components: {}` when the API declares no reusable components — placed after `paths` and before the `x-amazon-apigateway-*` extensions. Everything else in the document matched byte-for-byte, including the empty `content: {}` under the 200 response, the `servers` block and `x-amazon-apigateway-security-policy: "TLS_1_0"`. The document's fixed parts are the `openapi`, `paths`, `components` and extension keys; its variable parts (the `info.title`, the `info.version`, the `servers[0].url` host and the `basePath` default) come from the API and stage named in the request. Emulator: the same document with the `components` key absent entirely (and, in the same cluster's body diff, the empty `content: {}` under the 200 response also dropped). Status 200 on both sides. Why: an OpenAPI consumer that walks `components` to resolve `$ref`s, or that round-trips the export back through ImportRestApi/PutRestApi, reads a document whose shape differs from the one AWS produces — a strict validator or a code generator keyed on the presence of `components` fails on an export AWS would have accepted, and a diffing client sees spurious changes when comparing an AWS export to an emulator export of the same API. Fix: the OpenAPI 3.0 document generator behind the GetExport handler's `oas30` export type (the serializer that composes the exported document), which must always emit the top-level `components` mapping (empty when there is nothing to put in it) and the empty `content` mapping for a response with no body schema. Trace: apigateway-deploy-and-export-api-definition#task#6.

No open issue carries this replay key (`owned_by` was empty and no stub blocks it). The open issue `20261029T200808Z-apigateway-api-gateway-list-envelopes-and-openapi-export-serialization.md` is adjacent in subject matter but does not own this fixture step, so it does not cover it.

## Evidence
Replay of trace set simparity-wire-aws-v1 (193 fixtures, 1873 steps; 103 divergence clusters). One cluster with empty `owned_by` and empty `stub_path`, service=apigateway:

GetExport, apigateway-deploy-and-export-api-definition#task#6, expected 200 / got 200, divergence_key `service=apigateway|GetExport|status=200->200|keys=$`:
`$: value_mismatch` — expected body (recorded):
```
openapi: "3.0.1"
info:
  title: "<api name>"
  version: "1.0"
servers:
- url: "https://<rest-api-id>.execute-api.<region>.amazonaws.com/{basePath}"
  variables:
    basePath:
      default: "<stage name>"
paths:
  /pets:
    get:
      responses:
        "200":
          description: "200 response"
          content: {}
components: {}
x-amazon-apigateway-security-policy: "TLS_1_0"
```
actual body: identical except that `content: {}` under the "200" response and the whole `components: {}` line are missing. (The recorded run's concrete title, host and stage are per-run values and are not part of the requirement; the missing `components`/`content` keys are.)

Smithy consulted (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json): GetExport — input {restApiId, stageName, exportType httpLabel required, parameters httpQueryParams, accepts httpHeader Accept}, output ExportResponse {contentType httpHeader Content-Type, contentDisposition httpHeader Content-Disposition, body httpPayload blob}, http GET /restapis/{restApiId}/stages/{stageName}/exports/{exportType} code 200, errors BadRequestException 400 / ConflictException 409 / LimitExceededException 429 / NotFoundException 404 / TooManyRequestsException 429 / UnauthorizedException 401. The model says nothing about the exported document's grammar, so the expected document shape is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture apigateway-deploy-and-export-api-definition and inspect step task#6: GET /restapis/{restApiId}/stages/{stageName}/exports/oas30.
3. Compare the returned YAML with the recorded body — the emulator's document omits the top-level `components: {}` mapping and the empty `content: {}` mapping under the "200" response; every other line matches.
4. The step answers 200, as the recording does; only the exported document's shape diverges.
