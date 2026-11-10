Issue-Source: trace-sweep-divergence
Issue-Trace: ivschat-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ivschat-error-paths ivschat-room-lifecycle
Issue-Shape-Id: com.amazonaws.ivschat#AmazonInteractiveVideoServiceChat
Issue-Key: ivschat|GetRoom|4xx->4xx|$.Code
Issue-Key: ivschat|GetRoom|4xx->4xx|$.Details
Issue-Key: ivschat|GetRoom|4xx->4xx|$.Message
Issue-Key: ivschat|GetRoom|4xx->4xx|$.RequestID
Issue-Key: ivschat|CreateChatToken|4xx->4xx|$.Code
Issue-Key: ivschat|CreateChatToken|4xx->4xx|$.Details
Issue-Key: ivschat|CreateChatToken|4xx->4xx|$.Message
Issue-Key: ivschat|CreateChatToken|4xx->4xx|$.RequestID
Issue-Key: ivschat|CreateRoom|4xx->4xx|$.Code
Issue-Key: ivschat|CreateRoom|4xx->4xx|$.Details
Issue-Key: ivschat|CreateRoom|4xx->4xx|$.Message
Issue-Key: ivschat|CreateRoom|4xx->4xx|$.RequestID
Issue-Key: ivschat|CreateLoggingConfiguration|4xx->4xx|$.Code
Issue-Key: ivschat|CreateLoggingConfiguration|4xx->4xx|$.Details
Issue-Key: ivschat|CreateLoggingConfiguration|4xx->4xx|$.Message
Issue-Key: ivschat|CreateLoggingConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivschat|DeleteLoggingConfiguration|4xx->4xx|$.Code
Issue-Key: ivschat|DeleteLoggingConfiguration|4xx->4xx|$.Details
Issue-Key: ivschat|DeleteLoggingConfiguration|4xx->4xx|$.Message
Issue-Key: ivschat|DeleteLoggingConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivschat|GetLoggingConfiguration|4xx->4xx|$.Code
Issue-Key: ivschat|GetLoggingConfiguration|4xx->4xx|$.Details
Issue-Key: ivschat|GetLoggingConfiguration|4xx->4xx|$.Message
Issue-Key: ivschat|GetLoggingConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivschat|UpdateLoggingConfiguration|4xx->4xx|$.Code
Issue-Key: ivschat|UpdateLoggingConfiguration|4xx->4xx|$.Details
Issue-Key: ivschat|UpdateLoggingConfiguration|4xx->4xx|$.Message
Issue-Key: ivschat|UpdateLoggingConfiguration|4xx->4xx|$.RequestID
Issue-Key: ivschat|ListRooms|4xx->4xx|$.Code
Issue-Key: ivschat|ListRooms|4xx->4xx|$.Details
Issue-Key: ivschat|ListRooms|4xx->4xx|$.Message
Issue-Key: ivschat|ListRooms|4xx->4xx|$.RequestID
Issue-Commit: daf74f430223eff1dc28404b54e4f3c7221767a2

# ivschat client-error responses carry no error envelope at all: code, message and modeled detail members absent on every refusal

