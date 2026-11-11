Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative|_poll_until#6|Timeout|last = probe()
Issue-Commit: a983232b5da502bccde79d757bd972103fe44705

# dynamodb__lambda__s3__secretsmanager: a CFN stack with a Condition-gated IAM policy never reaches a terminal status, so the conditional secretsmanager:GetSecretValue deny/allow pair is never observed

> _Note: this issue was identified against the code base at commit `a983232b5da502bccde79d757bd972103fe44705`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the deploy-then-invoke sequence for a template whose AWS::IAM::Policy is gated on a Condition makes no observable progress — the check is cut off by the suite's 60 s cap before either the negative or the positive branch is reached.

AWS: recorded behavior only for the status sequence and the function payload; Smithy models CreateStack/DescribeStacks and the Parameters/Conditions template features used here. CreateStack on this template (AWS::SecretsManager::Secret, AWS::IAM::Role, a Condition-gated AWS::IAM::Policy granting secretsmanager:GetSecretValue, and AWS::Lambda::Function) settles to a terminal DescribeStacks StackStatus of CREATE_COMPLETE and exposes Outputs.FunctionName. Invoking that function with AllowSecretsAccess="false" returns a payload {"ok": false, "error_code": "AccessDeniedException"} — the role has no GetSecretValue statement, so Secrets Manager refuses the read with AccessDeniedException; redeploying the identical template with AllowSecretsAccess="true" returns {"ok": true, "secret": "<the SecretString deployed by the stack>"}.

Emulator: the check runs to the harness's 60 s cap. Its helper polls DescribeStacks for a terminal status in {CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED} and then polls Invoke for the payload predicate (90 s budget each); neither predicate is satisfied inside the cap, so no terminal stack status, no Outputs.FunctionName, and neither the AccessDeniedException refusal nor the successful secret read is ever observed.

Why: a client that deploys a stack and waits on DescribeStacks hangs rather than being told the stack succeeded or failed, and a Condition-gated IAM grant in a template can be shown neither to deny nor to allow the downstream call — so permission-boundary behaviour expressed in CloudFormation cannot be relied on at all.

Fix: the CloudFormation stack-progress state machine (DescribeStacks must reach a terminal status, and Outputs must carry OutputValue, for a stack holding AWS::SecretsManager::Secret / AWS::IAM::Role / Condition-gated AWS::IAM::Policy / AWS::Lambda::Function), and the lambda Invoke path the check exercises against the stack's function.

Related open issues in the same service group that do NOT carry this replay key (so they do not cover this check): 20261104T232517Z-dynamodb-lambda-s3-secretsmanager-a-sam-auto-expand.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md.

Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative.

## Evidence
Check dynamodb__lambda__s3__secretsmanager.deploy-cfn-missing-secretsmanager-policy-negative, reported as [mismatch] with detail "Check timed out after 60.0s".

Expected (AWS): DescribeStacks StackStatus reaches CREATE_COMPLETE and Outputs.FunctionName is present; Invoke of that function with AllowSecretsAccess="false" returns {"ok": false, "error_code": "AccessDeniedException"}; the same stack with AllowSecretsAccess="true" returns {"ok": true, "secret": "<the stack's SecretString>"}.

Actual (emulator): the terminal-status poll (DescribeStacks) and the subsequent Invoke-payload poll both run without their predicate ever being met, and the check is killed at the 60 s cap. No StackStatus verdict, no Outputs.FunctionName, and no invoke payload is produced, so neither the AccessDeniedException negative nor the positive secret read is ever reached.

## Steps to Reproduce
1. CreateStack with a template that has Parameters.AllowSecretsAccess (AllowedValues true/false), Conditions.GrantSecretsPolicy = Fn::Equals[Ref AllowSecretsAccess, "true"], and resources: AWS::SecretsManager::Secret (SecretString), AWS::IAM::Role trusting lambda.amazonaws.com, a Condition-gated AWS::IAM::Policy granting secretsmanager:GetSecretValue on the secret, and AWS::Lambda::Function; Outputs.FunctionName = Ref Fn. Pass AllowSecretsAccess="false" and Capabilities CAPABILITY_IAM, CAPABILITY_NAMED_IAM.
2. Poll DescribeStacks for a terminal StackStatus. It never reaches CREATE_COMPLETE/CREATE_FAILED/ROLLBACK_COMPLETE/ROLLBACK_FAILED.
3. (Unreachable in the emulator) Invoke the function from Outputs.FunctionName and expect {"ok": false, "error_code": "AccessDeniedException"}; redeploy with AllowSecretsAccess="true" and expect {"ok": true, "secret": "<SecretString>"}.
