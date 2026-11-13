Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring
Issue-Key: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring|_poll_until#1|Timeout|time.sleep(interval_s)
Issue-Commit: 1f070c140fc1a5d7eca76d9160e1267472cce55c

# lambda__rds__secretsmanager: the CFN-wired RDS rotation-Lambda scenario never settles — the stack never reaches a terminal status and no rotation ever promotes a new AWSCURRENT (1 cause)

> _Note: this issue was identified against the code base at commit `1f070c140fc1a5d7eca76d9160e1267472cce55c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause.

Cause 1 of 1: the end-to-end scenario that deploys an RDS DB instance + Secrets Manager secret + custom rotation Lambda via CloudFormation and then rotates the secret never reaches any verdict against the emulator — it is cut off at the harness' 60 s cap with no stack status, no resource status and no error to compare. Failing check: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring.

NOTE: open issue 20261113T060617Z-lambda-rds-secretsmanager-the-cloudformation-rds-secrets-manager-rotation-lambda-wiring.md describes the same scenario hanging, but the replay did not report this check as OWNED by it (its recorded Issue-Key does not match the key this run produced), so the failure is still live and is filed here. A fixer should treat the two as one cause and close both together.

AWS: recorded behavior only — neither the cloudformation nor the secretsmanager Smithy model constrains convergence time. On AWS the scenario terminates: (a) DescribeStacks leaves CREATE_IN_PROGRESS for a terminal status (CREATE_COMPLETE, or CREATE_FAILED/ROLLBACK_COMPLETE carrying a ResourceStatusReason), so the poll ends; (b) the AWS::SecretsManager::RotationSchedule created by that stack binds RotationLambdaARN and RotationRules to the secret and (RotateImmediatelyOnUpdate defaulting to true) kicks off a rotation, so DescribeSecret reports RotationEnabled true and the modeled RotationLambdaARN / RotationRules members (Smithy: DescribeSecret output members RotationEnabled, RotationLambdaARN, RotationRules, LastRotatedDate, VersionIdsToStages); (c) RotateSecret either returns ARN/Name/VersionId (Smithy: RotateSecret output ARN, Name, VersionId) or refuses with InvalidRequestException (httpError 400) whose message states the previous rotation isn't complete — both are terminal answers; (d) the rotation runs the function's createSecret/setSecret/testSecret/finishSecret steps so a NEW version holds AWSCURRENT, the original version loses it, and DescribeSecret.LastRotatedDate becomes non-null.

Emulator: no verdict at all. The replay detail is exactly "Error: Check timed out after 60.0s". Because nothing settles, the emulator produced no stack status, no resource status, no HTTP status, no error code and no error message for this scenario. Hypothesis (neither the model nor the recording states which): either a stack resource (AWS::RDS::DBInstance, the rotation AWS::Lambda::Function, or AWS::SecretsManager::RotationSchedule) stays in a transient status the emulator never advances so DescribeStacks never leaves *_IN_PROGRESS, or the deploy does settle and the subsequent rotation never promotes a new AWSCURRENT version, leaving the GetSecretValue(VersionStage=AWSCURRENT) poll spinning on the original VersionId. Both ends of the scenario are unobservable from the emulator's answers, which is itself the defect: every step must terminate.

Why: a client cannot validate RDS credential rotation at all. A deploy that would succeed and one that would roll back are indistinguishable because both hang, so an operator polling DescribeStacks never finishes and never learns whether the rotation function, its secretsmanager.amazonaws.com invoke grant and the secret's rotation rules exist. Likewise a caller that calls RotateSecret and polls for the new AWSCURRENT version blocks forever instead of seeing the staging ladder complete — automation built on "rotate, then read the new credential" deadlocks rather than failing fast.

Fix: the stack-resource provisioning/convergence path behind CreateStack in the cloudformation service crate for AWS::RDS::DBInstance, AWS::Lambda::Function, AWS::Lambda::Permission, AWS::SecretsManager::SecretTargetAttachment and AWS::SecretsManager::RotationSchedule — every resource must reach a terminal status so the stack does — plus the secretsmanager rotation driver behind RotateSecret/the RotationSchedule resource, which must actually invoke the rotation function through its four steps and move AWSCURRENT, or refuse with a modeled error. Not request validation of CreateStack or RotateSecret.

Traces: lambda__rds__secretsmanager.deploy-cfn-rds-rotation-lambda-wiring.

## Evidence
Expected (AWS): the scenario terminates. DescribeStacks leaves *_IN_PROGRESS for a terminal status inside the poll budget; GetSecretValue(SecretId=<secret ARN>, VersionStage="AWSCURRENT") eventually returns a VersionId different from the pre-rotation one and a SecretString whose "password" key differs; DescribeSecret returns LastRotatedDate non-null and the pre-rotation version either absent from VersionIdsToStages or no longer carrying "AWSCURRENT". Where RotateSecret is refused because a rotation is already in flight, AWS answers HTTP 400 InvalidRequestException whose message states that the previous rotation isn't complete — fixed error-class text, with the secret identifier taken from the request.

Actual (emulator): no verdict. Replay report detail, verbatim: "Error: Check timed out after 60.0s". No stack status, resource status, HTTP status, error code or error message was produced for any step of the scenario, so nothing could be compared.

## Steps to Reproduce
1. CreateStack with a template declaring: an AWS::SecretsManager::Secret using GenerateSecretString (SecretStringTemplate '{"username":"dbadmin"}', GenerateStringKey password); an AWS::RDS::DBInstance (mysql, db.t3.micro) whose MasterUsername/MasterUserPassword come from {{resolve:secretsmanager:...}} dynamic references; an AWS::SecretsManager::SecretTargetAttachment binding the secret to the instance; an AWS::IAM::Role and an AWS::Lambda::Function implementing the four-step rotation handler; an AWS::Lambda::Permission granting lambda:InvokeFunction to secretsmanager.amazonaws.com; and an AWS::SecretsManager::RotationSchedule with RotationRules AutomaticallyAfterDays 1.
2. Poll DescribeStacks for a terminal stack status and read the SecretArn output.
3. GetSecretValue(VersionStage="AWSCURRENT") and record VersionId and the password.
4. Call RotateSecret(SecretId=<secret ARN>), tolerating the InvalidRequestException that says the previous rotation isn't complete.
5. Poll GetSecretValue(VersionStage="AWSCURRENT") for a different VersionId, then DescribeSecret for LastRotatedDate.
6. Observe that the scenario never terminates: the check is killed at its 60 s cap with no stack status and no rotated version.
