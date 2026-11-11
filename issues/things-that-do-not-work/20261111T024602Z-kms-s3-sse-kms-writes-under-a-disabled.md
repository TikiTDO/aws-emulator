Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.sep-kms-key-scope-sse-kms-write-under-a-disabled-key-rejected-and-no-object-created kms__s3.sseinv-aws-managed-s3-key-refuses-lifecycle-changes-rejected
Issue-Key: kms__s3.sep-kms-key-scope-sse-kms-write-under-a-disabled-key-rejected-and-no-object-created|run#17|AssertionError|assert put_code == "KMS.DisabledException", (
Issue-Key: kms__s3.sseinv-aws-managed-s3-key-refuses-lifecycle-changes-rejected|_error#13|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: 6a777fbd6800e714176d50f5d7d416617b079449

# kms__s3: SSE-KMS writes under a disabled key are refused as KMS.KMSInvalidStateException instead of KMS.DisabledException, and KMS lets the account disable/schedule deletion of an AWS-managed key (2 causes)

> _Note: this issue was identified against the code base at commit `6a777fbd6800e714176d50f5d7d416617b079449`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Two net-new root causes, both grounded in kms__s3 checks.

Cause 1 of 2: an S3 SSE-KMS write whose key is Disabled is refused with the wrong KMS error code.
AWS: S3 surfaces the KMS failure of the GenerateDataKey it performs for the write by prefixing the KMS error code with "KMS." For a key whose KeyState is Disabled, KMS answers DisabledException (Smithy: kms GenerateDataKey declares DisabledException, errorKind client, HTTP 409, awsQueryError code "Disabled" — separately from KMSInvalidStateException, which is for a key in a state such as PendingDeletion/Unavailable), so PutObject and CopyObject both fail with error code "KMS.DisabledException" and the message "<key ARN> is disabled." — fixed text " is disabled." after the key ARN taken from the request — and nothing is stored (HeadObject on the refused keys answers 404 and ListObjectsV2 shows no object). Recorded behavior: the same PutObject under an Enabled key B succeeds, and after EnableKey on A the same PutObject succeeds and reads back with SSEKMSKeyId == A.
Emulator: the write is correctly refused but with error code 'KMS.KMSInvalidStateException' instead of 'KMS.DisabledException' (so the message assertion on "<key ARN> is disabled." is never even reached).
Why: a client that branches on the KMS error class — DisabledException means "re-enable the key and retry", KMSInvalidStateException means the key is in an unusable lifecycle state such as pending deletion — takes the wrong remediation, and retry/alerting rules keyed on the code never match.
Fix: the KMS error classification used by the S3 SSE-KMS write path (PutObject/CopyObject data-key generation): a Disabled key must map to DisabledException, with KMSInvalidStateException reserved for the other invalid states, and S3's "KMS."-prefixed code and the message must be carried through unchanged.
Failing check: kms__s3.sep-kms-key-scope-sse-kms-write-under-a-disabled-key-rejected-and-no-object-created.

Cause 2 of 2: KMS lifecycle operations on an AWS-managed key are accepted instead of denied.
AWS: an AWS-managed key (KeyManager=AWS, e.g. the aws/s3 key behind an SSE-KMS bucket default) is readable by the account — DescribeKey and GetKeyRotationStatus succeed — but its lifecycle is not the caller's to change: ScheduleKeyDeletion and DisableKey are refused with AccessDeniedException whose message is AWS's fixed template "User: <caller ARN> is not authorized to perform: kms:<Action> on resource: <key ARN> because no resource-based policy allows the kms:<Action> action" (the caller ARN from STS GetCallerIdentity, the action — ScheduleKeyDeletion / DisableKey — and the key ARN are the variable parts), and the key stays Enabled. Smithy declares neither call as unconditionally permitted; the refusal is the AWS-managed-key protection and is recorded behavior.
Emulator: ScheduleKeyDeletion(KeyId=<aws/s3 key ARN>, PendingWindowInDays=7) SUCCEEDS — the check's error helper raises "the call succeeded; real AWS rejects it, so there is no error code". The account can therefore schedule deletion of (and, on the same path, disable) an AWS-managed key, which would break every SSE-KMS bucket default using it.
Why: a client's guardrail tests see a destructive call on a service-managed key accepted, and local state can be driven into a configuration AWS cannot reach (an AWS-managed key pending deletion / disabled), making subsequent SSE-KMS behavior unreproducible.
Fix: KMS key-lifecycle authorization — ScheduleKeyDeletion and DisableKey must refuse AccessDeniedException for a key whose KeyManager is AWS, leaving KeyState Enabled, while DescribeKey/GetKeyRotationStatus stay open to the account.
Failing check: kms__s3.sseinv-aws-managed-s3-key-refuses-lifecycle-changes-rejected.

