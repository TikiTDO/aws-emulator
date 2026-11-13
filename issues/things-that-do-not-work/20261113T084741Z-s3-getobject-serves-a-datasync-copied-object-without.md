Issue-Source: trace-sweep-divergence
Issue-Trace: datasync-s3-to-s3-task-execution-copies-object
Issue-Step: task#12
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datasync-s3-to-s3-task-execution-copies-object
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: s3|GetObject|2xx->2xx|$headers.x-amz-meta-last-modified
Issue-Related: 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md
Issue-Commit: 25060605044cad8473b79fae67a906480ef14ab5

# S3 GetObject serves a DataSync-copied object without its content type, user metadata or CRC64NVME checksum

> _Note: this issue was identified against the code base at commit `25060605044cad8473b79fae67a906480ef14ab5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One root cause, grounded in one wire divergence cluster.

=== Cause 1 of 1: an object materialized by a DataSync S3->S3 transfer is stored without its metadata, so GetObject returns 200 but omits Content-Type, the x-amz-meta-* user metadata and the checksum header ===

AWS: Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json, GetObject) binds GET /{Bucket}/{Key+} with success code 200, the object bytes on the httpPayload streaming member Body, and these response headers among others: ContentType -> Content-Type, Metadata -> httpPrefixHeaders "x-amz-meta-", ChecksumCRC64NVME -> x-amz-checksum-crc64nvme, with httpChecksum responseAlgorithms including CRC64NVME. A GET of an object therefore returns the Content-Type that was recorded when the object was stored (not a fallback), every user-metadata entry stored with it re-emitted as an x-amz-meta-<name> header, and the object's full-object checksum header for objects stored with one. A DataSync S3->S3 transfer preserves the source object's content type and user metadata on the destination object and the destination object carries a CRC64NVME full-object checksum, so after a terminal execution a GET of the transferred key returns 200 with Content-Type equal to the source object's (text/plain for a text object), the source object's x-amz-meta-* entries, and x-amz-checksum-crc64nvme. The recording's particular checksum value and metadata values fill that template; the requirement is that the headers are present and derived from the stored object, not that they equal one run's bytes.

Emulator: the GET now answers 200 with the right body, but the stored object has no metadata behind it: Content-Type comes back as the generic fallback application/octet-stream rather than the source object's text/plain, no x-amz-meta-* header is emitted at all (the recorded destination object carries an x-amz-meta-last-modified entry), and x-amz-checksum-crc64nvme is absent even though a CRC64NVME was computable for the stored bytes.

Why: a client that consumes the destination of a transfer reads the object's metadata, not just its bytes. Content-Type application/octet-stream makes a browser or an HTTP client download a text object instead of rendering it and makes content-sniffing pipelines mis-route it; a missing x-amz-meta-* entry silently loses application state the producer attached to the object, so a consumer keyed off user metadata sees it as unset rather than as preserved by the copy; and a missing x-amz-checksum-crc64nvme means an integrity check requested with ChecksumMode ENABLED has nothing to verify, so a verification step either fails or skips silently. Code that asserts a DataSync copy is faithful cannot pass even though the bytes arrived.

Fix: the write path that DataSync's transfer phase uses to materialize destination objects in S3 object state, plus S3's object state record it writes into -- the transfer must copy the source object's content type and user-metadata map onto the destination object and compute/store its CRC64NVME, so S3's GetObject serializer has values to bind to Content-Type, the x-amz-meta-* prefix headers and x-amz-checksum-crc64nvme. GetObject's own serializer needs no change beyond emitting headers for metadata that is present.

Related but not covering this: open issue 20261110T181940Z-s3-getobject-returns-404-nosuchkey-for-the-destination.md covers the earlier, stronger symptom at this same fixture step (the object was absent entirely and the GET 404'd with NoSuchKey); the object is now served with 200 and the correct body, and the replay report lists this cluster as unowned. The remaining defect is the missing stored metadata on the materialized object, which that issue's cause (nothing written at all) does not describe. Open issue 20261110T181940Z-datasync-describetaskexecution-reports-a-no-op-transfer-zero.md owns the control-plane view (task#11) of the same transfer and likewise does not cover these response headers.

Traces: datasync-s3-to-s3-task-execution-copies-object#task#12

## Evidence
Fixture datasync-s3-to-s3-task-execution-copies-object, step task#12, operation GetObject, service s3, verdict mismatch. Expected status 200, actual status 200 (status matches; response headers diverge).

  $headers.content-type: value_mismatch (expected 'text/plain', got 'application/octet-stream')
  $headers.x-amz-checksum-crc64nvme: missing (expected a CRC64NVME value -- the recording shows 'vF5rpmWptb0=' for its bytes; the requirement is the header, computed from the stored object)
  $headers.x-amz-meta-last-modified: missing (expected present, value shaped-not-asserted)

Model references: GetObject http {method: GET, uri: /{Bucket}/{Key+}, code: 200}; Body = httpPayload streaming blob; ContentType -> httpHeader Content-Type; Metadata -> httpPrefixHeaders x-amz-meta-; ChecksumCRC64NVME -> httpHeader x-amz-checksum-crc64nvme; httpChecksum.responseAlgorithms includes CRC64NVME; errors = [InvalidObjectState(403), NoSuchKey(404)].

## Steps to Reproduce
Replay fixture datasync-s3-to-s3-task-execution-copies-object and inspect step task#12.

Manually: PutObject a small object into a source bucket with Content-Type text/plain and at least one user-metadata entry (e.g. x-amz-meta-last-modified); create an empty destination bucket; CreateLocationS3 for both; CreateTask over them; StartTaskExecution and poll DescribeTaskExecution until the execution is terminal; then GetObject the transferred key from the destination bucket. Observe 200 with the correct body but Content-Type application/octet-stream, no x-amz-meta-* headers and no x-amz-checksum-crc64nvme header, instead of the source object's content type, its preserved user metadata and the object's CRC64NVME checksum header.
