Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-model-card-lifecycle-and-export-job
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-model-card-lifecycle-and-export-job
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.KeyCount
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Prefix
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# S3 ListObjectsV2 echoes the percent-encoded prefix instead of the decoded value

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
Cause 1 of 1: ListObjectsV2 called with prefix=model-card-exports/ (sent on the wire as the percent-encoded query value model-card-exports%2F). AWS: the ListBucketResult echoes the prefix the caller asked for, percent-decoded - <Prefix>model-card-exports/</Prefix>. The emulator echoes the raw, still-encoded query value <Prefix>model-card-exports%2F</Prefix>, i.e. the query-string decoding is not applied to the prefix parameter before it is stored for the echo (and, since the same raw value is what the listing is filtered on, any object under that prefix would also be filtered out by a literal '%2F' comparison). Why: a client that round-trips Prefix from the response into its next request, or that strips the prefix from returned keys to compute relative names, gets a value that does not exist in the bucket; delimiter/prefix pagination over encoded keys breaks. Fix: the ListObjectsV2 handler's query-parameter binding (percent-decode prefix, delimiter, start-after and marker before use and before echoing), not the serializer. AWS: recorded behavior only for the echo (the Smithy model binds Prefix as an httpQuery member; percent-decoding of an httpQuery value is protocol behavior). Traces: sagemaker-model-card-lifecycle-and-export-job#task#6. NOTE on the same step's KeyCount/Contents divergence: the recording expects KeyCount 1 and a Contents entry for the exported model-card PDF, and the emulator reports KeyCount 0 with no Contents. That object is absent because the SageMaker model-card export job never produces it - filed against sagemaker as 'SageMaker lifecycle, identity and round-trip fidelity defects across ten operations' (cause 7, DescribeModelCardExportJob Status 'Failed', no ExportArtifacts) - so the S3 defect to fix here is the prefix handling.

## Evidence
Step sagemaker-model-card-lifecycle-and-export-job#task#6, ListObjectsV2, status 200 both sides. $.ListBucketResult.Prefix: expected 'model-card-exports/', got 'model-card-exports%2F'. Same response: $.ListBucketResult.KeyCount expected '1', got '0'; $.ListBucketResult.Contents expected an entry with Key 'model-card-exports/<model-card>/<export job>.pdf', ChecksumAlgorithm 'CRC64NVME', ChecksumType 'FULL_OBJECT', StorageClass 'STANDARD', got None - that absence is the downstream effect of the SageMaker export job never writing the object (filed separately against sagemaker).

## Steps to Reproduce
Replay the wire corpus; the diverging step is fixture sagemaker-model-card-lifecycle-and-export-job step task#6. Equivalent minimal repro: PUT any object, then GET /<bucket>?list-type=2&prefix=model-card-exports%2F and compare <Prefix> in the ListBucketResult - AWS returns 'model-card-exports/', the emulator returns 'model-card-exports%2F'.
