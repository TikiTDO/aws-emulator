Issue-Source: trace-sweep-divergence
Issue-Trace: dms-s3-full-load-task-migrates-csv-rows
Issue-Step: task#24
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dms-s3-full-load-task-migrates-csv-rows
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.KeyCount
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# s3 ListObjectsV2 reports KeyCount 0 and no Contents for the DMS full-load target prefix

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
One net-new wire divergence, from fixture dms-s3-full-load-task-migrates-csv-rows step task#24 (service 's3', operation ListObjectsV2). The emulator is serving (emulator_up=true, bootstrap_signal=false, /__admin/health 200 and the data listener accepts), so this is a fidelity divergence and not a bootstrap gap. The cluster carries NO stub_path and NO owned_by: the open DMS lifecycle issue 20261109T161403Z-dms-replication-task-lifecycle-never-completes-task-settings.md asserts in prose that this task#24 cluster is owned by an s3 ListObjectsV2 issue, but replay now reports owned_by empty for it, so nothing open carries this replay key and it is filed here. Related but NOT covering: 20261110T160554Z-s3-listobjectsv2-of-the-appflow-destination-object-diverges.md and 20261111T040237Z-s3-listobjectsv2-of-an-appflow-written-destination-object.md (those are about an extra key segment and missing per-object checksum members on an object that DOES exist, a different fault from the listing being empty), and 20261110T181940Z-s3-getobject-returns-404-nosuchkey-for-the-destination.md (GetObject of a DataSync destination). The sibling GetObject 404 at step task#25 of this same fixture IS owned (20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md) and is excluded from this issue.

Cause 1 of 1: a GET of the bucket with list-type=2 over the prefix a completed DMS full-load wrote to reports an EMPTY listing instead of the loaded object. Fixture dms-s3-full-load-task-migrates-csv-rows, step task#24 (ListObjectsV2).
AWS: Smithy com.amazonaws.s3#ListObjectsV2 -> GET /{Bucket}?list-type=2, HTTP 200, output shape ListObjectsV2Output serialized with xmlName 'ListBucketResult'. Its KeyCount member is the number of keys returned by this request, and Contents is a flattened list of com.amazonaws.s3#Object entries, one per key matching Prefix. Each Object reports Key, LastModified ('Creation date of the object'), ETag ('a hash of the object'), ChecksumAlgorithm (flattened list), ChecksumType, Size ('Size in bytes of the object') and StorageClass. So for a prefix holding exactly one object, AWS answers KeyCount 1 and exactly one Contents entry with those members populated; the objects that exist under the prefix are a function of what was written to the bucket, not of the listing request. For the DMS S3 target endpoint the written object is recorded behavior only as far as the S3 model goes: the recording shows a full-load of one selected table materializing one object under the target folder, keyed <folder>/<schema>/<table>/LOAD00000001.csv, with Size equal to the CSV byte length, StorageClass STANDARD, ChecksumType FULL_OBJECT and ChecksumAlgorithm CRC64NVME (the recording's own Key 'tgtdata/hr/employee/LOAD00000001.csv', ETag and Size 34 are that run's values filling that template, not requirements in themselves).
Emulator: the same request answers HTTP 200 with $.ListBucketResult.KeyCount '0' and NO $.ListBucketResult.Contents element at all - the target prefix is empty, because nothing ever wrote the full-load object into the bucket. Replay detail: "$.ListBucketResult.KeyCount: value_mismatch (expected '1', got '0')" and "$.ListBucketResult.Contents: missing (... got None)". The status and the envelope are right; only the object is absent. Hypothesis: the DMS full-load execution path never performs the target-endpoint PUT (the same fixture's task never leaves 'running', per the open DMS lifecycle issue), so S3 has no object to list; the ListObjectsV2 handler itself is reporting its store faithfully.
Why: a client that drives a migration and then reads its output through S3 - list the target prefix, then read the LOAD00000001.csv object - sees an empty bucket and concludes the migration produced no data. Every downstream consumer keyed on the listing (a crawler, a loader, an 'object exists' precondition, a paginator counting KeyCount) silently processes zero rows instead of the migrated table, and the follow-up GetObject of the listed key cannot succeed because the key is never listed.
Fix: the write side of the S3 target plane for this flow - the component that materializes a DMS S3 target endpoint's full-load output into the S3 object store (the DMS replication-task execution path's target writer), so that the object store the ListObjectsV2 handler reads actually holds one object per loaded table under the endpoint's bucket folder, with Size, ETag, StorageClass, ChecksumAlgorithm and ChecksumType set the way a normal single-part PUT sets them. The ListObjectsV2 handler's own projection of KeyCount/Contents needs no change if the object is present.
Traces: dms-s3-full-load-task-migrates-csv-rows#task#24.

