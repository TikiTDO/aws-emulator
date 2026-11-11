Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.sse-kms-nonexistent-alias-rejected-and-no-object-created kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created
Issue-Key: kms__s3.sse-kms-nonexistent-alias-rejected-and-no-object-created|run#18|AssertionError|assert put.get("SSEKMSKeyId") == key_arn, (
Issue-Key: kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created|run#16|AssertionError|assert created.get("SSEKMSKeyId") == key["Arn"], (
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# kms__s3: SSE-KMS writes echo the request's alias verbatim in SSEKMSKeyId instead of resolving it to the KMS key ARN (PutObject and CreateMultipartUpload)

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
One net-new root cause in the kms__s3 suite, reached by two checks. It is NOT covered by the open issue 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md, which carries other checks' replay keys (HeadObject alias reporting and the write-under-a-disabled-key refusal) and does not carry these two; name that issue when merging.

Cause 1 of 1: an SSE-KMS write whose x-amz-server-side-encryption-aws-kms-key-id names a KMS ALIAS is answered with that alias string in SSEKMSKeyId, not with the ARN of the key the alias targets.
AWS: Smithy: PutObjectRequest.SSEKMSKeyId (httpHeader x-amz-server-side-encryption-aws-kms-key-id) is documented "Specifies the KMS key ID (Key ID, Key ARN, or Key Alias) to use for object encryption" — so an alias is a legal input form. PutObjectOutput.SSEKMSKeyId is documented "If present, indicates the ID of the KMS key that was used for object encryption." S3 resolves whichever input form it was given against KMS and reports back the full ARN of the key that actually encrypted the object, i.e. arn:aws:kms:<region>:<account>:key/<key-id>, never the alias it was asked with. CreateMultipartUpload carries the identical SSEKMSKeyId member and the identical rule, so the initiated upload also reports the resolved key ARN. Naming an alias that does not exist is refused before anything is stored with the KMS-originated NotFoundException relayed by S3.
Emulator: the alias string is stored and echoed verbatim. PutObject answers SSEKMSKeyId = 'alias/<alias-name>' where the key's ARN was required, and CreateMultipartUpload answers header x-amz-server-side-encryption-aws-kms-key-id: 'alias/<alias-name>' (ServerSideEncryption 'aws:kms', HTTP 200). The alias is never resolved against KMS at all, so both the positive control and the ghost-alias refusal leg rest on an unresolved string.
Why: a client reading back which key protected an object gets an alias, which is a mutable pointer, not the key identity — key-inventory, compliance-reporting and cross-account decrypt paths that compare SSEKMSKeyId to a key ARN all mismatch, and an alias that is later retargeted silently changes what the recorded value appears to mean. Because the alias is never resolved, a write naming a nonexistent alias is not refused on the KMS lookup either, so an object can be reported as SSE-KMS encrypted under a key that does not exist.
Fix: the SSE-KMS key-selection step shared by the PutObject and CreateMultipartUpload handlers — resolve the supplied Key ID / Key ARN / Key Alias against KMS before the write, store the resolved key ARN on the object, report that ARN in SSEKMSKeyId (and in the response header), and relay KMS's NotFoundException when the alias or key does not resolve.
Traces: kms__s3.sse-kms-nonexistent-alias-rejected-and-no-object-created (PutObject), kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created (CreateMultipartUpload).

## Evidence
Check kms__s3.sse-kms-nonexistent-alias-rejected-and-no-object-created, verdict [mismatch]:
  assert put.get("SSEKMSKeyId") == key_arn
  AssertionError: positive control: PutObject under a REAL alias must resolve to the real key ARN, got 'alias/sim2real-behav-ssekms-real-<suffix>', expected 'arn:aws:kms:<region>:<account>:key/<key-id>'

Check kms__s3.sseinv-sse-kms-header-naming-a-missing-alias-rejected-and-no-object-created, verdict [mismatch]:
  assert created.get("SSEKMSKeyId") == key["Arn"]
  AssertionError: the control CreateMultipartUpload reported HTTP 200 with headers including
    'x-amz-server-side-encryption': 'aws:kms',
    'x-amz-server-side-encryption-aws-kms-key-id': 'alias/sim2real-sseinv-<suffix>'
  and body fields ServerSideEncryption 'aws:kms', SSEKMSKeyId 'alias/sim2real-sseinv-<suffix>'.

Expected in both: SSEKMSKeyId equal to the ARN of the key the alias targets (arn:aws:kms:<region>:<account>:key/<uuid>). Actual: the alias string from the request, echoed unchanged. The concrete alias names, key ids, bucket names, request ids and host ids above are per-run values that fill the template; the requirement is the FORM — a key ARN, resolved from whichever of Key ID / Key ARN / Key Alias the request supplied.

## Steps to Reproduce
1. kms.create_key(...) and kms.create_alias(AliasName="alias/<name>", TargetKeyId=<key id>).
2. s3.create_bucket(...), then s3.put_object(Bucket=..., Key=..., Body=..., ServerSideEncryption="aws:kms", SSEKMSKeyId="alias/<name>") and read SSEKMSKeyId from the response: AWS reports the key ARN, the emulator reports "alias/<name>".
3. s3.create_multipart_upload(Bucket=..., Key=..., ServerSideEncryption="aws:kms", SSEKMSKeyId="alias/<name>") and read SSEKMSKeyId / the x-amz-server-side-encryption-aws-kms-key-id response header: same divergence.
4. Repeat both with SSEKMSKeyId="alias/<name>-missing": AWS refuses with KMS's NotFoundException and stores nothing; the emulator accepts the unresolved alias.
5. Re-replay the two kms__s3 checks in trace_ids to confirm the fix.
