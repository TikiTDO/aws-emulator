Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.e2e-delivery-pipeline-log-flow cloudfront__logs.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected logs.destination-policy-enforces-cross-account-subscription logs.e2e-subscription-filter-invalid-role-rejected logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected
Issue-Key: logs.e2e-delivery-pipeline-log-flow|_error#21|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: cloudfront__logs.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to fail, but it succeeded: {result!r}")
Issue-Key: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: logs.destination-policy-enforces-cross-account-subscription|run#25|AssertionError|assert code_before_policy == "AccessDeniedException", (
Issue-Key: logs.e2e-subscription-filter-invalid-role-rejected|_error#32|AssertionError|raise AssertionError(
Issue-Key: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected|run#9|AssertionError|raise AssertionError(
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudWatch Logs vended-delivery and subscription plumbing is over-permissive: PutDeliverySource validates nothing, PutSubscriptionFilter skips destination authorization and role assumability, PutTransformer accepts a misplaced vended parser (6 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliverySource

## Description
Six net-new root causes in the CloudWatch Logs (`logs`) surface. All six are refusals real AWS performs that this emulator does not perform (or performs with the wrong error code). Cause count: 6 — each is numbered below with its own failing check ids and must be closed before this issue is submitted.

NOTE ON SERVICE ATTRIBUTION: two of the six causes are grounded in checks whose id carries a compound leading token (`logs__cloudfront.`, `cloudfront__logs.`). The operation under test in both is `logs:PutDeliverySource`, a CloudWatch Logs operation, so this issue is filed against service `logs`; CloudFront only supplies the distribution ARN, and both services are bootstrapped.

RELATED OPEN ISSUES (neither covers these causes): 20261023T113357Z-cloudwatch-logs-putdeliverysource-stub.md still describes PutDeliverySource as a 501 scaffold — the operation now answers HTTP 200 with a well-formed `deliverySource`, so what remains is the missing validation described in causes 1-3. 20261024T150703Z-logs-cloudwatch-logs-cross-service-stubs-make-every-subscription.md records (cause 1 of 7) that the kinesis CreateStream stub made every subscription-destination check unrunnable; CreateStream now answers, the checks run, and the logs-side assertions it said were "completely unverified" now fail for the distinct reasons in causes 4-6. 20261024T150703Z-cloudwatch-logs-puttransformer-accepts-a-transformer-with-no.md covers PutTransformer accepting a chain with NO parser at all; cause 6 is the different rule that a vended parser must be FIRST.

=== Cause 1 of 6: PutDeliverySource accepts a resourceArn naming a resource that does not exist ===
AWS: Smithy PutDeliverySource takes name/resourceArn/logType (all @required; resourceArn target `Arn` is free-form) and models ConflictException, ResourceNotFoundException, ServiceQuotaExceededException, ThrottlingException, ValidationException (all 400) plus ServiceUnavailableException (500). The reference is resolved, not merely stored: for a logType=S3_SERVER_ACCESS_LOGS source whose resourceArn names a bucket that was never created, the recorded response is 400 ResourceNotFoundException with message "The specified bucket does not exist".
Emulator: the call answers HTTP 200 and returns a `deliverySource` echoing the bogus ARN (service="s3", logType="S3_SERVER_ACCESS_LOGS"), creating a delivery source that points at nothing.
Why: a client wiring a vended-log pipeline gets a green PutDeliverySource for a typo'd or deleted resource; the failure surfaces much later (or never, since nothing is ever delivered), and a client's not-found handling for this operation never runs.
Fix: the PutDeliverySource handler's validation in services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_delivery_source.rs — resolve resourceArn against the owning service's state before persisting.
Traces: logs.e2e-delivery-pipeline-log-flow.

=== Cause 2 of 6: PutDeliverySource accepts a logType the resource's service does not vend ===
AWS: `logType` is model-valid for anything matching ^[\w]*$ (length 1..255), but the service narrows it per resource: for a CloudFront distribution ARN only the log types CloudFront vends are accepted (DescribeConfigurationTemplates(service=cloudfront) lists ACCESS_LOGS and nothing else). The recorded rejection for logType=ERROR_LOGS on a real distribution ARN is 400 ValidationException, message "This service is not allowed for this LogSource." (example text), and GetDeliverySource for that name afterwards is ResourceNotFoundException — the refusal leaves nothing behind.
Emulator: the call answers HTTP 200 and returns a `deliverySource` with service="cloudfront", logType="ERROR_LOGS", persisting a source for a log type that service cannot vend.
Why: this is a service rule narrower than the model, so a model-driven client that offers every ^[\w]*$ log type is never told which ones the resource actually supports; the delivery it then creates can never produce data.
Fix: the PutDeliverySource handler's logType/resource-service compatibility check (same file), keyed off the same table DescribeConfigurationTemplates serves.
Traces: logs__cloudfront.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected.

=== Cause 3 of 6: PutDeliverySource accepts a CloudFront ACCESS_LOGS delivery source outside the log type's home region ===
AWS: PutDeliverySource is a regional operation modelled identically in every CloudWatch Logs region, but CloudFront's vended-log control plane is pinned to us-east-1. Recorded behavior: the same request (CloudFront distribution ARN, logType=ACCESS_LOGS) is 400 ValidationException outside us-east-1 — message "PutDeliverySource action is not supported for this LogType in region <region>" (example text) — and creates nothing there (GetDeliverySource is ResourceNotFoundException), while succeeding in us-east-1.
Emulator: the out-of-region call answers HTTP 200 and returns a delivery source whose own arn is in the calling region (`arn:aws:logs:<other-region>:...:delivery-source/<name>`), so the region pinning is invisible.
Why: a client that provisions CloudFront access-log delivery in its default region is told it worked; against AWS that code path is a hard 400, so the emulator hides a deployment bug instead of surfacing it.
Fix: the PutDeliverySource handler's region/logType admission check (same file).
Traces: cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected.

=== Cause 4 of 6: PutSubscriptionFilter against a Destination with no access policy is refused with the wrong error code ===
AWS: a CloudWatch Logs Destination is unusable until an access policy is attached with PutDestinationPolicy — with no policy attached, even the destination-owning account's PutSubscriptionFilter against that destination ARN is refused with AccessDeniedException (recorded behavior; AccessDeniedException is not in PutSubscriptionFilter's modelled error set, whose client errors are InvalidOperationException, InvalidParameterException, LimitExceededException, OperationAbortedException, ResourceNotFoundException). Once any policy is attached the owning account's calls succeed.
Emulator: the same call is refused with error code InvalidParameterException instead of AccessDeniedException.
Why: the authorization boundary is reported as a malformed-input error, so a client cannot distinguish "the destination is not shared with me yet" (fix: attach a destination policy) from "my request is wrong" (fix: change the arguments), and retry/catch logic keyed on AccessDeniedException never fires.
Fix: the destination-authorization branch of the PutSubscriptionFilter handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_subscription_filter.rs) — classify the missing/denying destination access policy as AccessDeniedException.
Traces: logs.destination-policy-enforces-cross-account-subscription.

