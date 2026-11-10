Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.concurrent-delete-same-identifier-race cloudcontrol.create-duplicate-identifier-rejected cloudcontrol.deploy-cfn-stack-deletion-removes-resource cloudcontrol.deploy-cfn-update-reflected-in-getresource cloudcontrol.deploy-cloudformation-resource-behaves cloudcontrol.deploy-cloudformation-stack-deletion-removes-resource cloudcontrol.e2e-create-update-observe-delete-flow cloudcontrol.e2e-create-use-observe-delete cloudcontrol.empty-string-vs-absent-property-preserved cloudcontrol.list-resource-requests-resource-type-filter-preserves-order
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.concurrent-delete-same-identifier-race|run#30|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.create-duplicate-identifier-rejected|run#31|AssertionError|assert got_message == expected_got, f"expected message {expected_got!r}, got {got_message!r}"
Issue-Key: cloudcontrol.deploy-cfn-stack-deletion-removes-resource|run#19|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.deploy-cfn-update-reflected-in-getresource|run#23|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.deploy-cloudformation-resource-behaves|run#29|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.deploy-cloudformation-stack-deletion-removes-resource|run#18|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.e2e-create-update-observe-delete-flow|run#26|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudcontrol.e2e-create-use-observe-delete|run#26|AssertionError|assert message_head == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: cloudcontrol.empty-string-vs-absent-property-preserved|run#27|AssertionError|assert get_message == expected, f"expected message {expected!r}, got {get_message!r}"
Issue-Key: cloudcontrol.list-resource-requests-resource-type-filter-preserves-order|run#14|AssertionError|assert get_message == expected, f"expected message {expected!r}, got {get_message!r}"
Issue-Commit: ac8063d11ee524ed56d43e71cb0e4e160867dbbf

# cloudcontrol: the AWS::SQS::Queue handler not-found message renders the identifier as a JSON primary-identifier object instead of the bare QueueUrl

