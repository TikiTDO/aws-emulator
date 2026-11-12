Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__iam__lambda__sqs.eventbridge-rule-no-permission-to-invoke-lambda
Issue-Key: events__iam__lambda__sqs.eventbridge-rule-no-permission-to-invoke-lambda|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# An EventBridge rule with no lambda:InvokeFunction grant produces no observable outcome: the Lambda target path is inert

> _Note: this issue was identified against the code base at commit `b1cd28f1ca277d6a032df605f83af5017e934043`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: the negative leg cannot be graded because the positive control — an EventBridge rule invoking a Lambda target that does have the grant — never fires.
AWS: EventBridge invokes a Lambda target only if the function's resource policy has a lambda:InvokeFunction statement for the service principal events.amazonaws.com with the rule ARN as SourceArn. Without it the invocation is refused with AccessDeniedException, the function does not run, and EventBridge dead-letters the envelope (if a DeadLetterConfig is set) and increments FailedInvocations. With the grant added by AddPermission, the next matching event invokes the function.
Emulator: neither leg happens — the function is never invoked with or without the grant, and no dead-letter or failure signal appears. The check ends "Check timed out after 60.0s".
Why: a client cannot distinguish "correctly refused for lack of permission" from "EventBridge never invokes anything", so an IAM regression in either direction is invisible; and the AddPermission repair leg cannot be confirmed.
Fix: the EventBridge Lambda-target invoker must call the function, evaluate the function's resource policy before invoking, and route the refusal to the dead-letter/FailedInvocations path.
Traces: events__iam__lambda__sqs.eventbridge-rule-no-permission-to-invoke-lambda

## Evidence
"Error: Check timed out after 60.0s" — after a matching PutEvents, the Lambda target is not invoked in either the granted or the ungranted state, and no dead-letter message or FailedInvocations datapoint appears. Expected: no invocation plus an AccessDeniedException-class dead-letter/failure in the ungranted state, and exactly one invocation once AddPermission grants lambda:InvokeFunction to events.amazonaws.com.

## Steps to Reproduce
CreateFunction, PutRule, PutTargets with the function ARN and no AddPermission; PutEvents a matching entry; observe the function's side effect and the rule's DLQ/FailedInvocations — both silent. AddPermission for events.amazonaws.com with SourceArn = the rule ARN and PutEvents again — still nothing.