=== Cause 5 of 6: PutSubscriptionFilter accepts a roleArn CloudWatch Logs cannot assume ===
AWS: PutSubscriptionFilter validates delivery access synchronously by assuming `roleArn` and test-delivering a message to the destination stream. A role whose trust policy grants ec2.amazonaws.com instead of logs.amazonaws.com cannot be assumed, so the call is refused with 400 InvalidParameterException (recorded message, as an example: "Could not deliver test message to specified Kinesis stream. Check if the given Kinesis stream is in ACTIVE state.") and no filter is persisted. The identical call with a correctly-trusted, correctly-permissioned role succeeds (positive control).
Emulator: the call answers HTTP 200 with an empty body and the filter is created, so a filter that can never deliver anything looks healthy.
Why: the single synchronous check that tells a client its IAM wiring is wrong is absent; the breakage becomes a silent permanent delivery outage with no error anywhere.
Fix: the roleArn trust/assumability and test-delivery validation in the PutSubscriptionFilter handler (same file).
Traces: logs.e2e-subscription-filter-invalid-role-rejected.

=== Cause 6 of 6: PutTransformer accepts a processor chain whose vended parser is not the first processor ===
AWS: Smithy PutTransformer takes logGroupIdentifier plus transformerConfig (a Processors list, @length 1..20) and models InvalidParameterException (400) among its client errors. A vended-log parser (parseWAF, parseVPC, parsePostgres, ...) is only legal as the FIRST processor of the chain; the chain [{"parseJSON": {}}, {"parseWAF": {}}] is refused with InvalidParameterException.
Emulator: PutTransformer ACCEPTS [{"parseJSON": {}}, {"parseWAF": {}}] (HTTP 200) and installs the transformer.
Why: the client is told an ingest transformer is live when AWS would have rejected the configuration; on AWS the same template/stack fails at deploy time, here it deploys and then transforms events by rules the service would never have run.
Fix: the transformerConfig ordering validation in the PutTransformer handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/put_transformer.rs) — the same validation point as the open "transformer with no leading parser" issue, which checks a different condition.
Traces: logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected.

