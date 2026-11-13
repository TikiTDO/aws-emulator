Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.rotation-lambda-missing-permission-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.rotation-lambda-missing-permission-rejected|_deploy_stack#4|Timeout|waiter.wait(
Issue-Commit: 9de0dc732b013e7abf09d4b752737b31a8a239ee

# secretsmanager: a rotation wired by a CloudFormation AWS::SecretsManager::RotationSchedule never progresses — neither the positive control's AWSCURRENT move nor the permission-starved Lambda's AccessDeniedException ever appears, so the check burns its whole budget

> _Note: this issue was identified against the code base at commit `9de0dc732b013e7abf09d4b752737b31a8a239ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Cause 1 of 1: a RotateSecret started against a secret whose rotation Lambda was wired by a CloudFormation AWS::SecretsManager::RotationSchedule never reaches any observable outcome, so the check never gets a verdict and is killed when its budget expires ("Check timed out after 60.0s").

AWS: recorded behavior only (the scenario is a sequence of modeled calls, not a single modeled field). On AWS, RotateSecret on the permitted secret starts a rotation that runs the rotation function through createSecret/setSecret/testSecret/finishSecret, and within the poll window DescribeSecret reports a NEW version carrying AWSCURRENT (VersionIdsToStages maps a version id other than the pre-rotation one to ["AWSCURRENT"]). On the sibling secret whose rotation Lambda's execution role has only logs:* and no secretsmanager:* permission, the invocation does happen and fails inside the function: the function's log group /aws/lambda/<function-name> receives an event containing "AccessDeniedException", and DescribeSecret still shows AWSCURRENT on the original version with at most one leftover AWSPENDING version. Both observations are reached well inside the check's poll windows.
Emulator: neither observation is ever reached. The check makes no progress at all within its budget and is terminated after 60 s — the positive control's DescribeSecret never shows AWSCURRENT on a new version, and no AccessDeniedException is ever filtered out of the bad function's log group (FilterLogEvents on /aws/lambda/<bad function> yields no matching event, or the group does not exist because the function was never invoked). Nothing about the rotation is observable, so the scenario cannot even reach its negative assertion.
Why: a client that calls RotateSecret and waits for AWSCURRENT to move — the documented, universal way to detect that a rotation finished — waits forever against the emulator; and a client that diagnoses a failed rotation from the rotation function's CloudWatch Logs output (the standard AWS runbook for a permission-starved rotation Lambda) finds no log evidence at all, so a broken rotation is indistinguishable from a rotation that has not started.
Fix: the rotation driver behind RotateSecret in the secretsmanager service — the component that must actually invoke the configured rotation Lambda for each rotation step, let its credentials be evaluated (so a role without secretsmanager:* produces an AccessDeniedException inside the function and that output reaches the function's log group), and advance AWSPENDING to AWSCURRENT on success while leaving AWSCURRENT in place on failure. This is reached through a secret whose rotation was configured by an AWS::SecretsManager::RotationSchedule, so the schedule's hand-off into that driver belongs in scope.
Traces: secretsmanager.rotation-lambda-missing-permission-rejected.

Related open issues look similar but do not carry this check's replay key and do not cover it: 20261028T061500Z-secretsmanager-rotatesecret-rotateimmediately-true-never-runs-the-rotation.md and 20261108T035337Z-secretsmanager-5-net-new-causes-a-deferred-rotateimmediately.md (RotateSecret/RotateImmediately variants), 20261109T163411Z-cloudformation-iam-lambda-secretsmanager-2-net-new-causes.md (RotationSchedule leaves RotationEnabled false), 20261109T002603Z-lambda-rds-secretsmanager-rotatesecret-with-a-rotation-lambda.md (un-invokable rotation Lambda on an RDS secret). This finding is the CloudFormation-wired good/bad rotation-Lambda pair under the secretsmanager service, including the missing AccessDeniedException log evidence.

## Evidence
Check secretsmanager.rotation-lambda-missing-permission-rejected, [mismatch]:

  Error: Check timed out after 60.0s

The check deploys one stack with: GoodRole (secretsmanager:* + logs:*) + GoodFunction + GoodPermission (Principal secretsmanager.amazonaws.com) + GoodSecret (GenerateSecretString PasswordLength 24) + GoodSchedule (AWS::SecretsManager::RotationSchedule, RotationRules AutomaticallyAfterDays 30), and the mirror Bad* set whose BadRole grants only logs:CreateLogGroup/CreateLogStream/PutLogEvents.

It then (a) calls RotateSecret on the good secret and polls DescribeSecret for up to 180 s for a version other than the pre-rotation one carrying AWSCURRENT, and (b) calls RotateSecret on the bad secret and polls FilterLogEvents(logGroupName="/aws/lambda/<BadFunctionName>", filterPattern='"AccessDeniedException"') for up to 120 s. Against the emulator neither poll ever satisfies its predicate — the whole 60 s budget elapses with no observable rotation progress and no log evidence, so the terminal assertions (AWSCURRENT still on the original bad version, at most one leftover AWSPENDING version) are never reached.

## Steps to Reproduce
1. CreateStack (Capabilities=[CAPABILITY_IAM, CAPABILITY_NAMED_IAM], OnFailure=DELETE) with two parallel sets of resources: an IAM role for lambda.amazonaws.com, a python3.12 rotation Lambda implementing the four rotation steps, an AWS::Lambda::Permission for Principal secretsmanager.amazonaws.com, an AWS::SecretsManager::Secret (GenerateSecretString PasswordLength 24) and an AWS::SecretsManager::RotationSchedule (RotationRules AutomaticallyAfterDays 30). The "Good" role grants secretsmanager:* and logs:*; the "Bad" role grants only logs:*.
2. Wait for CREATE_COMPLETE and read outputs GoodSecretArn, BadSecretArn, BadFunctionName.
3. DescribeSecret on each secret and record the version id holding AWSCURRENT.
4. RotateSecret(SecretId=GoodSecretArn); poll DescribeSecret for a different version id holding AWSCURRENT. Observe it never changes.
5. RotateSecret(SecretId=BadSecretArn); poll FilterLogEvents(logGroupName="/aws/lambda/<BadFunctionName>", filterPattern='"AccessDeniedException"'). Observe no matching event ever appears (the log group may not exist at all).
6. The scenario makes no progress and the check is terminated after its 60 s budget.
