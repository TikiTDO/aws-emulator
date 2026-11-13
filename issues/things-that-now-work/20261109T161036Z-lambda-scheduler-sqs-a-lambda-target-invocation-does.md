Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty
Issue-Key: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty|run#21|AssertionError|assert record["event"] == payload, (
Issue-Commit: 58511c6cfb9f29f34cba9f5bd79b625dce957a70

# lambda__scheduler__sqs: a Lambda-target invocation does not carry the invoking schedule's own Target.Input — the function receives another schedule's payload

> _Note: this issue was identified against the code base at commit `58511c6cfb9f29f34cba9f5bd79b625dce957a70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
One root cause, grounded in the failing check below. The check is not marked ALREADY FILED or OWNED in the replay report. Related but NOT covering: open issue 20261109T141116Z-lambda-scheduler-sqs-scheduler-never-dead-letters-a-failed-lambda-target.md (DeadLetterConfig never receives a denied or deleted-target invocation) and open issue 20261109T141116Z-lambda-scheduler-scheduler-with-a-lambda-target-5-causes-target.md (Target.RoleArn never authorized at invocation time) — both describe delivery-failure handling and neither carries this check id or the Input-binding divergence below.

=== Cause 1 of 1: the payload delivered to a Lambda target is not the invoking schedule's Target.Input ===
Failing check: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty
AWS: Smithy (com.amazonaws.scheduler) — Target is required on CreateSchedule/UpdateSchedule and Target.Input (TargetInput, @length min 1) is "The text, or well-formed JSON, passed to the target"; for a Lambda target the Input must be well-formed JSON and is handed to the function as the event. Target.Input belongs to ONE schedule record: every occurrence of that schedule invokes its own Target.Arn with its own Input, byte-for-byte as submitted (GetSchedule echoes it unchanged), parsed by the Lambda runtime into an object when it is a JSON object — including nested objects and arrays, mixed number types, booleans, null, non-ASCII text and embedded quotes. No schedule's Input is ever observable at another schedule's target, and a schedule whose Target.RoleArn lacks lambda:InvokeFunction never invokes the function at all (its event goes to its own DeadLetterConfig).
Emulator: the function, invoked once, receives the payload of the OTHER schedule in the account — the twin created with Input {"marker": "twin-<id>"} whose role grants only sqs:SendMessage — while the schedule under test, whose Input is the full JSON document, never gets its own Input delivered. GetSchedule echoed the correct Input for the schedule under test (that assertion passed), so the divergence is in the dispatch path that binds an occurrence to a target payload, not in the control-plane store. Hypothesis: the dispatcher resolves the payload per target ARN (both schedules point at the same function ARN) instead of per schedule record, so whichever record it reaches first supplies the Input for the invocation; neither the model nor the recording states this, so it is a hypothesis about the cause.
Why: Target.Input is the entire contract between a schedule and its target. A client that runs several schedules against one function — the normal fan-in shape, one function with per-schedule parameters — has its handler executed with another schedule's parameters: the wrong tenant, the wrong report window, the wrong account id, with no error anywhere and the control plane still echoing each schedule's own Input. It is a silent cross-schedule payload leak, and it also makes the denied twin's event appear at the target AWS would never have invoked. Every downstream assertion of the check (the handler receiving a dict, the healthy DLQ staying empty, the twin's dead-letter carrying the twin's Input) is unreachable behind it.
Fix: the invocation path of the dispatch component in services/service-com-amazonaws-scheduler-awschronosservice/src/ must carry the Input of the schedule record that owns the occurrence through to the target invocation — keyed by (group, name) like the rest of the store, never by Target.Arn — and must not invoke the target for a schedule whose Target.RoleArn is not authorized.
Traces: lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator: 34/171 checks passed; lambda__scheduler__sqs 0/4 (3 failed, 1 unloadable). Two of the three failures are OWNED by 20261109T141116Z-lambda-scheduler-sqs-scheduler-never-dead-letters-a-failed-lambda-target.md and excluded; this one is net-new. The concrete markers below are one run's values, not requirements.

lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty, at the first assertion after the delivery:
  AssertionError: the function received {'marker': 'twin-73e406cc58a9'}, not the schedule's exact Input {'marker': 'exact-58be2c3eb153', 'nested': {'list': [1, 2.5, -3, True, False, None], 'empty': {}, 'deep': {'k': ['v']}}, 'unicode': 'café ✓ 日本', 'quote': 'she said "hi"', 'big': 12345678901234}

Expected (AWS): the echo handler's record carries event == the JSON object submitted as the schedule's Target.Input, event_type == 'dict', and function_arn == the target function's ARN; the twin (same function ARN, same RetryPolicy, its own DLQ, a role WITHOUT lambda:InvokeFunction) never invokes the function and instead dead-letters an envelope whose Payload is the twin's own Input; the healthy schedule's DLQ stays empty.
Actual (emulator): the single invocation observed at the function carries the TWIN's one-key payload. The immediately preceding assertion — get_schedule(Name=<schedule under test>)['Target']['Input'] == the submitted JSON string — PASSED, which isolates the divergence to the dispatch/invocation path rather than storage or serialization of the control plane.

## Steps to Reproduce
All calls via boto3 against the emulator. Setup as the check does: two SQS queues (a sink and a DLQ) plus a third DLQ for the twin, each read back for its QueueArn; an IAM role trusting lambda.amazonaws.com with sqs:SendMessage on the sink (plus logs:*); a python Lambda whose handler forwards json.dumps({'event': event, 'event_type': type(event).__name__, 'function_arn': context.invoked_function_arn}) to SINK_URL; an IAM role trusting scheduler.amazonaws.com with lambda:InvokeFunction on that function and sqs:SendMessage on the healthy DLQ; and a SECOND scheduler role granting only sqs:SendMessage on the twin DLQ.

1. create_schedule(Name=A, ScheduleExpression='at(<now + 10 s>)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn': fn_arn, 'RoleArn': <healthy scheduler role>, 'Input': json.dumps({'marker':'exact-<id>','nested':{'list':[1,2.5,-3,True,False,None],'empty':{},'deep':{'k':['v']}},'unicode':'café ✓ 日本','quote':'she said "hi"','big':12345678901234}, ensure_ascii=False), 'RetryPolicy': {'MaximumRetryAttempts':1,'MaximumEventAgeInSeconds':60}, 'DeadLetterConfig': {'Arn': dlq_arn}}).
2. create_schedule(Name=B, same expression/window/state, Target={'Arn': fn_arn (the SAME function), 'RoleArn': <role without lambda:InvokeFunction>, 'Input': json.dumps({'marker':'twin-<id>'}), same RetryPolicy, 'DeadLetterConfig': {'Arn': twin_dlq_arn}}).
3. get_schedule(Name=A) — Target.Input is echoed correctly (passes).
4. Receive from the sink queue and json.loads the body. On AWS record['event'] equals schedule A's payload object, record['event_type'] == 'dict' and record['function_arn'] == fn_arn; on the emulator record['event'] is schedule B's {'marker': 'twin-<id>'}.
Check lambda__scheduler__sqs.xsvc-lambda-3-scheduler-lambda-target-receives-exact-input-dlq-empty.
