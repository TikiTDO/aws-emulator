Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations
Issue-Key: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations|run#14|Timeout|for m in _drain_json_messages(ctx, results["url"], wait_s=3):
Issue-Commit: 084fe38e94c6ae3d12f6c6daafcb56ce742ec067

# lambda__pipes__sqs: an SQS event source mapping with ScalingConfig.MaximumConcurrency never drains its backlog, so the concurrency cap is never observable

> _Note: this issue was identified against the code base at commit `084fe38e94c6ae3d12f6c6daafcb56ce742ec067`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause of service `lambda__pipes__sqs`, grounded in the one failing behavioral check of this run that the replay report did NOT mark ALREADY FILED or OWNED.

Related open issues record the same theme but do not carry this run's replay key for this check, so the report left the failure unowned and it is re-filed here against the current checkout: 20261111T093805Z-lambda-pipes-sqs-an-sqs-event-source-mapping.md (key `...|run#14|Timeout|time.sleep(1)`) and 20261110T232651Z-lambda-pipes-sqs-the-sqs-event-source-mapping.md (key is a timeout inside _drain_json_messages). Neither covers this run's failure key. 20261109T174004Z-lambda-pipes-sqs-2-net-new-causes-a-18a916.md and 20261104T213346Z-lambda-pipes-sqs-5-net-new-causes-an.md cover other lambda__pipes__sqs causes (pipe enrichment, plain ESM invocation, CFN wiring) and do not name this check.

Cause 1 of 1: an SQS event source mapping created with ScalingConfig.MaximumConcurrency never processes its queued backlog to completion, so neither the concurrency bound nor the drain is observable.

AWS:      Smithy (com.amazonaws.lambda) models CreateEventSourceMapping with EventSourceArn, FunctionName, BatchSize, Enabled and ScalingConfig (member MaximumConcurrency) for an SQS source, and GetEventSourceMapping reporting State. The polling semantics themselves are recorded behavior only: an Enabled mapping on an SQS queue invokes the function once per batch and deletes the batch on success, and with ScalingConfig.MaximumConcurrency = N the poller keeps AT MOST N invocations in flight while still scaling UP to N whenever a backlog allows. So a backlog of M messages whose handler takes T seconds drains in roughly ceil(M/N) waves, the per-invocation [start,end] windows overlap exactly N-deep at the peak, the wall clock is at least (M/N - 1) * T, and every message is delivered exactly once. No error is involved: every call in the scenario succeeds (HTTP 200) on the recording.

Emulator: the check never reaches its comparison — `Error: Check timed out after 60.0s`. The mapping is created and reports State=Enabled, the 8-message backlog is accepted by SendMessage, but the sleeping handler's per-invocation result records never accumulate to 8 within the budget, so peak overlap, per-message delivery and the elapsed-time bound are all ungraded. A cap-honouring run of this workload (8 messages, MaximumConcurrency=2, a 5s handler) spends about 20s in processing and finishes well inside the window, so the timeout means the capped poller either never invokes or progresses far slower than one wave per handler duration.

Why:      a client that sets MaximumConcurrency to protect a downstream dependency (a connection pool, a rate-limited API) gets a mapping whose backlog is never drained in any usable time. The setting can be neither honoured nor verified, and because nothing completes a client cannot distinguish a throttled-but-working mapping from a stalled one — every ScalingConfig-based design silently produces no work.

Fix:      the SQS event-source-mapping poller/scheduler in the lambda service — the scheduling loop must keep up to ScalingConfig.MaximumConcurrency invocations in flight concurrently, scale UP to that cap while a backlog waits (not merely stay under it), invoke once per batch, and delete each batch from the source queue on success.

Traces:   lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), this run: Total 135/171 passed (78.9%); Mismatched 31, Errored 5, Skipped 0, Unloadable 0. No bootstrap signal — lambda__pipes__sqs 31/41 (76%), and inside this very check SQS CreateQueue/SendMessage/ReceiveMessage, IAM CreateRole/AttachRolePolicy/PutRolePolicy and Lambda CreateFunction/CreateEventSourceMapping/GetEventSourceMapping all answered (the mapping reached State=Enabled).

Failing check: [mismatch] lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations
  Error: Check timed out after 60.0s

What the check does and expects: it creates a source queue (VisibilityTimeout=90) and a results queue, a python3.12 function whose handler sleeps SLEEP_SECONDS (5) and then reports {bodies, start, end} to the results queue, and an ESM with BatchSize=1, Enabled=True and ScalingConfig={'MaximumConcurrency': 2}; it sends 8 messages and collects 8 result records.
Expected: all 8 invocations complete; the peak number of overlapping [start,end] windows equals 2 (never above the cap, and not below it while a backlog waits); wall clock >= (8/2 - 1) * 5s = 15s.
Actual: fewer than 8 result records ever arrive and the check is cut off at the 60s harness cap, so no overlap, no per-message delivery and no drain time can be measured.

The message bodies, queue/function/role names, ESM UUID and the timestamps in this check are per-run values and are not part of the requirement; the requirement is that a MaximumConcurrency=N mapping drains an M-message backlog with a peak overlap of exactly N and a wall clock of at least (M/N - 1) * T.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. CreateQueue a source queue (Attributes={'VisibilityTimeout':'90'}) and a results queue.
3. CreateRole for lambda.amazonaws.com with the basic-execution and SQS-queue-execution managed policies plus an inline sqs:SendMessage grant on the results queue.
4. CreateFunction (Runtime=python3.12, Timeout=30) whose handler records time.time() before and after a 5-second sleep and sends {bodies, start, end} to the results queue; wait for the function to be Active.
5. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<source queue arn>, BatchSize=1, Enabled=True, ScalingConfig={'MaximumConcurrency': 2}) and poll GetEventSourceMapping until State=Enabled.
6. SendMessage 8 distinct bodies to the source queue, then poll ReceiveMessage on the results queue until 8 records are collected.
7. Observe the 8 records never accumulate and the check is killed at the 60s harness cap. With a cap-honouring poller the workload completes in roughly 20s of processing; then compute the peak overlap of the collected [start,end] windows and the total elapsed time and compare against MaximumConcurrency=2.
8. After fixing, re-replay lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations.
