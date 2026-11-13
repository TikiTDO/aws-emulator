Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.cancel-bogus-request-token-must-fail cloudcontrol.cancel-completed-request-fails cloudcontrol.cancel-terminal-request-rejected cloudcontrol.cancel-window-closes-after-terminal-state cloudcontrol.concurrent-cancel-and-complete-race-ordering cloudcontrol.delete-resource-wrong-state-blocked
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.cancel-bogus-request-token-must-fail|run#10|AssertionError|assert control_code == "ValidationException", (
Issue-Key: cloudcontrol.cancel-completed-request-fails|run#9|AssertionError|assert code == "ValidationException", (
Issue-Key: cloudcontrol.cancel-terminal-request-rejected|run#8|AssertionError|assert cancel_code == "ValidationException", cancel_code
Issue-Key: cloudcontrol.cancel-window-closes-after-terminal-state|run#9|AssertionError|assert code == "ValidationException", (
Issue-Key: cloudcontrol.concurrent-cancel-and-complete-race-ordering|run#7|AssertionError|assert cancel_code == "ValidationException", (
Issue-Key: cloudcontrol|PutObject|NoSuchBucket
Issue-Commit: 699db40da250628f4cc12f3dc175d789bc27ad1e

# cloudcontrol: 2 net-new causes — CancelResourceRequest answers NotImplemented instead of AWS's cancel refusal semantics, and AWS::S3::Bucket CreateResource reports SUCCESS without creating the bucket in S3

