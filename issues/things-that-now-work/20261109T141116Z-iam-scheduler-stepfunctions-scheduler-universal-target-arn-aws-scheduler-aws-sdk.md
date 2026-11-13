Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__scheduler__stepfunctions.universal-target-sdk-service-id-vs-endpoint-prefix
Issue-Key: iam__scheduler__stepfunctions.universal-target-sdk-service-id-vs-endpoint-prefix|_poll_until#9|AssertionError|raise AssertionError(f"waited {timeout_s}s for {description}; last observation was {last!r}")
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# scheduler universal target arn:aws:scheduler:::aws-sdk:sfn:startExecution never starts a Step Functions execution

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
This issue carries 1 root cause, in the EventBridge Scheduler occurrence/dispatch component's handling of UNIVERSAL targets.

Cause 1 of 1: a schedule whose Target.Arn is arn:aws:scheduler:::aws-sdk:sfn:startExecution is accepted but never issues the states:StartExecution call, so no execution is ever recorded.
AWS: Smithy (com.amazonaws.scheduler) — the Target shape documents Scheduler's universal targets ("over 6,000 API operations across more than 270 services"); Target.Arn is required (@length 1..1600), Target.Input is "The text, or well-formed JSON, passed to the target". The service segment of a universal target ARN is the SDK service id, which for Step Functions is `sfn` (not the wire/endpoint prefix `states`) and the operation segment is the SDK's lowerCamelCase operation name — this distinction is exactly the check's subject. At each occurrence Scheduler must assume Target.RoleArn and call StartExecution with Target.Input as the request body, so Step Functions records an execution with the Name given in the Input (recorded behavior: ListExecutions on the state machine reports it within the delivery window).
Emulator: CreateSchedule accepts the universal target and GetSchedule echoes it, but Step Functions never records any execution — the check waits 180 s and its last observation is None. The aws-sdk universal target is never dispatched, so the sfn-vs-states service-id resolution the check grades cannot even be reached.
Why: a client that drives a state machine off a schedule through a universal target sees a healthy ENABLED schedule and no executions. The scheduled workflow silently never starts and nothing is reported. In addition, when a dispatcher is added it can get the service-id segment wrong (accepting `states` or rejecting `sfn`) and still look healthy — this check is the one that grades that mapping.
Fix: the occurrence/dispatch component of the scheduler crate (services/service-com-amazonaws-scheduler-awschronosservice/src/) must resolve arn:aws:scheduler:::aws-sdk:<service>:<operation> by SDK service id — `sfn` for Step Functions — and lowerCamelCase operation name, then issue that call with Target.Input as the request body under Target.RoleArn.
Traces: iam__scheduler__stepfunctions.universal-target-sdk-service-id-vs-endpoint-prefix.

## Evidence
Replay of simparity-behavioral-aws-v1: iam__scheduler__stepfunctions 0/1.

[mismatch] iam__scheduler__stepfunctions.universal-target-sdk-service-id-vs-endpoint-prefix
AssertionError: waited 180s for Step Functions to record an execution named <name> started by the universal target arn:aws:scheduler:::aws-sdk:sfn:startExecution; last observation was None

Expected: within the delivery window, stepfunctions.list_executions on the target state machine reports an execution whose Name is the one carried in Target.Input.
Actual: no execution at all after 180 s ('last observation was None'). The execution name above is one run's value, not a requirement.

## Steps to Reproduce
1. stepfunctions.create_state_machine for a trivial state machine and note its ARN.
2. iam.create_role trusting scheduler.amazonaws.com with an inline states:StartExecution policy on that ARN.
3. scheduler.create_schedule(Name=n, ScheduleExpression='at(<now+~1min>)' or 'rate(1 minute)', ScheduleExpressionTimezone='UTC', FlexibleTimeWindow={'Mode':'OFF'}, State='ENABLED', Target={'Arn':'arn:aws:scheduler:::aws-sdk:sfn:startExecution', 'RoleArn':role_arn, 'Input': json.dumps({'StateMachineArn': <arn>, 'Name': <exec name>, 'Input': '{}'})}).
4. Poll stepfunctions.list_executions(stateMachineArn=<arn>) past the nominal instant. On AWS an execution named <exec name> appears. On the emulator none ever does (observed: none after 180 s).
Check: iam__scheduler__stepfunctions.universal-target-sdk-service-id-vs-endpoint-prefix.
