Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.create-only-property-mutation-rejected cloudcontrol.server-side-computed-readonly-property-rejected-on-write cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource-dup-removed
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.create-only-property-mutation-rejected|run#28|AssertionError|assert status == 400, f"GetResource after delete: expected HTTP 400, got {status!r}"
Issue-Key: cloudcontrol.server-side-computed-readonly-property-rejected-on-write|_error#15|AssertionError|raise AssertionError(f"the read-only patch was expected to be refused but returned {result!r}")
Issue-Key: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource|run#20|AssertionError|assert final.get("ErrorCode") == "GeneralServiceException", (
Issue-Key: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource-dup-removed|run#11|AssertionError|assert final.get("ErrorCode") == "GeneralServiceException", (
Issue-Commit: ac8063d11ee524ed56d43e71cb0e4e160867dbbf

# cloudcontrol: 3 net-new causes — ResourceNotFoundException answered at HTTP 404 instead of 400, a readOnlyProperties patch accepted instead of refused, and a downstream handler create failure reported as InvalidRequest instead of GeneralServiceException

> _Note: this issue was identified against the code base at commit `ac8063d11ee524ed56d43e71cb0e4e160867dbbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateResource

## Description
THREE net-new root causes in the cloudcontrol (Cloud Control API, awsJson1_0) service. They are independent defects; a fixer must close all three before submitting. Model citations are from services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json.

Cause 1 of 3: a resource-level not-found refusal is served with HTTP status 404 where Cloud Control answers HTTP 400.
AWS: Smithy annotates ResourceNotFoundException with @httpError 404, but the recorded behavior of this awsJson1_0 service carries HTTP 400 for that shape — the recording is the wire truth and the modeled httpError must not be used as the response status. The error code (ResourceNotFoundException) and the handler-wrapped Message are unchanged; only the status is at issue here.
Emulator: answers HTTP 404 (the modeled value) for the same refusal.
Why: HTTP status is what SDK retry/classification layers and any status-code-based routing branch on, so a refusal AWS classifies as a 400 client error is misclassified; a proxy or gateway in front of the client also treats 404 as "no such endpoint" rather than "no such resource".
Fix: the status mapping for ResourceNotFoundException on the resource-level handlers' refusal path (the GetResource/UpdateResource/DeleteResource handlers in the cloudcontrol crate and its shared error-to-HTTP mapping) must emit 400 rather than the shape's modeled httpError.
Why it is not already covered: open issue 20261103T024056Z-cloudcontrolapi-refusals-answer-404-or-200-where-aws.md records the same 404-vs-400 divergence from the WIRE corpus (its Causes 4 and 5), but the replay reports no owning issue for this behavioral check and that issue is open, so the status is still being served wrong; it is named here so the two can be closed together.
Failing check: cloudcontrol.create-only-property-mutation-rejected ("GetResource after delete: expected HTTP 400, got 404" — the check's create-only assertions, including the NotUpdatableException message, passed).

Cause 2 of 3: UpdateResource ACCEPTS a PatchDocument that writes a readOnlyProperties path instead of refusing it at the front door.
AWS: recorded behavior only — ValidationException is not in the on-disk model; it is Cloud Control's front-door input-validation code. A patch whose op targets a property the type's registry schema declares in readOnlyProperties (for AWS::SQS::Queue, /properties/Arn) is refused synchronously, before any RequestToken is minted, with error code ValidationException and the message whose fixed text is "Invalid patch update: readOnlyProperties [" + the comma-separated JSON pointers of the offending read-only properties, as the type's registry schema declares them + "] cannot be updated". The pointer list is the variable part, taken from the request's patch paths.
Emulator: accepts the patch and answers HTTP 200 with a ProgressEvent (Operation UPDATE, OperationStatus IN_PROGRESS, a freshly minted RequestToken) — no refusal at all. The analogous createOnlyProperties arm IS enforced with the matching "Invalid patch update: createOnlyProperties [...] cannot be updated" diagnostic, so only the read-only arm is missing.
Why: this is over-permissive. A caller that patches a server-computed property (an Arn, a Url, a generated id) gets a request token and believes the write is queued; it must then poll an operation AWS would never have started, and it never learns its patch was invalid. Worse, if the update later lands, a service-computed value is overwritten with a client-supplied one.
Fix: the PatchDocument validation step of the UpdateResource handler must check every patch path against the resource type's readOnlyProperties, alongside the createOnlyProperties check it already performs, and refuse with the front-door ValidationException and the readOnlyProperties pointer-list diagnostic before a request record is created.
Failing check: cloudcontrol.server-side-computed-readonly-property-rejected-on-write.

Cause 3 of 3: a create that the OWNING service's handler rejects is reported with ProgressEvent.ErrorCode "InvalidRequest" where AWS reports "GeneralServiceException".
AWS: ProgressEvent.ErrorCode targets HandlerErrorCode, whose modeled values include both InvalidRequest and GeneralServiceException. Recorded behavior: when the create passes Cloud Control's own schema validation but the downstream service refuses it (an SQS FIFO name/flag inconsistency — a ".fifo" name with FifoQueue=False, or FifoQueue=True with a name lacking the suffix), the request reaches OperationStatus FAILED with ErrorCode "GeneralServiceException", the catch-all handler code for a downstream service exception, and a StatusMessage carrying the owning service's own wording.
Emulator: reaches FAILED with the right kind of StatusMessage but ErrorCode "InvalidRequest".
Why: the two HandlerErrorCode values mean different things to a client — InvalidRequest says the caller's input was rejected by the handler as malformed, GeneralServiceException says the downstream service raised an exception. Error handling and retry classification branch the wrong way, and a client cannot tell a request it can fix from a service-side failure it should retry or escalate.
Fix: the failure-classification step of the CreateResource handler (where a downstream provisioning error is turned into a terminal ProgressEvent) must map an exception raised by the owning service to HandlerErrorCode GeneralServiceException, reserving InvalidRequest for input the handler itself rejects.
Failing checks: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource, cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource-dup-removed.

## Evidence
All quotes are from the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Generated queue names, request tokens and event timestamps fill the templates and are not part of the requirement.

Cause 1 — cloudcontrol.create-only-property-mutation-rejected: "AssertionError: GetResource after delete: expected HTTP 400, got 404" (cc.get_resource of a deleted AWS::SQS::Queue; the error code ResourceNotFoundException matched, only the HTTP status diverged).

Cause 2 — cloudcontrol.server-side-computed-readonly-property-rejected-on-write: "AssertionError: the read-only patch was expected to be refused but returned {'ProgressEvent': {'TypeName': 'AWS::SQS::Queue', 'Identifier': 'https://sqs.<region>.amazonaws.com/<account>/<queue-name>', 'RequestToken': '<uuid>', 'Operation': 'UPDATE', 'OperationStatus': 'IN_PROGRESS', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}". The call under test is cc.update_resource(TypeName="AWS::SQS::Queue", Identifier=<queue url>, PatchDocument='[{"op":"replace","path":"/Arn","value":"<some arn>"}]'); the requirement is error code ValidationException with Message "Invalid patch update: readOnlyProperties [/properties/Arn] cannot be updated" — fixed text "Invalid patch update: readOnlyProperties [" ... "] cannot be updated", variable part the pointer list derived from the patch paths.

Cause 3 — cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource: "expected the downstream SQS handler's rejection to surface as ErrorCode='GeneralServiceException', got {... 'OperationStatus': 'FAILED', 'StatusMessage': 'The name of a FIFO queue can only include alphanumeric characters, hyphens, or underscores, must end with .fifo suffix and be 1 to 80 in length.', 'ErrorCode': 'InvalidRequest'}". cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource-dup-removed: "a FAILED create must carry ErrorCode GeneralServiceException, got {... 'OperationStatus': 'FAILED', 'StatusMessage': 'Can only include alphanumeric characters, hyphens, or underscores. 1 to 80 in length', 'ErrorCode': 'InvalidRequest'}". OperationStatus FAILED is correct in both; only ErrorCode diverges (the checks explicitly do not grade the StatusMessage, which carries a per-run downstream request id).

## Steps to Reproduce
Boot the emulator from this checkout and point botocore at it; cc = client("cloudcontrol", endpoint_url=<emulator>).

Cause 1: create an AWS::SQS::Queue through cc.create_resource, poll to SUCCESS, cc.delete_resource and poll to SUCCESS, then cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<the queue url>). Inspect ResponseMetadata.HTTPStatusCode of the ResourceNotFoundException: it must be 400; the emulator answers 404.

Cause 2: create an AWS::SQS::Queue through cc.create_resource, poll to SUCCESS, read its Arn with cc.get_resource, then cc.update_resource(TypeName="AWS::SQS::Queue", Identifier=<queue url>, PatchDocument='[{"op":"replace","path":"/Arn","value":"arn:aws:sqs:us-east-1:111122223333:bogus"}]'). It must raise error code ValidationException with Message "Invalid patch update: readOnlyProperties [/properties/Arn] cannot be updated" and mint no RequestToken; the emulator returns HTTP 200 with an IN_PROGRESS ProgressEvent.

Cause 3: cc.create_resource(TypeName="AWS::SQS::Queue", DesiredState=json.dumps({"QueueName": "<name>.fifo", "FifoQueue": False})) — or a FifoQueue=True name lacking the ".fifo" suffix — and poll cc.get_resource_request_status to the terminal event. OperationStatus must be FAILED with ErrorCode "GeneralServiceException"; the emulator reports ErrorCode "InvalidRequest".

Re-run the four checks in trace_ids to confirm.
