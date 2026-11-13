Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s
Issue-Key: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s|_poll#12|Timeout|time.sleep(interval_s)
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# SSM GetParameter never resolves /aws/reference/secretsmanager, so a CloudFormation-provisioned secret's Parameter Store reference never works or 404s

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: the Parameter Store front door to Secrets Manager is not served ===
AWS: ssm GetParameter on the reserved path /aws/reference/secretsmanager/<secret-name> resolves through to Secrets Manager and returns the AWSCURRENT SecretString as Parameter.Value. Once the backing secret is gone (here: deleted by deleting the CloudFormation stack that owns it, which removes it outright with no recovery window), the same read starts failing with ParameterNotFound rather than continuing to serve the value. Recorded behaviour of the reference path; the Secrets Manager side of the claim is pinned by the Smithy model only in that DescribeSecret 404s for a deleted secret.
Emulator: the reference never resolves. The check's positive control polls GetParameter on the reference for up to 90 s before it may delete anything, and the check runs out at the suite's 60 s cap ('Check timed out after 60.0s'), so neither the resolving read nor the post-deletion ParameterNotFound is ever observed.
Why: consuming a secret through Parameter Store is a first-class, documented path used to avoid teaching an application the Secrets Manager API. A client gets an unresolvable parameter for a secret that exists, and - worse for an emulator used to test teardown - no way to verify that a deleted secret stops being served.
Fix: the SSM GetParameter handler's reserved-path resolution (/aws/reference/secretsmanager/<name>, presented as a SecureString, resolved against the secret's AWSCURRENT value) and its not-found mapping onto ParameterNotFound when the backing secret is absent.
Traces: cloudformation__secretsmanager__ssm.cloudformation-stack-deletion-removes-the-secret-and-the-ssm-reference-then-404s.

## Evidence
'Error: Check timed out after 60.0s'. The check creates the stack, waits for CREATE_COMPLETE, then polls _get_parameter(ssm, '/aws/reference/secretsmanager/<stack>-secret', decrypt=True) until it answers 'OK' (timeout_s=90.0, interval_s=3.0) before deleting anything - the cap is reached inside that positive control, i.e. the reference never resolved.

## Steps to Reproduce
cloudformation CreateStack with an AWS::SecretsManager::Secret and an Output of its ARN; wait for CREATE_COMPLETE; ssm GetParameter(Name='/aws/reference/secretsmanager/<secret-name>', WithDecryption=True) - never resolves. Then DeleteStack and re-read: the expected follow-on is ParameterNotFound with the referencing message.
