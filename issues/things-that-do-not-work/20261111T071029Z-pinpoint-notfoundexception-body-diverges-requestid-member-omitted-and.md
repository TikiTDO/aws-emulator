Issue-Source: trace-sweep-divergence
Issue-Trace: pinpoint-app-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pinpoint-app-lifecycle
Issue-Shape-Id: com.amazonaws.pinpoint#Pinpoint
Issue-Key: pinpoint|GetApp|4xx->4xx|$.Message
Issue-Key: pinpoint|GetApp|4xx->4xx|$.RequestID
Issue-Commit: 75aadf11fdb481bb43f0e5a1a6b0f44b1e719510

# pinpoint NotFoundException body diverges: RequestID member omitted and ApplicationId appended to Message

> _Note: this issue was identified against the code base at commit `75aadf11fdb481bb43f0e5a1a6b0f44b1e719510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApp

## Description
Cause 1 of 1: the 404 NotFoundException the Pinpoint emulator serves for a missing application is shaped and worded differently from the service's. Fixture pinpoint-app-lifecycle, step task#4 (GetApp on an application id that does not exist).

AWS: Smithy: GetApp -> GET /v1/apps/{ApplicationId}, success 200 with ApplicationResponse as the httpPayload; modeled errors include NotFoundException with @httpError 404. Smithy: NotFoundException is a structure with TWO members, Message (__string, "The message that's returned from the API.") and RequestID (__string, "The unique identifier for the request or response."); the model declares no other members. The recorded response for an unknown application id is HTTP 404 with a JSON body carrying BOTH members: Message is the fixed text "Application not found" with NO variable part — the service does not interpolate the requested ApplicationId into the message — and RequestID carries the per-request identifier (a per-run value, so the corpus asserts only that the member is present and shaped, not its literal value). Error code/type for this class is NotFoundException at status 404.

Emulator: the status and the error class are right (404), but the body is not. It (a) omits the RequestID member entirely — $.RequestID is absent where the recording has a shaped string — and (b) emits Message as "Application not found: <applicationId>", appending a colon, a space and the requested application id to the service's fixed text. Replay detail: "$.RequestID: missing (expected '<shaped-not-asserted>', got None)" and "$.Message: value_mismatch (expected 'Application not found', got 'Application not found: <applicationId>')" — the trailing hex id in the recording is the fixture's generated application id, a per-run value, not part of the requirement.

Why: a client that keys off the error body gets two things wrong. Any caller or SDK middleware that logs or correlates on RequestID (the Pinpoint REST error envelope's request-correlation member) reads null against the emulator, so correlation and retry-diagnostic paths that are exercised against AWS are never exercised here. And a caller that compares, hashes or asserts on the message text — including test suites and error-classification code pinned to the service's fixed wording — sees a message that varies with the requested id, so an assertion that passes against AWS fails against the emulator and an emulator-authored message leaks a request value AWS does not echo.

Fix: the error serializer / error-construction path for Pinpoint's NotFoundException (the shared Pinpoint REST-JSON error envelope, as used by the GetApp handler's not-found branch). Two changes: populate the RequestID member on every Pinpoint exception body from the request's generated request id, and emit Message as the service's fixed text for the class without interpolating the request's ApplicationId.

Traces: pinpoint-app-lifecycle#task#4 (GetApp, expected 404 / actual 404, body mismatch on $.Message and $.RequestID).

## Evidence
Fixture pinpoint-app-lifecycle, step task#4, operation GetApp, verdict mismatch, expected_status 404, actual_status 404.
Divergence key: service=pinpoint|GetApp|status=404->404|keys=$.Message,$.RequestID

Expected (recorded AWS response, 404):
  $.Message  = "Application not found"            (fixed text; no request value interpolated)
  $.RequestID = <shaped string, per-run value, presence asserted only>

Actual (emulator, 404):
  $.Message  = "Application not found: <applicationId>"   (service's fixed text plus ": " plus the requested ApplicationId)
  $.RequestID = absent (None)

Model basis: Smithy com.amazonaws.pinpoint#NotFoundException {Message: __string, RequestID: __string}, errorKind client, httpError 404; bound to GetApp (GET /v1/apps/{ApplicationId}).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture pinpoint-app-lifecycle up to step task#4: call Pinpoint GetApp with GET /v1/apps/{ApplicationId} using an application id that was never created (or was deleted earlier in the fixture).
3. Observe the 404 response body. The emulator returns {"Message": "Application not found: <applicationId>"} with no RequestID member; AWS returns Message exactly "Application not found" together with a RequestID member.
