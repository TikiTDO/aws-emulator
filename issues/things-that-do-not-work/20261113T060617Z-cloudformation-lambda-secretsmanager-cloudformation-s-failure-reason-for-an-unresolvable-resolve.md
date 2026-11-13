Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back
Issue-Key: cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back|run#13|AssertionError|assert failures == [
Issue-Commit: 65a54ca45d4af5e17b89290ffb091f639a3756dd

# CloudFormation's failure reason for an unresolvable {{resolve:secretsmanager:...:SecretString:<key>}} is emulator-authored instead of AWS's "Could not find a value associated with JSONKey in SecretString"

> _Note: this issue was identified against the code base at commit `65a54ca45d4af5e17b89290ffb091f639a3756dd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
ONE root cause. Cause 1 of 1: when a resource property uses a Secrets Manager dynamic reference whose JSON key does not exist in the secret's SecretString, the stack does roll back (status and resource status are right), but the CREATE_FAILED event's ResourceStatusReason is the emulator's own sentence instead of AWS's fixed dynamic-reference message.

AWS: recorded behavior only (the CloudFormation Smithy model does not constrain ResourceStatusReason text; the reason is produced by the dynamic-reference resolver). For a {{resolve:secretsmanager:<secret>:SecretString:<json-key>}} whose <json-key> is absent from the secret, the resource fails with ResourceStatusReason exactly "Could not find a value associated with JSONKey in SecretString". That whole string is fixed text — AWS does not interpolate the secret ARN or the key name into it. Everything else the check asserts already holds: the stack ends ROLLBACK_COMPLETE, the only CREATE_FAILED resource is the Lambda function that carried the reference, and the rolled-back stack leaves no function behind.

Emulator: ResourceStatusReason is "Unable to resolve the dynamic reference '{{resolve:secretsmanager:<secret-arn>:SecretString:<key>}}': the secret <secret-arn> holds no <key> key" — an emulator-authored sentence that additionally interpolates the secret ARN (twice) and the key name, neither of which AWS puts in this reason.

Why: a client or CI gate that matches on the CloudFormation failure reason to classify a deploy failure (missing-secret-key vs any other dynamic-reference problem) misclassifies it, and a reason string copied from AWS docs or a real deploy never matches. The interpolated secret ARN also widens the reason into a per-environment value where AWS emits a constant.

Fix: the dynamic-reference resolver behind CreateStack in the cloudformation service crate — the secretsmanager SecretString JSON-key branch must emit AWS's fixed reason text for a missing key rather than composing its own.

Traces: cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back.

## Evidence
Expected (AWS): describe_stack_events shows exactly one failure — ("Fn", "CREATE_FAILED", "Could not find a value associated with JSONKey in SecretString") — and the stack ends ROLLBACK_COMPLETE.

Actual (emulator), replay report detail verbatim: "AssertionError: the function must fail on the unresolvable key and nothing else; failures were [('Fn', 'CREATE_FAILED', \"Unable to resolve the dynamic reference '{{resolve:secretsmanager:arn:aws:secretsmanager:<region>:<account>:secret:<name>:SecretString:no_such_key}}': the secret arn:aws:secretsmanager:<region>:<account>:secret:<name> holds no no_such_key key\")]". The secret ARN, region, account and generated secret name are per-run values, not part of the requirement; the requirement is the fixed AWS sentence.

## Steps to Reproduce
1. CreateSecret with SecretString = {"username": "app", "api_key": "..."}.
2. CreateStack with an AWS::Lambda::Function whose Environment.Variables.API_KEY is "{{resolve:secretsmanager:<secret-arn>:SecretString:no_such_key}}" (plus the IAM role), Capabilities=[CAPABILITY_IAM].
3. Wait for the stack to settle: it reaches ROLLBACK_COMPLETE (correct).
4. DescribeStackEvents and read the CREATE_FAILED ResourceStatusReason for the function: it is the emulator's "Unable to resolve the dynamic reference ..." sentence instead of "Could not find a value associated with JSONKey in SecretString".