## Evidence
Cause 1 — logs.e2e-delivery-pipeline-log-flow: AssertionError: expected a rejection, but the call succeeded and returned {'deliverySource': {'name': 'sim2real-behav-delivery-src-bogus-...', 'arn': 'arn:aws:logs:us-west-2:000000000000:delivery-source/...', 'resourceArns': ['arn:aws:s3:::sim2real-behav-delivery-missing-...'], 'service': 's3', 'logType': 'S3_SERVER_ACCESS_LOGS'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected 400 ResourceNotFoundException, message "The specified bucket does not exist".

Cause 2 — logs__cloudfront.obs-cloudfront-3-delivery-source-unsupported-log-type-rejected: AssertionError: expected the call to fail, but it succeeded: {'deliverySource': {..., 'service': 'cloudfront', 'logType': 'ERROR_LOGS', 'tags': {...}}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected 400 ValidationException ("This service is not allowed for this LogSource."), then GetDeliverySource -> ResourceNotFoundException.

Cause 3 — cloudfront__logs.xsvc-cloudfront-3-access-logs-delivery-source-outside-us-east-1-rejected: AssertionError: expected the call to be rejected, but it succeeded: {'deliverySource': {'arn': 'arn:aws:logs:us-west-2:000000000000:delivery-source:...', 'resourceArns': ['arn:aws:cloudfront::000000000000:distribution/...'], 'service': 'cloudfront', 'logType': 'ACCESS_LOGS'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected 400 ValidationException "PutDeliverySource action is not supported for this LogType in region us-west-2".

Cause 4 — logs.destination-policy-enforces-cross-account-subscription: AssertionError: expected AccessDeniedException subscribing to a destination with no access policy in place, even for the owning account, got InvalidParameterException. (Destination was created by PutDestination against a live Kinesis stream and DescribeDestinations confirmed it carries no accessPolicy.)

Cause 5 — logs.e2e-subscription-filter-invalid-role-rejected: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}. Expected 400 InvalidParameterException for a roleArn whose trust policy grants ec2.amazonaws.com rather than logs.amazonaws.com; the positive control (correctly-trusted role, same log group and stream) succeeded in the same run.

Cause 6 — logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected: AssertionError: PutTransformer ACCEPTED [{'parseJSON': {}}, {'parseWAF': {}}]; a vended parser must be the first processor and real AWS rejects this chain with InvalidParameterException.

## Steps to Reproduce
Cause 1: logs PutDeliverySource(name=<new>, resourceArn='arn:aws:s3:::<bucket-that-was-never-created>', logType='S3_SERVER_ACCESS_LOGS') -> observe HTTP 200 and a persisted deliverySource; expected 400 ResourceNotFoundException.
Cause 2: create a CloudFront distribution, then logs PutDeliverySource(name=<new>, resourceArn=<distribution ARN>, logType='ERROR_LOGS') in us-east-1 -> observe HTTP 200; expected 400 ValidationException, and GetDeliverySource(name) must then be ResourceNotFoundException. Control: the same ARN with logType='ACCESS_LOGS' is accepted.
Cause 3: create a CloudFront distribution, then call logs PutDeliverySource(name=<new>, resourceArn=<distribution ARN>, logType='ACCESS_LOGS') against a region OTHER than us-east-1 -> observe HTTP 200; expected 400 ValidationException and nothing created in that region. Control: the same call in us-east-1 succeeds.
Cause 4: kinesis CreateStream + logs PutDestination(destinationName, targetArn=<stream>, roleArn=<role>) with NO PutDestinationPolicy, then logs PutSubscriptionFilter(logGroupName=<own group>, destinationArn=<destination arn>, filterPattern='') -> observe error code InvalidParameterException; expected AccessDeniedException.
Cause 5: create an IAM role whose trust policy names ec2.amazonaws.com, then logs PutSubscriptionFilter(logGroupName=<group>, filterName=<new>, filterPattern='', destinationArn=<kinesis stream arn>, roleArn=<that role>) -> observe HTTP 200 and a persisted filter; expected 400 InvalidParameterException and no filter.
Cause 6: logs PutTransformer(logGroupIdentifier=<group>, transformerConfig=[{'parseJSON': {}}, {'parseWAF': {}}]) -> observe HTTP 200; expected 400 InvalidParameterException because the vended parser is not the first processor.
