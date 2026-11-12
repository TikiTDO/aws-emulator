Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback codedeploy.batch-get-deployment-targets-lambda-target-shape codedeploy.lifecycle-event-hook-status-gates-deployment-progress
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback|run#38|AssertionError|assert rollback_message == (
Issue-Key: codedeploy.batch-get-deployment-targets-lambda-target-shape|run#23|AssertionError|assert "AllowTraffic" in event_names, (
Issue-Key: codedeploy.lifecycle-event-hook-status-gates-deployment-progress|run#30|AssertionError|assert fail_hook_status == "Failed", (
Issue-Commit: 89057c52301fd36ae7e24000691cb660eaaa73a8

# CodeDeploy Lambda deployments: rollbackInfo.rollbackMessage is re-authored, and lambdaTarget.lifecycleEvents carries neither AllowTraffic nor the BeforeAllowTraffic hook verdict (3 causes)

> _Note: this issue was identified against the code base at commit `89057c52301fd36ae7e24000691cb660eaaa73a8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDeploymentTarget

## Description
Three net-new root causes in the CodeDeploy emulator, all grounded in failing behavioral checks of the `codedeploy` suite section. They are independent fixes; close all three before submitting.

Cause 1 of 3: the auto-rollback's rollbackInfo.rollbackMessage is worded differently from AWS's.
Failing check: codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback
AWS: recorded behavior only — Smithy (com.amazonaws.codedeploy) types RollbackInfo.rollbackMessage as a free `Description` and documents it as "Information that describes the status of a deployment rollback". When a deployment under an autoRollbackConfiguration with event DEPLOYMENT_FAILURE fails and CodeDeploy spawns a codeDeployRollback deployment, the triggering deployment's rollbackInfo reads rollbackDeploymentId = the spawned deployment and rollbackMessage with this FIXED text: "Deployment <TRIGGERING_DEPLOYMENT_ID> terminated. Automatic rollback is triggered with a DeploymentId <ROLLBACK_DEPLOYMENT_ID>." — i.e. the literal words "Deployment ", " terminated. Automatic rollback is triggered with a DeploymentId " and the trailing "." are fixed; the only variable parts are the two deployment ids, which come from the request/state (the id of the deployment that was terminated, then the id of the rollback it spawned). The subject of the sentence is the TERMINATED deployment, not the rollback.
Emulator: it serves a differently-authored sentence whose subject is the rollback deployment and which has no "terminated." clause and no trailing period — of the form "Deployment <ROLLBACK_DEPLOYMENT_ID> was created to roll deployment <TRIGGERING_DEPLOYMENT_ID> back to the previously deployed revision". (The ids in the replay output, d-C6OUDRAS1 / d-B2TZD89KR, are that run's values, not part of the requirement.) rollbackDeploymentId, rollbackTriggeringDeploymentId and the rollback's creator were not the thing that diverged here.
Why: a client that reads or asserts on the rollback reason (incident automation, log scraping, CI/CD gates, test fixtures) does not match AWS's text, and the emulator's sentence inverts which deployment the message is about, so automation that extracts "the deployment that was terminated" from the leading id extracts the rollback instead.
Fix: the message construction in the auto-rollback path of the deployment progression driver — the code that writes rollbackInfo on the triggering deployment when a DEPLOYMENT_FAILURE rollback is spawned.
Note: open issue 20261109T135826Z-codedeploy-traffic-shifting-is-not-clock-paced-or.md lists this same check under its Cause 3 (ListDeploymentTargets returning no targets); that cause's replay key is a different assertion (`_hook_event_status#15 assert target_ids`) and the check now progresses past it, so that issue does not carry this divergence.
Traces: codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback.

Cause 2 of 3: a Lambda deployment target's lifecycleEvents list is empty — the AllowTraffic event (and the rest of the standard Lambda sequence) is never materialised.
Failing check: codedeploy.batch-get-deployment-targets-lambda-target-shape
AWS: Smithy LambdaTarget.lifecycleEvents is a LifecycleEventList of LifecycleEvent{lifecycleEventName, status, startTime, endTime, diagnostics}, documented as "The lifecycle events of the deployment to this target Lambda function", with status in LifecycleEventStatus (Pending|InProgress|Succeeded|Failed|Skipped|Unknown). For a Lambda deployment the target therefore reports the Lambda lifecycle sequence — BeforeAllowTraffic, AllowTraffic, AfterAllowTraffic — and a Succeeded all-at-once deployment's target carries an AllowTraffic event (the hook that flips the alias) in Succeeded status. BatchGetDeploymentTargets returns that target as deploymentTargetType "LambdaTarget".
Emulator: for a Succeeded CodeDeployDefault.LambdaAllAtOnce deployment, the single lambdaTarget returned by BatchGetDeploymentTargets has lifecycleEvents == [] — "expected lifecycleEvents to include AllowTraffic (the alias-flip hook), got []". The target id and status were served; only the event list is empty.
Why: lifecycleEvents is the only place a client can see which lifecycle step a deployment is on, which step failed, and when each started/ended. An empty list reads as a successful response with nothing to inspect, so progress-tracking and failure-diagnosis logic silently concludes the deployment had no lifecycle at all.
Fix: the deployment target state for the Lambda compute platform must record a LifecycleEvent per lifecycle step as the deployment progresses, and the GetDeploymentTarget / BatchGetDeploymentTargets serializers must emit it under lambdaTarget.lifecycleEvents.
Note: Cause 3 of open issue 20261109T135826Z-codedeploy-traffic-shifting-is-not-clock-paced-or.md describes the stronger symptom (no targets at all) for a different check and does not carry this check's replay key.
Traces: codedeploy.batch-get-deployment-targets-lambda-target-shape.

