Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__stepfunctions.metrics-states-stream-activity-metrics-attribute-worker-outcomes-to-the-activity-arn-series cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves cloudwatch__stepfunctions.metrics-states-consumed-capacity-counts-start-execution-api-calls cloudwatch__stepfunctions.metrics-states-version-and-alias-count-increase-on-publish-and-alias-create cloudwatch__stepfunctions.metrics-states-version-count-unmoved-by-a-rejected-publish-state-machine-version cloudwatch__stepfunctions.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version cloudwatch__stepfunctions.obs-sfn-1-redrive-to-succeeded-counts-once-and-second-redrive-rejected cloudwatch__stepfunctions.obs-sfn-2-alias-split-executions-attribute-to-alias-and-routed-version-for-express
Issue-Key: cloudwatch__stepfunctions.metrics-states-stream-activity-metrics-attribute-worker-outcomes-to-the-activity-arn-series|_poll#27|AssertionError|raise AssertionError(f"timed out after {_METRIC_BUDGET_S}s waiting for {what}; last {last!r}")
Issue-Key: cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves|_poll#31|AssertionError|raise AssertionError(f"timed out after {_METRIC_BUDGET_S}s waiting for {what}; last {last!r}")
Issue-Key: cloudwatch__stepfunctions.metrics-states-consumed-capacity-counts-start-execution-api-calls|_poll_sum_at_least#14|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__stepfunctions.metrics-states-version-and-alias-count-increase-on-publish-and-alias-create|_poll_metric#10|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__stepfunctions.metrics-states-version-count-unmoved-by-a-rejected-publish-state-machine-version|_poll_metric#12|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__stepfunctions.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version|_ledger#28|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__stepfunctions.obs-sfn-1-redrive-to-succeeded-counts-once-and-second-redrive-rejected|_ledger#35|AssertionError|raise AssertionError(
Issue-Key: cloudwatch__stepfunctions.obs-sfn-2-alias-split-executions-attribute-to-alias-and-routed-version-for-express|run#17|AssertionError|assert resp["stateMachineArn"] == alias_arn, (
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__stepfunctions: 5 net-new causes — AWS/States activity, ConsumedCapacity, version/alias-count and redrive metric families are never published, alias/version-dimensioned execution series are absent, and a sync execution started through an alias reports an unqualified stateMachineArn

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 5: the AWS/States ACTIVITY metric family is never published, although the execution family is.
AWS: an activity task published to an activity ARN increments ActivitiesScheduled, ActivitiesStarted, ActivitiesSucceeded, ActivitiesFailed and ActivitiesTimedOut under {ActivityArn}, so worker outcomes are attributable to the activity series while the state machine's ExecutionsStarted/Succeeded/Failed stay under {StateMachineArn}; a task whose heartbeat/timeout expires counts ActivitiesTimedOut and fails its execution, and a late SendTaskSuccess after the timeout is refused and moves nothing. Recorded behavior only for the series content (the metric names are documented, not Smithy shapes).
Emulator: every activity counter reads 0.0 while the execution counters are correct (ExecutionsStarted 2.0, Succeeded 1.0, Failed 1.0 in the same run), so no worker outcome is ever metered and the timeout arm's control succeeds with ActivitiesSucceeded still 0.
Why: activity-based (worker-poll) workflows are unobservable: a customer cannot see scheduled backlog, worker failures or task timeouts, which are the only signals for a stuck worker fleet.
Fix: the Step Functions activity task lifecycle (GetActivityTask/SendTaskSuccess/SendTaskFailure/heartbeat timeout) must emit the AWS/States activity counters under {ActivityArn}.
Failing checks: cloudwatch__stepfunctions.metrics-states-stream-activity-metrics-attribute-worker-outcomes-to-the-activity-arn-series, cloudwatch__stepfunctions.metrics-states-stream-late-activity-result-rejected-after-task-timeout-and-only-timed-out-moves.

Cause 2 of 5: AWS/States ConsumedCapacity is never published for StartExecution API calls.
AWS: StartExecution calls are metered into AWS/States ConsumedCapacity dimensioned on {APIName: 'StartExecution'}, so the sum over a burst equals the number of calls made.
Emulator: the series never reaches the expected count and reads an empty datapoint list with a sum of 0 after 180 s.
Why: the API-level capacity signal that tells a customer they are approaching the StartExecution quota does not exist, so throttling arrives with no warning.
Fix: the StartExecution request path must increment AWS/States ConsumedCapacity under the APIName dimension.
Failing check: cloudwatch__stepfunctions.metrics-states-consumed-capacity-counts-start-execution-api-calls.

Cause 3 of 5: AWS/States VersionCount and AliasCount are never published.
AWS: publishing a state-machine version raises VersionCount under {ResourceArn: <state machine ARN>} and creating an alias raises AliasCount on the same dimension; a REJECTED PublishStateMachineVersion leaves the count unmoved — so the count is the observable that distinguishes an accepted publish from a refused one.
Emulator: VersionCount reads None after a successful publish (both after publish-on-create and after an explicit first publish), so neither the increase nor the 'unmoved by a rejected publish' assertion can be evaluated.
Why: a customer cannot see how many versions/aliases a state machine carries or detect that a publish was refused, and the quota on versions becomes invisible.
Fix: the state-machine version/alias write path must publish VersionCount and AliasCount under {ResourceArn} on each accepted mutation.
Failing checks: cloudwatch__stepfunctions.metrics-states-version-and-alias-count-increase-on-publish-and-alias-create, cloudwatch__stepfunctions.metrics-states-version-count-unmoved-by-a-rejected-publish-state-machine-version.

