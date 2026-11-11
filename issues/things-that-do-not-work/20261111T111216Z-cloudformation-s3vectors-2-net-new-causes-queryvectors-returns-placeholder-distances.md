Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3vectors.cfn-s3vectors-index-in-place-update-retained-replacement-queries-nearest-then-not-found cloudformation__s3vectors.cfn-s3vectors-vector-bucket-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__s3vectors.cfn-s3vectors-index-in-place-update-retained-replacement-queries-nearest-then-not-found|_assert_ranking#22|AssertionError|assert abs(v["distance"] - distance) < _DISTANCE_TOLERANCE, (
Issue-Key: cloudformation__s3vectors.cfn-s3vectors-vector-bucket-in-place-update-retained-replacement-then-not-found|_expect#25|AssertionError|raise AssertionError(
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# 2 net-new causes: QueryVectors returns placeholder distances, and AWS::S3Vectors::VectorBucket in-place updates are refused

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
QueryVectors

## Description
Two independent root causes behind the S3 Vectors CloudFormation checks.

Cause 1 of 2: QueryVectors does not compute distances from the stored vectors.
AWS: QueryVectors ranks the vectors in an index against the query vector and returns, per result, the key and the distance under the index's distance metric — a real-valued number derived from the stored vector components (for cosine distance, 1 - cosine similarity). For the recorded fixture the middle vector's distance is 0.996078372001648; the ranking order and the actual values both come out of the data.
Emulator: the distances are flat placeholders that happen to follow rank order — QueryVectors answered [{'distance': 0.0, 'key': 'near'}, {'distance': 1.0, 'key': 'mid'}, {'distance': 2.0, 'key': 'far'}] where the mid vector's distance had to be ~0.9961. The ordering is right; the values are not computed at all.
Why: a client that thresholds on distance (relevance cut-offs, nearest-neighbour acceptance, re-ranking, RAG retrieval scoring) makes the wrong decision on every query, and distances are not comparable across queries or indexes.
Fix: the S3 Vectors QueryVectors handler's distance computation over the index's stored vectors and its configured distance metric.
Failing check: cfn-s3vectors-index-in-place-update-retained-replacement-queries-nearest-then-not-found.

Cause 2 of 2: in-place update of AWS::S3Vectors::VectorBucket is refused with an emulator-authored reason.
AWS: recorded behavior only. Changing a mutable property of a vector bucket (encryption configuration, tags) is an in-place update; the bucket's identity is unchanged, the resource goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE and the stack reaches UPDATE_COMPLETE.
Emulator: the resource is UPDATE_FAILED and the stack ends 'UPDATE_ROLLBACK_COMPLETE' with StatusReason 'The following resource(s) failed to update: [Bucket]. '; the reason is 'NotImplemented: this emulator does not change the properties of an existing AWS::S3Vectors::VectorBucket in place; the resource was left as it is.' (fixed text 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.'; variable part = resource type). AWS has no NotImplemented error for a supported property change.
Why: a CloudFormation-managed vector bucket cannot be edited; every such deploy rolls back.
Fix: the AWS::S3Vectors::VectorBucket resource handler's update path in the CloudFormation provisioner.
Failing check: cfn-s3vectors-vector-bucket-in-place-update-retained-replacement-then-not-found.

## Evidence
Cause 1: AssertionError: mid must be at distance 0.996078372001648, QueryVectors answered [{'distance': 0.0, 'key': 'near'}, {'distance': 1.0, 'key': 'mid'}, {'distance': 2.0, 'key': 'far'}]
Cause 2: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Bucket]. '), expected 'UPDATE_COMPLETE'; failed events: [('Bucket', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::S3Vectors::VectorBucket in place; the resource was left as it is.')]

## Steps to Reproduce
Cause 1: CreateStack an AWS::S3Vectors::VectorBucket + Index; PutVectors three vectors at known distances from a probe; call QueryVectors with the probe and read the per-result distance.
Cause 2: CreateStack an AWS::S3Vectors::VectorBucket, then UpdateStack changing only a mutable property; read DescribeStackEvents.
