Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__ssm.eventbridge-rule-without-lambda-permission-blocks-invocation events__lambda__ssm.eventbridge-source-arn-account-extraction
Issue-Key: events__lambda__ssm.eventbridge-rule-without-lambda-permission-blocks-invocation|_poll_until#24|Timeout|time.sleep(interval_s)
Issue-Key: events__lambda__ssm.eventbridge-source-arn-account-extraction|_poll_until#26|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# EventBridge never invokes a Lambda target, so neither the missing-lambda-permission refusal nor the SourceArn account extraction is observable (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1 — a matched rule does not invoke its Lambda target, so the permission-gated and identity-derived behaviours around that invocation never occur.
AWS: EventBridge invokes a Lambda target only if the function's resource policy grants lambda:InvokeFunction to events.amazonaws.com; WITHOUT that permission the invocation is refused and the function never runs, and WITH it the function runs and sees the invoking rule's ARN, from which the rule's account can be extracted. Both halves are observed at the function's own side effect (here, an SSM parameter write), so an emulator that never invokes the function produces the same "no effect" as a correct permission refusal — and the positive control that distinguishes them never fires.
Emulator: the function is never invoked in either configuration. Both checks complete CreateFunction, PutRule, PutTargets (FailedEntryCount 0) and PutEvents (FailedEntryCount 0 with an EventId), then poll SSM for the parameter the function would write; it never appears and the replay cuts each check off at 60 s.
Why: the client gets no processing from a rule that reports healthy, and — worse for anyone testing against this emulator — a missing lambda:InvokeFunction permission looks exactly like a working configuration, because both produce nothing. The event's source-ARN/account provenance that handlers use for multi-account routing is never delivered either.
Fix: the Lambda target of the EventBridge target dispatcher in services/service-com-amazonaws-eventbridge-awsevents — the invocation itself, the lambda resource-policy check that must gate it, and the rule ARN it passes as the invocation's source. Related: 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the Invoke API stub; it does not carry these checks' replay keys and the dispatcher here never reaches Invoke.
Traces: events__lambda__ssm.eventbridge-rule-without-lambda-permission-blocks-invocation, events__lambda__ssm.eventbridge-source-arn-account-extraction.

## Evidence
Both checks report "Check timed out after 60.0s". Each had already received 2xx for CreateFunction, PutRule, PutTargets (FailedEntryCount 0) and PutEvents (FailedEntryCount 0 plus an EventId) — and, in the positive arm, for lambda AddPermission(Principal='events.amazonaws.com', SourceArn=<rule arn>). The terminal read, ssm GetParameter for the parameter the function writes, never succeeds. Expected: with the permission granted, the parameter written (carrying the rule ARN's account); without it, the parameter absent — but only after the positive arm has been shown to work.

## Steps to Reproduce
1. CreateFunction(fn) whose handler writes the invoking event's source ARN to an SSM parameter.
2. PutRule(Name=r, EventPattern='{"source":["x"]}'); PutTargets(Rule=r, Targets=[{Id:'t1', Arn:<fn arn>}]) — with NO lambda AddPermission.
3. PutEvents a matching event; poll ssm GetParameter -> absent (indistinguishable from non-delivery).
4. Add lambda AddPermission(FunctionName=fn, Principal='events.amazonaws.com', Action='lambda:InvokeFunction', SourceArn=<rule arn>) and PutEvents again -> the parameter is still never written.
