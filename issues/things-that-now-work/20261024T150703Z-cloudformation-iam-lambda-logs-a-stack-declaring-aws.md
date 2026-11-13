Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected|run#16|WaiterError|awslambda.get_waiter("function_active_v2").wait(FunctionName=collide_fn)
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission|run#14|WaiterError|awslambda.get_waiter("function_active_v2").wait(FunctionName=fn)
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# cloudformation__iam__lambda__logs: a stack declaring AWS::Lambda::Function reaches a terminal state without the function existing, so the FunctionActiveV2 waiter fails with ResourceNotFoundException (1 cause)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: CloudFormation does not provision the AWS::Lambda::Function it declares ===
AWS: a stack that declares AWS::Lambda::Function (plus its AWS::IAM::Role and AWS::Logs::LogGroup) only reaches CREATE_COMPLETE once the function exists; GetFunction / the FunctionActiveV2 waiter then resolve it by name or ARN, and the stack's implicit-vs-declared log-group behaviour (a declared log group colliding with the implicit /aws/lambda/<name> group, and an implicit group denied when the role lacks logs:CreateLogGroup) is observable afterwards.
Emulator: the deploy returns, but the function named by the template is absent — lambda GetFunction answers ResourceNotFoundException 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>', so the waiter raises WaiterError and neither check reaches the log-group assertion it exists to grade.
Why: a client deploying a Lambda by template gets a stack that claims to have succeeded and no function, so every downstream step fails with a not-found error far from its cause. Two distinct CloudFormation log-group behaviours (declared/implicit collision and the permission-denied implicit group) are left entirely unverified. Open issue 20261023T205437Z-cloudformation-reports-create-complete-for-resources-it-never.md records this pattern for other resources/checks and does not carry these two.
Fix: the CloudFormation resource provisioner for AWS::Lambda::Function (and the stack-status computation that must not reach a success state when a resource was not created).
Traces: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected, cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission.

## Evidence
cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected: botocore.exceptions.WaiterError: Waiter FunctionActiveV2 failed: An error occurred (ResourceNotFoundException): Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-collfn-<id>.
cloudformation__iam__lambda__logs.deploy-logs-2-implicit-log-group-denied-without-create-permission: botocore.exceptions.WaiterError: Waiter FunctionActiveV2 failed: An error occurred (ResourceNotFoundException): Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-xsvc-ctlfn-<id>.
Expected: the function declared by the template resolves (GetFunction 200, State ACTIVE) once the stack is deployed; only then are the declared-vs-implicit log-group assertions reached.

## Steps to Reproduce
cfn CreateStack with a template declaring AWS::IAM::Role, AWS::Lambda::Function (Runtime python3.x, Handler, inline/zip Code, Role: !GetAtt Role.Arn) and an AWS::Logs::LogGroup named /aws/lambda/<function name>; wait for the stack to reach a terminal state; then call lambda GetFunction(FunctionName=<declared name>) or wait on the FunctionActiveV2 waiter — ResourceNotFoundException: Function not found.
