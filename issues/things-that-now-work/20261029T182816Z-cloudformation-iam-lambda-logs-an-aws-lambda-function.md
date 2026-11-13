Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke cloudformation__iam__lambda__logs.cfn-declared-log-group-receives-lambda-logs
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke|run#13|WaiterError|awslambda.get_waiter("function_active_v2").wait(FunctionName=function_name)
Issue-Key: cloudformation__iam__lambda__logs.cfn-declared-log-group-receives-lambda-logs|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# cloudformation__iam__lambda__logs: an AWS::Lambda::Function in a CREATE_COMPLETE stack does not exist, and the declared-log-group check hangs to the 60 s cap (2 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two root causes. Cause count: 2.

=== Cause 1 of 2: a stack that reaches CREATE_COMPLETE never materialises its AWS::Lambda::Function ===
AWS: when CreateStack reports CREATE_COMPLETE for a template declaring an AWS::Lambda::Function, GetFunction/Invoke on the physical id resolve; a resource that could not be created would have failed the stack instead. Recorded behavior only.
Emulator: the stack reports CREATE_COMPLETE but the function is absent — 'botocore.exceptions.WaiterError: Waiter FunctionActiveV2 failed: An error occurred (ResourceNotFoundException): Function not found: arn:aws:lambda:<region>:<account>:function:<stack>-<logical-id>-<suffix>'. Expected HTTP 200 from GetFunction; actual 400 ResourceNotFoundException 'Function not found: <arn>' (the arn is this run's value; the requirement is that the declared resource exists).
Why: a client that deploys a stack and then uses its outputs/resources is told the deploy succeeded and then gets ResourceNotFoundException on the very resource the stack claims to have created — the stack status becomes meaningless as a readiness signal.
Fix: the CloudFormation resource-provisioning path for AWS::Lambda::Function (create the real resource and bind its physical id before marking the resource, and hence the stack, CREATE_COMPLETE). Same root cause as the open issue 20261029T072657Z-cloudformation-iam-lambda-logs-a-stack-declaring-aws-e5cb3e.md, whose replay keys do not include this check.
Traces: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-created-on-first-invoke.

=== Cause 2 of 2: the CFN-declared log group never receives the function's logs, so the check hangs to the 60 s cap ===
AWS: a template that declares both an AWS::Logs::LogGroup and an AWS::Lambda::Function whose LoggingConfig names it delivers the function's invocation records into that declared group; the check polls the group for them.
Emulator: terminated with 'Error: Check timed out after 60.0s' — no event is ever delivered to the declared group.
Why: the LoggingConfig wiring is unobservable: a client that waits for logs in the group it declared waits forever instead of receiving a delivery or an error.
Fix: the function-logging destination wiring (LoggingConfig/LogGroup) plus the invocation log emitter that must write into it.
Traces: cloudformation__iam__lambda__logs.cfn-declared-log-group-receives-lambda-logs.

## Evidence
Cause 1 — failing check deploy-logs-2-implicit-log-group-created-on-first-invoke (verdict: error). Replay tail: 'botocore.exceptions.WaiterError: Waiter FunctionActiveV2 failed: An error occurred (ResourceNotFoundException): Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-implfn-9fb313446959' after the stack reported CREATE_COMPLETE. The two sibling checks with the identical tail are already covered by 20261029T072657Z-cloudformation-iam-lambda-logs-a-stack-declaring-aws-e5cb3e.md; this one is not.
Cause 2 — failing check cfn-declared-log-group-receives-lambda-logs (verdict: mismatch), replay detail 'Error: Check timed out after 60.0s'; the unsatisfied condition is the arrival of an invocation record in the declared log group.

## Steps to Reproduce
Cause 1: cloudformation CreateStack with a template declaring an IAM role and an AWS::Lambda::Function; wait for CREATE_COMPLETE; then lambda GetFunction (or the FunctionActiveV2 waiter) on the stack's physical function name. Observed: ResourceNotFoundException 'Function not found: <arn>'. Expected: 200.
Cause 2: deploy a template declaring an AWS::Logs::LogGroup plus a function whose LoggingConfig.LogGroup names it; invoke the function; poll the declared group for its records. Observed: no event within 60 s.