> _Note: this issue was identified against the code base at commit `699db40da250628f4cc12f3dc175d789bc27ad1e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelResourceRequest

## Description
Two distinct root causes in the Cloud Control API emulator, both grounded in failing behavioral checks that are NOT marked ALREADY FILED/OWNED by the replay report.

Cause 1 of 2: CancelResourceRequest refuses every call with NotImplemented, so the cancel REFUSAL semantics AWS defines for a token that is unknown or already terminal are unreachable.
AWS: Smithy (com.amazonaws.cloudcontrol#CloudApiService) models CancelResourceRequest with input RequestToken (required, @length 1-128, pattern ^[-A-Za-z0-9+/=]+$) and the errors RequestTokenNotFoundException (client, HTTP 404) and ConcurrentModificationException (server, HTTP 500). Recorded behavior for the two cases these checks exercise: (a) a RequestToken that was never issued is refused with error code RequestTokenNotFoundException and message "Request with token <RequestToken> was not found" — fixed text "Request with token " ... " was not found", with the token taken verbatim from the request; (b) a RequestToken that exists but whose request has already reached a terminal OperationStatus (SUCCESS, FAILED, CANCEL_COMPLETE) is refused with error code ValidationException and message "Request <RequestToken> is already in status <OperationStatus>" — fixed text "Request " ... " is already in status ", with the token taken from the request and the status read from that request's own recorded ProgressEvent (the checks observe SUCCESS). Only PENDING/IN_PROGRESS requests are cancelable. An unknown token must also stay absent from ListResourceRequests.
Emulator: every CancelResourceRequest call is answered with error code NotImplemented (HTTP 501, message "operation CancelResourceRequest is scaffolded but not implemented in this emulator"), for a freshly minted, resolvable token as well as for a well-formed random UUID. So the emulator cannot distinguish "unknown token" from "terminal request", and a client sees a 501 server-side refusal where AWS gives a 404 RequestTokenNotFoundException or a 400 ValidationException.
Why: a client that cancels a request it has just submitted, or retries a cancel after the request finished, gets a NotImplemented it must treat as an emulator/infrastructure fault instead of the deterministic classified refusal AWS returns; code that branches on RequestTokenNotFoundException vs ValidationException (give up vs. read the terminal state) never takes either branch, and code that distinguishes a cancelable window from a closed one cannot be exercised at all.
Fix: the CancelResourceRequest handler and its request-token/state lookup in the cloudcontrol crate (services/service-com-amazonaws-cloudcontrol-cloudapiservice/src/ops/cancel_resource_request.rs plus the request-request store it must read) — it must resolve the token against the recorded requests, refuse an unknown token as RequestTokenNotFoundException and a terminal one as ValidationException with the messages above. NOTE: open issue 20261102T095426Z-cloudcontrol-cancelresourcerequest-stub.md asks for the operation to be implemented but carries only 4 other check ids and does not state these two refusal classes or their wording; the five checks below are not in its Issue-Trace-Ids and must be re-replayed against whatever handler lands.
Traces: cloudcontrol.cancel-bogus-request-token-must-fail, cloudcontrol.cancel-completed-request-fails, cloudcontrol.cancel-terminal-request-rejected, cloudcontrol.cancel-window-closes-after-terminal-state, cloudcontrol.concurrent-cancel-and-complete-race-ordering.

Cause 2 of 2: CreateResource for AWS::S3::Bucket reaches OperationStatus SUCCESS but no bucket exists in S3 afterwards.
AWS: recorded behavior only — a CreateResource on TypeName AWS::S3::Bucket whose request reaches a terminal OperationStatus SUCCESS has actually provisioned the bucket in S3, so a subsequent native s3:PutObject into that bucket name succeeds (and s3:HeadBucket answers 200). Smithy: CreateResource returns a ProgressEvent and models the failure modes (HandlerFailureException, GeneralServiceException, ...) a provisioning failure must surface through instead of a SUCCESS.
Emulator: GetResourceRequestStatus reports the create's ProgressEvent as OperationStatus SUCCESS, but the immediately following s3.put_object on the same bucket name is refused with NoSuchBucket, "The specified bucket does not exist" — the Cloud Control request was recorded as successful in Cloud Control's own private store while nothing was created in the S3 service.
Why: a client that uses Cloud Control to provision a bucket and then writes objects through the S3 API (the normal pattern, and the only way to set up any bucket-dependency scenario such as deleting a non-empty bucket) gets NoSuchBucket from a resource Cloud Control just told it was created; every downstream S3-dependent flow built on a Cloud Control-provisioned bucket is unreachable, and failures are reported as successes.
Fix: the AWS::S3::Bucket resource provider / provisioning path behind the CreateResource handler in the cloudcontrol crate — the create handler must write through to the S3 service's state (as the native S3 bucket store), or resolve the request to FAILED with the modeled handler error when it cannot, rather than marking its own index entry SUCCESS. Related but distinct open issues cover other types and are not this one: 20261102T041105Z-...-dynamodb (AWS::DynamoDB::Table), 20261102T044041Z-cloudcontrol-sqs-... (AWS::SQS::Queue), 20261102T095426Z-cloudcontrol-6-net-new-causes-... (Logs, CloudWatch, SQS, SSM); no filed issue names AWS::S3::Bucket.
Traces: cloudcontrol.delete-resource-wrong-state-blocked.

## Evidence
Cause 1 (CancelResourceRequest):
- cloudcontrol.cancel-bogus-request-token-must-fail — AssertionError: a just-submitted request token must resolve, not be treated as unknown, got NotImplemented. Expected for the terminal token: code ValidationException, message "Request <token> is already in status SUCCESS"; expected for the never-issued UUID: code RequestTokenNotFoundException, message "Request with token <token> was not found".
- cloudcontrol.cancel-completed-request-fails — AssertionError: cancel of an already-terminal request must fail, got NotImplemented (expected ValidationException).
- cloudcontrol.cancel-terminal-request-rejected — assert cancel_code == "ValidationException" -> AssertionError: NotImplemented.
- cloudcontrol.cancel-window-closes-after-terminal-state — AssertionError: expected ValidationException canceling a terminal request, got 'NotImplemented'.
- cloudcontrol.concurrent-cancel-and-complete-race-ordering — AssertionError: expected ValidationException for a cancel arriving after the create had already reached a terminal state, got 'NotImplemented'.
The refusal body observed on the wire is HTTP 501, Error.Code "NotImplemented", Error.Message "operation CancelResourceRequest is scaffolded but not implemented in this emulator".

Cause 2 (AWS::S3::Bucket):
- cloudcontrol.delete-resource-wrong-state-blocked — the check's assertion that the create reached SUCCESS PASSED (GetResourceRequestStatus returned OperationStatus SUCCESS for the AWS::S3::Bucket CreateResource), and the very next call failed: botocore.errorfactory.NoSuchBucket: An error occurred (NoSuchBucket) when calling the PutObject operation: The specified bucket does not exist. Expected: PutObject succeeds because the bucket Cloud Control reported as created exists in S3.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and create a client for cloudcontrol.
2. cc.create_resource(TypeName="AWS::SQS::Queue", DesiredState='{"QueueName":"<name>"}') and poll cc.get_resource_request_status(RequestToken=<token>) until OperationStatus is SUCCESS.
3. cc.cancel_resource_request(RequestToken=<that token>) — observe HTTP 501 / NotImplemented; AWS answers ValidationException "Request <token> is already in status SUCCESS".
4. cc.cancel_resource_request(RequestToken=str(uuid.uuid4())) — observe HTTP 501 / NotImplemented; AWS answers RequestTokenNotFoundException "Request with token <token> was not found", and that token is absent from ListResourceRequests.

Cause 2:
1. cc.create_resource(TypeName="AWS::S3::Bucket", DesiredState='{"BucketName":"<lowercase-name>","Tags":[{"Key":"sim2real","Value":"x"}]}').
2. Poll cc.get_resource_request_status until the request is terminal — it reports OperationStatus SUCCESS.
3. s3.put_object(Bucket="<same name>", Key="blocker.txt", Body=b"blocking-content") — observe NoSuchBucket "The specified bucket does not exist" (s3.head_bucket also 404s), proving the bucket was never provisioned despite the SUCCESS.
