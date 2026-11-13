Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters
Issue-Key: iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters|run#28|AttributeError|received = [json.loads(ctrl["Body"])["event"].get("marker")]
Issue-Commit: 455edd8297574e8c9cd5843518c18f9edb691c55

# scheduler universal target aws-sdk:lambda:invoke delivers Input.Payload to the function as a JSON STRING, so the handler's event is a str instead of the payload object

> _Note: this issue was identified against the code base at commit `455edd8297574e8c9cd5843518c18f9edb691c55`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence-dispatch path for UNIVERSAL targets (and/or the invoke body it hands the Lambda runtime).

Cause 1 of 1: a schedule whose Target.Arn is `arn:aws:scheduler:::aws-sdk:lambda:invoke` now DOES invoke the named function (the invocation is observable), but the payload the function receives is the JSON TEXT of Target.Input's `Payload` member rather than the JSON value it encodes, so a Python handler's `event` is a `str` and not the `dict` AWS passes.

AWS: Smithy (com.amazonaws.scheduler) documents Target.Arn (required, @length 1..1600) and Target.Input as "The text, or well-formed JSON, passed to the target"; for a universal target the Input IS the request body of the named SDK operation. Smithy (com.amazonaws.lambda) Invoke.Payload is a blob (`@httpPayload`) — the bytes of that member are the invocation body, byte for byte. Recorded behavior: with Target.Input `{"FunctionName": <fn>, "InvocationType": "Event", "Payload": "{\"marker\": \"<value>\"}"}`, Scheduler calls lambda:Invoke with that payload string's bytes as the body, and the managed Python runtime deserializes that body into the handler's `event`, so `event` is the object `{"marker": "<value>"}` and `event["marker"]` is the value from the request. Nothing in either model makes the body a quoted JSON string.

Emulator: the function runs, but what it is handed is a string. The sink handler, which forwards `json.dumps({"event": event})`, produces a body whose `event` member is a str, and the check dies with `AttributeError: 'str' object has no attribute 'get'` on `json.loads(ctrl["Body"])["event"].get("marker")`. The marker a client sent can therefore never be read as a field; every handler that indexes its event (`event["key"]`) raises instead of running.

Why: a client relying on AWS's behavior writes a handler that reads fields off `event`. Against the emulator that handler fails on its first field access — or, worse, silently treats the whole payload as an opaque string — so no universal-target Lambda workload behaves as it does on AWS, and the negative arms the same check exists for (a FunctionName/Qualifier mismatch and a whitespace-only FunctionName must dead-letter with ERROR_CODE InvalidParameterValueException / ValidationException and AWS's exact ERROR_MESSAGE, plus SCHEDULE_ARN) can never be reached.

Fix: the universal-target dispatch in the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/, the occurrence/dispatch component that parses `arn:aws:scheduler:::aws-sdk:<service>:<operation>` and renders Target.Input into that operation's request). Hypothesis (neither the model nor the recording states which end): either that dispatch re-encodes the `Payload` member (passing the JSON text as a JSON string value, double-encoding it) instead of passing its bytes as the Invoke body, or the in-process Lambda runtime does not deserialize the invocation body before handing it to the handler; the fix belongs wherever the body is composed, and the invoke body must be the payload's bytes as sent.

Traces: iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters (fails at the positive control, before the two dead-letter arms). This is NOT covered by the open report 20261109T141116Z-iam-lambda-logs-scheduler-sqs-scheduler-universal-target-arn-aws-scheduler-aws-sdk.md: that report's cause is that the universal target NEVER invokes the function (its replay key is the `_await_message` timeout), and the invocation now arrives — this is the next divergence on the same check, and the suite report lists it unowned.

## Evidence
Failing check: [error] iam__lambda__logs__scheduler__sqs.scheduler-lambda-universal-target-invalid-input-dead-letters

Replay detail (end):
  File "check_iam__lambda__logs__scheduler__sqs_scheduler_lambda_universal_target_invalid_input_dead_letters.py", line 114, in run
    received = [json.loads(ctrl["Body"])["event"].get("marker")]
  AttributeError: 'str' object has no attribute 'get'

Setup (from the check): Target={'Arn': 'arn:aws:scheduler:::aws-sdk:lambda:invoke', 'RoleArn': <scheduler role>, 'Input': json.dumps({'FunctionName': <fn arn>, 'InvocationType': 'Event', 'Payload': json.dumps({'marker': <control marker>})}), 'RetryPolicy': {'MaximumRetryAttempts': 0, 'MaximumEventAgeInSeconds': 60}, 'DeadLetterConfig': {...}}. The function forwards json.dumps({'event': event}) to an SQS sink.

Expected (AWS): the sink message body parses to {'event': {'marker': <control marker>}} — event is the payload OBJECT.
Actual (emulator): the body's 'event' member is a string (the JSON text of the payload), so indexing it as an object raises AttributeError.

Unreached arms of the same check, still unverified because of this: the qualifier-mismatch target must dead-letter with ERROR_CODE 'InvalidParameterValueException' and ERROR_MESSAGE 'The derived qualifier from the function name does not match the specified qualifier.'; the whitespace FunctionName target must dead-letter with ERROR_CODE 'ValidationException' and the modeled pattern-violation message; both must carry SCHEDULE_ARN of their own schedule, and the correctly-formed sibling must NOT be dead-lettered.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an SQS sink queue and a Lambda function whose handler forwards json.dumps({'event': event}) to that queue.
3. Create a role trusting scheduler.amazonaws.com with lambda:InvokeFunction and sqs:SendMessage.
4. scheduler.create_schedule(ScheduleExpression='at(<~75s from now>)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn':'arn:aws:scheduler:::aws-sdk:lambda:invoke','RoleArn':<role>,'Input': json.dumps({'FunctionName': <fn arn>, 'InvocationType':'Event','Payload': json.dumps({'marker':'m1'})})}).
5. Receive the sink message after the occurrence and json.loads its Body: expect Body['event'] == {'marker': 'm1'} (a dict); observe a string.
6. Re-replay the check id in trace_ids.
