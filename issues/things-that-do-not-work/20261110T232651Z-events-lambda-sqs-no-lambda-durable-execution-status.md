Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs|_await_rule_live#10|AssertionError|raise AssertionError(f"rule never delivered a warm-up {status} event for {fn_name!r} within 240s")
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# events__lambda__sqs: no Lambda durable-execution status-change event reaches EventBridge — the warm-up SUCCEEDED event is never delivered, so the timed-out status can never be graded

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause of service `events__lambda__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — a durable execution's status-change events are not published to EventBridge.
AWS: recorded behavior only (Lambda's durable-execution EventBridge integration is not modelled in Smithy). A durable-execution function invoked asynchronously publishes a status-change event on the default bus for each terminal status of the execution — `SUCCEEDED` for a completed execution and `TIMED_OUT` for one that exceeds its execution timeout — carrying the function name and the execution's status in its detail, so a rule matching the function and status delivers one envelope per execution to its target.
Emulator: nothing is delivered. The check's warm-up leg, which only needs the SUCCEEDED status-change event for a one-second durable execution, fails with `rule never delivered a warm-up SUCCEEDED event for '<function>' within 240s`; the TIMED_OUT assertion is therefore never reached.
Why: a client that observes durable executions through EventBridge (alerting on timed-out executions, reconciling long-running work) receives no events at all and cannot tell a succeeded execution from a timed-out one.
Fix: the Lambda durable-execution runtime's status-transition path must publish the status-change event to the EventBridge default bus, which the rule then matches. Open issues 20261101T210635Z-events-lambda-sqs-6-net-new-causes-eventbridge.md and 20261109T171600Z-lambda-eventbridge-4-net-new-causes-no-aws.md cover sibling durable-execution checks and do not carry this check's replay key.
Traces: events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs

## Evidence
Check events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs
AssertionError: rule never delivered a warm-up SUCCEEDED event for '<function-name>' within 240s
(The function name is a per-run value. The requirement is one status-change envelope per terminal execution status — SUCCEEDED for the warm-up execution, TIMED_OUT for the execution that exceeds its timeout — delivered to the matching rule's queue.)

## Steps to Reproduce
1. Boot the emulator and replay events__lambda__sqs.xsvc-lambda-1-durable-execution-timeout-emits-timed-out-status-change-to-sqs.
2. Create a durable-execution Python function whose handler waits for event['wait_seconds'] and optionally raises.
3. Create a rule matching that function's durable-execution status-change events, targeting an SQS queue whose policy grants events.amazonaws.com SendMessage.
4. Invoke asynchronously with {"wait_seconds": 1} and ReceiveMessage: expect one envelope reporting SUCCEEDED. Then invoke with a wait that exceeds the function's timeout: expect one envelope reporting TIMED_OUT. The emulator delivers neither.
