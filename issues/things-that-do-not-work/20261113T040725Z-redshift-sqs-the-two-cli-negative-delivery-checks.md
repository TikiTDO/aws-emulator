Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cli-eventbridge-put-rule-wrong-target-arn-negative redshift__sqs.cli-sqs-purge-queue-then-no-stale-redshift-event-negative
Issue-Key: redshift__sqs.cli-eventbridge-put-rule-wrong-target-arn-negative|_poll_for_statement_message#20|Timeout|time.sleep(interval_s)
Issue-Key: redshift__sqs.cli-sqs-purge-queue-then-no-stale-redshift-event-negative|run#15|Timeout|time.sleep(min(5.0, settle_deadline - time.monotonic()))
Issue-Commit: 39c6dbb2798d7d5c6447151f9d119a36c56109b9

# redshift__sqs: the two CLI negative-delivery checks still stall — a Redshift Data statement run with --with-event publishes no 'Redshift Data Statement Status Change' event, so their positive control never lands (1 cause)

> _Note: this issue was identified against the code base at commit `39c6dbb2798d7d5c6447151f9d119a36c56109b9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Cause 1 of 1: a Redshift Data API statement submitted with event emission enabled publishes no EventBridge event, so an EventBridge rule whose SQS target is correctly wired and correctly policied never delivers anything, and both CLI negative-delivery checks are killed at the harness ceiling while waiting for their positive control.

AWS: recorded behavior only (the event envelope is not part of the Redshift/Redshift Data Smithy model). `redshift-data execute-statement --with-event` makes the service publish an event on each statement state change with source 'aws.redshift-data', detail-type 'Redshift Data Statement Status Change', a detail carrying statementId and state (FINISHED, or FAILED with the engine's error text) and a resources entry naming the cluster ('cluster:<cluster-id>'). A rule matching that source/detail-type with an SQS target delivers the event to the queue within seconds; EventBridge's PutTargets does not validate that a same-account target queue exists (FailedEntryCount 0), and a target ARN naming a nonexistent queue silently delivers nowhere; SQS PurgeQueue removes only what was already enqueued, within its documented 60-second window.

Emulator: the statement is accepted and reaches FINISHED, but no statement-status-change event is ever published, so the SQS receive poll never returns a message. Both checks therefore never reach their actual assertions — each spends its positive-control poll (180 s in one, the stale-event setup poll in the other) and the replay reports 'Error: Check timed out after 60.0s'. Other checks in the same run create clusters and run Data API statements to completion against this emulator, so the stall is the absent event publication/delivery, not cluster provisioning or statement execution.

Why: no client can observe statement completion asynchronously — completion fan-out and failure alerting are unobservable and a consumer waiting on the queue hangs indefinitely. It also makes the two negative claims untestable: whether a mis-typed target ARN leaks the event to the queue it was meant to name, and whether a purged stale event stays gone, can never be decided because the control delivery that must precede the absence assertion never happens.

Fix: the Redshift Data API statement lifecycle must publish the statement-status-change event to EventBridge (source 'aws.redshift-data', detail-type 'Redshift Data Statement Status Change', detail.statementId/state, resources naming the cluster) when the statement requests event emission, and the EventBridge->SQS target delivery path must carry it to the target queue.

Note: open issue 20261104T045922Z-redshift-sqs-no-redshift-data-statement-status-change.md records the same missing publication and names these two checks in its prose, but it does not carry their current replay keys (they now fail as a bare harness-level 'Check timed out after 60.0s' rather than inside _poll_for_statement_message), so they are filed here; related open issues 20261029T082153Z-redshift-sqs-a-data-api-statement-submitted-with.md and 20261029T193114Z-redshift-sqs-3-net-new-causes-eventbridge-sqs.md cover the same absence from other recorded units.

Traces: redshift__sqs.cli-eventbridge-put-rule-wrong-target-arn-negative, redshift__sqs.cli-sqs-purge-queue-then-no-stale-redshift-event-negative.

## Evidence
Replay of simparity-behavioral-aws-v1 (redshift__sqs 37/40 passed).

[mismatch] redshift__sqs.cli-eventbridge-put-rule-wrong-target-arn-negative — 'Error: Check timed out after 60.0s'. The check creates a cluster, PutRule for the Redshift Data statement-status-change pattern, two queues (one correctly policied for the rule, one 'intended' queue whose target ARN is mutated to a nonexistent queue name), PutTargets (FailedEntryCount asserted 0 — this passes), executes 'SELECT 1', waits for FINISHED, then polls the correctly-wired queue with timeout_s=180.0 and asserts 'positive control: correctly-wired target never got the event' before it can assert the mis-wired queue stayed empty. The poll never returns a message, so the harness kills the check.

[mismatch] redshift__sqs.cli-sqs-purge-queue-then-no-stale-redshift-event-negative — 'Error: Check timed out after 60.0s'. Same setup; its first step polls for the stale statement's event with timeout_s=180.0 and asserts 'setup failed: the stale event never reached the queue'. Nothing arrives, so neither the purge-then-absence assertions nor the fresh-event positive control is ever reached.

Statement ids, queue/rule names and cluster ids are per-run values; the requirement is that the event be published and delivered with the envelope and detail described above.

## Steps to Reproduce
1. Create a Redshift cluster and wait for 'available'.
2. `aws events put-rule` matching source 'aws.redshift-data' / detail-type 'Redshift Data Statement Status Change'; create an SQS queue and set its queue policy to allow events.amazonaws.com sqs:SendMessage for that rule ARN; `aws events put-targets` with the queue ARN (FailedEntryCount is 0).
3. `aws redshift-data execute-statement --with-event` with 'SELECT 1' on that cluster; wait until DescribeStatement reports FINISHED.
4. Poll `aws sqs receive-message` on the queue for a message whose detail.statementId equals the submitted statement id: no message ever arrives (poll 180 s).
5. Repeat with a second target whose ARN names a nonexistent queue in the same account (to see that the control delivery, not the mis-wired one, is what is missing), and with a PurgeQueue between a stale and a fresh statement.
6. Re-replay the checks listed in trace_ids.