Cause 4 of 5: execution metrics are not published under the alias and version dimensions, and the redrive family is absent.
AWS: an execution started through an alias is metered three times over — under {StateMachineArn}, under the alias ARN and under the routed version ARN — so a weighted alias's traffic split is visible; and a redriven execution increments ExecutionsRedriven plus RedrivenExecutionsSucceeded/Failed exactly once per redrive.
Emulator: only the {StateMachineArn} series carries data (Started[sm] = 6.0) while Started[alias], Started[v1], Started[v2], Succeeded[alias], AliasCount and VersionCount are all 0.0; and after a redrive to success ExecutionsRedriven and RedrivenExecutionsSucceeded both stay 0.0 while the base execution counters are right.
Why: alias-based canary/blue-green deployments cannot be judged — a customer cannot see which version served which executions — and a redrive produces no record, so re-run success cannot be distinguished from the original failure.
Fix: the execution start/termination metric emission must publish under the alias and routed-version dimensions in addition to the state machine, and the RedriveExecution path must emit the redrive counters.
Failing checks: cloudwatch__stepfunctions.obs-sfn-1-alias-split-executions-attribute-to-alias-and-routed-version, cloudwatch__stepfunctions.obs-sfn-1-redrive-to-succeeded-counts-once-and-second-redrive-rejected.

Cause 5 of 5: a synchronous EXPRESS execution started through an alias reports the UNQUALIFIED state machine ARN.
AWS: StartSyncExecution against an alias ARN returns stateMachineArn qualified with the alias — 'arn:aws:states:<region>:<account>:stateMachine:<name>:<alias>' — so the caller can see which alias routed the run. Smithy (sfn 2016-11-23): StartSyncExecutionOutput$stateMachineArn. The requirement is the qualified form; the name and alias come from the request.
Emulator: the response carries the bare 'arn:aws:states:<region>:<account>:stateMachine:<name>' with the ':<alias>' qualifier stripped.
Why: a caller loses the routing information for the run it just made, so alias-aware clients and logs cannot attribute a sync express result to the alias that served it.
Fix: the StartSyncExecution handler/serializer — echo the qualified ARN the request named rather than resolving it down to the state machine.
Failing check: cloudwatch__stepfunctions.obs-sfn-2-alias-split-executions-attribute-to-alias-and-routed-version-for-express.

## Evidence
Cause 1 — Expected {('act','ActivitiesScheduled'):2.0, ('act','ActivitiesStarted'):2.0, ('act','ActivitiesSucceeded'):1.0, ('act','ActivitiesFailed'):1.0, ('sm','ExecutionsStarted'):2.0, ...}; actual all four 'act' counters 0.0 with the 'sm' counters correct. Timeout arm: expected {'slow ActivitiesTimedOut':1.0,'slow ExecutionsFailed':1.0,'ctrl ActivitiesSucceeded':1.0,'ctrl ExecutionsSucceeded':1.0}; actual all 0.0 except ctrl ExecutionsSucceeded 1.0.
Cause 2 — Expected ConsumedCapacity[{APIName: StartExecution}] >= 12; actual 'did not reach at least 12 within 180s; last summed 0: []'.
Cause 3 — Expected VersionCount[{ResourceArn: <sm arn>}] == 1.0 after publish; actual 'last read None' (both checks).
Cause 4 — Expected {'Started[sm]':6.0,'Started[alias]':6.0,'Started[v1]':5.0,'Started[v2]':1.0,'Succeeded[alias]':6.0,'AliasCount max':1.0,'VersionCount max':2.0}; actual only Started[sm] 6.0, everything else 0.0. Redrive: expected ExecutionsRedriven 1.0 and RedrivenExecutionsSucceeded 1.0; actual both 0.0.
Cause 5 — Expected stateMachineArn 'arn:aws:states:<region>:<account>:stateMachine:<name>:<alias>'; actual 'arn:aws:states:<region>:<account>:stateMachine:<name>' (names/account are per-run; the missing ':<alias>' qualifier is the divergence).

## Steps to Reproduce
Cause 1: CreateActivity + a state machine with an activity task; run two executions (one worker success, one failure) and poll AWS/States Activities* under {ActivityArn}; separately let one task time out and then SendTaskSuccess late.
Cause 2: issue 12 StartExecution calls and poll AWS/States ConsumedCapacity under {APIName: 'StartExecution'}.
Cause 3: PublishStateMachineVersion and CreateStateMachineAlias, then poll VersionCount/AliasCount under {ResourceArn}; repeat with a publish that is refused.
Cause 4: create two versions and a weighted alias, start 6 executions through the alias, and poll ExecutionsStarted/Succeeded under the state machine, alias and version ARNs; then RedriveExecution a failed execution and poll ExecutionsRedriven/RedrivenExecutionsSucceeded.
Cause 5: StartSyncExecution against an EXPRESS state machine's alias ARN and read stateMachineArn from the response.
