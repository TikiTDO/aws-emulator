Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.stop-deployment-already-completed-blocked codedeploy.alarm-rollback-returns-live-traffic-to-prior-version
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.stop-deployment-already-completed-blocked|run#28|AssertionError|assert error == (
Issue-Key: codedeploy.alarm-rollback-returns-live-traffic-to-prior-version|wait_for_observations_after#45|AssertionError|assert got >= n, (
Issue-Commit: 3163fad6f190575ee3da7920eeb1a958f0b14066

# CodeDeploy: StopDeployment's DeploymentAlreadyCompletedException message is re-authored, and concurrent Lambda alias Invoke is too slow to fill a post-shift traffic window (2 causes)

> _Note: this issue was identified against the code base at commit `3163fad6f190575ee3da7920eeb1a958f0b14066`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
Two net-new root causes in the codedeploy surface, both grounded in failing behavioral checks that the replay report did NOT mark as already filed or owned.

Cause 1 of 2: StopDeployment on an already-terminal deployment raises the right error CODE but with a re-authored message.
Failing check ids: codedeploy.stop-deployment-already-completed-blocked.
AWS: Smithy (com.amazonaws.codedeploy#CodeDeploy_20141006) models StopDeployment with error DeploymentAlreadyCompletedException (client error, HTTP 400, httpError defaulted) and does not constrain the message text, so the message is recorded behavior only. The recording shows AWS refusing StopDeployment on a deployment that has already reached a terminal state with HTTP 400, error code DeploymentAlreadyCompletedException and the message template: fixed text "The deployment " + <deploymentId> + " is already in state " + <TERMINAL_STATUS_UPPERCASED>. The only variable parts are the deploymentId taken from the request and the deployment's terminal status taken from the emulator's state (FAILED / SUCCEEDED / STOPPED); no literal id from any one run is part of the requirement. The surrounding assertions passed, so the deployment did reach Failed with errorInformation.code NO_INSTANCES and stayed Failed after the refusal — only the message diverges.
Emulator: same code and status, but the message reads "The deployment <deploymentId> has already completed, current state FAILED." — different fixed text ("has already completed, current state " instead of " is already in state ") plus a trailing period AWS does not emit.
Why: clients and SDK-based tooling that branch on, log, or surface the refusal reason compare or parse this sentence (error-message assertions in deployment orchestration, CloudWatch/log alarm patterns, user-facing reporting). A re-authored sentence makes a correctly-refused StopDeployment look like a different refusal and breaks any matcher calibrated against AWS's wording, even though the code is right.
Fix: the StopDeployment handler's terminal-state refusal path — the place that constructs the DeploymentAlreadyCompletedException message — must emit AWS's template with the request's deploymentId and the stored terminal status. No state-model change is needed; the deployment record already holds the status it interpolates.

Cause 2 of 2: the Lambda alias Invoke data plane cannot sustain enough throughput to fill the canary window that an alarm rollback is measured against.
Failing check ids: codedeploy.alarm-rollback-returns-live-traffic-to-prior-version.
AWS: recorded behavior only — neither the CodeDeploy nor the Lambda model constrains invoke latency or concurrency. The recording this check encodes has 8 concurrent workers, each sleeping 0.4 s between synchronous RequestResponse invokes of a warm trivial alias (the handler only returns a constant marker), sustaining ~20 invokes/s, so 300 observations land in ~15 s, far inside the 120 s budget; real Lambda serves concurrent callers of one alias in parallel and returns in tens of milliseconds.
Emulator: in the canary window (after AdditionalVersionWeights appeared on the alias and before the alarm is driven to ALARM) only 232 observations landed in the 120 s budget where 300 were needed — an aggregate rate of ~1.9 invokes/s against the ~20/s the same requester records on AWS, roughly an order of magnitude short. Every observation is appended unconditionally after each invoke attempt, and no ClientError, non-200 StatusCode or FunctionError was recorded, so the shortfall is wall-clock latency inside Invoke, not refused or dropped calls. Hypothesis: concurrent alias invokes are serialized behind a single lock or a single executor in the invoke path rather than each invoke genuinely costing seconds.
Why: a client cannot observe the effect a CodeDeploy traffic shift or an alarm-triggered rollback exists for. Every distinguishing property — that a minority of live invokes reach the canary version while the shift is in flight, and that live invokes stop reaching the rolled-back version afterwards — is a statement about a population of invokes inside a window the shift's own clock pacing bounds. At ~1.9/s such a window holds too few invokes to assert anything, so the rollback is unverifiable through the data plane even when the control plane reports it; more generally any fan-out of synchronous invokes (load generator, canary, orchestration fan-out step) sees throughput far off AWS and times out on AWS-calibrated budgets.
Fix: the Lambda data-plane Invoke handler and its admission/dispatch path (the component that admits a RequestResponse invocation, resolves the alias and its weighted routing, and runs the handler) must serve concurrent invocations of one alias in parallel, with a warm trivial handler's round trip costing milliseconds. The alias/weight state model does not need to change.
Related but NOT covering this: open issue 20261111T052901Z-codedeploy-concurrent-synchronous-lambda-alias-invoke-is-7x.md owns the PRE-SHIFT baseline failure of this same check (wait_for_observations, 500 observations in 180 s). That phase now passes and the check fails later, in wait_for_observations_after on the canary window, so its replay key does not carry this failure; same throughput cause, new failing phase. Also related but not covering: 20261109T135826Z-codedeploy-traffic-shifting-is-not-clock-paced-or.md owns the control-plane side of traffic shifting only.

Traces: codedeploy.stop-deployment-already-completed-blocked (Cause 1); codedeploy.alarm-rollback-returns-live-traffic-to-prior-version (Cause 2).

## Evidence
Cause 1 — codedeploy.stop-deployment-already-completed-blocked, mismatch. AssertionError at check line 83 (`assert error == (...)`):
  actual   ('DeploymentAlreadyCompletedException', 'The deployment d-2AFCTY2ZG has already completed, current state FAILED.')
  expected ('DeploymentAlreadyCompletedException', 'The deployment <deploymentId> is already in state FAILED')
Status (HTTP 400) and error code match; only the message text diverges. Fixed text required by AWS: "The deployment " ... " is already in state " ... ; variable parts: the deploymentId from the StopDeployment request and the terminal status from the emulator's deployment record (here FAILED). The id d-2AFCTY2ZG is a per-run value, not part of the requirement. Everything earlier in the check passed: the deployment reached Failed with errorInformation {code: NO_INSTANCES, message: "The deployment failed because no instances were found for your deployment group. ..."}, StopDeployment on the still-active deployment of the sibling group was accepted and drove it to Stopped, and the refused stop left the terminal deployment Failed.

Cause 2 — codedeploy.alarm-rollback-returns-live-traffic-to-prior-version, mismatch. AssertionError at check line 207 -> wait_for_observations_after (line 463): "only 232 observations landed after t=291.8s within 120s, needing 300". The requester is 8 workers x 0.4 s interval (WORKERS=8, INTERVAL_S=0.4) invoking <fn>:<alias> synchronously; BREACH_OBSERVE_TARGET=300 must land inside the canary window before the alarm is forced to ALARM.
  Expected (recorded AWS): ~20 invokes/s aggregate, 300 observations in ~15 s of the 120 s budget.
  Actual (emulator): 232 observations in 120 s, ~1.9 invokes/s.
The counts 232/300 and the 120 s timeout are this run's values; the requirement is the RATE. No invoke was recorded as a ClientError, a non-200 StatusCode or a FunctionError, so the emulator answered every call — too slowly.

## Steps to Reproduce
Cause 1:
1. codedeploy.CreateApplication(computePlatform="Server") and CreateDeploymentGroup with an onPremisesInstanceTagFilters entry whose key/value can never match any registered instance.
2. CreateDeployment against that group with an S3 revision; poll GetDeployment until terminal. It reaches status Failed with errorInformation.code NO_INSTANCES.
3. Call StopDeployment(deploymentId=<that deployment>). Expected: HTTP 400, code DeploymentAlreadyCompletedException, message "The deployment <deploymentId> is already in state FAILED". Actual: same code, message "The deployment <deploymentId> has already completed, current state FAILED.".
4. GetDeployment again: status is still Failed (this part matches).

Cause 2:
1. Create a Lambda function with a trivial handler returning a constant marker; publish two versions and point an alias at the current one.
2. Start 8 concurrent threads, each looping: lambda.Invoke(FunctionName="<fn>:<alias>", Payload=b"{}"), record the completion, sleep 0.4 s.
3. Create a CodeDeploy Lambda canary deployment (10% / 1 min) on that alias with an associated CloudWatch alarm, and wait until AdditionalVersionWeights appears on the alias.
4. From 5 s after that weight appeared, count completed invokes for 120 s. Real AWS: ~2400 (~20/s), 300 reached in ~15 s. Emulator: 232 (~1.9/s), 300 never reached, so the check aborts before it can force the alarm to ALARM and observe the rollback.
5. Replay codedeploy.alarm-rollback-returns-live-traffic-to-prior-version.
