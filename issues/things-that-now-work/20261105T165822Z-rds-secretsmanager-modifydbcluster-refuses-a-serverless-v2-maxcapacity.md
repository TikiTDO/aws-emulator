Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected
Issue-Key: rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected|run#9|AssertionError|assert error.get("Code") == "InvalidParameterValue", (
Issue-Commit: 6c23f9e5c79169c881c9ebb1fb44ffab91a88dbf

# rds__secretsmanager: ModifyDBCluster refuses a Serverless v2 MaxCapacity below MinCapacity with the wrong error code (InvalidParameterCombination) and emulator-invented wording

> _Note: this issue was identified against the code base at commit `6c23f9e5c79169c881c9ebb1fb44ffab91a88dbf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBCluster

## Description
Cause 1 of 1: ModifyDBCluster rejects an inverted Aurora Serverless v2 ACU range, but classifies and words the refusal differently from AWS.
AWS: Smithy: ModifyDBCluster input member ServerlessV2ScalingConfiguration -> shape ServerlessV2ScalingConfiguration with members MinCapacity / MaxCapacity (DoubleOptional, 'half-step increments'); the model declares no error for the min>max rule, so the classification is recorded behavior only. Recorded behavior: AWS refuses the call HTTP 400 with Error.Type='Sender', Error.Code='InvalidParameterValue' and the FIXED message 'Serverless v2 minimum capacity must be less than or equal to maximum capacity'. The whole sentence is fixed text — it names neither the submitted MinCapacity/MaxCapacity values nor the member path. The refusal is also total: the live ServerlessV2ScalingConfiguration is unchanged and no PendingModifiedValues entry is queued.
Emulator: refuses with Error.Code='InvalidParameterCombination' (wrong error code for this class) and Message='ServerlessV2ScalingConfiguration.MinCapacity (4) must not exceed MaxCapacity (1)' — the emulator's own template, which additionally interpolates the request's two capacity values (4 and 1 are this run's request values, not part of the requirement) and names the member path AWS does not mention.
Why: the error CODE is the contract clients branch on. A caller that retries or surfaces InvalidParameterValue for a bad scaling range (including the AWS CLI/SDK error-matching and any IaC tool that distinguishes a bad single value from a bad parameter combination) takes the wrong branch, because the emulator reports a combination fault. On top of that, no client matching or logging AWS's fixed sentence ever sees it, and the per-request capacity values in the message explode the message's cardinality for log grouping.
Fix: the ModifyDBCluster handler's ServerlessV2ScalingConfiguration validation must raise the InvalidParameterValue class (Type=Sender, HTTP 400) and emit AWS's fixed sentence verbatim, with no member path and no interpolated capacity values. The already-correct 'no effect, no pending change' behavior must be preserved.
Traces: rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected. Note the open issue 20261104T053605Z-rds-6-net-new-causes-applyimmediately-false-is.md touches Serverless v2 representation but does not carry this check's replay key or this refusal's class/wording; it does not cover this cause.

## Evidence
Check rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected
  Setup: CreateDBCluster(Engine='aurora-postgresql', ServerlessV2ScalingConfiguration={MinCapacity: 0.5, MaxCapacity: 1.0}) -> available
  Call: ModifyDBCluster(DBClusterIdentifier=<cluster>, ApplyImmediately=true, ServerlessV2ScalingConfiguration={MinCapacity: 4.0, MaxCapacity: 1.0})
  Expected: HTTP 400, Error.Type='Sender', Error.Code='InvalidParameterValue', Error.Message='Serverless v2 minimum capacity must be less than or equal to maximum capacity'
  Actual:   {'Type': 'Sender', 'Code': 'InvalidParameterCombination', 'Message': 'ServerlessV2ScalingConfiguration.MinCapacity (4) must not exceed MaxCapacity (1)'}
  Assertion: AssertionError: a MaxCapacity below MinCapacity must be rejected InvalidParameterValue, got {'Type': 'Sender', 'Code': 'InvalidParameterCombination', 'Message': 'ServerlessV2ScalingConfiguration.MinCapacity (4) must not exceed MaxCapacity (1)'}
  (The '4' and '1' in the emulator's message are the values this request submitted; AWS's sentence contains no values. The fixed text is the entire AWS sentence.)

## Steps to Reproduce
1. CreateDBSubnetGroup over two AZs.
2. CreateDBCluster(DBClusterIdentifier=<name>, Engine='aurora-postgresql', EngineVersion=<newest aurora-postgresql>, MasterUsername/MasterUserPassword, ServerlessV2ScalingConfiguration={MinCapacity: 0.5, MaxCapacity: 1.0}); poll DescribeDBClusters until Status='available'.
3. Call ModifyDBCluster(DBClusterIdentifier=<name>, ApplyImmediately=true, ServerlessV2ScalingConfiguration={MinCapacity: 4.0, MaxCapacity: 1.0}).
4. Inspect the ClientError: Error.Code is 'InvalidParameterCombination' (AWS: 'InvalidParameterValue') and Error.Message is the emulator's member-path/value template (AWS: 'Serverless v2 minimum capacity must be less than or equal to maximum capacity').
5. The subsequent assertions (live range unchanged, no PendingModifiedValues, and the valid widening to 1-8 ACU applying) are the positive control for the same call site.

Replay: behavioral suite check rds__secretsmanager.refarch-aur-serverless-scaling-max-below-min-rejected.
