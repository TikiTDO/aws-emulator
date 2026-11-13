Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.e2e-cancel-request-mid-flight-resource-absent
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.e2e-cancel-request-mid-flight-resource-absent|run#8|AssertionError|raise AssertionError(
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# cloudcontrol: a mid-flight CancelResourceRequest never reaches CANCEL_COMPLETE — no request is ever cancellable, so a cancelled create's resource is left in place

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelResourceRequest

## Description
Cause 1 of 1: a Cloud Control request can never be cancelled while it is in flight, so the CANCEL_IN_PROGRESS -> CANCEL_COMPLETE outcome and its consequence (the half-created resource is not left behind) are unreachable in the emulator. Failing check: cloudcontrol.e2e-cancel-request-mid-flight-resource-absent.

AWS: Smithy (com.amazonaws.cloudcontrol#CloudApiService) models CancelResourceRequest with input RequestToken (required, @length 1-128, pattern ^[-A-Za-z0-9+/=]+$), output ProgressEvent, and the errors RequestTokenNotFoundException (client, HTTP 404) and ConcurrentModificationException (server, HTTP 500). CreateResource is modeled as asynchronous: it returns a ProgressEvent carrying a RequestToken, and the request moves through the non-terminal statuses PENDING / IN_PROGRESS before a terminal one. Recorded behavior: while a request is PENDING or IN_PROGRESS, CancelResourceRequest is ACCEPTED (2xx) and returns a ProgressEvent for that same RequestToken whose OperationStatus is CANCEL_IN_PROGRESS; GetResourceRequestStatus on the token then settles on OperationStatus CANCEL_COMPLETE, and the resource the cancelled create was provisioning does not exist afterwards — GetResource for its identifier is refused with ResourceNotFoundException, the identifier is absent from ListResources, and the native service agrees (for AWS::SNS::Topic, sns:GetTopicAttributes is refused with code NotFound and the fixed message "Topic does not exist"). Only once the request is terminal does cancel stop being accepted (that terminal refusal is a separate, already-filed cause).

Emulator: no CancelResourceRequest ever takes effect, so no request ever reaches CANCEL_COMPLETE. Two emulator behaviors combine to produce this and both must be addressed for the check to close: (a) CancelResourceRequest is a scaffold stub that refuses every call with HTTP 501, Error.Code "NotImplemented", message "operation CancelResourceRequest is scaffolded but not implemented in this emulator" — it never returns the CANCEL_IN_PROGRESS ProgressEvent AWS returns, and never transitions the request; (b) CreateResource resolves its request to a terminal OperationStatus SUCCESS with no observable PENDING/IN_PROGRESS window, so even a correct cancel handler would have nothing cancellable to act on. Over 5 attempts the replayed check found every AWS::SNS::Topic create already terminal SUCCESS and never observed CANCEL_COMPLETE.

Why: a client that submits a create and then cancels it (the documented way to abort a provisioning request, e.g. after a user aborts or a dependency check fails) gets a 501 it must treat as an infrastructure fault rather than an accepted cancellation, and the resource the create was provisioning is created and left behind instead of being rolled back. Any code path that branches on OperationStatus CANCEL_IN_PROGRESS / CANCEL_COMPLETE is dead against the emulator, and cleanup logic that relies on cancel to prevent a resource from existing silently leaks resources.

Fix: the CancelResourceRequest handler in the cloudcontrol crate (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/cancel_resource_request.rs) together with the request/progress-event state machine that CreateResource and GetResourceRequestStatus share: cancel of a non-terminal request must be accepted and return a ProgressEvent for that RequestToken with OperationStatus CANCEL_IN_PROGRESS, the request must settle at CANCEL_COMPLETE, and the cancelled create must not leave the resource in Cloud Control's index or in the backing native service; the create path must expose a non-terminal window (PENDING/IN_PROGRESS) rather than resolving synchronously to SUCCESS.

Related open issues that do NOT cover this check — its replay key is in none of their Issue-Trace-Ids: 20261102T102724Z-cloudcontrol-2-net-new-causes-cancelresourcerequest-answers-notimplemented.md covers only the cancel REFUSAL classes (RequestTokenNotFoundException for an unknown token, ValidationException for a terminal one) and the AWS::S3::Bucket create write-through; the auto-filed stub issue for CancelResourceRequest asks for the operation to exist but states neither the accepted-cancel path (CANCEL_IN_PROGRESS -> CANCEL_COMPLETE) nor the requirement that the cancelled create leave no resource behind, nor the missing non-terminal create window. This check must be re-replayed against whatever handler lands.

Traces: cloudcontrol.e2e-cancel-request-mid-flight-resource-absent.

## Evidence
cloudcontrol.e2e-cancel-request-mid-flight-resource-absent — [mismatch]
AssertionError raised at check_cloudcontrol_e2e_cancel_request_mid_flight_resource_absent.py line 63: "could not win the CreateResource/CancelResourceRequest race ... (the CREATE reached SUCCESS before our CancelResourceRequest call landed, every time)" after _MAX_ATTEMPTS = 5 attempts.

Per attempt the check does: create_resource(TypeName="AWS::SNS::Topic", DesiredState={"TopicName": <unique>, "Tags":[...]}) -> cancel_resource_request(RequestToken=<token>) (ClientError swallowed) -> poll get_resource_request_status until terminal.
Expected (AWS): at least one attempt's request settles at OperationStatus CANCEL_COMPLETE, after the cancel is accepted with a CANCEL_IN_PROGRESS ProgressEvent; then GetResource(TypeName="AWS::SNS::Topic", Identifier=<topic arn>) -> ResourceNotFoundException, the arn absent from ListResources, and sns:GetTopicAttributes -> code "NotFound", message "Topic does not exist" (fixed text; the ARN is the request-derived variable part).
Actual (emulator): every attempt's request is terminal SUCCESS — CANCEL_COMPLETE never observed. The cancel call itself is refused with HTTP 501, Error.Code "NotImplemented", Error.Message "operation CancelResourceRequest is scaffolded but not implemented in this emulator" (fixed text apart from the operation name), i.e. the emulator neither accepts the cancel nor exposes a non-terminal window in which a cancel could land, and the topic the cancelled create provisioned remains present.

## Steps to Reproduce
1. Boot the emulator and build cloudcontrol + sns clients.
2. ev = cc.create_resource(TypeName="AWS::SNS::Topic", DesiredState='{"TopicName":"<unique>","Tags":[{"Key":"sim2real","Value":"x"}]}')["ProgressEvent"] — observe OperationStatus is already terminal (SUCCESS) with no PENDING/IN_PROGRESS window; AWS returns a non-terminal status first.
3. cc.cancel_resource_request(RequestToken=ev["RequestToken"]) — observe HTTP 501 / NotImplemented; AWS (for a still-running request) answers 2xx with a ProgressEvent whose OperationStatus is CANCEL_IN_PROGRESS.
4. Poll cc.get_resource_request_status(RequestToken=ev["RequestToken"]) to terminal — observe SUCCESS; AWS reports CANCEL_COMPLETE.
5. cc.get_resource(TypeName="AWS::SNS::Topic", Identifier="arn:aws:sns:<region>:<account>:<topic name>") and sns.get_topic_attributes(TopicArn=<same arn>) — observe the topic EXISTS; AWS refuses both (ResourceNotFoundException, and SNS NotFound "Topic does not exist") because the cancelled create left no resource. Repeat up to 5 times as the check does: no attempt ever reaches CANCEL_COMPLETE.
