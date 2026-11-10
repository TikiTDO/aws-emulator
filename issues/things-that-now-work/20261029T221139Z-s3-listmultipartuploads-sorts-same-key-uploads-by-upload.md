Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3.list-multipart-uploads-ordering
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3.list-multipart-uploads-ordering|run#7|AssertionError|assert a_ids == [uid_a1, uid_a2], (
Issue-Commit: 1c82a6d99cb1f7308f6a13b71a678a6984dedc7f

# s3: ListMultipartUploads sorts same-key uploads by upload-id instead of by initiation time

> _Note: this issue was identified against the code base at commit `1c82a6d99cb1f7308f6a13b71a678a6984dedc7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMultipartUploads

## Description
One net-new root cause in the S3 multipart listing read-back.

Cause 1 of 1: ListMultipartUploads applies the wrong secondary sort key for uploads that share an object key.

AWS: the S3 Smithy model documents the response order of this operation explicitly, under "Sorting of multipart uploads in response" / General purpose bucket: "Key-based sorting - Multipart uploads are initially sorted in ascending order based on their object keys." and "Time-based sorting - For uploads that share the same object key, they are further sorted in ascending order based on the upload initiation time. Among uploads with the same key, the one that was initiated first will appear before the ones that were initiated later." So the ordering contract is a two-level sort: object key ascending, then MultipartUpload$Initiated ascending — NOT upload id. Upload-id lexicographic order only governs the pagination marker arithmetic (ListMultipartUploadsRequest$UploadIdMarker: "multipart uploads have upload IDs lexicographically greater than the specified upload-id-marker"), which is a separate concern from the order entries are reported in. There is no error involved; this is a 200 response whose body is ordered wrongly.

Emulator: the handler builds the candidate list and sorts it on the (key, upload_id) tuple, so within one key the entries come back in upload-id lexicographic order. Upload ids are opaque, effectively random tokens, so the resulting order within a key is arbitrary with respect to initiation time. In the replay, three in-progress uploads were created in the order (key "mmu-b-obj"), (key "mmu-a-obj" first), (key "mmu-a-obj" second); the key-level ordering was correct, but the two same-key uploads came back reversed relative to their initiation order: the response listed the second-initiated upload before the first-initiated one. The check asserted the two same-key UploadIds equal [first-initiated, second-initiated] and got them swapped.

Why: a client that enumerates in-progress uploads for one key and relies on the documented ascending-initiation order gets the wrong upload. Two concrete breakages: (1) cleanup/abort tooling that keeps the newest in-progress upload for a key and aborts the older ones will abort the wrong upload, discarding parts that were just uploaded; (2) a resume flow that picks uploads[0] for a key to continue the earliest attempt resumes an arbitrary attempt instead. Because the ordering is stable but keyed off an opaque id, the defect is silent — the response is a well-formed 200 and nothing signals that the order is not the documented one — and it is not reproducible from the request alone, since whether the order happens to agree with initiation time depends on how the two minted ids compare lexicographically.

Fix: the ordering belongs in the ListMultipartUploads handler's listing/sort step (the comparator that orders the collected in-progress uploads before the page is cut). It must sort by object key ascending, then by the recorded initiation instant ascending, keeping the upload id only as a deterministic tie-break for uploads recorded at the same instant. The key-marker / upload-id-marker page-resume comparisons are a separate contract and should not be changed by this; note that the handler's own module doc currently states the behavior as "in key order and, within one key, in upload-id order", so that doc statement is part of the defect and must be corrected too.

Traces: s3.list-multipart-uploads-ordering. Relevant file/line citations: services/service-com-amazonaws-s3-amazons3/src/ops/list_multipart_uploads.rs lines 362-372 (the comment and the `in_progress.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)))` comparator) and lines 3-9 (the module doc asserting upload-id order). No currently open issue covers this: the open S3 multipart issues 20261029T072224Z-s3-uploadpart-does-not-return-x-amz-checksum.md, 20261025T160428Z-s3-multipart-analytics-3-net-new-root-causes.md and 20261029T211510Z-s3-refusal-fidelity-abortmultipartupload-is-not-a-no.md concern UploadPart checksum members, analytics configuration listing and AbortMultipartUpload refusal fidelity respectively, not listing order.

## Evidence
Check: s3.list-multipart-uploads-ordering (verdict: mismatch)

Setup performed by the check: one bucket; three CreateMultipartUpload calls in this order — Key="mmu-b-obj", then Key="mmu-a-obj" (call it A1), then Key="mmu-a-obj" again (call it A2). Then ListMultipartUploads on the bucket with no markers.

Expected (AWS, per the model's documented sorting rules): Uploads ordered by Key ascending, and within Key="mmu-a-obj" the entry for A1 (initiated first) before the entry for A2 (initiated second). The key-ascending assertion is the first one in the check and it PASSED, so only the same-key secondary order diverges.

Actual (emulator): the two Key="mmu-a-obj" entries were returned in the opposite order — A2 before A1.

Replay failure:
  File "check_s3_list_multipart_uploads_ordering.py", line 75, in run
    assert a_ids == [uid_a1, uid_a2]
  AssertionError: expected same-key uploads ordered by initiation time [WImf8xA51cmdKzqSmQsHQ-VP9cDaqr4dP1crHwk1KvM,JRAdO6eNRs-Ok9C4-XIe10_O77oLJcaBPWKDfuurNuI], got ['JRAdO6eNRs-Ok9C4-XIe10_O77oLJcaBPWKDfuurNuI', 'WImf8xA51cmdKzqSmQsHQ-VP9cDaqr4dP1crHwk1KvM']

The two upload-id strings above are per-run minted values, not part of the requirement; the requirement is the relative order (first-initiated before later-initiated). Note that 'J' < 'W' lexicographically, which is exactly the order the emulator produced — i.e. the observed order is the (key, upload-id) sort, confirming the comparator as the cause rather than an incidental flake.

Status/code/message: not applicable — this is a 200 OK response with a wrongly ordered Uploads list; no error code or message diverges.

Emulator source (services/service-com-amazonaws-s3-amazons3/src/ops/list_multipart_uploads.rs):
  line 372: in_progress.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
  where the tuple is (upload.key, upload_id, upload) built at line 370 — the initiation instant (upload.initiated_at, serialized at line 481 as MultipartUpload$Initiated) is available on the record but never consulted by the comparator.
  lines 6-8 of the module doc: "...in key order and, within one key, in upload-id order..." — the documented intent itself is the divergent rule.

Model citation: smithy_query(query="doc", shape="ListMultipartUploads") on services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json returns the "Sorting of multipart uploads in response" section quoted in the description.

## Steps to Reproduce
1. Build and boot the emulator, then point an S3 client (botocore/AWS SDK) at it.
2. Create a bucket (add CreateBucketConfiguration.LocationConstraint when the region is not us-east-1).
3. Call CreateMultipartUpload three times, in this order and WITHOUT completing or aborting any of them:
   a. Bucket=<bucket>, Key="mmu-b-obj"            -> record UploadId as UID_B
   b. Bucket=<bucket>, Key="mmu-a-obj"            -> record UploadId as UID_A1  (initiated first for this key)
   c. Bucket=<bucket>, Key="mmu-a-obj"            -> record UploadId as UID_A2  (initiated second for this key)
4. Call ListMultipartUploads with Bucket=<bucket> and no prefix/delimiter/markers.
5. Collect the UploadIds of the entries whose Key == "mmu-a-obj", preserving response order.
6. Expected (AWS): [UID_A1, UID_A2] — ascending initiation time within the shared key. Also expected: the Key values across the whole listing are ascending ("mmu-a-obj" entries before "mmu-b-obj"), which the emulator already satisfies.
7. Observed (emulator): the two ids come back ordered by their lexicographic upload-id value rather than by initiation time. With the ids minted in this run that was [UID_A2, UID_A1]. To make the divergence deterministic regardless of which ids get minted, repeat step 3b/3c until UID_A2 < UID_A1 lexicographically; the emulator then always reports A2 first while AWS always reports A1 first.
8. Teardown: AbortMultipartUpload for UID_B, UID_A1, UID_A2 and delete the bucket.

Re-replay s3.list-multipart-uploads-ordering to confirm the fix.