> _Note: this issue was identified against the code base at commit `daf74f430223eff1dc28404b54e4f3c7221767a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRoom

## Description
Eight net-new causes, one per refusing ivschat operation. The shared fault: the emulator picks the right HTTP status for a refusal but serializes NO error envelope with it — the error type, the message and the exception shape's modeled members are all absent, so an SDK cannot classify or describe the failure. ivschat is restJson1 (Smithy: AmazonInteractiveVideoServiceChat, protocols=[restJson1], sdk_id=ivschat), so a modeled error must be returned as the exception's JSON document plus the error-type discriminator the protocol uses; the replay reports the normalized Code/Message/Details as None for all eight, i.e. nothing at all was emitted. Note the open behavioral issue 20261023T054149Z-ivschat-logging-configuration-fidelity-missing-s3-destination-validation covers logging-configuration MESSAGE TEMPLATES and missing S3-destination validation only; it does not cover the absent error envelope reported here, and it is a different corpus.

Cause 1 of 8: GetRoom on an unknown room ARN.
AWS: GetRoom models ResourceNotFoundException @httpError 404 (Smithy: GetRoom errors = AccessDeniedException 403, ResourceNotFoundException 404, ValidationException 400). The shape requires three members: message, resourceId and resourceType (Smithy: ResourceNotFoundException.message/resourceId/resourceType all @required). Recorded behavior shows the message is built from the requested identifier as the fixed text "Resource: " + the requested room ARN + " not found"; the only variable part is that ARN, taken from the request. resourceId is that same requested ARN and resourceType is the fixed token "ROOM". The response also carries a request id, whose presence is required and whose value is per-run.
Emulator: answers 404 with no error body — code, message, resourceId/resourceType all missing (replay: "$.Code: missing (expected 'ResourceNotFoundException', got None)", likewise $.Message, $.Details, $.RequestID).
Why: a client gets a 404 it cannot turn into a ResourceNotFoundException. botocore and the AWS SDKs classify a restJson1 error from the error-type discriminator and the body; with neither, the call surfaces as an unmodeled/opaque HTTP failure, so catch-by-exception-type, retry policy and any message or resourceId parsing all break.
Fix: the ivschat crate's error serializer (the shared restJson1 error-response path used by GetRoom's not-found branch) must emit the exception document and its discriminator, populating the required message/resourceId/resourceType.
Traces: ivschat-error-paths#task#1 (also ivschat-room-lifecycle#teardown#7).

Cause 2 of 8: CreateChatToken against an unknown room ARN.
AWS: CreateChatToken models ResourceNotFoundException @httpError 404 (Smithy: CreateChatToken errors = AccessDeniedException 403, PendingVerification 403, ResourceNotFoundException 404, ValidationException 400). Same envelope as Cause 1: fixed text "Resource: <roomIdentifier> not found" with the ARN from the request's roomIdentifier, resourceId = that ARN, resourceType = "ROOM".
Emulator: 404 with an empty body; code, message and details all absent.
Why: as Cause 1 — the token-minting path's only negative outcome is unclassifiable.
Fix: the CreateChatToken handler's not-found branch, through the same shared error serializer.
Traces: ivschat-error-paths#task#2.

Cause 3 of 8: CreateRoom with a name violating the member pattern.
AWS: name targets RoomName, @length max 128 and @pattern ^[a-zA-Z0-9-_]*$ (Smithy: CreateRoomRequest.name -> RoomName). A violation is ValidationException @httpError 400 (a service-common error on every ivschat operation), whose shape requires message and reason and carries fieldList (Smithy: ValidationException.message/reason @required, fieldList optional list). Recorded behavior: message is the fixed form "<Operation>.<member> does not match the required pattern <pattern>" — here "CreateRoom.name does not match the required pattern ^[a-zA-Z0-9-_]*$"; the variable parts are the operation+member path and the pattern taken from the model. reason is the fixed token FIELD_VALIDATION_FAILED and fieldList holds one entry whose name is the member path and whose message repeats that sentence.
Emulator: 400 with no body — no code, no message, no reason, no fieldList.
Why: the status alone cannot tell a client WHICH field it got wrong; fieldList is the machine-readable form clients bind form errors to, and reason is how they distinguish field validation from the other ValidationExceptionReason values.
Fix: CreateRoom's input validation plus the ValidationException arm of the ivschat error serializer.
Traces: ivschat-error-paths#task#3.

Cause 4 of 8: CreateLoggingConfiguration with a name violating the member pattern.
AWS: same rule as Cause 3 applied to CreateLoggingConfiguration.name; ValidationException 400 with message "CreateLoggingConfiguration.name does not match the required pattern ^[a-zA-Z0-9-_]*$", reason FIELD_VALIDATION_FAILED and a one-entry fieldList naming CreateLoggingConfiguration.name.
Emulator: 400 with an empty body.
Why: as Cause 3.
Fix: CreateLoggingConfiguration's input validation plus the shared ValidationException serializer.
Traces: ivschat-error-paths#task#4.

Cause 5 of 8: DeleteLoggingConfiguration on an unknown identifier.
AWS: DeleteLoggingConfiguration models ResourceNotFoundException @httpError 404 (Smithy: errors = AccessDeniedException 403, ConflictException 409, PendingVerification 403, ResourceNotFoundException 404). Recorded behavior: fixed text "Resource: <identifier> not found" with the requested logging-configuration ARN, resourceId = that ARN, resourceType = the fixed token "logging-configuration" (lower-case here, unlike the room's "ROOM").
Emulator: 404 with no error body.
Why: as Cause 1, and the resourceType token is how a client tells which kind of resource was missing.
Fix: DeleteLoggingConfiguration's not-found branch plus the shared error serializer.
Traces: ivschat-error-paths#task#5.

Cause 6 of 8: GetLoggingConfiguration on an unknown identifier.
AWS: ResourceNotFoundException 404 (Smithy: GetLoggingConfiguration errors = AccessDeniedException 403, ResourceNotFoundException 404), same envelope as Cause 5.
Emulator: 404 with no error body.
Why: as Cause 5.
Fix: GetLoggingConfiguration's not-found branch plus the shared error serializer.
Traces: ivschat-error-paths#task#6.

Cause 7 of 8: UpdateLoggingConfiguration on an unknown identifier.
AWS: ResourceNotFoundException 404 (Smithy: UpdateLoggingConfiguration errors = AccessDeniedException 403, ConflictException 409, PendingVerification 403, ResourceNotFoundException 404), same envelope as Cause 5.
Emulator: 404 with no error body.
Why: as Cause 5.
Fix: UpdateLoggingConfiguration's not-found branch plus the shared error serializer.
Traces: ivschat-error-paths#task#8.

Cause 8 of 8: ListRooms with maxResults above the modeled ceiling.
AWS: maxResults targets MaxRoomResults, an integer with @range min 1 max 50 (Smithy: ListRoomsRequest.maxResults -> MaxRoomResults). Exceeding it is ValidationException 400. Recorded behavior: the message is the fixed form "<Operation>.<member> is required to have a maximum value of <max>, found <value>" with the limit from the model and the value from the request, rendered as decimals — for a request of 100 against the ceiling of 50 that is "ListRooms.maxResults is required to have a maximum value of 50.0, found 100.0". reason is FIELD_VALIDATION_FAILED and fieldList holds one entry named ListRooms.maxResults carrying that same sentence.
Emulator: 400 with no error body, so the range refusal is indistinguishable from the pattern refusals above.
Why: a paginating client cannot learn the page-size ceiling or which member it exceeded.
Fix: ListRooms' input validation (range check on maxResults) plus the shared ValidationException serializer.
Traces: ivschat-error-paths#task#9.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, fixture ivschat-error-paths. Every cause reports the status correctly and the whole envelope as absent.

Cause 1 — ivschat-error-paths#task#1 (GetRoom, status 404 expected and 404 actual):
  $.Code: missing (expected 'ResourceNotFoundException', got None)
  $.Message: missing (expected 'Resource: arn:aws:ivschat:us-west-2:000000000000:room/abcdef123456 not found', got None)
  $.Details: missing (expected {'resourceId': 'arn:aws:ivschat:us-west-2:000000000000:room/abcdef123456', 'resourceType': 'ROOM'}, got None)
  $.RequestID: missing (got None) — presence required; the recorded value is per-run and is not a requirement.
  Same cluster: ivschat-room-lifecycle#teardown#7.
  Fixed text of the message: "Resource: " and " not found". Variable part: the room ARN from the request.

Cause 2 — ivschat-error-paths#task#2 (CreateChatToken, 404 -> 404): identical four missing keys; expected Code 'ResourceNotFoundException', Message 'Resource: <room-arn> not found', Details {'resourceId': <room-arn>, 'resourceType': 'ROOM'}.

Cause 3 — ivschat-error-paths#task#3 (CreateRoom, 400 -> 400):
  expected Code 'ValidationException'; Message 'CreateRoom.name does not match the required pattern ^[a-zA-Z0-9-_]*$'; Details {'reason': 'FIELD_VALIDATION_FAILED', 'fieldList': [{'name': 'CreateRoom.name', 'message': '<same sentence>'}]}; all got None.
  Fixed text: " does not match the required pattern ". Variables: the "CreateRoom.name" member path and the pattern from the model.

Cause 4 — ivschat-error-paths#task#4 (CreateLoggingConfiguration, 400 -> 400): same four keys missing; expected Message 'CreateLoggingConfiguration.name does not match the required pattern ^[a-zA-Z0-9-_]*$' and fieldList entry named 'CreateLoggingConfiguration.name'.

Cause 5 — ivschat-error-paths#task#5 (DeleteLoggingConfiguration, 404 -> 404): expected Code 'ResourceNotFoundException', Message 'Resource: arn:aws:ivschat:us-west-2:000000000000:logging-configuration/abcdef123456 not found', Details {'resourceId': <that arn>, 'resourceType': 'logging-configuration'}; all got None.

Cause 6 — ivschat-error-paths#task#6 (GetLoggingConfiguration, 404 -> 404): identical to Cause 5.

Cause 7 — ivschat-error-paths#task#8 (UpdateLoggingConfiguration, 404 -> 404): identical to Cause 5.

Cause 8 — ivschat-error-paths#task#9 (ListRooms, 400 -> 400): expected Code 'ValidationException'; Message 'ListRooms.maxResults is required to have a maximum value of 50.0, found 100.0'; Details {'reason': 'FIELD_VALIDATION_FAILED', 'fieldList': [{'name': 'ListRooms.maxResults', 'message': '<same sentence>'}]}; all got None.
  Fixed text: " is required to have a maximum value of ", ", found ". Variables: the member path, the modeled maximum (50) and the requested value, both rendered as decimals.

## Steps to Reproduce
1. Boot the emulator and replay fixture ivschat-error-paths from trace set simparity-wire-aws-v1; all eight steps below come from it.
2. Cause 1 — ivschat.get_room(identifier='arn:aws:ivschat:<region>:<account>:room/<id-never-created>'). Expect HTTP 404 with error type ResourceNotFoundException, message 'Resource: <that arn> not found', resourceId = that ARN, resourceType = 'ROOM'. The emulator answers 404 with an empty body. (step task#1)
3. Cause 2 — ivschat.create_chat_token(roomIdentifier=<same unknown room arn>, userId=<any>). Same expected envelope. (step task#2)
4. Cause 3 — ivschat.create_room(name='<name containing a character outside [a-zA-Z0-9-_]>'). Expect 400 ValidationException with message 'CreateRoom.name does not match the required pattern ^[a-zA-Z0-9-_]*$', reason FIELD_VALIDATION_FAILED and a fieldList entry named CreateRoom.name. (step task#3)
5. Cause 4 — ivschat.create_logging_configuration(name='<same kind of illegal name>', destinationConfiguration=<any valid>). Same shape, member path CreateLoggingConfiguration.name. (step task#4)
6. Cause 5 — ivschat.delete_logging_configuration(identifier='arn:aws:ivschat:<region>:<account>:logging-configuration/<id-never-created>'). Expect 404 ResourceNotFoundException, message 'Resource: <that arn> not found', resourceType 'logging-configuration'. (step task#5)
7. Cause 6 — ivschat.get_logging_configuration(identifier=<same unknown arn>). Same. (step task#6)
8. Cause 7 — ivschat.update_logging_configuration(identifier=<same unknown arn>, name=<any valid>). Same. (step task#8)
9. Cause 8 — ivschat.list_rooms(maxResults=100). Expect 400 ValidationException with message 'ListRooms.maxResults is required to have a maximum value of 50.0, found 100.0', reason FIELD_VALIDATION_FAILED, fieldList entry named ListRooms.maxResults. (step task#9)
10. In every case inspect the raw response body and the error-type discriminator: the emulator emits neither, so an SDK cannot build the modeled exception.
