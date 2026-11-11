Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception cloudwatch__dynamodb__lambda.lambda-ddb-throttle-triggers-cloudwatch-alarm cloudwatch__dynamodb__lambda.lambda-ddb-batchwrite-unprocessed-items-metric
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception|_wait_until_stable#12|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-throttle-triggers-cloudwatch-alarm|run#11|AssertionError|assert total_throttled > 0, (
Issue-Key: cloudwatch__dynamodb__lambda.lambda-ddb-batchwrite-unprocessed-items-metric|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# cloudwatch__dynamodb__lambda: 2 net-new causes — a DynamoDB-stream event source mapping ignores MaximumRetryAttempts and retries a failing batch without bound, and DynamoDB never throttles a write burst (no UnprocessedItems, no ProvisionedThroughputExceededException, no ThrottledRequests)

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two independent root causes, both grounded in failing checks of this service. Each must be closed before submitting.

=== Cause 1 of 2: a DynamoDB Streams event source mapping ignores MaximumRetryAttempts, so a batch whose handler always throws is retried without bound and AWS/Lambda Errors climbs forever ===
AWS: Smithy: CreateEventSourceMapping.MaximumRetryAttempts — "(Kinesis, DynamoDB Streams, Amazon MSK, and self-managed Apache Kafka) Discard records after the specified number of retries. The default value is infinite (-1). When set to infinite (-1), failed records are retried until the record expires." With MaximumRetryAttempts=1 and BisectBatchOnFunctionError=false, the ESM invokes the function once, retries the failing batch exactly once, then discards it (no on-failure destination configured). That is exactly 2 failing invocations, so the function's AWS/Lambda Errors Sum rises to 2 and then stops changing.
Emulator: `AssertionError: Errors sum did not stabilize within 60.0s (last=18.0)` — the Errors sum was already 18 and still moving every time it was sampled, i.e. the mapping keeps re-invoking the always-failing batch and never discards it after the configured retry count.
Why: a client that bounds retries to protect a downstream (and to keep an always-poisonous record from blocking a shard) gets an unbounded retry storm instead: runaway invocations, runaway cost, and an Errors metric that can never be alarmed on with a fixed threshold. Poison-pill handling (discard-after-N, bisect, on-failure destination) cannot be exercised at all.
Fix: the stream-poller retry accounting in the Lambda event-source-mapping component — honour MaximumRetryAttempts (and BisectBatchOnFunctionError) per batch and discard the batch when the budget is exhausted; its state must carry the per-batch attempt count.
Traces: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception.

=== Cause 2 of 2: DynamoDB does not enforce provisioned write throughput — a 1-WCU table under sustained 25-item BatchWriteItem bursts returns neither UnprocessedItems nor ProvisionedThroughputExceededException, and no AWS/DynamoDB ThrottledRequests series appears ===
AWS: Smithy: BatchWriteItem.RequestItems is @length(1..25) and its output carries UnprocessedItems (the same map shape) for items the service declined to write; BatchWriteItem and PutItem both model ProvisionedThroughputExceededException (client, HTTP 400), whose message is AWS's fixed text "The level of configured provisioned throughput for the table was exceeded. Consider increasing your provisioning level with the UpdateTable API." (no variable parts). A table provisioned at 1 WCU/1 RCU starts with at most ~300 s of banked burst capacity, so rapid-fire 25-item batches (1000 WCU-worth per invocation) exceed it: AWS answers either with a partial result carrying UnprocessedItems or by refusing the call outright with ProvisionedThroughputExceededException, and publishes AWS/DynamoDB ThrottledRequests under (TableName, Operation=BatchWriteItem) — and, for the refused single writes, ProvisionedThroughputExceededException per attempt.
Emulator: the burst is always fully accepted. `AssertionError: expected at least one ProvisionedThroughputExceededException from the burst; actual total_throttled=0` for 80 back-to-back PutItems against a minimally-provisioned table; and the BatchWriteItem variant exhausts its budget (`Error: Check timed out after 60.0s`) while invoking up to 30 rounds of 40×25-item batches without ever being handed a single UnprocessedItem, so it never reaches its ThrottledRequests poll.
Why: throttling is a first-class part of DynamoDB's contract. A client that implements UnprocessedItems re-drive loops, exponential backoff on ProvisionedThroughputExceededException, or a ThrottledRequests alarm that drives capacity changes gets none of it exercised: the code path is dead against the emulator and fails only in production, and capacity planning signals read as permanently healthy.
Fix: provisioned-throughput accounting in the DynamoDB table write path (token/burst bank per table, used by PutItem/BatchWriteItem) — it must return UnprocessedItems or refuse with ProvisionedThroughputExceededException when the bank is exhausted — plus the AWS/DynamoDB ThrottledRequests publication for those refusals.
Traces: cloudwatch__dynamodb__lambda.lambda-ddb-throttle-triggers-cloudwatch-alarm, cloudwatch__dynamodb__lambda.lambda-ddb-batchwrite-unprocessed-items-metric.