## Evidence
Replay of trace set simparity-wire-aws-v1: 12 fixtures replayed, 156 steps, 57 passed, 97 mismatched, 2 blocked, 7 divergence clusters, score 36.5%. emulator_up=true, bootstrap_signal=false, emulator_detail '/__admin/health answered 200 and the :44735 data listener accepts'. Service token 's3' is bootstrapped.

[mismatch] dms-s3-full-load-task-migrates-csv-rows#task#24 ListObjectsV2 - expected HTTP 200, got HTTP 200; body diverges:
  $.ListBucketResult.KeyCount: value_mismatch (expected '1', got '0')
  $.ListBucketResult.Contents: missing - expected one Object entry {Key: '<target-folder>/<schema>/<table>/LOAD00000001.csv' (recorded as 'tgtdata/hr/employee/LOAD00000001.csv'), LastModified: <shaped-not-asserted>, ETag: <hash of the object bytes>, ChecksumAlgorithm: 'CRC64NVME', ChecksumType: 'FULL_OBJECT', Size: <CSV byte length> (recorded 34), StorageClass: 'STANDARD'}; actual: None.
  divergence_key: service=s3|ListObjectsV2|status=200->200|keys=$.ListBucketResult.Contents,$.ListBucketResult.KeyCount
  stub_path: (empty)   owned_by: (empty)   affected_count: 1

Corroborating (NOT filed here, owned by 20261102T111455Z-s3-scalar-parameter-refusals-leak-smithy-coercion-wording.md): step task#25 GetObject of that same key answers HTTP 404 with Code 'NoSuchKey' / Message 'The specified key does not exist.' where the recording has HTTP 200 and the CSV body - i.e. the object genuinely is not in the store, which is consistent with the empty listing rather than a listing-projection bug.

Model consulted: smithy_query query=operation operation=ListObjectsV2 and query=shape shape=Object on services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json - ListObjectsV2 is GET /{Bucket}?list-type=2 at HTTP 200, output xmlName 'ListBucketResult', with KeyCount (integer) and Contents (flattened ObjectList); com.amazonaws.s3#Object carries Key, LastModified, ETag, ChecksumAlgorithm, ChecksumType, Size and StorageClass.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay wire fixture dms-s3-full-load-task-migrates-csv-rows from trace set simparity-wire-aws-v1; step task#24 (ListObjectsV2) diverges with KeyCount 0 and no Contents.
3. Shorter repro:
   a. CreateBucket for the migration target bucket.
   b. Stand up the DMS side: CreateReplicationSubnetGroup over two subnets in two AZs, CreateReplicationInstance (wait for 'available'), CreateEndpoint twice with EngineName 's3' (EndpointType 'source' and 'target', the target's S3Settings naming the bucket and a BucketFolder).
   c. CreateReplicationTask with MigrationType 'full-load' and one selection rule for a single schema/table, then StartReplicationTask and wait for the task to finish.
   d. GET /<bucket>?list-type=2&prefix=<BucketFolder>/<schema>/<table>/ : the emulator answers HTTP 200 with <ListBucketResult><KeyCount>0</KeyCount></ListBucketResult> and no <Contents>, where AWS answers KeyCount 1 and one Contents entry for <BucketFolder>/<schema>/<table>/LOAD00000001.csv with its Size, ETag, StorageClass STANDARD, ChecksumType FULL_OBJECT and ChecksumAlgorithm CRC64NVME.
   e. GET the key that should have been listed: HTTP 404 NoSuchKey, confirming the object was never written rather than merely omitted from the listing.
