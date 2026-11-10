Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery
Issue-Key: dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: 2d0b651e448fe6a44fa56d00833f67c02b248ecb

# dynamodb__lambda__sqs: an SQS queue policy denying sqs:ReceiveMessage never changes event-source-mapping delivery — the check hangs to the 60 s budget

> _Note: this issue was identified against the code base at commit `2d0b651e448fe6a44fa56d00833f67c02b248ecb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetQueueAttributes (Policy) / Lambda event source mapping polling

## Description
Cause 1 of 1: the queue-policy authorization of an SQS event source mapping is never observable — neither the suppression nor the recovery leg converges, and the check is cut off at the harness's 60 s budget. Failing check: dynamodb__lambda__sqs.queue-policy-deny-suppresses-esm-delivery.

AWS: Smithy: SetQueueAttributes carries the Policy attribute; Lambda's CreateEventSourceMapping polls the queue with the function's execution role. Recorded behavior: an explicit Deny of sqs:ReceiveMessage with Principal "*" in the queue policy silently stops delivery on an otherwise working mapping — the message stays queued (ApproximateNumberOfMessages + ...NotVisible stays non-zero), the mapping keeps reporting State Enabled and no refusal is surfaced to the caller; a Deny aimed only at the execution role does NOT stop delivery; removing the policy delivers that same message.

Emulator: the check reaches no verdict at all — "Error: Check timed out after 60.0s". Neither the suppressed leg (message withheld while Enabled) nor the control legs (role-scoped Deny still delivering, policy removal releasing the message) settle inside the budget.

Why: a client that uses a queue policy as the authorization boundary for its ESM cannot tell whether the policy is enforced at all: it observes neither suppression nor delivery, so a Deny it believes is protecting the queue may be inert and a Deny it removed may never release the backlog.

Fix: the Lambda event-source-mapping poller's SQS authorization path — evaluate the queue's resource policy (and the execution role) on every receive, withhold the invocation without changing the mapping state, and resume once the policy is removed.

Traces: the failing check id above.

## Evidence
Expected (AWS): with the Principal "*" Deny of sqs:ReceiveMessage in place, the message stays queued and the mapping stays Enabled; with a Deny aimed at the execution role, delivery continues; with the policy removed, the same message is delivered to the function (which records it in DynamoDB).
Actual (emulator): Error: Check timed out after 60.0s — none of the three legs reaches a verdict.

## Steps to Reproduce
1. Create a DynamoDB table, an SQS queue and a Lambda with an enabled SQS event source mapping that records each message.
2. Confirm a baseline message is delivered.
3. SetQueueAttributes with a Policy containing an explicit Deny of sqs:ReceiveMessage for Principal "*"; send a message; poll the queue depth and the table.
4. Replace it with a Deny scoped to the execution role, then remove the policy entirely; observe no leg ever converges before the 60 s cap.
