Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version
Issue-Key: cloudwatch__stepfunctions.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version|_ledger#28|Timeout|time.sleep(_POLL_S)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# Step Functions publishes no alias/version-attributed AWS/States execution metrics, so an alias traffic split cannot be observed (1 cause)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartExecution

## Description
One net-new root cause.

Cause 1 of 1: executions started through a state-machine alias are not attributed to AWS/States series for the alias and the routed version.
AWS: StartExecution against a state-machine alias ARN publishes AWS/States execution metrics (ExecutionsStarted / ExecutionsSucceeded and friends) split across the dimension sets naming the alias and the version the alias routed to, so the split can be read back per alias and per version. Recorded behavior: the per-alias and per-version series both appear with the counts the run produced.
Emulator: the check starts its executions and then polls for those series, and is killed at the harness 60 s cap — no alias- or version-dimensioned AWS/States datapoints ever appear.
Why: alias-based traffic shifting is the Step Functions deployment primitive; without per-alias/per-version metrics a client cannot verify what fraction of executions a new version received, so a canary or linear rollout cannot be monitored or automatically rolled back.
Fix: AWS/States metric publication from the Step Functions execution path, dimensioned by the alias and the resolved version, into the cloudwatch metric store.
Traces: cloudwatch__sfn.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version.

## Evidence
"Error: Check timed out after 60.0s" (cloudwatch__sfn.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version) — the check polls for the alias- and version-dimensioned AWS/States series and never sees a datapoint.

## Steps to Reproduce
1. CreateStateMachine, PublishStateMachineVersion twice, CreateStateMachineAlias with a routing configuration across the two versions.
2. StartExecution repeatedly against the alias ARN and let the executions finish.
3. Poll ListMetrics / GetMetricStatistics in AWS/States for the alias-dimensioned and version-dimensioned execution series -> no datapoints ever appear; the check hangs to the 60 s cap.
