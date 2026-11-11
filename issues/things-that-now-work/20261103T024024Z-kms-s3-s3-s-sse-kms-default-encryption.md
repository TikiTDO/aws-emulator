Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.sseinv-default-change-applies-only-to-objects-written-after-it kms__s3.sseinv-explicit-sse-kms-header-overrides-an-sse-s3-bucket-default kms__s3.sseinv-dsse-kms-default-reports-its-own-algorithm-not-aws-kms kms__s3.sseinv-prod-bucket-estate-round-trips-versioning-bpa-tls-policy-and-kms-default kms__s3.sseinv-bucket-key-flag-surfaces-on-objects-only-when-enabled kms__s3.sseinv-customer-key-reference-forms-round-trip-verbatim-and-objects-carry-the-arn kms__s3.sseinv-nonexistent-default-key-accepted-at-config-then-put-object-rejected kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created kms__s3.sseinv-default-key-pending-deletion-put-object-rejected kms__s3.sseinv-e2e-report-unencrypted-s-buckets-lifecycle
Issue-Key: kms__s3.sseinv-default-change-applies-only-to-objects-written-after-it|run#19|AssertionError|assert ":key/" in key_arn, f"kms.txt names no key ARN: {kms_obj!r}"
Issue-Key: kms__s3.sseinv-explicit-sse-kms-header-overrides-an-sse-s3-bucket-default|run#13|AssertionError|assert headed.get("SSEKMSKeyId") == aws_s3, (
Issue-Key: kms__s3.sseinv-dsse-kms-default-reports-its-own-algorithm-not-aws-kms|run#13|AssertionError|assert head.get("SSEKMSKeyId") == aws_s3, (
Issue-Key: kms__s3.sseinv-prod-bucket-estate-round-trips-versioning-bpa-tls-policy-and-kms-default|run#32|KeyError|manager = kms.describe_key(KeyId=head["SSEKMSKeyId"])["KeyMetadata"]["KeyManager"]
Issue-Key: kms__s3.sseinv-bucket-key-flag-surfaces-on-objects-only-when-enabled|run#9|AssertionError|assert "BucketKeyEnabled" not in off, (
Issue-Key: kms__s3.sseinv-customer-key-reference-forms-round-trip-verbatim-and-objects-carry-the-arn|run#10|AssertionError|assert head.get("SSEKMSKeyId") == key["Arn"], (
Issue-Key: kms__s3.sseinv-nonexistent-default-key-accepted-at-config-then-put-object-rejected|_rejected#7|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created|_error_code#10|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it, got {result!r}")
Issue-Key: kms__s3.sseinv-default-key-pending-deletion-put-object-rejected|run#17|Timeout|time.sleep(3)
Issue-Key: kms__s3.sseinv-e2e-report-unencrypted-s-buckets-lifecycle|phase#4|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: dc7d1cb059ddc05800728693f0165ec8a2a1b13e

# kms__s3: S3's SSE-KMS default-encryption path never reaches KMS — the aws/s3 managed key is not resolved, a bare key id is not expanded to an ARN, BucketKeyEnabled is over-reported, missing aliases and a pending-deletion key are accepted, and a CFN bucket's aws:kms default is ignored (7 causes)

