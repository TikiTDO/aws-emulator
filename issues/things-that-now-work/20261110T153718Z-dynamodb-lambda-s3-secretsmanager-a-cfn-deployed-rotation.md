Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack
Issue-Key: dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack|_poll_until#9|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 76dff93be94f91562bba6a25a3aecfc1e2578190

# dynamodb__lambda__s3__secretsmanager: a CFN-deployed rotation Lambda plus RotationSchedule never rotates — the 4-step rotation protocol never runs and AWSCURRENT never moves

> _Note: this issue was identified against the code base at commit `76dff93be94f91562bba6a25a3aecfc1e2578190`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
One net-new root cause in the dynamodb__lambda__s3__secretsmanager suite. It is NOT covered by the open issues for this heading (20261104T232517Z-dynamodb-lambda-s3-secretsmanager-a-sam-auto-expand.md, 20261108T113236Z-dynamodb-lambda-s3-secretsmanager-s3-batch-operations-never-runs-a-lambdainvoke-job.md, 20261108T031654Z-dynamodb-lambda-s3-secretsmanager-3-net-new-causes-779faf.md), which carry other checks' replay keys; this check carries none.

Cause 1 of 1: a secret with an attached rotation Lambda and RotationSchedule never actually rotates, so AWSCURRENT never advances to a version the rotation function wrote.
AWS: recorded behavior only for the end-to-end effect; the surface is modelled. With a rotation Lambda attached (RotateSecret / an AWS::SecretsManager::RotationSchedule), RotateSecret with ForceRotateSecrets invokes the function four times for the same ClientRequestToken, with Step createSecret, setSecret, testSecret and finishSecret in that order; the function's createSecret call puts a new version staged AWSPENDING and finishSecret moves the AWSCURRENT stage onto it. Afterwards GetSecretValue (VersionStage AWSCURRENT) returns the value the rotation function generated, under a new VersionId, and the previous version carries AWSPREVIOUS.
Emulator: the check is reported "Error: Check timed out after 60.0s" with no assertion reached — the wait for AWSCURRENT to move to a new version never completes, so neither the four-step invocation order nor the rotated value nor the stage labels were ever compared.
Why: no secret ever rotates. Credential-rotation automation — the single most common Secrets Manager use case — silently does nothing, and because the stage labels never change a caller cannot tell a stalled rotation from a completed one.
Fix: the Secrets Manager rotation driver — RotateSecret (and the scheduled trigger) must invoke the attached Lambda with the createSecret/setSecret/testSecret/finishSecret steps for one ClientRequestToken, honour the AWSPENDING version the function creates, and move AWSCURRENT/AWSPREVIOUS on finishSecret.
Traces: dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack.

## Evidence
Check dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack, verdict [mismatch]:
  Error: Check timed out after 60.0s
with no assertion reached.

The awaited requirement, from the check's own docstring: "A CFN-deployed secret + rotation Lambda + RotationSchedule actually rotates: forcing rotation drives the 4-step Lambda rotation protocol end to end, moving AWSCURRENT to a new version whose value was written by the rotation function."

Expected: after RotateSecret(ForceRotateSecrets=True), GetSecretValue(VersionStage="AWSCURRENT") returns a NEW VersionId whose SecretString is the value the rotation function wrote, with the prior version labelled AWSPREVIOUS. Actual: AWSCURRENT never moves inside the run budget, so the check is killed with nothing compared. Secret names, ARNs, version ids and generated values are per-run values.

## Steps to Reproduce
1. Deploy a stack with an AWS::SecretsManager::Secret, a rotation Lambda (with its execution role and resource policy allowing secretsmanager to invoke it) and an AWS::SecretsManager::RotationSchedule binding them.
2. Call RotateSecret(SecretId=..., ForceRotateSecrets=True).
3. AWS invokes the function four times with Step createSecret, setSecret, testSecret, finishSecret for one ClientRequestToken; poll GetSecretValue(VersionStage="AWSCURRENT") until the VersionId changes and compare the value to what the function wrote; confirm the old version is AWSPREVIOUS.
4. The emulator never moves AWSCURRENT. Re-replay dynamodb__lambda__s3__secretsmanager.deploy-cfn-secret-rotation-lambda-stack to confirm the fix.
