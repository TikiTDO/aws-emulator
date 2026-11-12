Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure
Issue-Key: dynamodb__lambda__stepfunctions.sfn-saga-compensate-on-failure|run#10|Timeout|execution = sfn.start_execution(stateMachineArn=sm["arn"], input=json.dumps({}))
Issue-Commit: fbef683f21970efb4631f2658975dd3b2c5af139

# dynamodb__lambda__stepfunctions: a saga whose Lambda task fails never reaches its Catch/compensation branch — every attempt ends FAILED States.Runtime, so the check spins to the harness cap (1 cause)

> _Note: this issue was identified against the code base at commit `fbef683f21970efb4631f2658975dd3b2c5af139`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause in this service group.

Cause 1 of 1: a saga state machine (optimized `dynamodb:putItem` reserve step, then a `lambda:invoke` charge step with `Catch: [{ErrorEquals: ["States.ALL"], Next: "Compensate", ResultPath: "$.error"}]`) never reaches the Catch target. Every execution the check starts terminates FAILED instead of SUCCEEDED, so its retry-until-SUCCEEDED loop re-starts executions until the harness 60 s cap kills the check and no compensation is ever observable.
Failing check: dynamodb__lambda__sfn.sfn-saga-compensate-on-failure.

AWS: a Task state whose Resource is `arn:aws:states:::lambda:invoke` invokes the function. When the handler raises, the Task fails with the Lambda error name (`Lambda.Unhandled`, or `Lambda.<ExceptionClass>`) and a `cause` that is the function's structured error document (`{"errorType", "errorMessage", "trace"}`); a `Catch` entry with `ErrorEquals: ["States.ALL"]` matches that failure, writes the error document to the declared `ResultPath` and transitions to the named state, which then runs — so the overall execution reaches `status="SUCCEEDED"` and the compensating function's side effect (here a DynamoDB PutItem of `pk="compensation-executed"`) is present. Smithy (sfn): DescribeExecution models `status`, `error` and `cause` as the carriers of this outcome; Smithy (lambda): `Invoke` is a modeled data-plane operation that runs the function.
Emulator: the execution never reaches SUCCEEDED; `Check timed out after 60.0s` while the check polls DescribeExecution and restarts FAILED runs. Consistently across this suite the emulator answers a Lambda-invoking Task with `status='FAILED'`, `error='States.Runtime'` and `cause="the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task"` — emulator-authored text AWS never emits — and the declared Catch is not taken, so the Compensate state never executes and the compensation item is never written.
Why: the single most common resilience pattern built on Step Functions — a failing step caught and compensated — is inert. A client sees FAILED where AWS reports SUCCEEDED-after-compensation, its `ErrorEquals` matching never fires, `$.error` is never populated, and the compensating side effect that is supposed to restore state is silently skipped, leaving the reserved DynamoDB item orphaned.
Fix: the Lambda `Invoke` data-plane handler in the lambda crate (execute the function instead of refusing) and, in the sfn crate, the interpreter's Task failure classification and `Catch` evaluation — a task failure must carry the AWS error name and the function's structured cause, must be matched against each `Catch` entry's `ErrorEquals` (including `States.ALL`), must be written to `ResultPath`, and must transition to `Next` so the compensating state actually runs.
Related open issues 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md and 20261027T173205Z-sfn-step-functions-never-dispatches-the-optimized-lambda-invoke.md describe neighbouring symptoms; neither carries this check's replay key, which is why it is filed here.
Traces: the failing check id listed above.

## Evidence
Cause 1 — Expected: `describe_execution` reports `status="SUCCEEDED"` for the saga (the Lambda task fails, `Catch` routes to `Compensate`, the compensating function runs) and `get_item(Key={"pk": {"S": "compensation-executed"}})` returns an item.
Actual: `Check timed out after 60.0s` — the execution never becomes SUCCEEDED, so the check's bounded retry loop (restart on FAILED) never exits and neither the compensation branch nor the compensation item is ever observed. The emulator's recorded answer for a Lambda-invoking Task in this suite is `status='FAILED'`, `error='States.Runtime'`, `cause="the 'Invoke' operation of Lambda is not implemented by this emulator, so nothing was run for this task"`, and the Catch target is not entered.

## Steps to Reproduce
1. Create a DynamoDB table, a Lambda function whose handler raises (`raise Exception('ChargeDeclined')`), and a second Lambda function that writes `pk="compensation-executed"` into the table.
2. Create an IAM role trusted by states.amazonaws.com granting `dynamodb:PutItem` on the table and `lambda:InvokeFunction` on both functions.
3. CreateStateMachine (STANDARD) with: `Step1_Reserve` = Task `arn:aws:states:::dynamodb:putItem` -> `Step2_Charge` = Task `arn:aws:states:::lambda:invoke` on the failing function with `Catch: [{"ErrorEquals": ["States.ALL"], "Next": "Compensate", "ResultPath": "$.error"}]`, and `Compensate` = Task `arn:aws:states:::lambda:invoke` on the compensating function with `Payload.$: "$.error"`.
4. StartExecution with `{}` and poll DescribeExecution to a terminal status; then GetItem for `pk="compensation-executed"`.
On AWS: status SUCCEEDED and the compensation item exists. On the emulator: the execution ends FAILED with error `States.Runtime`, the Catch branch is never entered and no compensation item appears.
