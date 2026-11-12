Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-list-types-invalid-api-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-list-types-invalid-api-error
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListTypes|4xx->4xx|$.message
Issue-Key: appsync|ListTypes|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: appsync|ListTypes|4xx->4xx|status
Issue-Commit: 97bcd61576aa0f22fd1da9220056da036e0a415a

# AppSync ListTypes refuses an unresolvable apiId as 400 BadRequestException instead of 404 NotFoundException "API not found."

> _Note: this issue was identified against the code base at commit `97bcd61576aa0f22fd1da9220056da036e0a415a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTypes

## Description
Cause 1 of 1 - ListTypes against an apiId that does not exist answers the wrong HTTP status, the wrong error code and an emulator-authored message (fixture appsync-list-types-invalid-api-error, step task#1).

AWS: Smithy (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice): ListTypes = GET /v1/apis/{apiId}/types with required `format` query; its modeled errors include NotFoundException (errorKind client, httpError 404, member message, documented "The resource specified in the request was not found. Check the resource, and then try again.") and BadRequestException (client, httpError 400, members message/reason/detail, documented "The request is not well formed. For example, a value is invalid or a required field is missing."). The recording shows AWS resolves the apiId path label and, when no API matches, refuses with HTTP 404, x-amzn-errortype: NotFoundException and the message "API not found." - that sentence is entirely fixed text: it has no variable parts and interpolates nothing from the request.

Emulator: answers HTTP 400 with x-amzn-errortype: BadRequestException and the message "Invalid API ID <apiId>" - the only variable part is the request's apiId spliced into emulator-authored prose (recorded instance: 'Invalid API ID sim2real-probe-appsync-nonexistent-6f6a5890', where the long token is the request's apiId and is not part of any template). So all three requirements diverge: status 404 -> 400, error code NotFoundException -> BadRequestException, message "API not found." -> "Invalid API ID <apiId>". Note this is a per-operation mapping, not a global one: AWS does refuse StartSchemaCreation's unresolvable apiId with BadRequestException "Invalid API ID <apiId>" (see open issue 20261111T012528Z-appsync-2-net-new-causes-startschemacreation-refuses-an.md, which is the mirror-image defect on that operation), so the fix must not simply invert the mapping globally.

Why: clients classify AWS errors by status and x-amzn-errortype. A 400/BadRequestException is a non-retryable "your request is malformed" signal, while 404/NotFoundException is "this resource is gone" - SDK error-handling branches, resource-existence probes and idempotent "list, create if absent" flows take the wrong path, and any assertion on the service's refusal sentence fails because the emulator leaks the requested apiId into a message AWS keeps constant.

Fix: the ListTypes handler's apiId resolution - when the API cannot be resolved, raise NotFoundException (404) with the fixed message "API not found." rather than BadRequestException with an interpolated message.

Traces: appsync-list-types-invalid-api-error#task#1.

Related open issues that do NOT cover this cluster (the replay reports it as owned by none of them): 20261029T174359Z-appsync-control-plane-error-semantics-not-found-bad.md covers inverted not-found/bad-request codes on other appsync operations; 20261111T012528Z-appsync-2-net-new-causes-startschemacreation-refuses-an.md covers StartSchemaCreation; 20261109T022338Z-appsync-schema-creation-plane-diverges-success-detail-text.md covers StartSchemaCreation's unknown-API refusal class.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200). One net-new divergence cluster, service=appsync, not stub-blocked, owned_by empty:

appsync-list-types-invalid-api-error#task#1 - operation ListTypes, expected_status 404, actual_status 400, verdict mismatch. divergence_key: service=appsync|ListTypes|status=404->400|keys=$.message,$headers.x-amzn-errortype,status. detail:
  status: expected 404, got 400
  $.message: value_mismatch (expected 'API not found.', got 'Invalid API ID sim2real-probe-appsync-nonexistent-6f6a5890')
  $headers.x-amzn-errortype: value_mismatch (expected 'NotFoundException', got 'BadRequestException')
affected_count 1.

Fixed text vs values: the expected message 'API not found.' is wholly fixed text with no interpolation. In the emulator's message, 'Invalid API ID ' is emulator-authored fixed text and 'sim2real-probe-appsync-nonexistent-6f6a5890' is the request's apiId (a per-run probe value, not a requirement).

Model evidence: ListTypes GET /v1/apis/{apiId}/types, required format query (TypeDefinitionFormat=SDL|JSON), errors BadRequestException(client,400)/ConcurrentModificationException(client,409)/InternalFailureException(server,500)/NotFoundException(client,404)/UnauthorizedException(client,401). NotFoundException httpError 404 with member message; BadRequestException httpError 400 with members message/reason/detail.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture appsync-list-types-invalid-api-error, step task#1: GET /v1/apis/<apiId-that-does-not-exist>/types?format=SDL.
3. Expect HTTP 404 with header x-amzn-errortype: NotFoundException and body {"message": "API not found."}.
4. Observe HTTP 400 with x-amzn-errortype: BadRequestException and body {"message": "Invalid API ID <the apiId sent>"}.
5. The step must pass before this issue is closed.
