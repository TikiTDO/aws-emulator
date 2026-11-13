Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3control.account-public-access-block-blocks-public-bucket-policy-rejected s3control.access-point-list-scopes-to-requested-bucket s3control.batch-job-describe-and-tagging-bogus-id-rejected s3control.access-grants-operations-require-instance-rejected
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control.account-public-access-block-blocks-public-bucket-policy-rejected|_error#17|AssertionError|raise AssertionError("call unexpectedly succeeded; an error was expected")
Issue-Key: s3-control|GetAccessPoint|NoSuchAccessPoint
Issue-Key: s3control.batch-job-describe-and-tagging-bogus-id-rejected|run#9|AssertionError|assert code_msg == code_want, (
Issue-Key: s3control.access-grants-operations-require-instance-rejected|run#5|AssertionError|assert code_msg == code_want, (
Issue-Commit: 1302d977b6b888f4927f8769f8d72a08ded5722c

# s3control: account-level BlockPublicPolicy not enforced on PutBucketPolicy, and three refusal messages interpolate the identifier instead of AWS's fixed text (4 causes)

> _Note: this issue was identified against the code base at commit `1302d977b6b888f4927f8769f8d72a08ded5722c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketPolicy, GetAccessPoint, DescribeJob/UpdateJobPriority/UpdateJobStatus, GetAccessGrantsInstance/ListAccessGrants/ListAccessGrantsLocations/ListCallerAccessGrants

## Description
Four independent root causes in the s3control emulator (and the S3 authorizer it feeds), all grounded in failing behavioral checks of suite simparity-behavioral-ga-prod-v1. Each must be closed before this issue is submitted. Checks the suite marked ALREADY FILED (CreateAccessGrantsLocation, UpdateJobPriority, GetJobTagging, DeleteStorageLensConfigurationTagging, UpdateStorageLensGroup stubs) and those marked OWNED by 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md / 20261024T005340Z-s3control-public-access-block-not-enforced-getaccesspoint-getaccesspointpolicystatus.md are deliberately excluded.

Cause 1 of 4: the ACCOUNT-level PublicAccessBlock is not evaluated when a public BUCKET policy is written, so the put is accepted where AWS refuses it.
AWS: recorded behavior only for the error (Smithy PutBucketPolicy, PUT /{Bucket}?policy, success 200, binds no errors; s3control PutPublicAccessBlock records the account-level PublicAccessBlockConfiguration). Recorded: with the bucket's own PublicAccessBlock deleted and the account-level configuration set to all four flags true, s3 PutBucketPolicy of a document whose Statement is Allow / Principal "*" / s3:GetObject on arn:aws:s3:::<bucket>/* is REFUSED with error code AccessDenied, HTTP 403. The message for this refusal is built from the request, so the rule is the requirement: "User: <caller-arn> is not authorized to perform: s3:PutBucketPolicy on resource: \"arn:aws:s3:::<bucket>\" because public policies are prevented by the BlockPublicPolicy setting in S3 Block Public Access." Nothing is stored (GetBucketPolicy then answers NoSuchBucketPolicy, example message "The bucket policy does not exist"), a SCOPED policy (Principal arn:aws:iam::<account>:root) is still accepted and classified IsPublic=false, and after s3control DeletePublicAccessBlock(AccountId) the identical public document becomes acceptable again — the block is a live evaluation of the document, not a sticky flag on the operation.
Emulator: the public PutBucketPolicy SUCCEEDS under the all-true account block ("call unexpectedly succeeded; an error was expected"); the account PAB record was confirmed served first (GetPublicAccessBlock reported BlockPublicPolicy=true), and the baseline arm (same document accepted with no account block, IsPublic=true) passed, so the account record exists but is never consulted by the bucket-policy write path.
Why: a client validating its public-access guardrails is told a wide-open bucket policy was accepted where AWS refuses it — a security-semantics divergence, and the account-level block's precedence over per-bucket configuration is unobservable.
Fix: evaluate the s3control account-level PublicAccessBlock record in the S3 PutBucketPolicy handler's validation (public-policy detection) before the policy is stored. This is the same missing enforcement as Cause 2 of open issue 20261024T005340Z-s3control-public-access-block-not-enforced-getaccesspoint-getaccesspointpolicystatus.md, but on a different target operation (PutBucketPolicy vs PutAccessPointPolicy) and a check that issue does not own; closing that one does not necessarily close this path.
Traces: s3control.account-public-access-block-blocks-public-bucket-policy-rejected.

Cause 2 of 4: GetAccessPoint's NoSuchAccessPoint refusal rewrites AWS's fixed message and appends the requested name.
AWS: Smithy GetAccessPoint (GET /v20180820/accesspoint/{Name}, 200) binds no errors; recorded behavior: after DeleteAccessPoint, GetAccessPoint for that name is refused with error code NoSuchAccessPoint and the service's FIXED message "The specified accesspoint does not exist" — one word, no identifier interpolated (the same constant that other s3control access-point lookups return).
Emulator: code NoSuchAccessPoint is correct, but the message is "The specified access point does not exist: <name>" — two words and the requested name appended.
Why: the message text is a constant of the service for this condition; a client (or a compatibility test) matching s3control's not-found text never matches, and the appended name makes the message request-dependent where AWS's is not.
Fix: the GetAccessPoint handler's not-found error construction (and the shared access-point not-found message constant), in the s3control error mapping.
Traces: s3control.access-point-list-scopes-to-requested-bucket (every listing/partition assertion before it passed; only this message diverged).

Cause 3 of 4: the job surface's "job not found" refusal appends the job id to AWS's fixed message.
AWS: Smithy DescribeJob (GET /v20180820/jobs/{JobId}) binds BadRequestException/NotFoundException/TooManyRequestsException (client, 400) and InternalServiceException (500); recorded behavior: a well-formed but unknown job id is refused uniformly across DescribeJob, UpdateJobPriority, UpdateJobStatus and GetJobTagging with error code InvalidRequest, HTTP 400, and the FIXED message "Job not found" — no id interpolated.
Emulator: code InvalidRequest is now correct (the NotImplemented half recorded as Cause 5 of open issue 20261022T213144Z-s3control-access-points-directory-bucket-listing-job-object.md no longer reproduces; this failure has a different replay key and was not marked owned), but the message is "Job not found: 00000000-0000-4000-8000-000000000000".
Why: the refusal is the service's constant for this condition; interpolating the id makes it request-dependent and breaks clients/tests that match the text, and the check never reaches its positive control (a real job answering all four operations).
Fix: the job-lookup not-found error construction shared by the DescribeJob / UpdateJobPriority / UpdateJobStatus / GetJobTagging handlers.
Traces: s3control.batch-job-describe-and-tagging-bogus-id-rejected.

Cause 4 of 4: the Access Grants no-instance refusal appends account and region to AWS's fixed message.
AWS: Smithy GetAccessGrantsInstance (GET /v20180820/accessgrantsinstance), ListAccessGrants, ListAccessGrantsLocations and ListCallerAccessGrants bind no errors; recorded behavior: in an account with no Access Grants instance all four reads are refused with error code AccessGrantsInstanceNotExistsError and the FIXED message "Access Grants Instance does not exist" (and DissociateAccessGrantsIdentityCenter with InvalidRequest / "The specified Access Grants Instance does not exist").
Emulator: code AccessGrantsInstanceNotExistsError is correct, but the message is "Access Grants Instance does not exist: account <account-id> has no S3 Access Grants instance in <region>." — the service's constant replaced by a sentence built from the request.
Why: same class as Causes 2 and 3 — a message AWS holds constant for a condition becomes request-dependent, so text-matching clients and the suite diverge; the check also stops before its positive control (CreateAccessGrantsInstance flipping all four reads to answering, the singleton duplicate-create refusal).
Fix: the shared Access-Grants precondition error construction used by the instance/grants/locations/caller-grants handlers.
Traces: s3control.access-grants-operations-require-instance-rejected.

## Evidence
Suite simparity-behavioral-ga-prod-v1 — s3control 3/33 passed (9%); s3__s3control 0/1; lambda__s3__s3control 0/1.

Cause 1: AssertionError: call unexpectedly succeeded; an error was expected — check_s3control_account_public_access_block_blocks_public_bucket_policy_rejected.py:71, at code, code_msg = _error(lambda: s3.put_bucket_policy(Bucket=bucket, Policy=public)). Immediately before, the check asserted GetPublicAccessBlock served BlockPublicPolicy=true (it did), the bucket's own PAB was deleted, and the identical document had been accepted and classified IsPublic=true with no account block. Expected: AccessDenied / HTTP 403, with GetBucketPolicy then answering NoSuchBucketPolicy.

Cause 2: AssertionError: GetAccessPoint on the deleted access point carried message 'The specified access point does not exist: sim2real-behav-apa2-106bb7142328' — check_s3control_access_point_list_scopes_to_requested_bucket.py:81. Code asserted one line earlier was NoSuchAccessPoint and passed; expected message 'The specified accesspoint does not exist'.

Cause 3: AssertionError: the refusal carried message 'Job not found: 00000000-0000-4000-8000-000000000000', expected 'Job not found' — check_s3control_batch_job_describe_and_tagging_bogus_id_rejected.py:51. The code assertion (InvalidRequest) passed.

Cause 4: AssertionError: the refusal carried message 'Access Grants Instance does not exist: account 000000000000 has no S3 Access Grants instance in us-west-2.', expected 'Access Grants Instance does not exist' — check_s3control_access_grants_operations_require_instance_rejected.py:56. The code assertion (AccessGrantsInstanceNotExistsError) passed.

## Steps to Reproduce
Replay the behavioral suite against the emulator and re-run exactly the checks in trace_ids.

1. Cause 1 — create a bucket; s3 DeletePublicAccessBlock(Bucket); PutBucketPolicy with Allow/Principal "*"/s3:GetObject on arn:aws:s3:::<bucket>/* (accepted, GetBucketPolicyStatus IsPublic=true); DeleteBucketPolicy; s3control PutPublicAccessBlock(AccountId, all four flags true) and poll GetPublicAccessBlock until served; re-issue the identical PutBucketPolicy — expect AccessDenied / HTTP 403 and GetBucketPolicy -> NoSuchBucketPolicy. Then put an account-scoped policy (expect accepted, IsPublic=false), s3control DeletePublicAccessBlock(AccountId), and the public document must be accepted again.
2. Cause 2 — CreateAccessPoint, DeleteAccessPoint, then GetAccessPoint(AccountId, Name) — expect code NoSuchAccessPoint with message exactly "The specified accesspoint does not exist".
3. Cause 3 — DescribeJob / UpdateJobPriority / UpdateJobStatus / GetJobTagging with JobId="00000000-0000-4000-8000-000000000000" — expect code InvalidRequest, HTTP 400, message exactly "Job not found".
4. Cause 4 — in an account with no Access Grants instance call GetAccessGrantsInstance, ListAccessGrants, ListAccessGrantsLocations and ListCallerAccessGrants — expect code AccessGrantsInstanceNotExistsError with message exactly "Access Grants Instance does not exist"; DissociateAccessGrantsIdentityCenter — expect InvalidRequest / "The specified Access Grants Instance does not exist".
