Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-stage-method-settings-path-grammar
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-stage-method-settings-path-grammar apigw-patch-deployment-stage-invalid-path apigateway-tag-stage-inheritance-vs-direct-tag-precedence
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateStage|4xx->4xx|$.message
Issue-Key: apigateway|TagResource|4xx->4xx|$.message
Issue-Commit: 64b86d5e14a7a25edd6b553817d01f64764c1ea9

# API Gateway 400 refusals replace AWS validation messages with emulator-authored text (2 causes)

> _Note: this issue was identified against the code base at commit `64b86d5e14a7a25edd6b553817d01f64764c1ea9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStage

## Description
Two net-new wire divergence clusters (service=apigateway) share one root cause: when input validation refuses a request, the emulator answers the right HTTP status (400) and the right error type (BadRequestException) but substitutes its own diagnostic sentence for the fixed message API Gateway returns for that error class. Only `$.message` diverges in both clusters.

Shared shape of the defect — AWS: Smithy `com.amazonaws.apigateway#UpdateStage` (PATCH /restapis/{restApiId}/stages/{stageName}, code 200) and `TagResource` (PUT /tags/{resourceArn}, code 204) both model `BadRequestException` with httpError 400; the model does not carry the message text, so the exact sentences below are taken from the recorded responses. For both of these error classes the recorded message is entirely FIXED text — it interpolates no member name, no patch path and no submitted value. Emulator: it generates a per-request sentence that names the patch path and echoes the rejected value, or states its own internal precondition. Why: a client that branches on the refusal — matching the message to tell a malformed-throttling patch apart from another BadRequestException, or surfacing AWS's tag-character rule to a user — sees text AWS never emits, so message-based handling and user-facing diagnostics both break even though the status and error code are right. Fix: the validation layer of each operation's handler (the stage patch-value validator for UpdateStage, the tag-key/value validator shared by TagResource), which must raise BadRequestException carrying AWS's fixed sentence rather than formatting its own.

Cause 1 of 2: UpdateStage refusing a non-numeric throttling value in a patchOperations path answers its own sentence. AWS: 400, `x-amzn-errortype: BadRequestException`, `$.message` = 'Invalid throttling setting' — fixed text, no variable part (AWS does not name the path or the value). Emulator: 400 with `$.message` = '/*/*/throttling/rateLimit takes a number value, got "abc"', i.e. a template whose variable parts are the method-setting path from the request and the rejected value from the request. Trace: apigateway-stage-method-settings-path-grammar#task#14 (cluster also covers apigateway-stage-method-settings-path-grammar#task#15, apigw-patch-deployment-stage-invalid-path#task#9 and #task#10).

Cause 2 of 2: TagResource refusing an invalid tag key answers an internal precondition instead of AWS's tag-validation message. AWS: 400, BadRequestException, `$.message` = 'Invalid tag key/value. Valid characters are letters, numbers, spaces, and the following: _ . : / = + - @. Tag key can be up to 128 characters. Tag value can be up to 256 characters.' — fixed text in full; AWS states the rule rather than echoing the offending key or value, so there is no variable part. Emulator: 400 with `$.message` = 'a tag key may not be empty'. Trace: apigateway-tag-stage-inheritance-vs-direct-tag-precedence#task#13.

No open issue carries these replay keys (`owned_by` was empty for both clusters and no stub blocks them). The open issues `20261029T200808Z-apigateway-api-gateway-patchoperations-grammar-op-enum-precedence-and.md`, `20261029T200808Z-apigateway-api-gateway-tag-vpc-link-stage-and-import.md` and `20261029T220031Z-apigateway-400-class-refusals-use-the-wrong-error.md` are adjacent in subject matter but own neither of the two fixture steps above, so they do not cover them.

## Evidence
Replay of trace set simparity-wire-aws-v1 (193 fixtures, 1873 steps; 103 divergence clusters). Two clusters with empty `owned_by` and empty `stub_path`, both service=apigateway:

1. UpdateStage, apigateway-stage-method-settings-path-grammar#task#14, expected 400 / got 400, affected_count 4: `$.message: value_mismatch (expected 'Invalid throttling setting', got '/*/*/throttling/rateLimit takes a number value, got "abc"')`. divergence_key `service=apigateway|UpdateStage|status=400->400|keys=$.message`. Other cluster members: apigateway-stage-method-settings-path-grammar#task#15, apigw-patch-deployment-stage-invalid-path#task#9, apigw-patch-deployment-stage-invalid-path#task#10.
2. TagResource, apigateway-tag-stage-inheritance-vs-direct-tag-precedence#task#13, expected 400 / got 400: `$.message: value_mismatch (expected 'Invalid tag key/value. Valid characters are letters, numbers, spaces, and the following: _ . : / = + - @. Tag key can be up to 128 characters. Tag value can be up to 256 characters.', got 'a tag key may not be empty')`. divergence_key `service=apigateway|TagResource|status=400->400|keys=$.message`.

In both clusters the status (400) and the `x-amzn-errortype` header matched; `$.message` is the only diverging key.

Smithy consulted (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json): UpdateStage — input {restApiId, stageName httpLabel required, patchOperations -> ListOfPatchOperation}, http PATCH /restapis/{restApiId}/stages/{stageName} code 200, errors BadRequestException 400 / ConflictException 409 / LimitExceededException 429 / NotFoundException 404 / TooManyRequestsException 429 / UnauthorizedException 401; TagResource — input {resourceArn httpLabel required, tags -> MapOfStringToString required}, http PUT /tags/{resourceArn} code 204, same error set. The model carries no message text, so both expected sentences come from the recorded responses.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Cause 1: replay apigateway-stage-method-settings-path-grammar and inspect step task#14 — PATCH of a stage with a method-setting throttling value that is not a number is refused 400 BadRequestException, but the body carries '/*/*/throttling/rateLimit takes a number value, got "abc"' instead of AWS's fixed 'Invalid throttling setting'. The same divergence appears at task#15 and at apigw-patch-deployment-stage-invalid-path#task#9/#task#10.
3. Cause 2: replay apigateway-tag-stage-inheritance-vs-direct-tag-precedence and inspect step task#13 — TagResource with an invalid tag key is refused 400 BadRequestException with 'a tag key may not be empty' instead of AWS's fixed 'Invalid tag key/value. Valid characters are letters, numbers, spaces, and the following: _ . : / = + - @. Tag key can be up to 128 characters. Tag value can be up to 256 characters.'
4. Both steps return AWS's status and error type; only the message text diverges.
