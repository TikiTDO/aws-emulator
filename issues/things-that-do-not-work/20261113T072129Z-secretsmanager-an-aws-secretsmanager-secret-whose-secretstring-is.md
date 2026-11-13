Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back|run#14|AssertionError|assert failures == [
Issue-Commit: b5518325ad9936970d86f9a3541971c01b6fb226

# secretsmanager: an AWS::SecretsManager::Secret whose SecretString is a {{resolve:secretsmanager:...:SecretString:<key>}} naming a missing JSON key fails with an emulator-authored reason instead of AWS's "Could not find a value associated with JSONKey in SecretString"

> _Note: this issue was identified against the code base at commit `b5518325ad9936970d86f9a3541971c01b6fb226`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: when an AWS::SecretsManager::Secret's SecretString property is a Secrets Manager dynamic reference whose JSON key is absent from the referenced secret, the deploy does roll back correctly, but the CREATE_FAILED event's ResourceStatusReason is the emulator's own sentence instead of AWS's fixed dynamic-reference message. Failing check: secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back.

Note: the open issue 20261113T060617Z-cloudformation-lambda-secretsmanager-cloudformation-s-failure-reason-for-an-unresolvable-resolve.md describes the same resolver text, but it is bound to a single check under a different service heading (cloudformation__lambda__secretsmanager.cfn-lambda-env-secretsmanager-missing-json-key-rejected-and-rolled-back, a Lambda Environment.Variables property) and does not carry this check's replay key, which the suite report confirms by not marking this failure OWNED. It is filed here with the secretsmanager check id so the fixer re-replays this one too; the resolver branch is likely shared, in which case closing both is one change.

AWS: recorded behavior only — CloudFormation's ResourceStatusReason text is not constrained by any Smithy model; the reason is produced by the dynamic-reference resolver. For a {{resolve:secretsmanager:<secret-id>:SecretString:<json-key>}} whose <json-key> is not a key of the secret's JSON SecretString, the consuming resource fails with ResourceStatusReason exactly "Could not find a value associated with JSONKey in SecretString". That entire string is fixed text: AWS interpolates neither the secret id/ARN nor the key name nor the property name into it. The rest of the contract already holds in the emulator: the stack settles ROLLBACK_COMPLETE, the only *_FAILED resource is the one carrying the reference, the secret that resource would have created does not exist afterwards (DescribeSecret gives ResourceNotFoundException "Secrets Manager can't find the specified secret."), and the identical template naming a key the secret DOES hold deploys and receives that key's value.

Emulator: ResourceStatusReason is "Unable to resolve the dynamic reference '{{resolve:secretsmanager:<secret-arn>:SecretString:<key>}}': the secret <secret-arn> holds no <key> key" — an emulator-authored sentence that additionally interpolates the secret ARN twice and the missing key name, none of which AWS puts in this reason.

Why: a client or CI gate that classifies a deploy failure by matching the CloudFormation failure reason (missing-secret-key versus any other dynamic-reference problem) never matches AWS's constant string and misclassifies the failure; a reason copied from AWS docs or from a real deploy log never compares equal. Interpolating the secret ARN also turns a constant into a per-environment value, so even a substring match becomes environment-dependent.

Fix: the dynamic-reference resolver behind CreateStack in the cloudformation service crate — the {{resolve:secretsmanager:...:SecretString:<json-key>}} branch must emit AWS's fixed reason text when the key is absent, for every consuming resource type (here AWS::SecretsManager::Secret.SecretString), rather than composing its own sentence.

Traces: secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back.

## Evidence
Check secretsmanager.deploy-cfn-dynamic-reference-to-a-missing-json-key-rejected-and-rolled-back, replay detail verbatim:
  AssertionError: the resource must fail on the unresolvable key and nothing else; failures were [('Copy', 'CREATE_FAILED', "Unable to resolve the dynamic reference '{{resolve:secretsmanager:arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-behav-dynref-miss-src-9f4883310486-poMIRH:SecretString:no_such_key}}': the secret arn:aws:secretsmanager:us-west-2:000000000000:secret:sim2real-behav-dynref-miss-src-9f4883310486-poMIRH holds no no_such_key key")]

Expected (AWS): DescribeStackEvents shows exactly one failing row — ('Copy', 'CREATE_FAILED', 'Could not find a value associated with JSONKey in SecretString') — and the stack ends ROLLBACK_COMPLETE.
Actual (emulator): same logical id and same 'CREATE_FAILED' status (both correct), but the reason is the emulator's own sentence.
Fixed text requirement: the whole reason string 'Could not find a value associated with JSONKey in SecretString' is constant. The secret ARN, region, account, generated secret name and the key name 'no_such_key' appearing in the emulator's reason are per-run request values and are NOT part of AWS's message at all.

## Steps to Reproduce
1. CreateSecret with SecretString = {"username": "app", "password": "pw-value"}.
2. CreateStack with a single resource Copy of type AWS::SecretsManager::Secret whose Properties.SecretString is "{{resolve:secretsmanager:<that secret's ARN>:SecretString:no_such_key}}".
3. Poll DescribeStacks until the status is terminal: it is ROLLBACK_COMPLETE (correct).
4. DescribeStackEvents and read the CREATE_FAILED row for Copy: its ResourceStatusReason is "Unable to resolve the dynamic reference '...': the secret ... holds no no_such_key key" instead of AWS's "Could not find a value associated with JSONKey in SecretString".
5. Positive control (already passing): the same template with key "password" reaches CREATE_COMPLETE and the new secret's value is "pw-value".
