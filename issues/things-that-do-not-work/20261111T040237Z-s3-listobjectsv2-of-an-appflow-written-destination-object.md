Issue-Source: trace-sweep-divergence
Issue-Trace: appflow-s3-to-s3-flow-run-transfers-records
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appflow-s3-to-s3-flow-run-transfers-records
Issue-Shape-Id: com.amazonaws.appflow#SandstoneConfigurationServiceLambda
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents.Key
Issue-Related: 20261110T160554Z-s3-listobjectsv2-of-the-appflow-destination-object-diverges.md
Issue-Commit: 6f5959473f4f355ac18f705f826bb321fb68622e

# s3 ListObjectsV2 of an AppFlow-written destination object: nested key segment and missing per-object checksum members

> _Note: this issue was identified against the code base at commit `6f5959473f4f355ac18f705f826bb321fb68622e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
ListObjectsV2 over the destination bucket/prefix that an AppFlow S3-to-S3 flow run wrote returns a 200 listing whose single Contents entry diverges from the recorded AWS listing in two independent ways. Both are observed on one replayed step (fixture appflow-s3-to-s3-flow-run-transfers-records, step task#9), which lists the flow's destination prefix after a successful run. There are 2 causes in this issue; both must be closed.

Cause 1 of 2: the flow-run output object is listed under an extra key segment with an appended extension.
AWS: recorded behavior only for the key LAYOUT. The recording shows an AppFlow S3 destination run depositing exactly ONE object whose key is <destinationPrefix>/<flowName>/<executionId> - the configured bucket prefix, the flow name and the flow-execution id, with no further path segment and no file-name extension. Smithy (services/service-com-amazonaws-s3-amazons3): ListObjectsV2 output Object.Key -> com.amazonaws.s3#ObjectKey, @length min 1, is echoed verbatim from the stored key, so the listing reports whatever key the writer chose; the defect is therefore in the key the writer composes, not in the serializer.
Emulator: the listed key repeats the execution id as a child path element and appends '.json', i.e. <destinationPrefix>/<flowName>/<executionId>/<executionId>.json. Replay reports $.ListBucketResult.Contents.Key value_mismatch - expected the three-segment key, got the four-segment key with the .json suffix. (The bucket name, flow name and execution id are per-run values; the requirement is the SHAPE of the key.)
Why: a client locates a run's output the documented way - compose the key from the destination prefix, the flow name and the executionId returned by StartFlow/DescribeFlowExecutionRecords - then GetObject/HeadObject that key. Against the emulator that key does not exist (NoSuchKey), and anything keyed on the exact object path (a bucket-notification suffix/prefix filter, a downstream Glue/Athena table location, an s3 key-equality policy condition) never matches. Prefix/delimiter walks also report one extra directory level.
Fix: the AppFlow flow-run S3 destination writer's output-key composition (the component that stores the run's object into S3 object state) - it must store the object AT the <prefix>/<flowName>/<executionId> key instead of nesting a per-execution file beneath it; the ListObjectsV2 handler then reports it unchanged.
Traces: appflow-s3-to-s3-flow-run-transfers-records#task#9.

Cause 2 of 2: listing entries carry no checksum algorithm or checksum type.
AWS: Smithy: Object.ChecksumAlgorithm -> com.amazonaws.s3#ChecksumAlgorithmList (xmlFlattened; member enum includes CRC64NVME) - 'The algorithm that was used to create a checksum of the object'; Object.ChecksumType -> com.amazonaws.s3#ChecksumType, enum COMPOSITE | FULL_OBJECT - 'The checksum type that is used to calculate the object's checksum value'. The recording shows AWS reporting ChecksumAlgorithm CRC64NVME and ChecksumType FULL_OBJECT for the single-part object the flow run wrote, i.e. S3's default full-object CRC64NVME checksum for a normal single-part write. The requirement is that any single-part write - including one performed by a service integration such as AppFlow - is listed with its default full-object checksum algorithm and FULL_OBJECT type.
Emulator: both members are absent from the Contents entry. Replay reports $.ListBucketResult.Contents.ChecksumAlgorithm missing (expected 'CRC64NVME', got None) and $.ListBucketResult.Contents.ChecksumType missing (expected 'FULL_OBJECT', got None). Hypothesis: the emulator neither computes/retains a checksum for service-written objects nor projects one into the listing.
Why: a client that uses the listing to decide whether an object needs integrity verification, or to choose which checksum to request on GetObject, sees an unchecksummed object; SDK/pipeline checksum validation silently skips verification, and code branching on FULL_OBJECT vs COMPOSITE takes the wrong branch.
Fix: S3 object state must record each write's checksum algorithm and checksum type (full-object CRC64NVME by default for a single-part write, including writes made by service integrations), and the ListObjectsV2 serializer must emit the flattened ChecksumAlgorithm list element and the ChecksumType element per Contents entry.
Traces: appflow-s3-to-s3-flow-run-transfers-records#task#9.

Not covered elsewhere: the open issue 20261110T160554Z-s3-listobjectsv2-of-the-appflow-destination-object-diverges.md describes the same two causes but the replay reports this divergence cluster as unowned (its recorded Issue-Key lines cover only the two checksum members, not $.ListBucketResult.Contents.Key), so the cluster is re-filed here in full and that issue should be closed together with it. The open issues 20261109T175957Z-s3-getobject-serves-the-appflow-written-object-with.md, 20261109T135237Z-s3-getobject-loses-object-response-headers-user-metadata.md and 20261110T030759Z-s3-getobject-omits-the-crc32-full-object-checksum.md all concern GetObject response headers/body rather than the ListObjectsV2 listing projection or the stored key layout.

## Evidence
Replayed step: fixture appflow-s3-to-s3-flow-run-transfers-records, step task#9, service s3, operation ListObjectsV2, verdict mismatch, expected_status 200, actual_status 200 (status matches; the body diverges). Divergence key: service=s3|ListObjectsV2|status=200->200|keys=$.ListBucketResult.Contents.ChecksumAlgorithm,$.ListBucketResult.Contents.ChecksumType,$.ListBucketResult.Contents.Key.

Expected (recorded AWS) vs actual (emulator):
- $.ListBucketResult.Contents.Key: value_mismatch. Expected '<destinationPrefix>/<flowName>/<executionId>' (this run recorded 'output/sim2real-probe-appflow-flow-e2dc10dc/775b3fd9-08c3-4301-a764-7c4a64bb794d'); got that same key plus an extra '/<executionId>.json' segment (this run: '.../775b3fd9-08c3-4301-a764-7c4a64bb794d/775b3fd9-08c3-4301-a764-7c4a64bb794d.json'). Prefix, flow name and execution id are per-run values supplied by the request/state; the fixed requirement is the three-segment layout with no appended extension.
- $.ListBucketResult.Contents.ChecksumAlgorithm: missing - expected 'CRC64NVME', got None.
- $.ListBucketResult.Contents.ChecksumType: missing - expected 'FULL_OBJECT', got None.

Smithy citations (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): Object.Key -> ObjectKey (@length min 1); Object.ChecksumAlgorithm -> ChecksumAlgorithmList (xmlFlattened, member enum ChecksumAlgorithm incl. CRC64NVME); Object.ChecksumType -> ChecksumType enum (COMPOSITE | FULL_OBJECT).

## Steps to Reproduce
1. Create the source and destination buckets and seed the source object(s) as fixture appflow-s3-to-s3-flow-run-transfers-records does.
2. Create an AppFlow S3-to-S3 flow whose destination connector properties set a bucket prefix (recorded: 'output'), call StartFlow and wait for the execution to finish.
3. Call S3 ListObjectsV2 on the destination bucket with prefix '<destinationPrefix>/<flowName>' (fixture appflow-s3-to-s3-flow-run-transfers-records, step task#9).
4. Compare the single Contents entry with the recorded response: the emulator's Contents.Key carries an extra '/<executionId>.json' segment beyond '<prefix>/<flowName>/<executionId>', and the entry has no ChecksumAlgorithm (expected CRC64NVME) and no ChecksumType (expected FULL_OBJECT).
5. Re-replay appflow-s3-to-s3-flow-run-transfers-records#task#9 to confirm: Contents.Key must equal the three-segment key and the entry must carry both checksum members.
