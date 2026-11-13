Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.list-insights-data-unsupported-operation-rejected cloudtrail.put-insight-selectors-insufficient-s3-policy-fails cloudtrail.start-logging-revoked-bucket-permission-denied
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.list-insights-data-unsupported-operation-rejected|_error_code#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: cloudtrail.put-insight-selectors-insufficient-s3-policy-fails|_expect_error#11|AssertionError|raise AssertionError(f"call unexpectedly succeeded where a rejection was required: {result!r}")
Issue-Key: cloudtrail.start-logging-revoked-bucket-permission-denied|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# CloudTrail accepts three calls real AWS refuses: CreateEventDataStore, PutInsightSelectors on an unpolicied bucket, and UpdateTrail after the bucket policy is revoked

## Resolution

Causes 2 and 3 are fixed: every write of a trail's log-delivery configuration now
re-resolves the trail's destination bucket and its policy at the instant of the call and
refuses before persisting anything — `PutInsightSelectors` on a trail resolves it at all
(it never did), and `UpdateTrail` resolves it on every call rather than only when a
destination member moves.

Cause 1 (CreateEventDataStore) is DECLINED, so the trace
`cloudtrail.list-insights-data-unsupported-operation-rejected` is not expected to replay
green: the refusal recorded there is an entitlement of the recorded ACCOUNT (CloudTrail
Lake closed to accounts that have not onboarded) and not a rule any parameter of the
request states — the model binds a 200 response plus `InvalidParameterException` for an
invalid parameter, and the recorded request's parameters are all valid — so the only way
to serve it would be an unconditional literal refusal, which would also make the
emulator's whole implemented CloudTrail Lake surface, and the Lake-capable branch another
recorded unit takes, unreachable.

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutInsightSelectors

## Description
There are 3 distinct causes in this issue; all 3 must be closed. Each is an over-acceptance: the emulator answers HTTP 200 with a fully-populated response where AWS refuses the call, so the refusal class is unreachable and the trail/Insights configuration the emulator reports was never validated. Causes 2 and 3 are the S3-bucket-policy pre-flight on operations OTHER than CreateTrail; the open issue 20261023T135552Z-cloudtrail-createtrail-updatetrail-skip-every-referenced-resource-and.md describes the create-time variant of that check but neither of these failing checks is carried by it (the suite did not mark them owned), so they are filed here and that issue is named for context, not as coverage.

Cause 1 of 3 — CreateEventDataStore is accepted and provisions a CloudTrail Lake event data store.
AWS: recorded behavior only (the Smithy model binds the error but not the precondition). Smithy (com-amazonaws-cloudtrail-cloudtrail-20131101.json) binds CreateEventDataStore error InvalidParameterException, errorKind client, httpError 400, awsQueryError code InvalidParameter. The check measured real AWS directly: a plain CreateEventDataStore(Name=<unique>, TerminationProtectionEnabled=false) is REFUSED with Error.Code 'InvalidParameterException' — CloudTrail Lake is closed to accounts that have not already onboarded, so no new event data store can be created at all. The refusal, not the message, is the requirement.
Emulator: HTTP 200 with a complete store: {'EventDataStoreArn': 'arn:aws:cloudtrail:<region>:<account>:eventdatastore/<uuid>', 'Status': 'CREATED', 'MultiRegionEnabled': False, 'OrganizationEnabled': False, 'RetentionPeriod': 366, 'TerminationProtectionEnabled': False, 'BillingMode': 'EXTENDABLE_RETENTION_PRICING'}.
Why: a client cannot test the eligibility-refusal branch at all, and worse, it is told a Lake event data store exists. Every downstream Lake call (StartQuery, ListInsightsData with an EDS InsightSource, EnableFederation) is then made against a resource AWS would never have handed out, so the emulator's Lake surface diverges from the first call onward.
Fix: CreateEventDataStore's handler/validation must refuse the request with InvalidParameterException (400, awsQueryError InvalidParameter) instead of allocating a store in state.
Traces: cloudtrail.list-insights-data-unsupported-operation-rejected.

Cause 2 of 3 — PutInsightSelectors does not evaluate the trail's destination S3 bucket policy.
AWS: Smithy binds PutInsightSelectors error InsufficientS3BucketPolicyException, errorKind client, httpError 403, awsQueryError code InsufficientS3BucketPolicy. When the trail's destination bucket carries no bucket policy at all (so CloudTrail may neither GetBucketAcl nor PutObject there), enabling Insights is refused with that code; restoring the policy makes the identical call succeed. Recorded message example: 'Incorrect S3 bucket policy is detected for bucket: <bucket>'. Insights also stay disabled after the refusal, so GetInsightSelectors then answers InsightNotEnabledException rather than an empty selector list.
Emulator: HTTP 200 — {'TrailARN': 'arn:aws:cloudtrail:<region>:<account>:trail/<name>', 'InsightSelectors': [{'InsightType': 'ApiCallRateInsight', 'EventCategories': ['Management']}]} — after the bucket's policy was deleted with s3 DeleteBucketPolicy. No policy evaluation happens, and Insights are recorded as enabled.
Why: a client is told Insights events will be delivered to a bucket CloudTrail cannot write to; the 403 remediation path (fix the bucket policy, retry) is untestable, and the effect-absent invariant (Insights remain disabled after a refused enable) cannot hold because there is no refusal.
Fix: PutInsightSelectors' validation must resolve the trail's destination bucket in S3 state, evaluate its policy for cloudtrail.amazonaws.com s3:GetBucketAcl/s3:PutObject, and raise InsufficientS3BucketPolicyException (403) before persisting any selector.
Traces: cloudtrail.put-insight-selectors-insufficient-s3-policy-fails.

