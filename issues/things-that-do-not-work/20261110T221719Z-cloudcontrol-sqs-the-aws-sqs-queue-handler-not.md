Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__sqs.cloudcontrol-lifecycle-visible-to-native-sqs-api cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found
Issue-Key: cloudcontrol__sqs.cloudcontrol-lifecycle-visible-to-native-sqs-api|run#34|AssertionError|assert _handler_not_found(
Issue-Key: cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found|run#19|AssertionError|assert cc_error == ("ResourceNotFoundException", expected_message), (
Issue-Commit: ac8063d11ee524ed56d43e71cb0e4e160867dbbf

# cloudcontrol__sqs: the AWS::SQS::Queue handler not-found message renders the identifier as a JSON primary-identifier object instead of the bare QueueUrl

> _Note: this issue was identified against the code base at commit `ac8063d11ee524ed56d43e71cb0e4e160867dbbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 1: in the cross-service Cloud Control/SQS checks, the handler-wrapped ResourceNotFoundException message that Cloud Control serves for a gone AWS::SQS::Queue carries the identifier serialized as the primary-identifier JSON object instead of the bare primary-identifier string.

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) models ResourceNotFoundException on GetResource/UpdateResource/DeleteResource (doc: "A resource with the specified identifier can't be found.") and ProgressEvent.ErrorCode targets HandlerErrorCode, whose values include NotFound — the verdict comes from the owning type's handler, so Cloud Control wraps that handler's own diagnostic. The model says nothing about how the identifier is rendered inside the diagnostic; for that the basis is recorded behavior only. The recorded template is the fixed text "<TypeName> Handler returned status FAILED: Resource of type '<TypeName>' with identifier '<identifier>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)", whose variable parts are the TypeName and Identifier from the request and the server-minted RequestToken. For AWS::SQS::Queue the recorded <identifier> is the queue URL exactly as the caller passed it — a bare string.

Emulator: the wrapper, the error code (ResourceNotFoundException) and the RequestToken are right, but the identifier appears as '{"/properties/QueueUrl":"<queue url>"}'.

Why: this message is the only place a synchronous Cloud Control refusal exposes the handler verdict and the resource it concerns. A cross-service caller that deletes a queue through SQS's own API and then reconciles Cloud Control's answer against the queue URL it holds cannot match the identifier it sent to the identifier in the refusal, so ignore-if-not-found and drift-reconciliation logic keyed on the URL silently fails; the JSON form also leaks Cloud Control's internal resource-model keying into a user-visible message.

Fix: the per-type identifier rendering in the shared not-found refusal composer behind the resource-level operations (the GetResource/UpdateResource/DeleteResource handlers in the cloudcontrol crate and the shared not-found message builder in its state module). This is the same root cause reported for the cloudcontrol-only checks in the companion issue filed in this same sweep (title "cloudcontrol: the AWS::SQS::Queue handler not-found message renders the identifier as a JSON primary-identifier object instead of the bare QueueUrl"); it is filed separately here because these two checks are reported under the cloudcontrol__sqs service heading. The rendering is PER RESOURCE TYPE and must not be changed globally: open issue 20261103T024056Z-cloudcontrolapi-refusals-answer-404-or-200-where-aws.md records from the wire corpus that for AWS::Logs::LogGroup AWS DOES use the '{"/properties/LogGroupName":"<name>"}' form in that same slot.

Not covered by the open SQS-related issues: 20261102T044041Z-cloudcontrol-sqs-2-net-new-causes-cloud-control.md and 20261109T185037Z-cloudcontrol-sqs-cloud-control-getresource-never-resolves-an-aws-sqs.md concern provisioning into SQS and read-through of natively created queues; both of those stages now pass in these checks (Cloud Control resolves the natively created queue with SQS's own values before the failing assertion), and the replay reports no owning issue for either check.

Traces: the two failing checks in trace_ids — each creates a queue (through Cloud Control or through SQS's own CreateQueue), deletes it, and grades the exact not-found message Cloud Control then serves.

## Evidence
Quotes from the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Generated queue names and request tokens fill the templates and are not part of the requirement.

cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found: "GetResource for the natively-deleted queue 'https://sqs.<region>.amazonaws.com/<account>/<queue-name>' answered ('ResourceNotFoundException', 'AWS::SQS::Queue Handler returned status FAILED: Resource of type \\'AWS::SQS::Queue\\' with identifier \\'{\"/properties/QueueUrl\":\"https://sqs.<region>.amazonaws.com/<account>/<queue-name>\"}\\' was not found. (HandlerErrorCode: NotFound, RequestToken: <uuid>)'), expected ('ResourceNotFoundException', \"AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier 'https://sqs.<region>.amazonaws.com/<account>/<queue-name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <uuid>)\")".

cloudcontrol__sqs.cloudcontrol-lifecycle-visible-to-native-sqs-api: the _handler_not_found assertion fails on "AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier '{\"/properties/QueueUrl\":\"https://sqs.<region>.amazonaws.com/<account>/<queue-name>\"}' was not found. (HandlerErrorCode: NotFound, RequestToken: <uuid>)".

Fixed text: "AWS::SQS::Queue Handler returned status FAILED: Resource of type '", "' with identifier '", "' was not found. (HandlerErrorCode: NotFound, RequestToken: ", ")". Variable parts: the TypeName and Identifier from the request and the server-minted RequestToken. Only the Identifier rendering diverges; the error code and the token agree on both sides.

## Steps to Reproduce
Boot the emulator from this checkout and point botocore at it; cc = client("cloudcontrol", endpoint_url=<emulator>), sqs = client("sqs", endpoint_url=<emulator>).

1. sqs.create_queue(QueueName="<a fresh name>") and keep the QueueUrl.
2. cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<that QueueUrl>) — resolves (this part works).
3. sqs.delete_queue(QueueUrl=<that QueueUrl>), then poll cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<that QueueUrl>) until it refuses, and read Error.Message.

Required: "AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier '<that QueueUrl>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)" — a bare queue URL. Observed: '{"/properties/QueueUrl":"<that QueueUrl>"}' in the identifier slot.

The same message is graded after a Cloud Control create/patch/delete lifecycle in cloudcontrol__sqs.cloudcontrol-lifecycle-visible-to-native-sqs-api. Re-run both checks in trace_ids to confirm.
