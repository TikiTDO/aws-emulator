Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.kmsenvelope-sse-kms-write-rejected-when-the-cmk-lives-in-a-different-region-than-the-bucket kms__s3.sep-kms-key-scope-sse-kms-write-under-a-disabled-key-rejected-and-no-object-created kms__s3.sse-kms-nonexistent-alias-rejected-and-no-object-created kms__s3.sse-kms-alias-resolves-to-key-arn-in-head-object kms__s3.sseinv-aws-managed-s3-key-refuses-lifecycle-changes-rejected kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key
Issue-Key: kms__s3.kmsenvelope-sse-kms-write-rejected-when-the-cmk-lives-in-a-different-region-than-the-bucket|_client_error#15|AssertionError|raise AssertionError(accepted)
Issue-Key: kms__s3.sep-kms-key-scope-sse-kms-write-under-a-disabled-key-rejected-and-no-object-created|_rejected#16|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: kms__s3.sse-kms-nonexistent-alias-rejected-and-no-object-created|_rejected#5|AssertionError|raise AssertionError(f"real AWS refuses this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: kms__s3.sse-kms-alias-resolves-to-key-arn-in-head-object|run#11|AssertionError|assert put.get("SSEKMSKeyId") == key_arn, (
Issue-Key: kms__s3.sseinv-aws-managed-s3-key-refuses-lifecycle-changes-rejected|run#7|AssertionError|assert ":key/" in key_arn, f"the object names no key ARN: {key_arn!r}"
Issue-Key: kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key|run#9|AssertionError|assert rule.get("BucketKeyEnabled") is False, (
Issue-Commit: aa31aa657622ba455ff8faedd6354f5956e3380b

# S3 never resolves or validates SSEKMSKeyId against KMS: cross-region, disabled and nonexistent keys are accepted, aliases are echoed unresolved, an aws:kms default names no key, and BucketKeyEnabled is omitted (6 causes)

