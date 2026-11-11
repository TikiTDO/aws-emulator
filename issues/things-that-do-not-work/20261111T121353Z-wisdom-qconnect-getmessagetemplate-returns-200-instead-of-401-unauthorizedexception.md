Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-message-template-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-message-template-lifecycle
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|GetMessageTemplate|4xx->2xx|$.message
Issue-Key: wisdom|GetMessageTemplate|4xx->2xx|status
Issue-Commit: 64434ac11a54835f0dc0d25437542d6f3223788c

# QConnect GetMessageTemplate returns 200 instead of 401 UnauthorizedException

> _Note: this issue was identified against the code base at commit `64434ac11a54835f0dc0d25437542d6f3223788c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMessageTemplate

## Description
The emulator has no authorization gate on QConnect (wisdom) GetMessageTemplate: a request the recording shows AWS rejecting with 401 is served the full message template with status 200. This bundle carries 1 cause.

Cause 1 of 1 - GetMessageTemplate serves the template where AWS answers 401 UnauthorizedException (fixture qconnect-message-template-lifecycle, step task#10).
AWS: Smithy (com.amazonaws.qconnect) GetMessageTemplate models UnauthorizedException among its errors, classified as a client error with @httpError(401) and a single `message : String` member; its documentation reads "You do not have permission to perform this action." For the caller/credential context this step exercises, the recorded response is HTTP 401 with the error shape UnauthorizedException (surfaced in the x-amzn-errortype header) and body member `message` = "Unauthorized". That message text is entirely fixed for this error class - it carries no value drawn from the request or from emulator state - so it must be reproduced verbatim.
Emulator: status 200 with a normal GetMessageTemplate success payload; `$.message` is absent (expected 'Unauthorized') and no x-amzn-errortype error header is emitted. The replay records "status: expected 401, got 200" and "$.message: missing (expected 'Unauthorized', got None)".
Why: the operation's authorization outcome is inverted. A client - or a test asserting that an unpermitted principal cannot read a template - is told the read succeeded and receives the template's content, so a negative-authorization path silently passes and any client that branches on 401 (to refresh credentials, to escalate, or to surface a permission error to a user) never takes that branch. The leak direction is the dangerous one: content that AWS withholds is returned.
Fix: the GetMessageTemplate handler's authorization check in the qconnect/wisdom service crate - evaluate the caller's permission for the requested knowledgeBaseId/messageTemplateId before the read and, when it is denied, return the modeled UnauthorizedException with HTTP status 401, the x-amzn-errortype error code UnauthorizedException, and the fixed message text "Unauthorized" instead of falling through to the success serializer.
Traces: qconnect-message-template-lifecycle#task#10.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, score 56.1%).

Fixture qconnect-message-template-lifecycle, step task#10, operation GetMessageTemplate, verdict mismatch, expected_status 401 / actual_status 200:
  status: expected 401, got 200
  $.message: missing (expected 'Unauthorized', got None)
  divergence_key: service=wisdom|GetMessageTemplate|status=401->200|keys=$.message,status

Smithy consulted: com.amazonaws.qconnect GetMessageTemplate errors list -> UnauthorizedException {errorKind: client, httpError: 401, members: {message: String}}, doc "You do not have permission to perform this action." Required wire shape for the failure: HTTP 401, errortype UnauthorizedException, body {"message": "Unauthorized"} (fixed text, no request- or state-derived parts).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture qconnect-message-template-lifecycle through step task#10: GET /knowledgeBases/{knowledgeBaseId}/messageTemplates/{messageTemplateId} in the unauthorized caller context that step records.
3. Observe HTTP 200 with the message-template payload. Expected: HTTP 401, x-amzn-errortype UnauthorizedException, body {"message": "Unauthorized"}.