## Evidence
Cause 1 — expected: CreateEventSourceMapping(EventSourceArn=<table stream>, BatchSize=5, StartingPosition=TRIM_HORIZON, MaximumRetryAttempts=1, BisectBatchOnFunctionError=false) over a handler that always raises; one PutItem into the table; AWS/Lambda Errors Sum for the function settles at exactly 2 (1 invocation + 1 retry, then the batch is discarded).
Actual: `[mismatch] cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception` → `AssertionError: Errors sum did not stabilize within 60.0s (last=18.0)` — 18 errors and still climbing, so the batch is never discarded.

Cause 2 — expected: against a table provisioned at 1 WCU, a loop of 80 PutItems with client retries disabled yields at least one 400 ProvisionedThroughputExceededException; and a Lambda issuing 40 rapid 25-item BatchWriteItem calls per invocation receives at least one UnprocessedItems entry, after which AWS/DynamoDB ThrottledRequests (TableName, Operation=BatchWriteItem) Sum is > 0.
Actual: `[mismatch] cloudwatch__dynamodb__lambda.lambda-ddb-throttle-triggers-cloudwatch-alarm` → `AssertionError: expected at least one ProvisionedThroughputExceededException from the burst; actual total_throttled=0`; `[mismatch] cloudwatch__dynamodb__lambda.lambda-ddb-batchwrite-unprocessed-items-metric` → `Error: Check timed out after 60.0s` with no UnprocessedItem ever returned.

## Steps to Reproduce
Cause 1:
1. CreateTable with StreamSpecification (NEW_AND_OLD_IMAGES); create a role with DynamoDB-stream read + basic execution; create a python function whose handler raises immediately.
2. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<stream arn>, BatchSize=5, Enabled=true, StartingPosition=TRIM_HORIZON, MaximumRetryAttempts=1, BisectBatchOnFunctionError=false); wait for State=Enabled.
3. PutItem one record into the table.
4. Poll GetMetricStatistics(AWS/Lambda, Errors, Dimensions=[FunctionName=<fn>], Statistics=[Sum]).
Observed: the sum keeps growing (18 and rising). Expected: it settles at 2.

Cause 2:
1. CreateTable with ProvisionedThroughput ReadCapacityUnits=1, WriteCapacityUnits=1.
2. From a Lambda with client retries disabled, issue 80 sequential PutItems of ~1 KB items.
Observed: all succeed, 0 throttles. Expected: at least one 400 ProvisionedThroughputExceededException ("The level of configured provisioned throughput for the table was exceeded. Consider increasing your provisioning level with the UpdateTable API.").
3. From the same table, issue 40 back-to-back 25-item BatchWriteItem calls per invocation, repeating up to 30 invocations.
Observed: UnprocessedItems is always empty. Expected: at least one UnprocessedItems entry, then AWS/DynamoDB ThrottledRequests Sum > 0 under (TableName, Operation=BatchWriteItem).
