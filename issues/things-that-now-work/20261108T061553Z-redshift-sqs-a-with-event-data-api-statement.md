Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cli-sqs-receive-redshift-completion-message-positive
Issue-Key: redshift__sqs.cli-sqs-receive-redshift-completion-message-positive|_run_shell#11|Timeout|return result
Issue-Commit: 89584cbdc793a0842735dd0ecc1a58bc9d0678bb

# redshift__sqs: a --with-event Data API statement publishes no 'Redshift Data Statement Status Change' event, so the CLI receive-message completion workflow never gets a message

> _Note: this issue was identified against the code base at commit `89584cbdc793a0842735dd0ecc1a58bc9d0678bb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Cause 1 of 1: a Redshift Data API statement submitted with event emission enabled runs to a terminal state but publishes nothing to EventBridge, so an EventBridge rule with an SQS target never delivers a completion message and any consumer waiting on that queue waits forever.

AWS: ExecuteStatement's WithEvent member is documented as "A value that indicates whether to send an event to the Amazon EventBridge event bus after the SQL statement runs." (Smithy: redshift-data ExecuteStatement, input member ExecuteStatementInput.WithEvent). With it set, each statement state change publishes an event on the default bus with source "aws.redshift-data" and detail-type "Redshift Data Statement Status Change", whose detail carries statementId (the id ExecuteStatement returned), statementName (the name supplied on the request), state (the statement's terminal status, FINISHED here) and a non-empty string principal; a rule matching that source/detail-type with an SQS target whose queue policy allows the rule ARN delivers the event to the queue within seconds, where `aws sqs receive-message` reads it. The envelope and detail member names are recorded behavior only (they are not in the Redshift Data Smithy model).

Emulator: the statement is accepted and DescribeStatement reaches FINISHED, but no event is ever published, so `aws sqs receive-message` never surfaces a message carrying that statementId. The check exhausts its 180 s delivery poll and is killed at the harness ceiling — "Error: Check timed out after 180.0s" — so none of the detail assertions (statementId, statementName, state == FINISHED, a non-empty principal) is ever reached. PutTargets reported FailedEntryCount 0 and the queue policy was set for the rule ARN, so the wiring the emulator accepted claims the delivery would happen.

Why: a client cannot observe statement completion asynchronously. Completion fan-out, failure alerting and dead-letter wiring built on the documented event are all unobservable, and a worker that blocks on ReceiveMessage for the completion event hangs instead of proceeding — the failure mode is a hang, not an error the client can handle.

Fix: the Redshift Data API statement lifecycle must publish the statement-status-change event (source, detail-type, detail.statementId/statementName/state/principal) when WithEvent is requested, and the EventBridge rule -> SQS target delivery path must carry it to the target queue.

Related open issue: 20261104T045922Z-redshift-sqs-no-redshift-data-statement-status-change.md records the same missing publication and names this check in its trace list, but it does not carry this failure's current replay key (the suite now reports it as a whole-check 180 s timeout rather than the inner poll timeout it was filed from), so this failure is not marked as owned by it. Fixing either must clear both; see also 20261029T082153Z-redshift-sqs-a-data-api-statement-submitted-with.md.

Traces: redshift__sqs.cli-sqs-receive-redshift-completion-message-positive.

## Evidence
Failing check (behavioral suite simparity-behavioral-aws-v1, service heading `### redshift__sqs`):

  [mismatch] redshift__sqs.cli-sqs-receive-redshift-completion-message-positive
  Error: Check timed out after 180.0s

Expected: after `aws redshift-data execute-statement --with-event` (statement name "sim2real.completion.check", SQL "SELECT 1") on an available cluster, with an EventBridge rule on source "aws.redshift-data" / detail-type "Redshift Data Statement Status Change", a queue policy allowing that rule's ARN and PutTargets of the queue ARN (FailedEntryCount 0), `aws sqs receive-message` returns an event whose detail.statementId equals the id ExecuteStatement returned, detail.statementName equals the submitted statement name, detail.state == "FINISHED" and detail.principal is a non-empty string.

Actual: DescribeStatement reaches FINISHED, but the queue stays empty for the whole 180 s poll and the check is terminated at the harness ceiling; no message with that statementId ever arrives, so the assertion "aws sqs receive-message never surfaced the completion event" and all following detail assertions are never evaluated. Cluster ids, queue/rule names and the statement id are per-run values; the requirement is the publication and delivery of the event with the envelope and detail members above.

## Steps to Reproduce
1. Create a Redshift cluster and wait for 'available'.
2. `aws events put-rule` for source "aws.redshift-data" / detail-type "Redshift Data Statement Status Change"; create an SQS queue, set its queue policy to allow the rule's ARN, and `aws events put-targets` with the queue ARN (FailedEntryCount comes back 0).
3. `aws redshift-data execute-statement --with-event --statement-name sim2real.completion.check --sql 'SELECT 1'` against that cluster and poll DescribeStatement until FINISHED.
4. Poll `aws sqs receive-message` on the queue for up to 180 s: no message whose detail.statementId equals the submitted statement id ever arrives.
5. Re-replay the check in trace_ids.
