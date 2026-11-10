Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__stepfunctions.absent-role-accepted-at-create-then-execution-fails cloudformation__stepfunctions.e2e-missing-state-machine-lifecycle cloudformation__stepfunctions.stop-execution-records-abort-and-truncates-state-history
Issue-Key: cloudformation__stepfunctions.absent-role-accepted-at-create-then-execution-fails|run#12|AssertionError|assert failed["error"] == "States.TaskFailed", (
Issue-Key: cloudformation__stepfunctions.e2e-missing-state-machine-lifecycle|phase#3|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Key: cloudformation__stepfunctions.stop-execution-records-abort-and-truncates-state-history|_poll#5|Timeout|time.sleep(interval_s)
Issue-Commit: 765add48a71cdb59193040bf215c1324536972c5

# cloudformation/stepfunctions: 3 net-new root causes — role failure reported as States.Runtime, StateMachineArn output is not an ARN, StopExecution never settles

> _Note: this issue was identified against the code base at commit `765add48a71cdb59193040bf215c1324536972c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
Three distinct net-new root causes. Open issue 20261024T043150Z-cloudformation-step-functions-lambda-resources-physical-resource-id.md owns other replay keys in this service; none of the keys below are carried by an open issue.

Cause 1 of 3: an execution that fails because the state machine's role is absent reports the wrong error name.
AWS: recorded behavior only — the state machine is created even though its RoleArn does not resolve, and the failure appears at the Task state that first needs the role: the execution history records a TaskFailed event and the execution's error is States.TaskFailed.
Emulator: the failure is reported as States.Runtime.
Why: a client that branches on States.TaskFailed (retry/catch configuration keys on it) never matches, and the failure is attributed to the interpreter instead of the task.
Fix: error classification in the execution engine's task-invocation path (role resolution failure must surface as States.TaskFailed).
Traces: cloudformation__stepfunctions.absent-role-accepted-at-create-then-execution-fails.

Cause 2 of 3: the stack's StateMachineArn output is the state machine's name, not its ARN.
AWS: recorded behavior only — Ref/GetAtt on AWS::StepFunctions::StateMachine yields the state-machine ARN, which ends in the name the stack also exports (arn:aws:states:<region>:<account>:stateMachine:<name>).
Emulator: the output is just '<name>', which does not end in an ARN form at all.
Why: every consumer of the exported value (StartExecution, IAM resource policies, cross-stack imports) gets an unusable identifier.
Fix: the AWS::StepFunctions::StateMachine resource handler's physical id / GetAtt resolution.
Traces: cloudformation__stepfunctions.e2e-missing-state-machine-lifecycle.

Cause 3 of 3: StopExecution's abort accounting never settles.
AWS: recorded behavior only — StopExecution marks the execution ABORTED, records an ExecutionAborted event with the supplied error/cause and truncates the state history at that point, all well inside the check's 60s budget.
Emulator: the check ends with 'Check timed out after 60.0s' — the execution never reaches a terminal state. Hypothesis: the stop request never transitions the running execution, so the history never closes.
Why: a client cannot stop a running execution, and anything waiting on its terminal status hangs.
Fix: the StopExecution handler and the execution state machine's abort transition.
Traces: cloudformation__stepfunctions.stop-execution-records-abort-and-truncates-state-history.

## Evidence
C1: "the failure is reported as 'States.Runtime', expected States.TaskFailed — the Task state is where the role is first needed".
C2: "[phase 1/7 deploy] the stack's StateMachineArn output '<name>' does not end in the name it also exports, '<name>'".
C3: 'Error: Check timed out after 60.0s'.
The state-machine name is a per-run value; the ARN shape and the error name are the requirements.

## Steps to Reproduce
C1: deploy a state machine whose RoleArn does not exist, StartExecution and read the execution's error and history. C2: deploy a stack with AWS::StepFunctions::StateMachine and read its StateMachineArn output. C3: start a long-running execution, call StopExecution with an error/cause and poll DescribeExecution/GetExecutionHistory for the ABORTED terminal state.
