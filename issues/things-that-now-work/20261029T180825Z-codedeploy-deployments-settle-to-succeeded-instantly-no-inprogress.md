Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.concurrent-deployment-same-group-rejected-while-active codedeploy.deployment-target-lifecycle-transitions codedeploy.lifecycle-event-hook-status-gates-deployment-progress codedeploy.deploy-codedeploy-deploymentgroup-cfn-autorollback-enforced codedeploy.lambda-deployment-all-at-once-shifts-alias
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.concurrent-deployment-same-group-rejected-while-active|run#21|AssertionError|assert dep1_state["status"] == "InProgress", f"expected dep1 {dep1_id!r} InProgress, got {dep1_state}"
Issue-Key: codedeploy.deployment-target-lifecycle-transitions|run#13|AssertionError|assert instance_target["status"] == "InProgress", (
Issue-Key: codedeploy.lifecycle-event-hook-status-gates-deployment-progress|run#29|AssertionError|assert fail_info["status"] == "Failed", (
Issue-Key: codedeploy.deploy-codedeploy-deploymentgroup-cfn-autorollback-enforced|run#27|AssertionError|assert dep2["status"] == "Failed", dep2
Issue-Key: codedeploy.lambda-deployment-all-at-once-shifts-alias|_poll#18|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# CodeDeploy deployments settle to Succeeded instantly: no InProgress window, hook status and per-group concurrency unenforced, alias never moved (4 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
A CodeDeploy deployment in this emulator is a record, not a process: CreateDeployment mints an id and the deployment is already terminal 'Succeeded' a few hundred milliseconds later (recorded createTime/startTime/completeTime 250ms apart), without consulting lifecycle hooks, without ever being observable as InProgress, and without performing the traffic shift. Four distinct observable consequences, four causes.

Cause 1 of 4: there is no non-terminal deployment state, so per-deployment-group serialization is unenforceable.
AWS: recorded behavior only — a deployment whose BeforeAllowTraffic hook has not yet reported stays 'InProgress' (GetDeployment and BatchGetDeployments both report it), and while a group has a non-terminal deployment a second CreateDeployment against that SAME group is refused with DeploymentLimitExceededException (Smithy: CreateDeployment models DeploymentLimitExceededException, HTTP 400); once the first deployment is terminal the group accepts a new one again.
Emulator: GetDeployment reports 'Succeeded' at the first poll (status jumps Created->Succeeded within ~250ms), so the InProgress assertion fails before the concurrency leg is even reached; nothing in the emulator can refuse the second CreateDeployment because no deployment is ever active.
Why: clients poll GetDeployment to wait for a deployment and handle DeploymentLimitExceededException to serialize pipeline stages; against the emulator the wait returns immediately and two concurrent pipeline runs both 'succeed', hiding a failure that is guaranteed in the account.
Fix: the deployment state machine (status transitions and the per-group active-deployment guard in CreateDeployment) in the codedeploy service crate.
Traces: codedeploy.concurrent-deployment-same-group-rejected-while-active.

Cause 2 of 4: deployment targets are reported terminal immediately instead of InProgress.
AWS: recorded behavior only — GetDeploymentTarget/ListDeploymentTargets become available once the deployment enumerates its target, and the target's status is 'InProgress' as soon as CodeDeploy dispatches to it, before any on-premises agent polls; it only becomes Succeeded after the agent reports.
Emulator: the enumerated instance target reads status 'Succeeded' straight away, with no agent having ever checked in.
Why: agent-side and target-progress tooling (and anything that waits for a target to leave InProgress) cannot distinguish 'dispatched' from 'done', so a deployment to a host that never ran anything looks complete.
Fix: deployment-target state tracking in the deployment state machine (GetDeploymentTarget/ListDeploymentTargets status derivation).
Traces: codedeploy.deployment-target-lifecycle-transitions.

Cause 3 of 4: a lifecycle hook reporting Failed does not fail the deployment, so AutoRollbackConfiguration never fires.
AWS: recorded behavior only, consistent with Smithy PutLifecycleEventHookExecutionStatus (status member LifecycleEventStatus incl. 'Failed') — when the BeforeAllowTraffic hook reports Failed the deployment terminates 'Failed', the traffic shift does not happen, and a deployment group with AutoRollbackConfiguration(Events=[DEPLOYMENT_FAILURE]) has CodeDeploy automatically start a rollback deployment that redeploys the last successful revision and becomes the group's lastSuccessfulDeployment.
Emulator: the deployment whose hook reported 'Failed' ends 'Succeeded' (observed deploymentId reads status 'Succeeded' with no errorInformation), and consequently no rollback deployment is created.
Why: hook-gated deployments are the only safety mechanism on the Lambda compute platform; an emulator that reports Succeeded regardless of hook status validates a deployment pipeline that would roll back in the account, and CFN stacks that declare auto-rollback appear to enforce it when they do not.
Fix: the deployment state machine — the hook-status gate before traffic shift, and the auto-rollback trigger on DEPLOYMENT_FAILURE.
Traces: codedeploy.lifecycle-event-hook-status-gates-deployment-progress, codedeploy.deploy-codedeploy-deploymentgroup-cfn-autorollback-enforced.

