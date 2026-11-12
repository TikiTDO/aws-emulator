Issue-Source: trace-sweep-divergence
Issue-Trace: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value
Issue-Step: task#13
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.ApplicationId
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.ConfigurationLocationUri
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.ConfigurationName
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.ConfigurationProfileId
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.ConfigurationVersion
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.DeploymentDurationInMinutes
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.DeploymentNumber
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.DeploymentStrategyId
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.EnvironmentId
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.EventLog
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.FinalBakeTimeInMinutes
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.GrowthFactor
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.GrowthType
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.PercentageComplete
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.StartedAt
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.State
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.Type
Issue-Key: appconfig|StopDeployment|2xx->4xx|$.VersionLabel
Issue-Key: appconfig|StopDeployment|2xx->4xx|status
Issue-Key: appconfig|GetDeployment|2xx->2xx|$.State
Issue-Key: appconfig|StartDeployment|2xx->2xx|$.DeploymentNumber
Issue-Related: 20261103T021132Z-appconfig-deployment-lifecycle-advances-instantly-through-deploy-bake.md
Issue-Related: 20261105T012540Z-appconfig-synthesized-response-content-diverges-deployment-eventlog-entries.md
Issue-Commit: a71435d592561fc10ca1c4dcda74d11758c7bd15

# appconfig StopDeployment refuses a baking deployment so the user rollback never happens (3 causes)

> _Note: this issue was identified against the code base at commit `a71435d592561fc10ca1c4dcda74d11758c7bd15`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDeployment

## Description
This issue bundles 3 net-new causes that share one root cause: an all-at-once deployment with a non-zero FinalBakeTimeInMinutes is driven to a TERMINAL state inside the emulator before the caller can stop it, so the user-requested rollback that AWS performs synchronously is refused outright, the deployment's reported end state is COMPLETE rather than ROLLED_BACK, and the deployment-number sequence for the environment is left one ahead of AWS's.

Related open issues that do NOT cover these clusters: 20261103T021132Z-appconfig-deployment-lifecycle-advances-instantly-through-deploy-bake.md describes an earlier shape of this fixture's stop/get steps (StopDeployment answering 202 with State ROLLING_BACK); the replay now reports StopDeployment answering 400 with no Deployment body at all and GetDeployment reporting COMPLETE, and reports both clusters as NOT owned by that or any other open issue. 20261106T033908Z-appconfig-stopdeployment-s-unknown-deploymentnumber-resourcenotfoundexception-message-omits.md covers only the unknown-DeploymentNumber 404 message, not a refusal of a known, in-flight deployment.