> _Note: this issue was identified against the code base at commit `dc7d1cb059ddc05800728693f0165ec8a2a1b13e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
SEVEN distinct net-new root causes in S3's server-side-encryption inventory surface, grounded in ten failing behavioral checks of this suite that the replay report marked neither ALREADY FILED nor OWNED. A fixer must close all seven before submitting. Smithy model consulted at services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json.

Related OPEN issues that do NOT carry these checks' replay keys (named for context only, none of them covers a cause below): 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md (explicit SSEKMSKeyId cross-region/disabled/ghost-alias on PutObject, alias->ARN on PutObject/HeadObject, and GetBucketEncryption's BucketKeyEnabled default — the causes below are the bucket-DEFAULT rule path, the CreateMultipartUpload path, and the OBJECT-level BucketKeyEnabled projection); 20261025T181311Z-kms-s3-s3-reports-no-sse-kms-key-arn-on.md (the same aws/s3-resolution defect for one single check id); 20261025T121424Z-kms-s3-s3-getobject-ignores-the-sse-kms.md (GetObject and a DISABLED key, not PendingDeletion on write); 20261029T175759Z-kms-s3-s3-never-calls-kms-for-a.md (bucket-key encryption context and key-policy denies).

----------------------------------------------------------------
Cause 1 of 7 — any SSE-KMS write that names no key id reports NO SSEKMSKeyId at all: the account's AWS-managed aws/s3 key is never resolved.
Failing checks: kms__s3.sseinv-default-change-applies-only-to-objects-written-after-it, kms__s3.sseinv-explicit-sse-kms-header-overrides-an-sse-s3-bucket-default, kms__s3.sseinv-dsse-kms-default-reports-its-own-algorithm-not-aws-kms, kms__s3.sseinv-prod-bucket-estate-round-trips-versioning-bpa-tls-policy-and-kms-default
AWS: Smithy binds SSEKMSKeyId to x-amz-server-side-encryption-aws-kms-key-id on PutObject's input AND output and on HeadObject's output, and KMSMasterKeyID is optional on ServerSideEncryptionByDefault — omitting it is a legal configuration. When no key is named — whether by a bucket default rule {SSEAlgorithm: 'aws:kms'} with no KMSMasterKeyID, by a rule {SSEAlgorithm: 'aws:kms:dsse'} with no KMSMasterKeyID, or by a per-request x-amz-server-side-encryption: aws:kms header with no key-id header — S3 encrypts under the account's AWS-managed key behind alias/aws/s3 and REPORTS that key's concrete ARN. The value's fixed shape is arn:<partition>:kms:<region>:<account>:key/<key-id>; region, account and key id come from the bucket's region and the emulator's own KMS state, and kms:DescribeKey on it must report KeyManager 'AWS'. That reported ARN is the only thing distinguishing 'SSE-KMS under the AWS-managed key' from SSE-KMS under a customer key.
Emulator: ServerSideEncryption comes back 'aws:kms' (or 'aws:kms:dsse') but SSEKMSKeyId is absent from the PutObject/HeadObject response entirely — `AssertionError: kms.txt names no key ARN: {... 'ServerSideEncryption': 'aws:kms', ...}` with no SSEKMSKeyId member, `AssertionError: a header without a key id must use alias/aws/s3 ('arn:aws:kms:...:key/<id>'), got None`, `AssertionError: a DSSE default without a key id uses alias/aws/s3 (...), got None`, and `KeyError: 'SSEKMSKeyId'` in the prod-estate check.
Why: a client auditing encryption posture cannot tell which key protects an object, and any code that feeds SSEKMSKeyId into kms:DescribeKey / a key-policy or grant check — the normal way to prove an object is under a managed vs. customer key — gets nothing and either crashes (as the prod-estate check does) or silently passes. Replication, CopyObject re-encryption and cross-account reads all key off this value.
Fix: the SSE-KMS resolution step shared by the PutObject / CreateMultipartUpload handlers and the HeadObject/GetObject serializer in the s3 crate — when the effective encryption selects aws:kms or aws:kms:dsse with no key reference, resolve alias/aws/s3 in KMS (materialising that AWS-managed key on first use, KeyManager AWS), store its ARN on the object, and emit it in x-amz-server-side-encryption-aws-kms-key-id on every read and write response.

----------------------------------------------------------------
Cause 2 of 7 — HeadObject reports BucketKeyEnabled: false on an object written while the bucket key was OFF; AWS omits the member entirely.
Failing check: kms__s3.sseinv-bucket-key-flag-surfaces-on-objects-only-when-enabled
AWS: Smithy binds HeadObject$BucketKeyEnabled (boolean) to the response header x-amz-server-side-encryption-bucket-key-enabled with no @default. The header is emitted ONLY when the object was written with an S3 Bucket Key in force; an object written with the bucket key off carries no such header, so the SDK's response has no BucketKeyEnabled member at all. (This is the OBJECT-level projection, and it is the opposite of the bucket-level rule, where GetBucketEncryption does report BucketKeyEnabled: false explicitly.)
Emulator: the header is emitted as false for the bucket-key-off object — `AssertionError: an object written with the bucket key off must omit BucketKeyEnabled, got False`.
Why: 'member absent' and 'member false' are different facts on an object, and clients use the presence of the header to decide whether an object benefits from bucket-key request-rate reduction; emitting it unconditionally makes every object look as if S3 made a bucket-key decision for it and breaks diff-based inventory tooling that compares HeadObject projections.
Fix: the HeadObject/GetObject SSE serializer — emit x-amz-server-side-encryption-bucket-key-enabled only when the object's stored bucket-key flag is true.

----------------------------------------------------------------
Cause 3 of 7 — an object written under a default rule whose KMSMasterKeyID is a BARE KEY ID reports the bare id instead of the resolved key ARN.
Failing check: kms__s3.sseinv-customer-key-reference-forms-round-trip-verbatim-and-objects-carry-the-arn
AWS: a default-encryption rule stores whatever reference form it was given verbatim (id, ARN or alias — GetBucketEncryption echoes it unchanged, and that leg passes), but every OBJECT written under any of the three spellings resolves through KMS to the SAME value: the key's full ARN, of the fixed shape arn:<partition>:kms:<region>:<account>:key/<key-id>, reported in x-amz-server-side-encryption-aws-kms-key-id. The bucket-level echo and the object-level resolution are deliberately different.
Emulator: an object written under the bare-id form reports the bare id — `AssertionError: an object written under the id form must name the key ARN 'arn:aws:kms:us-west-2:000000000000:key/<uuid>', got '<uuid>'`.
Why: clients compare an object's SSEKMSKeyId against a key ARN (inventory, compliance scans, cross-account checks) and mismatch on every object written under the id form, and a bare uuid cannot be resolved without already knowing the region and account.
Fix: the same SSE-KMS resolution step as Cause 1 — resolve the effective key reference (bare id, ARN or alias) through KMS and store/report its ARN, keeping the bucket rule's stored text untouched.

----------------------------------------------------------------
Cause 4 of 7 — a write under a default rule naming an alias that does NOT exist is accepted with HTTP 200 and the ghost alias echoed as the object's key id.
Failing check: kms__s3.sseinv-nonexistent-default-key-accepted-at-config-then-put-object-rejected
AWS: PutBucketEncryption accepts a rule naming an alias that does not exist (that leg passes — AWS validates nothing at config time), but every subsequent write is refused at the envelope call with the relayed KMS code KMS.NotFoundException and a message whose fixed text is "Alias <alias ARN> is not found." where <alias ARN> is arn:aws:kms:<region>:<account>:alias/<name> built from the bucket's region, the account and the alias the rule names. No object is created. Re-pointing the same bucket at the AWS-managed key makes the identical write succeed.
Emulator: HTTP 200 — `AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {... 'x-amz-server-side-encryption': 'aws:kms', 'x-amz-server-side-encryption-aws-kms-key-id': 'alias/<ghost>', 'SSEKMSKeyId': 'alias/<ghost>'}` — and the object exists, claiming encryption under a key that does not exist.
Why: a typo'd alias in a bucket default is a real and common deployment bug; AWS surfaces it on the first write, the emulator produces objects that claim encryption under a nonexistent key and reports an alias string where a key ARN belongs.
Fix: the PutObject handler's bucket-default SSE-KMS branch must resolve the rule's KMSMasterKeyID through KMS before storing anything and relay the KMS.NotFoundException refusal with AWS's alias-ARN message, creating no object.

----------------------------------------------------------------
Cause 5 of 7 — CreateMultipartUpload whose SSE-KMS header names a missing alias is accepted and leaves an in-progress upload behind.
Failing check: kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created
AWS: Smithy binds CreateMultipartUpload$SSEKMSKeyId to x-amz-server-side-encryption-aws-kms-key-id on input and output. The alias is resolved through KMS at initiation time, BEFORE any part can be stored: a header naming a missing alias is refused KMS.NotFoundException with the fixed text "Alias <alias ARN> is not found." (the alias ARN built from region, account and the requested alias name), and ListMultipartUploads afterwards shows no upload. The identical call under a real alias succeeds end to end.
Emulator: HTTP 200 with an UploadId — `AssertionError: the call succeeded; real AWS rejects it, got {... 'SSEKMSKeyId': 'alias/<name>-missing', 'UploadId': '<id>'}` — so a multipart upload is initiated against a key that cannot exist.
Why: multipart is how large objects are written, and AWS's contract is that the key is validated once at initiation rather than per part; an emulator that defers nothing lets a client upload gigabytes before anything fails, and leaves orphaned uploads that are billed and must be aborted.
Fix: the CreateMultipartUpload handler must run the same SSE-KMS resolution as PutObject before the upload is registered, relaying KMS.NotFoundException and registering no upload.

----------------------------------------------------------------
Cause 6 of 7 — writes under a bucket default whose customer key is PendingDeletion keep succeeding indefinitely.
Failing check: kms__s3.sseinv-default-key-pending-deletion-put-object-rejected
AWS: once ScheduleKeyDeletion moves the default's customer key to KeyState PendingDeletion, the envelope GenerateDataKey behind every write fails, so PutObject is refused with the relayed code KMS.KMSInvalidStateException whose message is the fixed template "<key ARN> is pending deletion." — the key's own ARN followed by that fixed sentence — and no object is created. The bucket's default-encryption rule itself is NOT changed by this and still reads back the same KMSMasterKeyID (that leg passes). The refusal is allowed to take a few seconds to propagate, so the check probes repeatedly with a bounded deadline.
Emulator: every probe write keeps returning HTTP 200 and creating an object, so the check never reaches a refusal and dies at the harness cap — `Error: Check timed out after 60.0s`. The key is confirmed PendingDeletion by DescribeKey in the same check, so S3's write path simply does not consult the key's state.
Why: scheduling a key for deletion is the standard way to make data under it unwritable, and a security runbook that depends on it cannot be rehearsed; worse, objects keep being created claiming encryption under a key that is on its way out.
Fix: the PutObject handler's SSE-KMS envelope path must consult the resolved key's KeyState before accepting the write and relay KMS.KMSInvalidStateException with AWS's "<key ARN> is pending deletion." message, creating no object.

----------------------------------------------------------------
Cause 7 of 7 — a bucket deployed by CloudFormation with an aws:kms BucketEncryption default encrypts writes as AES256.
Failing check: kms__s3.sseinv-e2e-report-unencrypted-s-buckets-lifecycle
AWS: a bucket whose stack template declares an SSE-KMS BucketEncryption default behaves exactly like one configured through PutBucketEncryption — a header-less PutObject into it reports ServerSideEncryption 'aws:kms' and an SSEKMSKeyId whose DescribeKey shows KeyManager 'AWS' (for a default naming no key), and the GetBucketEncryption partition over the stack's estate separates the SSE-KMS buckets from the SSE-S3 ones.
Emulator: the seeded object into the stack's KMS bucket comes back with x-amz-server-side-encryption: AES256 — `AssertionError: [phase 2/5 setup] the seed reports {... 'x-amz-server-side-encryption': 'AES256', ... 'ServerSideEncryption': 'AES256', ...}` — so the deployed bucket's aws:kms default was never applied to the write (and consequently no SSEKMSKeyId is reported either). The bucket names and versioning the same stack produced are correct, so the stack itself deployed.
Why: the single most common way a bucket's encryption default reaches production is a stack template; a client that verifies 'everything my stack writes is SSE-KMS' is told AES256, i.e. the emulator reports an estate as unencrypted-by-S3-managed-keys when the template asked for KMS, which is the exact inversion of the audit the scenario performs.
Fix: the CloudFormation AWS::S3::Bucket provisioning path must install the template's BucketEncryption rule into the same bucket-default state PutBucketEncryption writes, so the PutObject default-encryption path picks it up (and, with Cause 1 fixed, reports the aws/s3 key ARN).

Traces: the ten check ids in trace_ids, grouped per cause above.

## Evidence
Replay report (suite simparity-behavioral-aws-v1, 242/378 passed), service kms__s3 5/25 (20%), 20 failed. The ten failures below were marked neither ALREADY FILED nor OWNED.

Cause 1 — four checks, one missing resolution:
  [mismatch] kms__s3.sseinv-default-change-applies-only-to-objects-written-after-it, line 42: assert ":key/" in key_arn ->
    "AssertionError: kms.txt names no key ARN: {... 'x-amz-server-side-encryption': 'aws:kms', ... 'ServerSideEncryption': 'aws:kms', 'Metadata': {}, 'StorageClass': 'STANDARD'}" — no SSEKMSKeyId member and no x-amz-server-side-encryption-aws-kms-key-id header.
  [mismatch] kms__s3.sseinv-explicit-sse-kms-header-overrides-an-sse-s3-bucket-default, line 32:
    "AssertionError: a header without a key id must use alias/aws/s3 ('arn:aws:kms:us-west-2:000000000000:key/8e563d28-06d3-484d-b4c0-6c4de5e5298f'), got None"
  [mismatch] kms__s3.sseinv-dsse-kms-default-reports-its-own-algorithm-not-aws-kms, line 35:
    "AssertionError: a DSSE default without a key id uses alias/aws/s3 ('arn:aws:kms:us-west-2:000000000000:key/f103aa22-7546-4644-9569-5e605837502f'), got None" (the aws:kms:dsse rule round-trip leg passed)
  [error] kms__s3.sseinv-prod-bucket-estate-round-trips-versioning-bpa-tls-policy-and-kms-default, line 74:
    "KeyError: 'SSEKMSKeyId'" at kms.describe_key(KeyId=head["SSEKMSKeyId"]) — versioning, BPA, the TLS-only policy, the policy status and the aws:kms rule all read back correctly first.
  Expected in all four: a non-empty SSEKMSKeyId of shape arn:<partition>:kms:<region>:<account>:key/<key-id> whose DescribeKey reports KeyManager 'AWS'. The ARNs quoted are this run's values.

Cause 2 — [mismatch] kms__s3.sseinv-bucket-key-flag-surfaces-on-objects-only-when-enabled, line 28:
  "AssertionError: an object written with the bucket key off must omit BucketKeyEnabled, got False"
  The bucket-level legs passed: PutBucketEncryption with BucketKeyEnabled=False reads back BucketKeyEnabled is False on the RULE; only the object projection diverges.

Cause 3 — [mismatch] kms__s3.sseinv-customer-key-reference-forms-round-trip-verbatim-and-objects-carry-the-arn, line 40:
  "AssertionError: an object written under the id form must name the key ARN 'arn:aws:kms:us-west-2:000000000000:key/19cadb26-0045-4a20-bdc6-ccd6ba4bdc53', got '19cadb26-0045-4a20-bdc6-ccd6ba4bdc53'"
  Expected: the object's SSEKMSKeyId is the key ARN for all three reference forms (id, ARN, alias) even though the bucket rule echoes the given spelling verbatim.

Cause 4 — [mismatch] kms__s3.sseinv-nonexistent-default-key-accepted-at-config-then-put-object-rejected, line 29 -> _rejected() line 111:
  "AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {..., 'x-amz-server-side-encryption': 'aws:kms', 'x-amz-server-side-encryption-aws-kms-key-id': 'alias/sim2real-sseinv-ghost-2305b50ebe56', ...}}, 'ServerSideEncryption': 'aws:kms', 'SSEKMSKeyId': 'alias/sim2real-sseinv-ghost-2305b50ebe56'}"
  Expected: HTTP 400, code "KMS.NotFoundException", message "Alias arn:aws:kms:<region>:<account>:alias/<ghost name> is not found." and ListObjectsV2 showing no object. (The PutBucketEncryption-accepts-it and the re-point-to-aws-managed-key control legs are part of the same check.)

Cause 5 — [mismatch] kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created, line 30 -> _error_code() line 125:
  "AssertionError: the call succeeded; real AWS rejects it, got {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'x-amz-server-side-encryption-aws-kms-key-id': 'alias/sim2real-sseinv-a5e02d6022ec-missing', ...}, 'ServerSideEncryption': 'aws:kms', 'SSEKMSKeyId': 'alias/...-missing', 'Bucket': '...', 'Key': 'refused.txt', 'UploadId': 'Qx24sv0hBeMR913C3IQQ8RrxmR9mqlyVUQ77IYbdH2Q'}"
  Expected: HTTP 400, ("KMS.NotFoundException", "Alias arn:aws:kms:<region>:<account>:alias/<name>-missing is not found.") and ListMultipartUploads returning no uploads.

Cause 6 — [mismatch] kms__s3.sseinv-default-key-pending-deletion-put-object-rejected:
  "Error: Check timed out after 60.0s"
  The check sets an aws:kms default naming a customer key, writes a control object, ScheduleKeyDeletion(PendingWindowInDays=7), asserts DescribeKey reports KeyState 'PendingDeletion' and that the S3 rule still names the same key ARN (both pass), then probes PutObject every 3 s under a 180 s deadline waiting for the refusal. Every probe returns 200 and creates an object, so the loop never breaks and the harness cap fires.
  Expected: within seconds, PutObject fails with code "KMS.KMSInvalidStateException" and message "<the key's ARN> is pending deletion.", with the refused key absent from ListObjectsV2.

Cause 7 — [mismatch] kms__s3.sseinv-e2e-report-unencrypted-s-buckets-lifecycle, phase 2/5 (setup), line 135/465:
  "AssertionError: [phase 2/5 setup] the seed reports {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {..., 'x-amz-version-id': 'dde73835ad7bfc308a11398554bc9c31', 'x-amz-server-side-encryption': 'AES256', ...}}, ... 'ServerSideEncryption': 'AES256', ...}"
  The stack's four bucket-name outputs and prefix check passed in phase 1 (deploy). Expected: the header-less PutObject into the stack's KMS bucket reports ServerSideEncryption 'aws:kms' and an SSEKMSKeyId whose DescribeKey shows KeyManager 'AWS'; observed AES256 and no key id, so the downstream SSE-KMS/SSE-S3 partition in phase 3 is unreachable.

## Steps to Reproduce
Boot the emulator from this checkout and replay the ten check ids in trace_ids. Per cause:

1. (Cause 1) CreateBucket; PutBucketEncryption Rules=[{ApplyServerSideEncryptionByDefault:{SSEAlgorithm:'aws:kms'}}] (no KMSMasterKeyID); PutObject with no SSE headers; HeadObject -> observe no SSEKMSKeyId member, expect an arn:...:key/<id> whose DescribeKey reports KeyManager 'AWS'. Repeat with SSEAlgorithm 'aws:kms:dsse'. Repeat with an AES256-default bucket and a per-request header ServerSideEncryption='aws:kms' and NO SSEKMSKeyId.
2. (Cause 2) PutBucketEncryption Rules=[{ApplyServerSideEncryptionByDefault:{SSEAlgorithm:'aws:kms'}, BucketKeyEnabled:false}]; PutObject; HeadObject -> observe BucketKeyEnabled False in the response, expect the member (and the x-amz-server-side-encryption-bucket-key-enabled header) to be ABSENT. Then flip the rule to BucketKeyEnabled:true, write a second object, and expect True on that one only.
3. (Cause 3) kms.CreateKey; PutBucketEncryption with KMSMasterKeyID=<the bare KeyId>; PutObject; HeadObject -> observe the bare uuid, expect the full key ARN. Repeat with the ARN form and the alias form: the rule echoes what it was given, the object always names the ARN.
4. (Cause 4) PutBucketEncryption with KMSMasterKeyID='alias/<a name that does not exist>' (accepted, and GetBucketEncryption echoes it); PutObject -> observe HTTP 200 and an object, expect HTTP 400 KMS.NotFoundException "Alias arn:aws:kms:<region>:<account>:alias/<name> is not found." and an empty bucket. Then re-point the rule at the AWS-managed key and confirm the write lands.
5. (Cause 5) kms.CreateKey + CreateAlias for a real alias; CreateMultipartUpload(ServerSideEncryption='aws:kms', SSEKMSKeyId='<that alias>-missing') -> observe HTTP 200 and an UploadId, expect HTTP 400 KMS.NotFoundException with the alias-ARN message and ListMultipartUploads empty; the identical call under the real alias must still succeed.
6. (Cause 6) kms.CreateKey; PutBucketEncryption with KMSMasterKeyID=<its ARN>; PutObject (succeeds); kms.ScheduleKeyDeletion(PendingWindowInDays=7); DescribeKey -> KeyState 'PendingDeletion'; PutObject again, retrying for up to 180 s -> observe every attempt returning 200, expect KMS.KMSInvalidStateException with "<key ARN> is pending deletion." and no object created.
7. (Cause 7) Deploy the check's cfn-sse-inventory template (four buckets; the prod/staging pair declaring an SSE-KMS BucketEncryption default, versioning enabled); PutObject('sample.csv') into the prod bucket with no SSE headers; HeadObject -> observe ServerSideEncryption 'AES256', expect 'aws:kms' plus an SSEKMSKeyId resolving to a KeyManager 'AWS' key; then GetBucketEncryption across the four buckets and confirm the SSE-KMS/SSE-S3 partition.
