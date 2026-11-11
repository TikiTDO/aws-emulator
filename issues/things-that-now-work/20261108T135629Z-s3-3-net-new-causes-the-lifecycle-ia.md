Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected s3.deep-s3-storage-class-restore-zero-days-rejected s3.s3-csv-get-if-none-match-on-the-current-etag-rejected-not-modified
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected|run#11|AssertionError|assert err.get("Message") == "'Days' in the 'Transition' action for StorageClass 'GLACIER' for filter '(prefix=tiered/)' must be 30 days more than 'filter '(pre
Issue-Key: s3.deep-s3-storage-class-restore-zero-days-rejected|run#9|AssertionError|assert refusal == ("InvalidArgument", _ZERO_DAYS_MESSAGE), (
Issue-Key: s3.s3-csv-get-if-none-match-on-the-current-etag-rejected-not-modified|run#9|AssertionError|assert hdrs.get("x-amz-version-id") == vid, (
Issue-Commit: 9e2a1177787692948f5e67ac213048cdaf07cba6

# s3: 3 net-new causes — the lifecycle IA→GLACIER gap refusal and the RestoreObject Days=0 refusal carry emulator-authored messages, and a 304 Not Modified omits x-amz-version-id

> _Note: this issue was identified against the code base at commit `9e2a1177787692948f5e67ac213048cdaf07cba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketLifecycleConfiguration

## Description
REOPENS 20261029T211510Z-s3-representation-divergences-lifecycle-transition-validator-restoreobject-202.md: its fix left the call refused. That issue carried the replay keys s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected and s3.deep-s3-storage-class-restore-zero-days-rejected and was marked fixed; both checks still fail on the refusal of the same operations — the refusal verdict and error code are now right, but the message text is still not AWS's. Cause 3 below is net-new and unrelated to that file.

Three root causes in this service; a fixer must close all three.

Cause 1 of 3: PutBucketLifecycleConfiguration's STANDARD_IA→GLACIER minimum-gap refusal message is misassembled.
AWS: a lifecycle rule that transitions to STANDARD_IA and then to GLACIER fewer than 30 days later is refused with HTTP 400, Code InvalidArgument, and this message: "'Days' in the 'Transition' action for StorageClass '<LATER_CLASS>' for filter '<FILTER>' must be 30 days more than 'filter '<FILTER>'' in the 'Transition' action for StorageClass '<EARLIER_CLASS>'". The fixed text is everything outside the angle brackets, including the trailing clause "in the 'Transition' action for StorageClass" with no repeated "'Days'" and no second "for filter" clause. The variable parts come from the request: the two StorageClass values of the offending transition pair and the rule's filter rendered as AWS renders it (for a prefix-only filter, "(prefix=<the rule's Prefix>)"); 30 is STANDARD_IA's documented minimum storage duration. Smithy: PutBucketLifecycleConfiguration models no per-case error shapes, so the InvalidArgument code and this sentence are recorded behavior only; the 30-day minimum is the documented rule. The accepted control (STANDARD_IA@30 → GLACIER@60) round-trips, so only the refusal text diverges.
Emulator: answers InvalidArgument but with "'Days' in the 'Transition' action for StorageClass 'GLACIER' for filter '(prefix=tiered/)' must be 30 days more than 'filter '(prefix=tiered/)'' 'Days' in the 'Transition' action for StorageClass 'STANDARD_IA' for filter '(prefix=tiered/)'" — it re-inserts "'Days'" after the "must be 30 days more than 'filter '…''" clause, drops AWS's "in the", and appends a second "for filter '…'" clause that AWS does not emit.
Why: the message is part of the error class. A client (or a test) that matches AWS's sentence to tell this refusal apart from the other InvalidArgument refusals PutBucketLifecycleConfiguration makes does not match it, so a legitimate 30-day-gap violation is reported as an unrecognised argument error.
Fix: the PutBucketLifecycleConfiguration validator's transition-ordering refusal message (its message assembly, not its verdict).
Traces: s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected

Cause 2 of 3: RestoreObject's non-positive Days refusal message is the emulator's own sentence.
AWS: RestoreObject with RestoreRequest.Days = 0 on an archived (GLACIER) object is refused with Code InvalidArgument and the message "restoration days should be at least 1". That sentence is entirely fixed text — it names neither the rejected value nor the member — and the refused call starts no restore (no x-amz-restore header appears on a subsequent HeadObject, and GetObject still answers InvalidObjectState). Smithy: RestoreObject models only ObjectAlreadyInActiveTierError, so the InvalidArgument code and this message are recorded behavior only.
Emulator: answers InvalidArgument with "Days must be at least 1".
Why: the message differs from AWS's for the same error class, so a client that distinguishes RestoreObject's argument refusals by message mis-routes this one; and because the previous fix rewrote the sentence rather than adopting AWS's, the refusal is still not reproducible against AWS.
Fix: the RestoreObject handler's Days validation message.
Traces: s3.deep-s3-storage-class-restore-zero-days-rejected

Cause 3 of 3: a 304 Not Modified from GetObject carries no x-amz-version-id.
AWS: GetObject with If-None-Match set to the object's current ETag answers 304 Not Modified with no body, and the response carries x-amz-version-id naming the version that was evaluated (on a versioned bucket, the current version's id). Smithy: GetObjectOutput.VersionId is bound to httpHeader x-amz-version-id, and it is serialized on the conditional-failure response as well as on a 200. The non-matching-ETag control correctly returns 200 with the body, so only the 304's headers diverge.
Emulator: the 304 response omits x-amz-version-id entirely (the header reads None).
Why: a cache revalidating an object it already holds is told "unchanged" but not which version is unchanged, so on a versioned bucket it cannot confirm that the entry it holds is still the current version rather than a stale one that happens to share an ETag, and it cannot pin a subsequent read to that version.
Fix: the GetObject conditional-request path's 304 serializer, which must emit the version-bound response headers (x-amz-version-id) alongside the empty body.
Traces: s3.s3-csv-get-if-none-match-on-the-current-etag-rejected-not-modified

## Evidence
Cause 1 — s3.deep-s3-storage-class-lifecycle-transition-ia-to-glacier-gap-rejected (mismatch):
Expected (AWS): Code InvalidArgument, Message "'Days' in the 'Transition' action for StorageClass 'GLACIER' for filter '(prefix=tiered/)' must be 30 days more than 'filter '(prefix=tiered/)'' in the 'Transition' action for StorageClass 'STANDARD_IA'"
Actual (emulator): {'Code': 'InvalidArgument', 'Message': "'Days' in the 'Transition' action for StorageClass 'GLACIER' for filter '(prefix=tiered/)' must be 30 days more than 'filter '(prefix=tiered/)'' 'Days' in the 'Transition' action for StorageClass 'STANDARD_IA' for filter '(prefix=tiered/)'"}
(The GLACIER/STANDARD_IA class names and the (prefix=…) filter rendering are request-derived; the rest of the sentence is fixed text.)

Cause 2 — s3.deep-s3-storage-class-restore-zero-days-rejected (mismatch):
Expected (AWS): ('InvalidArgument', 'restoration days should be at least 1')
Actual (emulator): ('InvalidArgument', 'Days must be at least 1')
AssertionError: RestoreObject Days=0 returned ('InvalidArgument', 'Days must be at least 1'), expected InvalidArgument

Cause 3 — s3.s3-csv-get-if-none-match-on-the-current-etag-rejected-not-modified (mismatch):
Expected (AWS): 304 with x-amz-version-id equal to the current version id
Actual (emulator): 304 with x-amz-version-id absent
AssertionError: the 304 must name the current version '<current version id>'; got None

## Steps to Reproduce
Replay the behavioral suite and run the three s3 checks named in trace_ids.
Cause 1: CreateBucket; PutBucketLifecycleConfiguration with one Enabled rule, Filter {Prefix: 'tiered/'}, Transitions [{Days:30, StorageClass:'STANDARD_IA'},{Days:60, StorageClass:'GLACIER'}] (accepted, round-trips); then PutBucketLifecycleConfiguration with the same rule but Transitions [{Days:30,'STANDARD_IA'},{Days:40,'GLACIER'}] and compare the InvalidArgument Message to AWS's sentence.
Cause 2: PutObject with StorageClass=GLACIER; RestoreObject with RestoreRequest {Days:0, GlacierJobParameters:{Tier:'Bulk'}} and compare the Code/Message pair; then confirm HeadObject carries no Restore header and GetObject still answers InvalidObjectState.
Cause 3: CreateBucket with versioning Enabled; PutObject; HeadObject to read the ETag; GetObject with IfNoneMatch set to a non-matching ETag (must be 200 with the body), then GetObject with IfNoneMatch set to the current ETag and inspect the 304's x-amz-version-id header.