> _Note: this issue was identified against the code base at commit `aa31aa657622ba455ff8faedd6354f5956e3380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
Six root causes in S3's SSE-KMS path. The common thread is that S3 treats SSEKMSKeyId as an opaque string instead of resolving it through KMS, but each cause is separately observable and separately fixable.

Cause 1 of 6 — PutObject accepts an SSEKMSKeyId naming a key in a different region from the bucket.
AWS: S3 resolves the key ARN against the KMS endpoint of the BUCKET's region; a key ARN from another region is refused (HTTP 400, KMS.NotFoundException class refusal) and no object is created.
Emulator: a bucket in ap-northeast-1 accepts a us-west-2 key ARN and stores the object.
Why: a client's cross-region misconfiguration — a real and common deployment bug — is silently accepted, and the object is reported as encrypted under a key S3 could not have used.
Fix: PutObject's SSE-KMS validation must resolve SSEKMSKeyId against the bucket region's KMS.
Traces: kms__s3.kmsenvelope-sse-kms-write-rejected-when-the-cmk-lives-in-a-different-region-than-the-bucket.

Cause 2 of 6 — PutObject accepts a DISABLED CMK and creates the object.
AWS: the envelope GenerateDataKey call fails on a Disabled key, so PutObject is refused (HTTP 400, KMS.DisabledException class refusal) and no object exists afterwards.
Emulator: HTTP 200 with x-amz-server-side-encryption: aws:kms and the disabled key's ARN echoed back; the object is created.
Why: 'disable the key to cut off writes' — a standard containment action — has no effect in the emulator, so incident-response runbooks cannot be tested.
Fix: PutObject's SSE-KMS path must consult KMS key state before accepting the write.
Traces: kms__s3.sep-kms-key-scope-sse-kms-write-under-a-disabled-key-rejected-and-no-object-created.

Cause 3 of 6 — PutObject accepts an SSEKMSKeyId naming an alias that does not exist.
AWS: the alias is resolved through KMS and a nonexistent alias is refused (HTTP 400) with no object created.
Emulator: HTTP 200, and the ghost alias string is stored and echoed as the object's key id.
Why: a typo'd alias produces objects that claim encryption under a key that does not exist.
Fix: the same SSEKMSKeyId resolution step, for the alias form.
Traces: kms__s3.sse-kms-nonexistent-alias-rejected-and-no-object-created.

Cause 4 of 6 — an alias given as SSEKMSKeyId is echoed verbatim instead of resolved to the key ARN.
AWS: PutObject's own response (and HeadObject afterwards) reports SSEKMSKeyId as the RESOLVED key ARN, never the alias the caller supplied.
Emulator: the response reports 'alias/<name>'.
Why: clients that compare an object's SSEKMSKeyId against a key ARN (inventory, compliance scans, cross-account checks) mismatch on every alias-written object.
Fix: resolve the alias in the PutObject/HeadObject SSE-KMS serializer so the stored and reported value is the key ARN.
Traces: kms__s3.sse-kms-alias-resolves-to-key-arn-in-head-object.

Cause 5 of 6 — with an 'aws:kms' bucket default and no KMSMasterKeyID, written objects carry no SSEKMSKeyId at all.
AWS: S3 picks the AWS-managed aws/s3 key; HeadObject reports ServerSideEncryption 'aws:kms' and a concrete SSEKMSKeyId key ARN whose DescribeKey shows KeyManager 'AWS'.
Emulator: HeadObject returns an empty SSEKMSKeyId (''), so no key can be followed from the object. (Related: the 'kms' service issue reports that no alias/aws/* AWS-managed key catalogue exists for S3 to resolve to.)
Why: 'SSE-KMS with the AWS-managed key' is indistinguishable from unencrypted in the object's metadata, and no tooling can audit which key protects an object.
Fix: S3's default-encryption path must resolve an omitted KMSMasterKeyID to the account's aws/s3 AWS-managed key and record it on the object.
Traces: kms__s3.sseinv-aws-managed-s3-key-refuses-lifecycle-changes-rejected.

Cause 6 of 6 — GetBucketEncryption omits BucketKeyEnabled instead of reporting false.
AWS: a stored rule that did not set a bucket key reports BucketKeyEnabled: false explicitly in the ServerSideEncryptionRule.
Emulator: the rule comes back as {'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}} with the member absent.
Why: clients reading rule['BucketKeyEnabled'] get a KeyError/None and cannot tell 'false' from 'unknown'; the member is a documented part of the rule representation.
Fix: the GetBucketEncryption serializer must emit BucketKeyEnabled with its default.
Traces: kms__s3.sseinv-kms-default-without-key-id-resolves-to-the-aws-managed-s3-key.

## Evidence
Cause 1: "AssertionError: PutObject accepted SSEKMSKeyId 'arn:aws:kms:us-west-2:000000000000:key/af7f1e3e-fb28-44e4-8c94-5df9ae8558e6' — a key in 'us-west-2' — for a bucket in 'ap-northeast-1'; S3 must resolve the ARN against its own region's KMS".
Cause 2: "AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ... 'x-amz-server-side-encryption': 'aws:kms', 'x-amz-server-side-encryption-aws-kms-key-id': 'arn:aws:kms:us-west-2:000000000000:key/28f2c9d2-b985-4cb8-bc9b-e844c03c2a9c'}, 'ServerSideEncryption': 'aws:kms', 'SSEKMSKeyId': '...'}" — expected HTTP 400 and no object.
Cause 3: "AssertionError: real AWS refuses this call, but it SUCCEEDED and returned {... 'x-amz-server-side-encryption-aws-kms-key-id': 'alias/sim2real-behav-ssekms-ghost-67ec7e040dd0', 'SSEKMSKeyId': 'alias/sim2real-behav-ssekms-ghost-67ec7e040dd0'}" — expected HTTP 400 and no object.
Cause 4: "AssertionError: PutObject's own response must report the ALIAS RESOLVED TO ITS KEY ARN, got 'alias/sim2real-behav-ssekms-34ca2547596e', expected 'arn:aws:kms:us-west-2:000000000000:key/84a54b9e-26dc-4eb2-aa81-e61bfc2ad2bc'".
Cause 5: "AssertionError: the object names no key ARN: ''" (head_object(...).get('SSEKMSKeyId') after PutObject into a bucket whose default rule is aws:kms with no key id).
Cause 6: "AssertionError: BucketKeyEnabled was omitted and must default to False, got {'ApplyServerSideEncryptionByDefault': {'SSEAlgorithm': 'aws:kms'}}".

## Steps to Reproduce
1. CreateKey in region X; CreateBucket in region Y; PutObject(ServerSideEncryption='aws:kms', SSEKMSKeyId=<X key arn>) — observe 200. 2. CreateKey, DisableKey, PutObject with it — observe 200 and an object created. 3. PutObject with SSEKMSKeyId='alias/<name that does not exist>' — observe 200. 4. CreateAlias for a real key, PutObject with SSEKMSKeyId='alias/<name>', inspect the PutObject response and HeadObject — observe the alias, not the key ARN. 5. PutBucketEncryption with ApplyServerSideEncryptionByDefault.SSEAlgorithm='aws:kms' and no KMSMasterKeyID; PutObject; HeadObject — observe SSEKMSKeyId ''. 6. GetBucketEncryption on that bucket — observe the rule with no BucketKeyEnabled member.
