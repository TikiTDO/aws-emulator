Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations
Issue-Key: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations|_drain_json_messages#14|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=max_messages, WaitTimeSeconds=wait_s)
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# lambda__pipes__sqs: the SQS event-source-mapping MaximumConcurrency check never reaches a verdict — the ESM's concurrency-limited delivery does not progress within the check's budget

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause of service `lambda__pipes__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — an SQS event-source mapping with a ScalingConfig MaximumConcurrency never produces the overlapping-invocation evidence the check grades, and the check burns its whole budget.
AWS: Smithy (lambda): `CreateEventSourceMapping` accepts `ScalingConfig.MaximumConcurrency` for an SQS source. Recorded behavior: the poller drains the queue with at most MaximumConcurrency concurrent invocations of the function — a backlog is processed in waves whose start/end timestamps show no more than that many overlapping executions, and every message is eventually delivered exactly once to a handler.
Emulator: the check never reaches its comparison — `Error: Check timed out after 60.0s`. The sleeping handler's result records (which report each invocation's start and end) never accumulate, so neither the concurrency bound nor the drain can be graded.
Why: a client relying on MaximumConcurrency to protect a downstream dependency gets no observable behaviour at all — the backlog is not drained within any usable time, so the setting can be neither honoured nor verified.
Fix: the SQS event-source-mapping poller in the lambda crate — its scheduling loop must drain the queue and invoke the function under the configured ScalingConfig.MaximumConcurrency. Open issue 20261104T213346Z-lambda-pipes-sqs-5-net-new-causes-an.md covers other lambda__pipes__sqs causes and does not carry this check's replay key.
Traces: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations

## Evidence
Check lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations
Error: Check timed out after 60.0s — the handler's per-invocation start/end records never arrived on the results queue, so the number of overlapping invocations and the drain of the source queue could not be observed.
Expected: with ScalingConfig.MaximumConcurrency set, the backlog is drained and at no point do more than MaximumConcurrency invocations overlap.

## Steps to Reproduce
1. Boot the emulator and replay lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations.
2. Create a source queue, a results queue and a function whose handler sleeps SLEEP_SECONDS and reports its start/end plus the bodies it received to the results queue.
3. CreateEventSourceMapping(EventSourceArn=<source queue>, FunctionName=<fn>, BatchSize=1, ScalingConfig={'MaximumConcurrency': N}).
4. Send a backlog of messages and read the results queue: expect every message processed and no more than N overlapping invocation windows.
