Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__s3.bucket-key-switches-the-kms-encryption-context-from-object-arn-to-bucket-arn kms__s3.kmsenvelope-deny-of-generate-data-key-blocks-the-default-encrypted-write-while-earlier-objects-stay-readable kms__s3.kmsenvelope-key-policy-deny-of-kms-decrypt-makes-the-sse-kms-object-unreadable-while-head-and-list-still-see-it
Issue-Key: kms__s3.bucket-key-switches-the-kms-encryption-context-from-object-arn-to-bucket-arn|_poll_until#36|Timeout|time.sleep(interval_s)
Issue-Key: kms__s3.kmsenvelope-deny-of-generate-data-key-blocks-the-default-encrypted-write-while-earlier-objects-stay-readable|run#28|Timeout|time.sleep(_POLL_S)
Issue-Key: kms__s3.kmsenvelope-key-policy-deny-of-kms-decrypt-makes-the-sse-kms-object-unreadable-while-head-and-list-still-see-it|run#29|Timeout|time.sleep(_POLL_S)
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# kms__s3: S3 never calls KMS for a server-side-encrypted write or read — 3 net-new causes: a bucket-key write does not switch the encryption context to the bucket ARN, a key-policy Deny of kms:GenerateDataKey does not refuse the default-encrypted PutObject, and a Deny of kms:Decrypt does not refuse GetObject

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutObject

## Description
THREE distinct net-new root causes, all of them a consequence of S3's SSE-KMS path never performing a real KMS call whose verdict can flow back into the S3 response. Each check polls for the refusal AWS produces and never sees it, so each is killed at the suite's 60 s cap; a faithful target refuses on the FIRST probe, so the timeout is the observation. Every one is grounded in a failing check the report marked neither ALREADY FILED nor OWNED. A fixer must close all three before submitting.

Related OPEN issues that do NOT carry these checks' replay keys (named for context only): 20261024T102105Z-kms-s3-s3-never-resolves-or-validates-ssekmskeyid-against-kms.md (SSEKMSKeyId resolution/validation), 20261025T121424Z-kms-s3-s3-getobject-ignores-the-sse-kms.md (GetObject ignoring the CMK's key STATE — the causes below are about the key POLICY and the encryption CONTEXT), and 20261024T140708Z-kms-s3-a-kms-viaservice-scoped-key-policy-is-not.md.

Cause 1 of 3 — a bucket-key write does not change the KMS encryption context, so a key policy scoped to the BUCKET ARN cannot gate it.
AWS: with BucketKeyEnabled=true S3 asks KMS for one data key per bucket and supplies the encryption context 'aws:s3:arn' = the BUCKET ARN; with BucketKeyEnabled=false it supplies the OBJECT ARN. A key policy whose condition requires kms:EncryptionContext:aws:s3:arn to equal the bucket ARN therefore PERMITS bucket-key writes and REFUSES non-bucket-key writes of the same bytes to the same bucket (AccessDenied, HTTP 403), and permits them again once the bucket key is re-enabled. Recorded behavior for the context values; the model carries no encryption-context semantics for S3.
Emulator: the first leg (bucket-key write permitted) passes vacuously, then the byte-identical write with BucketKeyEnabled=false is never refused — the check polls PutObject for 120 s waiting for the refusal to appear and the harness kills it at 60 s. S3 supplies no encryption context KMS can condition on, so the bucket-key/object-key distinction is unobservable.
Why: the bucket-key setting is the main SSE-KMS cost control, and the ONLY way a customer can prove it is really in effect is a key policy conditioned on the encryption context. Here the setting is a stored flag; a policy written to require per-bucket context passes in the emulator and refuses every write in production (or vice versa).
Fix: the S3 PutObject/GetObject SSE-KMS envelope path — issue the KMS GenerateDataKey/Decrypt call with the aws:s3:arn encryption context (bucket ARN when the bucket key is enabled, object ARN otherwise) and let the KMS verdict decide the S3 response.
Traces: kms__s3.bucket-key-switches-the-kms-encryption-context-from-object-arn-to-bucket-arn (60 s timeout).