Cause 1 of 3: StopDeployment refuses to stop an in-flight (baking) deployment with 400 instead of rolling it back with 202.
  AWS: Smithy: StopDeployment DELETE /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, http code 202, output shape Deployment; errors BadRequestException (400), ResourceNotFoundException (404), InternalServerException (500). Stopping a deployment that has not reached a terminal state is a SUCCESS on AWS, not a client error: the recorded 202 response carries the fully populated Deployment — State 'ROLLED_BACK', PercentageComplete 0.0, DeploymentNumber, ApplicationId, EnvironmentId, ConfigurationProfileId, ConfigurationName, ConfigurationLocationUri, ConfigurationVersion, VersionLabel, DeploymentStrategyId, DeploymentDurationInMinutes, FinalBakeTimeInMinutes, GrowthType, GrowthFactor, Type 'USER', StartedAt, and an EventLog whose newest entry is EventType 'ROLLBACK_COMPLETED' / TriggeredBy 'USER' / Description 'Deployment rolled back by user request'. BadRequestException is reserved for a malformed or inapplicable request; a known deployment in a non-terminal state is neither.
  Emulator: the same DELETE answers HTTP 400 and returns NO Deployment representation at all — every modeled output member is absent from the body. Hypothesis: because the emulator has already driven this deployment to a terminal state inside StartDeployment (it reports BAKING at 100% on creation and COMPLETE moments later, see Cause 2), its StopDeployment guard classifies the stop as inapplicable and refuses it.
  Why: a client that starts a deployment and then aborts it (the standard safe-rollback path, and what the AWS console's Stop deployment button does) is told its request was malformed. It cannot roll back at all: the deployment stays applied, the client's error branch surfaces a 400 that no retry or parameter change can clear, and no ROLLED_BACK state is ever reachable.
  Fix: StopDeployment's handler and the deployment state machine it drives — a deployment in DEPLOYING/BAKING must be accepted, reverted within the call, and returned as a 202 Deployment with State ROLLED_BACK, PercentageComplete 0.0 and a USER-triggered ROLLBACK_COMPLETED event appended; the 400 guard must only fire for requests AWS itself refuses.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#13.

Cause 2 of 3: after the stop, GetDeployment reports the deployment as completed rather than rolled back, and its event log omits ActionInvocations and the PERCENTAGE_UPDATED event.
  AWS: Smithy: GetDeployment GET /applications/{ApplicationId}/environments/{EnvironmentId}/deployments/{DeploymentNumber}, code 200, output Deployment; State -> DeploymentState enum (values include DEPLOYING, BAKING, COMPLETE, ROLLING_BACK, ROLLED_BACK); EventLog -> DeploymentEvents of DeploymentEvent{EventType, TriggeredBy, Description, ActionInvocations, OccurredAt}; DeploymentEvent.ActionInvocations is the list of extension actions invoked for that event. Recorded behavior: a stopped deployment reads State 'ROLLED_BACK' with PercentageComplete 0.0, and every event carries an ActionInvocations member — an EMPTY list when no extension action fired. The log is newest-first and has four entries: 'Deployment rolled back by user request' (ROLLBACK_COMPLETED, USER), 'Deployment bake time started' (BAKE_TIME_STARTED, APPCONFIG), 'Configuration available to 100.00% of clients' (PERCENTAGE_UPDATED, APPCONFIG), 'Deployment started' (DEPLOYMENT_STARTED, USER). The Description texts are fixed strings for their event classes except PERCENTAGE_UPDATED, whose variable part is the rollout percentage formatted to two decimals; BAKE_TIME_STARTED's text carries no duration.
  Emulator: State 'COMPLETE' and PercentageComplete 100.0; the newest event is EventType 'DEPLOYMENT_COMPLETED' / TriggeredBy 'APPCONFIG' / Description 'Deployment completed' — there is no rollback event of any kind; the bake event's Description is 'Deployment bake time started for 10 minute(s)' instead of AWS's 'Deployment bake time started' (the duration suffix is emulator-authored); the PERCENTAGE_UPDATED event is absent entirely, so the log is three entries where AWS has four; and ActionInvocations is omitted (null) on every event where AWS returns [].
  Why: a client that calls StopDeployment and then polls GetDeployment for a terminal rollback state is told the configuration it tried to abort is fully deployed to 100% of clients — the opposite of the operational outcome, and it has no event-log evidence that a rollback was ever requested. A client iterating the event log to attribute extension invocations receives null where the model guarantees a list, which is a null-dereference/deserialization hazard in a typed SDK, and audit logic keyed on EventType never sees the PERCENTAGE_UPDATED milestone.
  Fix: the deployment state machine's rollback transition plus the Deployment/DeploymentEvent serializer — record a USER-triggered ROLLBACK_COMPLETED event and a terminal ROLLED_BACK state with PercentageComplete 0.0, record PERCENTAGE_UPDATED when the rollout reaches a percentage, emit the bake event's AWS text without the duration suffix, and always serialize ActionInvocations, defaulting to an empty list.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#14.

Cause 3 of 3: the environment's deployment-number sequence is one ahead, because a StartDeployment AWS refuses still consumes a number.
  AWS: Smithy: StartDeployment POST /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, http code 201, output Deployment with DeploymentNumber -> Integer; errors ConflictException (409), BadRequestException (400), ResourceNotFoundException (404). Recorded behavior: DeploymentNumber is a per-environment counter incremented only by deployments AppConfig actually creates — a StartDeployment refused while another deployment is in flight (409 ConflictException) creates nothing and consumes no number, so the next accepted deployment in this trajectory is number 3.
  Emulator: the next accepted StartDeployment returns DeploymentNumber 4 — one higher than AWS — because the earlier conflicting StartDeployment on the same environment was accepted with 201 instead of refused, creating a deployment record and advancing the counter. (That wrongly-accepted 409 step itself is already owned by 20261103T021132Z-appconfig-request-validation-and-refusal-divergences-on-startdeployment.md; the counter drift it leaves behind is what is filed here.)
  Why: DeploymentNumber is the identifier a client passes to GetDeployment/StopDeployment and the key it uses to correlate deployments it started. Numbers allocated to refused requests make the emulator's numbering disagree with AWS's for every subsequent deployment in the environment, so recorded or scripted clients address the wrong deployment, and ListDeployments histories cannot be compared.
  Fix: StartDeployment's allocation of the per-environment deployment counter in the appconfig state layer — the number must be reserved only after every refusal check (in-flight conflict, validation, resource resolution) has passed, so a refused request leaves the sequence untouched.
  Traces: appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#16.

## Evidence
Cause 1 — appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value#task#13, StopDeployment: status expected 202, got 400. Every output member of the expected Deployment is missing from the emulator's body: $.State (expected 'ROLLED_BACK'), $.PercentageComplete (expected 0.0), $.DeploymentNumber (expected 2), $.ApplicationId, $.EnvironmentId, $.ConfigurationProfileId, $.ConfigurationName, $.ConfigurationLocationUri (expected 'hosted'), $.ConfigurationVersion, $.VersionLabel, $.DeploymentStrategyId (expected 'AppConfig.AllAtOnce'), $.DeploymentDurationInMinutes (expected 0), $.FinalBakeTimeInMinutes (expected 10), $.GrowthType (expected 'LINEAR'), $.GrowthFactor (expected 100.0), $.Type (expected 'USER'), $.StartedAt, and $.EventLog (expected four events, newest 'Deployment rolled back by user request' / ROLLBACK_COMPLETED / USER). The identifiers and timestamps quoted here are this recording's values filling the template, not requirements.

Cause 2 — ...#task#14, GetDeployment (200 both sides): $.State expected 'ROLLED_BACK', got 'COMPLETE'; $.PercentageComplete expected 0.0, got 100.0; $.EventLog[0].EventType expected 'ROLLBACK_COMPLETED', got 'DEPLOYMENT_COMPLETED'; $.EventLog[0].Description expected 'Deployment rolled back by user request', got 'Deployment completed'; $.EventLog[0].TriggeredBy expected 'USER', got 'APPCONFIG'; $.EventLog[1].Description expected 'Deployment bake time started', got 'Deployment bake time started for 10 minute(s)'; $.EventLog[2] expected PERCENTAGE_UPDATED / APPCONFIG / 'Configuration available to 100.00% of clients', got DEPLOYMENT_STARTED / USER / 'Deployment started'; $.EventLog[3] missing (expected the DEPLOYMENT_STARTED / USER entry); $.EventLog[0..2].ActionInvocations missing (expected [], got None).

Cause 3 — ...#task#16, StartDeployment (201 both sides): $.DeploymentNumber expected 3, got 4.

## Steps to Reproduce
1. Create an AppConfig application, environment and hosted configuration profile with two hosted configuration versions, and use a strategy with DeploymentDurationInMinutes 0, GrowthType LINEAR, GrowthFactor 100 and FinalBakeTimeInMinutes 10 (AppConfig.AllAtOnce plus a bake time).
2. Call StartDeployment for the first configuration version.
3. Call StopDeployment for that deployment number while it is still in flight: AWS answers 202 with the full Deployment, State ROLLED_BACK, PercentageComplete 0.0 and a USER-triggered ROLLBACK_COMPLETED event; the emulator answers 400 with no Deployment body (Cause 1).
4. Call GetDeployment on the same deployment number: AWS reports State ROLLED_BACK / PercentageComplete 0.0 with a four-entry event log including PERCENTAGE_UPDATED and ActionInvocations: [] on every entry; the emulator reports State COMPLETE / PercentageComplete 100.0, a DEPLOYMENT_COMPLETED event, no rollback event, no PERCENTAGE_UPDATED event, no ActionInvocations, and the bake description suffixed with the duration (Cause 2).
5. Start a further deployment on the same environment after an earlier StartDeployment was issued while one was in flight (which AWS refuses 409): AWS numbers the new deployment 3, the emulator numbers it 4 (Cause 3).
6. Replay fixture appconfig-lacs-traj-allatonce-bake-blocks-redeploy-stop-rolls-back-value, steps task#13, task#14 and task#16.
