Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.ecracc-fargate-container-secrets-without-an-execution-role-rejected ecs.ecracc-private-registry-credentials-without-an-execution-role-rejected ecs.ecsdeploy-awsfirelens-log-driver-without-router-rejected ecs.ecsnt-fargate-task-definition-with-an-unsupported-cpu-memory-pair-rejected ecs.register-task-definition-soft-ulimit-above-hard-rejected-and-consumes-no-revision ecs.register-taskdef-network-mode-port-mapping-conflict-rejected ecs.escc-minimum-healthy-percent-above-one-hundred-rejected ecs.escc-floor-and-ceiling-both-one-hundred-rejected-on-update ecs.op-contract-listAccountSettings ecs.access-denied-across-mutating-ops
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.ecracc-fargate-container-secrets-without-an-execution-role-rejected|_error#12|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: ecs.ecracc-private-registry-credentials-without-an-execution-role-rejected|_error#11|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: ecs.ecsdeploy-awsfirelens-log-driver-without-router-rejected|_error#6|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: ecs.ecsnt-fargate-task-definition-with-an-unsupported-cpu-memory-pair-rejected|_error#3|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: ecs.register-task-definition-soft-ulimit-above-hard-rejected-and-consumes-no-revision|run#8|AssertionError|raise AssertionError(f"softLimit 8192 > hardLimit 4096 was ACCEPTED as {bad['taskDefinitionArn']!r}")
Issue-Key: ecs.register-taskdef-network-mode-port-mapping-conflict-rejected|_refusal#3|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ecs.escc-minimum-healthy-percent-above-one-hundred-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Key: ecs.escc-floor-and-ceiling-both-one-hundred-rejected-on-update|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Key: ecs.op-contract-listAccountSettings|_error#12|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: ecs.access-denied-across-mutating-ops|_error#15|AssertionError|raise AssertionError(f"expected the call to be denied, but it succeeded and returned {result!r}")
Issue-Commit: 3a80585caf3e9f62e646ed534a139af0bb6604b7

# ECS is overly permissive: ten validations/authorizations AWS enforces are missing, so invalid requests succeed with 200