Cause 2 of 3 — a key-policy Deny of kms:GenerateDataKey does not refuse a default-encrypted PutObject.
AWS: a PutObject carrying NO encryption headers into a bucket whose default encryption is aws:kms with a customer CMK makes S3 mint a data key under that CMK. Adding a key-policy statement that denies only kms:GenerateDataKey makes the next such write fail AccessDenied, HTTP 403, with KMS's explicit-deny verdict as the message — fixed text 'User: <caller ARN> is not authorized to perform: kms:GenerateDataKey on resource: <key ARN> with an explicit deny in a resource-based policy', where the caller ARN and key ARN come from the request and state. The refused write creates nothing (a later GetObject of that key is 404), while objects written earlier stay readable because kms:Decrypt was left alone.
Emulator: the write keeps succeeding — the check polls for the refusal and is killed at the 60 s cap. Default encryption is a stored setting rather than cryptography, so no GenerateDataKey call is made and no deny can apply.
Why: 'revoke the key and the writes stop' is the primary containment control for an SSE-KMS bucket. An emulator that keeps accepting writes after the key policy denies data-key minting makes every containment rehearsal pass, and the 404-after-refusal contract (no partial object) is untestable.
Fix: the S3 write path's SSE-KMS envelope — call KMS GenerateDataKey for every default-encrypted write, surface a KMS deny as S3 AccessDenied/403 with KMS's verdict message, and create no object.
Traces: kms__s3.kmsenvelope-deny-of-generate-data-key-blocks-the-default-encrypted-write-while-earlier-objects-stay-readable (60 s timeout).

Cause 3 of 3 — a key-policy Deny of kms:Decrypt does not make an SSE-KMS object unreadable.
AWS: denying kms:Decrypt on the object's CMK makes GetObject fail AccessDenied, HTTP 403, with the same explicit-deny message template ('User: <caller ARN> is not authorized to perform: kms:Decrypt on resource: <key ARN> with an explicit deny in a resource-based policy'), while the object is NOT gone: HeadObject still returns 200 with the identical ETag, ContentLength and SSEKMSKeyId and ListObjectsV2 still reports it; restoring the statement returns the exact original bytes.
Emulator: GetObject keeps returning the body — the check polls for the refusal and is killed at the 60 s cap. S3 never calls KMS on the read, so SSE-KMS is a stored header rather than cryptography.
Why: the unreadable-but-not-gone distinction is what makes KMS-based crypto-shredding and incident containment meaningful; an emulator that serves the plaintext regardless cannot grade any of it, and a client's assumption that 'denied kms:Decrypt' stops reads is validated falsely.
Fix: the S3 GetObject read path — call KMS Decrypt for the object's data key on every SSE-KMS read and map the KMS deny to AccessDenied/403 while leaving HeadObject/ListObjectsV2 unaffected.
Traces: kms__s3.kmsenvelope-key-policy-deny-of-kms-decrypt-makes-the-sse-kms-object-unreadable-while-head-and-list-still-see-it (60 s timeout).

## Evidence
All three failures are reported as "Error: Check timed out after 60.0s", and in each the surviving probe is a bounded poll for a refusal that must appear immediately.

Cause 1 — the check's own plumbing: _poll_until(lambda: _put_outcome(s3, bucket, key_off, body_off), lambda pair: pair[0] is not None, timeout_s=120, what="PutObject to start being refused after BucketKeyEnabled=false — S3 must switch to the object-ARN encryption context ..."). The preceding leg (the same bytes PUT with BucketKeyEnabled=true under a key policy that requires the bucket-ARN context) was permitted, so the key-policy condition is simply never evaluated.

Cause 2 — the check asserts code=='AccessDenied', HTTPStatusCode==403 and message=='User: <caller ARN> is not authorized to perform: kms:GenerateDataKey on resource: <key ARN> with an explicit deny in a resource-based policy', then that the object the refused write did not create is a 404. Its own comment on the reached state: "default encryption is a stored setting rather than cryptography". GetBucketEncryption correctly reports the single aws:kms rule naming the CMK, so only the KMS call is missing.

Cause 3 — the check asserts code=='AccessDenied' with the comment "S3 can only produce this by actually calling KMS on the read", HTTPStatusCode==403 and message=='User: <caller ARN> is not authorized to perform: kms:Decrypt on resource: <key ARN> with an explicit deny in a resource-based policy', with HeadObject still 200 and the same SSEKMSKeyId. The CMK was reported Enabled at creation, so key state is not the variable here.

## Steps to Reproduce
Replay the three check ids in trace_ids (each is killed at the 60 s cap). By hand:
1. CreateKey; PutKeyPolicy with a condition requiring kms:EncryptionContext:aws:s3:arn == <bucket ARN>; PutBucketEncryption(aws:kms, KMSMasterKeyID=<key>, BucketKeyEnabled=true); PutObject (permitted); PutObject the same bytes with BucketKeyEnabled=false — observe it is still permitted instead of AccessDenied/403.
2. CreateKey; PutBucketEncryption(aws:kms, KMSMasterKeyID=<key>); PutObject with no encryption headers (succeeds); add a key-policy statement with Effect=Deny on kms:GenerateDataKey for the caller; PutObject again — observe HTTP 200 instead of AccessDenied/403, and the object exists.
3. CreateKey; PutObject with ServerSideEncryption=aws:kms and SSEKMSKeyId=<key>; add a key-policy Deny on kms:Decrypt for the caller; GetObject — observe the body is returned instead of AccessDenied/403 (HeadObject must stay 200).