Cause 3 of 3: a BeforeAllowTraffic hook that reports Failed is not recorded on the lifecycle event it gated.
Failing check: codedeploy.lifecycle-event-hook-status-gates-deployment-progress
AWS: PutLifecycleEventHookExecutionStatus(deploymentId, lifecycleEventHookExecutionId, status=Failed) records the hook's verdict on the lifecycle event it was issued for, so GetDeploymentTarget(deploymentId, targetId).deploymentTarget.lambdaTarget.lifecycleEvents contains an entry with lifecycleEventName "BeforeAllowTraffic" and status "Failed" (LifecycleEventStatus enum), and the deployment ends Failed with the alias left on the pre-deployment version.
Emulator: the deployment does end Failed and the alias does stay put, but no BeforeAllowTraffic entry exists on the target, so the event's status reads as absent — "the BeforeAllowTraffic lifecycle event on deployment 'd-…' reads None, expected 'Failed'" (that deployment id is a per-run value). The hook's reported status is accepted by PutLifecycleEventHookExecutionStatus and then not reflected on the event. The analogous AfterAllowTraffic case does get recorded, so the gap is specific to the pre-traffic event.
Why: a client cannot tell WHICH lifecycle step failed or what the hook decided — it only sees a Failed deployment. Hook-driven validation pipelines that read the per-event status to report or branch on the failing step get no data, and the hook verdict it did report back to CodeDeploy appears to have been discarded.
Fix: the lifecycle-event/hook execution state in the deployment progression driver must create the BeforeAllowTraffic event when the hook is dispatched and stamp the status PutLifecycleEventHookExecutionStatus reports onto that event (with startTime/endTime), which the GetDeploymentTarget handler then serves.
Traces: codedeploy.lifecycle-event-hook-status-gates-deployment-progress.

## Evidence
Cause 1 — Expected rollbackInfo.rollbackMessage: "Deployment <TRIGGERING_DEPLOYMENT_ID> terminated. Automatic rollback is triggered with a DeploymentId <ROLLBACK_DEPLOYMENT_ID>." (fixed text; the two ids come from state). Actual: "Deployment d-C6OUDRAS1 was created to roll deployment d-B2TZD89KR back to the previously deployed revision" — a differently-worded sentence, subject inverted, no trailing period. Replay: "AssertionError: the rollbackMessage must name the deployment that was terminated ('d-B2TZD89KR'); it reads 'Deployment d-C6OUDRAS1 was created to roll deployment d-B2TZD89KR back to the previously deployed revision'" (codedeploy.after-allow-traffic-hook-failure-triggers-deployment-failure-rollback). The ids are per-run values.

Cause 2 — Expected: BatchGetDeploymentTargets(deploymentId, targetIds).deploymentTargets[0].lambdaTarget.lifecycleEvents includes a LifecycleEvent with lifecycleEventName "AllowTraffic" for a Succeeded LambdaAllAtOnce deployment. Actual: "AssertionError: expected lifecycleEvents to include AllowTraffic (the alias-flip hook), got []" — the list is empty (codedeploy.batch-get-deployment-targets-lambda-target-shape).

Cause 3 — Expected: GetDeploymentTarget(...).deploymentTarget.lambdaTarget.lifecycleEvents contains {lifecycleEventName: "BeforeAllowTraffic", status: "Failed"} after that hook calls PutLifecycleEventHookExecutionStatus(status='Failed'). Actual: no such entry, so the status reads as missing — "AssertionError: the BeforeAllowTraffic lifecycle event on deployment 'd-RMPA1X9OL' reads None, expected 'Failed'" (codedeploy.lifecycle-event-hook-status-gates-deployment-progress). The deployment's own terminal status Failed and the un-moved alias matched AWS.

## Steps to Reproduce
Cause 1: create a Lambda application and a deployment group whose autoRollbackConfiguration is enabled for DEPLOYMENT_FAILURE; CreateDeployment with an AppSpecContent revision whose Hooks declare an AfterAllowTraffic function that calls PutLifecycleEventHookExecutionStatus(status='Failed'); poll GetDeployment(deploymentId) until rollbackInfo.rollbackDeploymentId is populated, then read rollbackInfo.rollbackMessage and compare it against "Deployment <terminated id> terminated. Automatic rollback is triggered with a DeploymentId <rollback id>."

Cause 2: create a Lambda application + deployment group with deploymentConfigName CodeDeployDefault.LambdaAllAtOnce, CreateDeployment with an AppSpecContent revision moving an alias from CurrentVersion to TargetVersion, poll GetDeployment to Succeeded, then ListDeploymentTargets(deploymentId) and BatchGetDeploymentTargets(deploymentId, targetIds) — observe deploymentTargets[0].lambdaTarget.lifecycleEvents == [] instead of the BeforeAllowTraffic/AllowTraffic/AfterAllowTraffic sequence.

Cause 3: same setup, but the AppSpec Hooks declare a BeforeAllowTraffic function (named CodeDeployHook_*) whose handler calls PutLifecycleEventHookExecutionStatus(deploymentId, lifecycleEventHookExecutionId, status='Failed'); poll GetDeployment to terminal (Failed), then for each ListDeploymentTargets id call GetDeploymentTarget and look for the BeforeAllowTraffic entry in lambdaTarget.lifecycleEvents — observe no such entry (status reads None) instead of status 'Failed'.