> _Note: this issue was identified against the code base at commit `3a80585caf3e9f62e646ed534a139af0bb6604b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterTaskDefinition

## Description
The ECS emulator accepts ten distinct request classes that real ECS refuses. Each is a separate missing cross-field check / authorization gate in a different handler; all ten are listed here as one bundle for the ECS service. Ten causes, numbered below — all ten must be closed before this issue is done.

Cause 1 of 10: RegisterTaskDefinition accepts a Fargate container that declares `secrets` with no `executionRoleArn`.
AWS: RegisterTaskDefinition models `executionRoleArn` as optional (Smithy: RegisterTaskDefinition.executionRoleArn, String, not required) but refuses the combination at the service: a container with `secrets` requires a task execution role, answered with HTTP 400 ClientException/InvalidParameterException (Smithy: RegisterTaskDefinition errors ClientException 400, InvalidParameterException 400). Recorded behavior: the audited check records a refusal, so there is no error code on success.
Emulator: returns 200 and registers the revision.
Why: a client that relies on ECS to catch the missing execution role gets a task definition that can never pull its secrets; the failure surfaces only at task launch time, and CI that asserts the refusal passes wrongly.
Fix: the RegisterTaskDefinition handler's validation (services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/ops/register_task_definition.rs).
Traces: ecs.ecracc-fargate-container-secrets-without-an-execution-role-rejected.

Cause 2 of 10: RegisterTaskDefinition accepts a container with `repositoryCredentials` (private-registry auth) and no `executionRoleArn`.
AWS: refused with HTTP 400 (ClientException/InvalidParameterException are the modeled client errors for this operation); recorded behavior only for the exact text.
Emulator: 200, revision registered (`networkMode=bridge` shape in the check).
Why: same class — an unusable task definition is created and the negative contract is lost.
Fix: RegisterTaskDefinition validation, same handler as Cause 1.
Traces: ecs.ecracc-private-registry-credentials-without-an-execution-role-rejected.

Cause 3 of 10: RegisterTaskDefinition accepts a container whose logConfiguration uses the `awsfirelens` log driver while no container in the definition carries a `firelensConfiguration` (no log router).
AWS: refused with HTTP 400 (modeled ClientException/InvalidParameterException); recorded behavior only for the message.
Emulator: 200 for a FARGATE/awsvpc definition with cpu=2048/memory=4096, executionRoleArn set, and a single non-router container.
Why: the task definition is accepted and then no container can emit logs; a deploy pipeline that depends on the refusal ships a broken definition.
Fix: RegisterTaskDefinition validation (cross-container FireLens check).
Traces: ecs.ecsdeploy-awsfirelens-log-driver-without-router-rejected.

Cause 4 of 10: RegisterTaskDefinition accepts an unsupported Fargate cpu/memory pair.
AWS: Fargate task size pairs are a closed set (cpu/memory are modeled as free-form Strings — Smithy: RegisterTaskDefinition.cpu/.memory String — so the constraint is enforced by the service, not the model); an unsupported pair is refused with HTTP 400 ClientException/InvalidParameterException. Recorded behavior: the check records a refusal for the pair it sends (memory="4096" against the family's cpu).
Emulator: 200, revision registered.
Why: a definition that Fargate will never run is accepted; capacity-planning and negative tests both break.
Fix: RegisterTaskDefinition validation (Fargate task-size table).
Traces: ecs.ecsnt-fargate-task-definition-with-an-unsupported-cpu-memory-pair-rejected.

Cause 5 of 10: RegisterTaskDefinition accepts a ulimit whose softLimit exceeds its hardLimit, and consumes a family revision doing so.
AWS: Smithy: Ulimit.softLimit and Ulimit.hardLimit are both required integers; a soft limit above the hard limit is refused with HTTP 400 and no revision is consumed.
Emulator: accepted softLimit 8192 > hardLimit 4096 and returned revision :1 (e.g. arn:aws:ecs:...:task-definition/<family>:1).
Why: the revision counter advances on a request AWS never persists, so subsequent revision assertions (and rollback-to-revision logic) are off by one.
Fix: RegisterTaskDefinition validation + revision allocation (allocate the revision only after validation).
Traces: ecs.register-task-definition-soft-ulimit-above-hard-rejected-and-consumes-no-revision.

Cause 6 of 10: RegisterTaskDefinition accepts a portMapping with hostPort != containerPort under `networkMode: awsvpc`.
AWS: with awsvpc networking the host port must equal the container port; the mismatch is refused with HTTP 400 ClientException/InvalidParameterException.
Emulator: 200, registering containerPort 80 / hostPort 8080 with networkMode awsvpc (requiresAttributes even include ecs.capability.task-eni).
Why: a definition that cannot be placed in awsvpc mode is accepted, and the emulator's own derived attributes contradict it.
Fix: RegisterTaskDefinition validation (network-mode/port-mapping cross-check).
Traces: ecs.register-taskdef-network-mode-port-mapping-conflict-rejected.

Cause 7 of 10: CreateService accepts deploymentConfiguration.minimumHealthyPercent above 100.
AWS: Smithy: CreateService.deploymentConfiguration -> DeploymentConfiguration.minimumHealthyPercent is a percentage of desiredCount; a value of 150 is refused with HTTP 400 (CreateService models ClientException/InvalidParameterException 400).
Emulator: 200, service created with minimumHealthyPercent=150/maximumPercent=200.
Why: a service whose deployment configuration ECS would reject exists in state, so later deployment math is computed from an impossible configuration.
Fix: CreateService validation (services/.../src/ops/create_service.rs).
Traces: ecs.escc-minimum-healthy-percent-above-one-hundred-rejected.

Cause 8 of 10: UpdateService accepts minimumHealthyPercent=100 together with maximumPercent=100.
AWS: that pair leaves the rolling-update scheduler no room to replace a task and is refused with HTTP 400 on UpdateService.
Emulator: 200, the update is applied.
Why: the service is left in a configuration that can never complete a deployment, and the caller never learns.
Fix: UpdateService validation (services/.../src/ops/update_service.rs).
Traces: ecs.escc-floor-and-ceiling-both-one-hundred-rejected-on-update.

Cause 9 of 10: ListAccountSettings accepts a `principalArn` naming a different AWS account.
AWS: Smithy: ListAccountSettings.principalArn is a String and the operation models InvalidParameterException 400 / AccessDeniedException 403; a principal in another account is refused rather than answered.
Emulator: HTTP 200 with `{"settings": []}`.
Why: a cross-account probe reads as "this principal has no settings" instead of being refused — a client cannot distinguish "no settings" from "not your account", and an authorization boundary disappears.
Fix: ListAccountSettings handler validation (services/.../src/ops/list_account_settings.rs).
Traces: ecs.op-contract-listAccountSettings.

Cause 10 of 10: mutating ECS operations ignore an explicit IAM Deny — AccessDeniedException is never returned.
AWS: Smithy: CreateCluster/DeleteCluster/UpdateService/RegisterTaskDefinition all model AccessDeniedException with httpError 403. A session assumed into a role whose inline policy is Allow "*" plus an explicit Deny on those four ecs: actions is denied with 403 AccessDeniedException (message built from the request: the session ARN, the action and the resource ARN).
Emulator: signs-and-serves regardless — CreateCluster by the restricted session returned 200 with a fully-formed cluster (clusterArn arn:aws:ecs:<region>:<account>:cluster/<name>, status ACTIVE).
Why: every negative authorization test passes wrongly, and an emulated run cannot exercise least-privilege policies at all; a caller believes its Deny is in force.
Fix: request authorization for the ECS handlers (the shared signing/authorization layer that resolves the signing principal, plus the per-operation action/resource mapping).
Traces: ecs.access-denied-across-mutating-ops.

Related open issue, not a duplicate: 20261022T152302Z-ecs-accepts-invalid-requests-with-200-and-misreports.md covers a different set of accepted-invalid requests; none of the ten causes above are in it.

## Evidence
Replay output (verdict mismatch in every case):
- ecs.ecracc-fargate-container-secrets-without-an-execution-role-rejected — `ecs.register_task_definition(family=..., **shape)` -> AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code.
- ecs.ecracc-private-registry-credentials-without-an-execution-role-rejected — `ecs.register_task_definition(family=..., networkMode="bridge", containerDefinitions=...)` -> same assertion.
- ecs.ecsdeploy-awsfirelens-log-driver-without-router-rejected — `register_task_definition(cpu="2048", memory="4096", networkMode="awsvpc", requiresCompatibilities=["FARGATE"], executionRoleArn=..., containerDefinitions=service_only)` -> same assertion.
- ecs.ecsnt-fargate-task-definition-with-an-unsupported-cpu-memory-pair-rejected — `_task_definition(ctx, family, memory="4096")` -> "the call succeeded; real AWS rejects it, so there is no error code".
- ecs.register-task-definition-soft-ulimit-above-hard-rejected-and-consumes-no-revision — "softLimit 8192 > hardLimit 4096 was ACCEPTED as 'arn:aws:ecs:us-west-2:000000000000:task-definition/<family>:1'".
- ecs.register-taskdef-network-mode-port-mapping-conflict-rejected — "expected a rejection, but the call SUCCEEDED and returned {'taskDefinition': {... 'portMappings': [{'containerPort': 80, 'hostPort': 8080, 'protocol': 'tcp'}], 'networkMode': 'awsvpc', 'revision': 1 ...}}" (HTTPStatusCode 200).
- ecs.escc-minimum-healthy-percent-above-one-hundred-rejected — `create_service(..., deploymentConfiguration={"minimumHealthyPercent": 150, "maximumPercent": 200})` -> "the call succeeded; real AWS rejects it".
- ecs.escc-floor-and-ceiling-both-one-hundred-rejected-on-update — `update_service(..., deploymentConfiguration={"minimumHealthyPercent": 100, "maximumPercent": 100})` -> "the call succeeded; real AWS rejects it".
- ecs.op-contract-listAccountSettings — `list_account_settings(name="containerInsights", principalArn=<other-account-arn>)` -> "real AWS rejects this call, but it SUCCEEDED and returned {'settings': [], ... 'HTTPStatusCode': 200}".
- ecs.access-denied-across-mutating-ops — `restricted.create_cluster(clusterName=...)` under an assumed role with an explicit Deny on ecs:CreateCluster/DeleteCluster/UpdateService/RegisterTaskDefinition -> "expected the call to be denied, but it succeeded and returned {'cluster': {'clusterArn': 'arn:aws:ecs:us-west-2:000000000000:cluster/denied-...', 'status': 'ACTIVE', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200}}".
Expected in all ten: HTTP 400 with a modeled ECS client error (ClientException / InvalidParameterException), or HTTP 403 AccessDeniedException for Cause 10. Actual: HTTP 200 with the mutation applied.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run the ten checks listed in trace_ids (all under the `ecs` service token).
3. Per cause, issue the call shown in Evidence against the emulator and observe HTTP 200 instead of a 400 client error (or, for Cause 10, assume a role whose policy explicitly Denies the four ecs: actions and observe CreateCluster returning 200).
