Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative
Issue-Key: lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative|run#30|Timeout|time.sleep(2)
Issue-Commit: 53bfbc08a5ec2983a62510ba6bf3546a998019d9

# lambda__pipes__sqs: the disabled/re-enabled SQS event-source-mapping delivery scenario never completes — the ESM lifecycle stalls past the budget

> _Note: this issue was identified against the code base at commit `53bfbc08a5ec2983a62510ba6bf3546a998019d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause.

Cause 1 of 1: an SQS event source mapping that is created, disabled, then re-enabled never completes its delivery cycle inside the check's budget, so neither the negative leg (a message sent while the mapping is Disabled is withheld and stays on the queue) nor the positive leg (the same mapping, re-enabled, delivers that message) is ever observable.

AWS: recorded behavior only for timing. On AWS, CreateEventSourceMapping on an SQS queue reaches State 'Enabled' and a warm-up message is delivered to the function within seconds; UpdateEventSourceMapping(Enabled=False) reaches 'Disabled' and a message sent while disabled is NOT delivered and remains on the source queue; UpdateEventSourceMapping(Enabled=True) returns the mapping to 'Enabled' and the withheld message is then delivered exactly as one Records batch. The whole sequence completes well inside a one-minute budget.

Emulator: the check is aborted with 'Check timed out after 60.0s' — the create/warm-up/disable/re-enable sequence does not converge, so the mapping's Enabled/Disabled gate on delivery is never exercised.

Why: a client relying on AWS's behavior cannot use the Enabled flag as a delivery gate: it gets neither a confirmed withhold while disabled nor a confirmed replay after re-enabling, so pausing a consumer is unverifiable and messages may be silently dropped or duplicated.

Fix: the SQS event-source-mapping poller and its state transitions — converge CreateEventSourceMapping/UpdateEventSourceMapping to Enabled/Disabled promptly and gate the poller on that state, so a disabled mapping leaves messages on the queue and a re-enabled one picks them up.

Related open issue 20261110T153718Z-lambda-s3-sqs-an-sqs-event-source-mapping-a933d9.md carries the same-named check under the lambda__s3__sqs service token and does not carry this check's replay key.

## Evidence
lambda__pipes__sqs.sqs-esm-disabled-mapping-no-invocation-negative [mismatch]:
Error: Check timed out after 60.0s

Scenario: source queue with VisibilityTimeout 30, function timeout 10, a forwarding handler that re-posts each record's body to a results queue, an SQS event source mapping created Enabled, then UpdateEventSourceMapping(Enabled=False), a message sent while disabled, then UpdateEventSourceMapping(Enabled=True). Expected: the warm-up message is delivered while enabled, the message sent while disabled is not delivered and stays on the source queue, and it is delivered once the mapping is re-enabled. Observed: the run never reaches those observations inside the budget.

## Steps to Reproduce
1. CreateQueue a source queue with VisibilityTimeout=30 and a results queue.
2. CreateFunction with timeout 10 whose handler forwards each record body to the results queue; CreateEventSourceMapping on the source queue.
3. Send a warm-up message and confirm delivery; UpdateEventSourceMapping(Enabled=False) and wait for State 'Disabled'.
4. Send a second message, confirm no invocation and that the message remains on the source queue; UpdateEventSourceMapping(Enabled=True) and confirm the withheld message is then delivered.
5. The sequence does not complete within 60 s.