## Evidence
Cause 1 — kms__s3.sep-kms-key-scope-sse-kms-write-under-a-disabled-key-rejected-and-no-object-created. Key A is disabled (DescribeKey KeyState=='Disabled' asserted first) and the control PutObject under enabled key B succeeds. Then:
  s3.put_object(Bucket=<b>, Key='denied/put.bin', Body=<payload>, ServerSideEncryption='aws:kms', SSEKMSKeyId=<A>)
  Expected (AWS): ClientError code 'KMS.DisabledException', message '<A> is disabled.'; HeadObject on 'denied/put.bin' -> 404; nothing under 'denied/'.
  Actual (emulator): AssertionError: PutObject with SSE-KMS under the disabled key A must fail with KMS.DisabledException, got 'KMS.KMSInvalidStateException'
  The same expectation holds for CopyObject into SSE-KMS under A (not reached because the PutObject assertion fires first).

Cause 2 — kms__s3.sseinv-aws-managed-s3-key-refuses-lifecycle-changes-rejected. A bucket is given an aws:kms default, an object is written, and HeadObject's SSEKMSKeyId names an AWS-managed key (DescribeKey KeyManager=='AWS'); GetKeyRotationStatus succeeds (positive control). Then:
  kms.schedule_key_deletion(KeyId=<aws-managed key ARN>, PendingWindowInDays=7)
  Expected (AWS): AccessDeniedException, message 'User: <caller ARN> is not authorized to perform: kms:ScheduleKeyDeletion on resource: <key ARN> because no resource-based policy allows the kms:ScheduleKeyDeletion action'; then the same for kms:DisableKey; DescribeKey still reports KeyState Enabled.
  Actual (emulator): AssertionError: the call succeeded; real AWS rejects it, so there is no error code

## Steps to Reproduce
Cause 1:
1. CreateKey A and CreateKey B (symmetric). CreateBucket; PutObject 'src/source.bin' (default encryption).
2. DisableKey(A); confirm DescribeKey(A).KeyMetadata.KeyState == 'Disabled'.
3. Control: PutObject(Key='ok/under-b.bin', ServerSideEncryption='aws:kms', SSEKMSKeyId=B) then GetObject -> body matches, SSEKMSKeyId == B.
4. PutObject(Key='denied/put.bin', ServerSideEncryption='aws:kms', SSEKMSKeyId=A) -> expect code 'KMS.DisabledException', message '<A> is disabled.' (emulator: 'KMS.KMSInvalidStateException').
5. CopyObject(Key='denied/copy.bin', CopySource='src/source.bin', ServerSideEncryption='aws:kms', SSEKMSKeyId=A) -> same code/message.
6. HeadObject both 'denied/*' keys -> 404; ListObjectsV2(Prefix='denied/') -> empty. EnableKey(A); step 4 now succeeds and GetObject reports SSEKMSKeyId == A.

Cause 2:
1. CreateBucket; PutBucketEncryption with SSEAlgorithm 'aws:kms' (no KMSMasterKeyID, i.e. the AWS-managed aws/s3 key); PutObject 'o.txt'.
2. HeadObject 'o.txt' -> SSEKMSKeyId is an AWS-managed key ARN; DescribeKey on it -> KeyManager 'AWS'; GetKeyRotationStatus succeeds.
3. ScheduleKeyDeletion(KeyId=<that ARN>, PendingWindowInDays=7) -> expect AccessDeniedException with the kms:ScheduleKeyDeletion denial message (emulator: succeeds).
4. DisableKey(KeyId=<that ARN>) -> expect AccessDeniedException with the kms:DisableKey denial message; DescribeKey still Enabled.