Cause 3 of 3 — UpdateTrail does not re-validate a destination bucket whose policy was revoked after the trail was created.
AWS: Smithy binds UpdateTrail error InsufficientS3BucketPolicyException, errorKind client, httpError 403, awsQueryError code InsufficientS3BucketPolicy. UpdateTrail re-runs the destination-bucket check on every call, so re-designating the same (now unpolicied) bucket is refused even though the trail was valid at CreateTrail time; recorded message example: 'Incorrect S3 bucket policy is detected for bucket: <bucket>'. The rejected update also leaves the trail untouched (S3KeyPrefix does not change). StartLogging, by contrast, has no such synchronous check and still succeeds — that part of the check passed.
Emulator: HTTP 200 and the update is applied — {'Name': '<trail>', 'S3BucketName': '<bucket>', 'S3KeyPrefix': 'denied-update', 'IsMultiRegionTrail': False, 'TrailARN': 'arn:aws:cloudtrail:<region>:<account>:trail/<trail>', 'RecursiveLogging': True} — for a trail whose bucket policy had just been deleted.
Why: a client that mutates a trail as a way of proving its destination is still writable gets a false positive, and configuration drift (a bucket policy removed out of band) is never surfaced; the 403 branch is unreachable.
Fix: UpdateTrail's validation must re-resolve S3BucketName and its policy from S3 state on every call and raise InsufficientS3BucketPolicyException (403) without mutating the stored trail.
Traces: cloudtrail.start-logging-revoked-bucket-permission-denied.

## Evidence
Cause 1 — cloudtrail.list-insights-data-unsupported-operation-rejected, verdict 'mismatch': AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'EventDataStoreArn': 'arn:aws:cloudtrail:us-west-2:000000000000:eventdatastore/b1c8a0e2-92d0-4e42-b6fd-b92b27cfc82f', 'Name': 'sim2real-ctins-eds-d071802dd447', 'Status': 'CREATED', 'MultiRegionEnabled': False, 'OrganizationEnabled': False, 'RetentionPeriod': 366, 'TerminationProtectionEnabled': False, 'BillingMode': 'EXTENDABLE_RETENTION_PRICING', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected Error.Code 'InvalidParameterException' (400).

Cause 2 — cloudtrail.put-insight-selectors-insufficient-s3-policy-fails, verdict 'mismatch': AssertionError: call unexpectedly succeeded where a rejection was required: {'TrailARN': 'arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-behav-trail-7922f9254330', 'InsightSelectors': [{'InsightType': 'ApiCallRateInsight', 'EventCategories': ['Management']}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected Error.Code 'InsufficientS3BucketPolicyException' (403) after s3 DeleteBucketPolicy on the trail's destination bucket. The follow-on assertions (GetInsightSelectors -> InsightNotEnabledException, then success once the policy is restored) were never reached.

Cause 3 — cloudtrail.start-logging-revoked-bucket-permission-denied, verdict 'mismatch': AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'Name': 'sim2real-ctstartlog-trail-ed1d6cf8ef9d', 'S3BucketName': 'sim2real-ctstartlog-7f9dfb73cd38', 'S3KeyPrefix': 'denied-update', 'IncludeGlobalServiceEvents': True, 'IsMultiRegionTrail': False, 'TrailARN': 'arn:aws:cloudtrail:us-west-2:000000000000:trail/sim2real-ctstartlog-trail-ed1d6cf8ef9d', 'LogFileValidationEnabled': False, 'IsOrganizationTrail': False, 'RecursiveLogging': True, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected Error.Code 'InsufficientS3BucketPolicyException' (403) and no change to the trail. The earlier legs of the same check passed: StartLogging succeeded for both the intact and the revoked-policy trail and GetTrailStatus reported IsLogging true for both, which is correct.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite (simparity-behavioral-ga-prod-v1); replay the checks named in trace_ids.
2. Cause 1: cloudtrail CreateEventDataStore(Name='<any-unique-name>', TerminationProtectionEnabled=false) -> expect 400 InvalidParameterException, observe 200 with an EventDataStoreArn and Status 'CREATED'.
3. Cause 2: s3 CreateBucket, put the CloudTrail AclCheck+Write bucket policy, cloudtrail CreateTrail(Name=T, S3BucketName=B), then s3 DeleteBucketPolicy(Bucket=B), then cloudtrail PutInsightSelectors(TrailName=T, InsightSelectors=[{InsightType: 'ApiCallRateInsight'}]) -> expect 403 InsufficientS3BucketPolicyException, observe 200 with the selector echoed; then GetInsightSelectors(TrailName=T) -> expect InsightNotEnabledException, observe the selector enabled.
4. Cause 3: build two trails the same way, s3 DeleteBucketPolicy on the second bucket only, cloudtrail StartLogging on both (both correctly succeed), then cloudtrail UpdateTrail(Name=T2, S3BucketName=B2, S3KeyPrefix='denied-update') -> expect 403 InsufficientS3BucketPolicyException with the trail unchanged, observe 200 and GetTrail showing S3KeyPrefix 'denied-update'. The identical UpdateTrail against the intact-policy trail must still succeed.