Cause 4 of 4: an AllAtOnce Lambda deployment reports Succeeded without moving the alias.
AWS: recorded behavior only — a CodeDeployDefault.LambdaAllAtOnce deployment whose AppSpec names CurrentVersion/TargetVersion moves the function alias to TargetVersion as part of reaching Succeeded; GetAlias then reports FunctionVersion == TargetVersion.
Emulator: the deployment reports terminal success but the alias never reaches TargetVersion, so the check's alias poll never converges and the check exhausts its 60s budget.
Why: the whole point of a Lambda deployment is the alias weight/version move; a client that deploys and then invokes the alias gets the old version while CodeDeploy claims success.
Fix: the Lambda traffic-shift step of the deployment state machine (alias update via the Lambda service state) for AppSpecContent revisions.
Traces: codedeploy.lambda-deployment-all-at-once-shifts-alias.

Related open issues that do NOT carry these checks' replay keys (the suite did not mark these checks as owned by them, and they must be closed together): "CodeDeploy deployments report success without executing: the Lambda alias never moves..." (20261024T114701Z), "CodeDeploy Lambda deployment reports Succeeded while the function alias is never moved to the AppSpec TargetVersion" (20261029T064006Z) and "CodeDeploy time-based traffic shifting and alarm-driven rollback are stored but never executed" (20261025T013642Z).

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout (89/135 passed; emulator serving, bootstrap_signal=false).

1. codedeploy.concurrent-deployment-same-group-rejected-while-active — expected dep1 'InProgress' while its 20s-sleeping BeforeAllowTraffic hook runs; got deploymentInfo status 'Succeeded' with createTime 17:58:15.921, startTime 17:58:16.171, completeTime 17:58:16.421 (500ms total), so the follow-up assertion that a second CreateDeployment on the same group raises DeploymentLimitExceededException was never reached.
2. codedeploy.deployment-target-lifecycle-transitions — AssertionError: "expected InProgress (target is enumerated as soon as codedeploy dispatches to it, before any on-premises agent ever polls), got 'Succeeded'".
3. codedeploy.lifecycle-event-hook-status-gates-deployment-progress — AssertionError: "deployment 'd-N6UO0O896' whose BeforeAllowTraffic hook reported 'Failed' ended 'Succeeded', expected 'Failed'". codedeploy.deploy-codedeploy-deploymentgroup-cfn-autorollback-enforced — same shape: the CFN-created group with AutoRollbackConfiguration(Events=[DEPLOYMENT_FAILURE]) returned status 'Succeeded' for the hook-failing deployment d-JYYG9RYDH (expected 'Failed' plus an automatic rollback deployment).
4. codedeploy.lambda-deployment-all-at-once-shifts-alias — check timed out after 60.0s waiting for the alias to reach the AppSpec TargetVersion after the deployment reported terminal success.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): CreateDeployment models DeploymentLimitExceededException (HTTP 400); PutLifecycleEventHookExecutionStatus takes status: LifecycleEventStatus and models LifecycleEventAlreadyCompletedException; deployment status/errorInformation are output members of GetDeployment. Timings, deployment ids and generated names above are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite; all four clusters reproduce.
2. Cause 1: create a Lambda application + deployment group whose BeforeAllowTraffic hook Lambda sleeps ~20s before calling PutLifecycleEventHookExecutionStatus; CreateDeployment; poll GetDeployment -> expect 'InProgress'; while it is non-terminal call CreateDeployment again on the same group -> expect DeploymentLimitExceededException; after it is terminal a new deployment must be accepted.
3. Cause 2: register an on-premises instance, create a Server deployment group matching its tags, CreateDeployment, then ListDeploymentTargets/GetDeploymentTarget -> expect target status 'InProgress' with no agent running.
4. Cause 3: same Lambda setup with the hook reporting status='Failed' -> expect the deployment terminal 'Failed'; with AutoRollbackConfiguration(Events=['DEPLOYMENT_FAILURE']) expect a new rollback deployment that reaches Succeeded and becomes the group's lastSuccessfulDeployment.
5. Cause 4: CreateDeployment with an AppSpecContent revision naming CurrentVersion=1/TargetVersion=2 on CodeDeployDefault.LambdaAllAtOnce; after Succeeded, GetAlias must report FunctionVersion == 2.
