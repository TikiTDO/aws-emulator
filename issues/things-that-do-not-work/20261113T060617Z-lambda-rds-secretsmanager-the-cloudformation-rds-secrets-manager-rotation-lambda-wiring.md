Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring|_poll_until#1|Timeout|last = probe()
Issue-Commit: 65a54ca45d4af5e17b89290ffb091f639a3756dd

# The CloudFormation RDS + Secrets Manager rotation-Lambda wiring deploy never settles: the check hangs to its 60 s cap

> _Note: this issue was identified against the code base at commit `65a54ca45d4af5e17b89290ffb091f639a3756dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause. Cause 1 of 1: the stack that wires an RDS instance, a Secrets Manager secret and its rotation Lambda together never reaches a terminal state against the emulator, so the check is killed by the harness at its 60 s budget with nothing to compare.

AWS: recorded behavior only (the cloudformation Smithy model does not constrain convergence time). On AWS this deploy settles inside the budget: the stack leaves CREATE_IN_PROGRESS for a terminal status (CREATE_COMPLETE, or CREATE_FAILED/ROLLBACK_COMPLETE with a reason), so DescribeStacks returns a status that can be asserted and the rotation wiring — the secret's RotationLambdaARN and RotationRules, and the Lambda's resource policy grant to secretsmanager.amazonaws.com — becomes readable.

Emulator: the check reaches no verdict. The replay detail is exactly "Error: Check timed out after 60.0s". Because nothing settles, no stack status, no resource status and no failure reason is observable. Hypothesis: a resource in the stack (the DB instance, the secret's rotation schedule, or the rotation function) stays in a transient status the emulator never advances; the recording does not say which, only that the deploy does not terminate.

Why: a client cannot validate RDS credential-rotation wiring at all. A deploy that would succeed and one that would roll back are indistinguishable — both just hang — so an operator gets no signal about whether the rotation Lambda, its grant and the secret's rotation rules were created, and a caller polling DescribeStacks never finishes.

Fix: the stack-resource provisioning/convergence path behind CreateStack in the cloudformation service crate for the resources in this stack (AWS::RDS::DBInstance, AWS::SecretsManager::RotationSchedule and the rotation AWS::Lambda::Function), not CreateStack's request validation — every resource must reach a terminal status so the stack does.

Traces: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring.

## Evidence
Expected (AWS): DescribeStacks leaves *_IN_PROGRESS inside the check budget and the rotation wiring (secret RotationEnabled/RotationLambdaARN/RotationRules and the rotation function's secretsmanager.amazonaws.com invoke grant) is readable; the check reaches a verdict.

Actual (emulator): no verdict. Replay report detail, verbatim: "Error: Check timed out after 60.0s". No stack status, resource status, error code or message was produced to compare.

## Steps to Reproduce
1. Run the lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring scenario: CreateStack a template declaring an RDS DB instance, a Secrets Manager secret for its credentials, a rotation Lambda function with its execution role, the Lambda permission granting secretsmanager.amazonaws.com invoke, and an AWS::SecretsManager::RotationSchedule binding them.
2. Poll DescribeStacks for a terminal status.
3. Observe that the stack never leaves *_IN_PROGRESS and the check is cut off at the 60 s cap.
