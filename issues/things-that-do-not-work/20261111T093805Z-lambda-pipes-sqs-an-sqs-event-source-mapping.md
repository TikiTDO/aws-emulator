Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations
Issue-Key: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations|run#14|Timeout|time.sleep(1)
Issue-Commit: 34a59b28db3b6549e829977bc135282939da6978

# lambda__pipes__sqs: an SQS event source mapping with ScalingConfig.MaximumConcurrency never drains its backlog — the concurrency-capped invocations never complete within the check's budget

> _Note: this issue was identified against the code base at commit `34a59b28db3b6549e829977bc135282939da6978`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause of service `lambda__pipes__sqs`, grounded in one failing behavioral check that the replay report did NOT mark ALREADY FILED or OWNED.

The open issue 20261110T232651Z-lambda-pipes-sqs-the-sqs-event-source-mapping.md records the same theme but does not carry this run's replay key for the check (its key is a timeout inside _drain_json_messages; this run's failure is the whole-check timeout), so the report left this failure unowned and it is re-filed here against the current code base. Open issue 20261104T213346Z-lambda-pipes-sqs-5-net-new-causes-an.md covers other lambda__pipes__sqs causes (plain ESM invocation, pipe delivery, CFN wiring) and does not name this check.

Cause 1 of 1: an SQS event source mapping created with ScalingConfig.MaximumConcurrency never processes the queued backlog to completion, so neither the concurrency bound nor the drain is observable.
AWS:      Smithy (com.amazonaws.lambda) models CreateEventSourceMapping with EventSourceArn, FunctionName, BatchSize, Enabled and ScalingConfig (MaximumConcurrency) for an SQS source, and GetEventSourceMapping reporting State. The polling itself is recorded behavior only: an Enabled mapping on an SQS queue invokes the function once per batch and deletes the batch on success, and with ScalingConfig.MaximumConcurrency = N the poller runs AT MOST N invocations concurrently while still scaling UP to N whenever the backlog allows — so a backlog of M messages whose handler takes T seconds is drained in roughly ceil(M/N) waves, the per-invocation start/end windows overlap exactly N-deep at the peak, and every message is delivered exactly once.
Emulator: the check never reaches its comparison: `Error: Check timed out after 60.0s`. The mapping is created (it reports State=Enabled) and the 8-message backlog is sent, but the sleeping handler's per-invocation result records never accumulate to 8 within the budget, so the peak overlap, the per-message delivery and the elapsed-time bound are all ungraded. A working, cap-honouring run of this workload (8 messages, MaximumConcurrency=2, a 5s handler) spends about 20s in processing and completes well inside the harness window, so the timeout means the capped poller either never invokes or progresses far slower than one wave per handler duration.
Why:      a client that sets MaximumConcurrency to protect a downstream dependency (a database connection pool, a rate-limited API) gets a mapping whose backlog is not drained in any usable time. The setting can be neither honoured nor verified, and because nothing completes, a client cannot distinguish a throttled-but-working mapping from a stalled one.
Fix:      the SQS event-source-mapping poller/scheduler in the lambda service — the scheduling loop must keep up to ScalingConfig.MaximumConcurrency invocations in flight, drain the queue in waves, and delete each batch on success (it must also scale UP to the cap, not merely stay under it).
Traces:   lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1), this run: Total 134/170 passed (78.8%); Mismatched 31, Errored 5, Skipped 0, Unloadable 0. No bootstrap signal — lambda__pipes__sqs 31/41 passed and in this very check SQS CreateQueue/SendMessage/ReceiveMessage, IAM CreateRole/AttachRolePolicy/PutRolePolicy and Lambda CreateFunction/CreateEventSourceMapping/GetEventSourceMapping all answered (the mapping reached State=Enabled).

Failing check: [mismatch] lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations
  Error: Check timed out after 60.0s

What the check does and what it expected: it creates a source queue (VisibilityTimeout=90) and a results queue, a function whose handler sleeps SLEEP_SECONDS and then reports {bodies, start, end} to the results queue, and an ESM with BatchSize=1 and ScalingConfig={'MaximumConcurrency': 2}; it sends 8 messages and collects the 8 result records. Expected: all 8 invocations complete, the peak number of overlapping [start,end] windows equals 2 (never above the cap, and not below it while a backlog waits), and the wall-clock is at least (8/2 - 1) * 5s — i.e. the cap is visible in both the overlap and the elapsed time. Actual: fewer than 8 result records ever arrive and the check is cut off at the 60s harness cap, so no overlap or drain can be measured.

The message bodies, queue/function/role names, UUIDs and timestamps in this check are per-run values and are not part of the requirement; the requirement is that a MaximumConcurrency=N mapping drains an M-message backlog with a peak overlap of exactly N.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and replay the behavioral suite (run_behavioral_suite).
2. CreateQueue a source queue (Attributes={'VisibilityTimeout':'90'}) and a results queue.
3. CreateRole for lambda.amazonaws.com with the basic-execution and SQS-queue-execution managed policies plus sqs:SendMessage on the results queue.
4. CreateFunction (python3.12) whose handler records time.time() before and after a 5-second sleep and sends {bodies, start, end} to the results queue, with Timeout=30; wait for the function to be Active.
5. CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<source queue arn>, BatchSize=1, Enabled=True, ScalingConfig={'MaximumConcurrency': 2}) and poll GetEventSourceMapping until State=Enabled.
6. SendMessage 8 distinct bodies to the source queue, then poll ReceiveMessage on the results queue until 8 records are collected.
7. Observe that the 8 records never accumulate (the check is killed at the 60s harness cap). With a cap-honouring poller the workload completes in roughly 20s of processing; compute the peak overlap of the collected [start,end] windows and the total elapsed time and compare against MaximumConcurrency=2.
8. After fixing, re-replay lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations.