> _Note: this issue was identified against the code base at commit `ac8063d11ee524ed56d43e71cb0e4e160867dbbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
REOPENS 20261102T095426Z-cloudcontrol-10-net-new-causes-cloud-control-refusal.md: its fix left the call refused. That issue's Cause 1 is the handler-wrapped ResourceNotFoundException message of the resource-level reads, and it names these very checks (cloudcontrol.e2e-create-update-observe-delete-flow, cloudcontrol.empty-string-vs-absent-property-preserved, cloudcontrol.list-resource-requests-resource-type-filter-preserves-order, cloudcontrol.create-duplicate-identifier-rejected, cloudcontrol.concurrent-delete-same-identifier-race) in its Issue-Trace-Ids. The wrapper is now present, but the refusal is still not AWS's: the identifier embedded in it is rendered in the wrong form for AWS::SQS::Queue, so every one of these checks still fails on the same refusal.

Cause 1 of 1: for AWS::SQS::Queue, the identifier inside the handler-wrapped not-found message is serialized as the primary-identifier JSON object instead of the bare primary-identifier string.

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice/smithy-model/com-amazonaws-cloudcontrol-cloudapiservice.json) models ResourceNotFoundException on GetResource/UpdateResource/DeleteResource (doc: "A resource with the specified identifier can't be found."), and ProgressEvent.ErrorCode targets HandlerErrorCode whose values include NotFound — the not-found verdict comes from the owning resource type's handler, so Cloud Control wraps that handler's own diagnostic into the message. The model does not state how the identifier is rendered inside that diagnostic; for this the basis is recorded behavior only. The recorded message template is the fixed text "<TypeName> Handler returned status FAILED: Resource of type '<TypeName>' with identifier '<identifier>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)". Its variable parts are the TypeName and Identifier taken from the request and the server-minted RequestToken. For AWS::SQS::Queue the recorded <identifier> is the queue URL exactly as the caller passed it — a bare string, no JSON wrapper.

Emulator: emits the correct wrapper but substitutes the JSON primary-identifier object for the identifier, i.e. '{"/properties/QueueUrl":"<queue url>"}' where AWS has just '<queue url>'. Everything else in the message (prefix, inner sentence, HandlerErrorCode, RequestToken) matches.

Why: this message is the only place a synchronous Cloud Control refusal exposes the handler verdict and the resource it concerns. A client that correlates the refusal back to the identifier it asked about (ignore-if-not-found logic, drift reconciliation, log scraping keyed on the queue URL) never matches, because the identifier it sent is no longer the identifier in the answer; and the literal JSON form leaks Cloud Control's internal resource-model keying into a user-visible message.

Fix: the per-type identifier rendering in the shared not-found refusal composer behind the resource-level operations (the GetResource/UpdateResource/DeleteResource handlers in the cloudcontrol crate and the shared not-found message builder in its state module). The rendering is PER RESOURCE TYPE and must not be changed globally: open issue 20261103T024056Z-cloudcontrolapi-refusals-answer-404-or-200-where-aws.md records, from the wire corpus, that for AWS::Logs::LogGroup AWS DOES use the '{"/properties/LogGroupName":"<name>"}' JSON form in the same message slot. The requirement is "the identifier as the owning type's handler renders it": bare for AWS::SQS::Queue, JSON /properties object for AWS::Logs::LogGroup. A fix that flips the rendering globally in either direction will break the other type.

Traces: the ten failing checks in trace_ids. Each one creates (or has CloudFormation/SQS create) an AWS::SQS::Queue, deletes it, and then grades the exact not-found message of a Cloud Control read or write against the gone identifier.

## Evidence
All quotes are from the replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Generated queue names and request tokens fill the templates and are not part of the requirement.

cloudcontrol.e2e-create-update-observe-delete-flow — expected message "AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier 'https://sqs.<region>.amazonaws.com/<account>/<queue-name>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)", got "AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier '{\"/properties/QueueUrl\":\"https://sqs.<region>.amazonaws.com/<account>/<queue-name>\"}' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)".

The fixed text is "AWS::SQS::Queue Handler returned status FAILED: Resource of type '", "' with identifier '", "' was not found. (HandlerErrorCode: NotFound, RequestToken: ", ")". The variable parts are the TypeName and the Identifier from the request and the server-minted RequestToken; only the Identifier rendering diverges. The error code (ResourceNotFoundException) and the RequestToken in the message are identical on both sides in every one of the ten checks.

Identical expected/actual pairs, differing only in the generated queue name and token, are reported for: cloudcontrol.concurrent-delete-same-identifier-race (loser-racer FAILED StatusMessage), cloudcontrol.create-duplicate-identifier-rejected (post-delete GetResource of the positive-control queue), cloudcontrol.deploy-cfn-stack-deletion-removes-resource, cloudcontrol.deploy-cfn-update-reflected-in-getresource, cloudcontrol.deploy-cloudformation-resource-behaves, cloudcontrol.deploy-cloudformation-stack-deletion-removes-resource (all four: GetResource after the owning CFN stack is deleted), cloudcontrol.e2e-create-use-observe-delete (graded on the message head up to "(HandlerErrorCode: NotFound"), cloudcontrol.empty-string-vs-absent-property-preserved, cloudcontrol.list-resource-requests-resource-type-filter-preserves-order.

## Steps to Reproduce
Boot the emulator from this checkout and point botocore at it; cc = client("cloudcontrol", endpoint_url=<emulator>).

1. cc.create_resource(TypeName="AWS::SQS::Queue", DesiredState=json.dumps({"QueueName": "<a fresh name>"})) and poll cc.get_resource_request_status(RequestToken=...) until OperationStatus == "SUCCESS"; keep ProgressEvent.Identifier (the queue URL).
2. cc.delete_resource(TypeName="AWS::SQS::Queue", Identifier=<that URL>) and poll to SUCCESS.
3. cc.get_resource(TypeName="AWS::SQS::Queue", Identifier=<that URL>) and read Error.Message of the ResourceNotFoundException.

Required: "AWS::SQS::Queue Handler returned status FAILED: Resource of type 'AWS::SQS::Queue' with identifier '<that URL>' was not found. (HandlerErrorCode: NotFound, RequestToken: <token>)" — the identifier is the bare queue URL. Observed: the identifier appears as '{"/properties/QueueUrl":"<that URL>"}'.

Same message via cc.update_resource / cc.delete_resource against the gone identifier, and via the FAILED ProgressEvent.StatusMessage of a second delete racing the first (replay cloudcontrol.concurrent-delete-same-identifier-race). Re-run the ten checks in trace_ids to confirm.
