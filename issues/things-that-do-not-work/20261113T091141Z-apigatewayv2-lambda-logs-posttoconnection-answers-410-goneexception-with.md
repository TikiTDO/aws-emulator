Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request
Issue-Key: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request|run#5|AssertionError|assert (code, message) == ("BadRequestException", _MALFORMED_MESSAGE), (
Issue-Commit: f886c6313691e264df098b8c73e16bb38687852c

# apigatewayv2__lambda__logs: PostToConnection answers 410 GoneException with an empty message for a connection id AWS refuses 400 BadRequestException "Invalid connectionId: <id>"

> _Note: this issue was identified against the code base at commit `f886c6313691e264df098b8c73e16bb38687852c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PostToConnection

## Description
One net-new root cause. The failing check is marked neither ALREADY FILED nor OWNED in the replay report; the open apigatewayv2 WebSocket issues in this family (20261104T160000Z-apigatewayv2-websocket-execute-api-upgrade-remains-unservable.md, 20261110T200558Z-apigatewayv2-lambda-logs-2-net-new-causes-a.md, 20261110T194854Z-apigatewayv2-lambda-logs-2-net-new-causes-a.md) are about the WebSocket front door and reply delivery and do not carry this replay key. This bundle has 1 cause; it must be closed before the issue is submitted.

Cause 1 of 1: a connection id that is not a valid connection identifier for the stage is classified Gone (410) instead of BadRequest (400), and the refusal carries no message.
Failing check: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request

AWS: Smithy (com.amazonaws.apigatewaymanagementapi#ApiGatewayManagementApi) PostToConnection is POST /@connections/{ConnectionId} with ConnectionId bound to an httpLabel and Data an httpPayload blob @length 0..131072; its modelled errors are ForbiddenException (403), GoneException (410), LimitExceededException (429) and PayloadTooLargeException (413). BadRequestException is NOT in that modelled set, so this refusal is recorded behavior only: a ConnectionId that is not a connection identifier this stage can resolve — here a syntactically base64-shaped token of API Gateway's own 11-byte id shape that the stage never issued — is refused with HTTP 400 and error code BadRequestException. Its message is a template: the fixed text is "Invalid connectionId: " and the variable part is the ConnectionId taken verbatim from the request URI (one recorded run shows "Invalid connectionId: AAAAAAAAAAAAAAA="; the id itself is request data, not part of the requirement). 410 GoneException is reserved for an id the stage DID issue and that is no longer connected, and AWS populates its message rather than sending an empty one. Hypothesis: API Gateway validates the id's internal encoding before it ever looks the connection up, which is why an unissued-but-well-shaped id is a request error rather than a gone connection.
Emulator: the same call answers error code GoneException with an empty message ('') — i.e. ('GoneException', '') where AWS answers ('BadRequestException', 'Invalid connectionId: <ConnectionId>').
Why: a WebSocket fan-out handler catches GoneException ONLY, and uses it as the signal to prune the stored connection row for that id. Against the emulator a request-validation failure is reported as a gone connection, so the handler deletes a row AWS would have kept — the emulator actively corrupts the client's connection table on a 400. The empty message additionally leaves any client that logs, matches or reports the refusal with nothing to read.
Fix: the PostToConnection handler's connection-id resolution — validate the id's shape and refuse an unresolvable/invalid id with BadRequestException at 400 before the gone-connection lookup — plus its error serializer, which must emit the message instead of an empty string.
Traces: apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request.

## Evidence
Cause 1 (verdict: mismatch) — apigatewayv2__lambda__logs.post-to-a-malformed-connection-id-is-rejected-bad-request:
  File "check_apigatewayv2__lambda__logs_post_to_a_malformed_connection_id_is_rejected_bad_request.py", line 37, in run
  assert (code, message) == ("BadRequestException", _MALFORMED_MESSAGE)
  AssertionError: a malformed connection id returned ('GoneException', '')

Check source: _MALFORMED_ID = "AAAAAAAAAAAAAAA=" (described in the check as "a well-formed-looking id (API Gateway's own 11-byte-base64 shape) that no connection ever used"), _MALFORMED_MESSAGE = f"Invalid connectionId: {_MALFORMED_ID}". The call under test is apigatewaymanagementapi.post_to_connection(ConnectionId=_MALFORMED_ID, Data=b"x") against the management endpoint of the deployed WebSocket stage.

Expected: error code BadRequestException (HTTP 400) with message "Invalid connectionId: " + the submitted ConnectionId.
Actual: error code GoneException (HTTP 410) with an empty message.

Smithy citation: PostToConnection input {ConnectionId: __string @httpLabel required, Data: Data @httpPayload blob @length 0..131072 required}, output Unit (200), errors [ForbiddenException 403, GoneException 410, LimitExceededException 429, PayloadTooLargeException 413] — BadRequestException absent from the modelled set, hence recorded behavior only.

The downstream effect the same check grades (and which this refusal gates): after the 400, the planted connections-table row for that id must survive the failed fan-out while the live UI connection on the same stage is still notified.

## Steps to Reproduce
1. Deploy the check's WebSocket API + stage and its DynamoDB connections table (the check's _estate(ctx, endpoint="https") fixture).
2. Build an apigatewaymanagementapi client against the stage's management endpoint.
3. Call PostToConnection(ConnectionId="AAAAAAAAAAAAAAA=", Data=b"x") — an id the stage never issued.
   Observed: ClientError code 'GoneException' with an empty Error.Message.
   Expected: ClientError code 'BadRequestException' (HTTP 400) with Error.Message 'Invalid connectionId: AAAAAAAAAAAAAAA=' (fixed text 'Invalid connectionId: ' plus the ConnectionId from the request).
4. Confirm the classification matters: plant a connections-table row whose connectionId is that id, open a live UI connection on the same stage, then drive the handler's fan-out. The 400 must not trigger the handler's GoneException-only prune — both the planted row and the live row must still be present, and the live UI must still receive its notification.
