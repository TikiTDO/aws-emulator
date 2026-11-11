Issue-Source: trace-sweep-divergence
Issue-Trace: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
Issue-Step: task#15
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs|DescribeServices|0->None|$
Issue-Key: ecs|DescribeServices|0->None|status
Issue-Commit: e0dc03155a79e3340fd5a6f413ff0e6f3f485ded

# ECS circuit-breaker deployment never reaches a terminal rolloutState, so a DescribeServices poll burns the whole fixture budget

> _Note: this issue was identified against the code base at commit `e0dc03155a79e3340fd5a6f413ff0e6f3f485ded`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeServices

## Description
Cause 1 of 1: a service whose deploymentConfiguration.deploymentCircuitBreaker is enabled never produces a terminal deployment state, so a DescribeServices wait step never converges and the whole fixture is abandoned.

AWS: DescribeServices returns, per service, a deployments list of Deployment records (Smithy: Deployment.rolloutState -> DeploymentRolloutState, enum values COMPLETED | FAILED | IN_PROGRESS; Deployment.rolloutStateReason -> String; Deployment.status -> PRIMARY | ACTIVE | INACTIVE; Deployment.runningCount / desiredCount / pendingCount / failedTasks). The model states the contract directly: "When a service deployment is started, it begins in an IN_PROGRESS state. When the service reaches a steady state, the deployment transitions to a COMPLETED state. If the service fails to reach a steady state and circuit breaker is turned on, the deployment transitions to a FAILED state. A deployment in FAILED state doesn't launch any new tasks." With DeploymentCircuitBreaker.rollback true (Smithy: DeploymentCircuitBreaker.enable and .rollback, both required booleans; "If rollback is on, when a service deployment fails, the service is rolled back to the last deployment that completed successfully"), the failing deployment goes FAILED with a rolloutStateReason attributing the failure to the circuit breaker, and the PRIMARY deployment then returns to the last completed revision with runningCount converging on desiredCount. The parallel ServiceDeployment record (Smithy: ServiceDeployment.status -> ServiceDeploymentStatus, .statusReason "Information about why the service deployment is in the current status. For example, the circuit breaker detected a failure.", .rollback, .deploymentCircuitBreaker, .stoppedAt) reaches a terminal state the same way, whether the rollout ended by circuit-breaker rollback or by an operator StopServiceDeployment. The essential requirement is that every deployment leaves IN_PROGRESS for a terminal state within a bounded time, so a client polling DescribeServices converges instead of spinning.

Emulator: step task#15 of this fixture is a DescribeServices wait step (no status assertion of its own - the replay records "status: expected , got None" and actual_status null, i.e. no response was ever accepted as final). The emulator keeps answering, but never with the deployment state the step waits for, so the step is retried until the fixture's entire 960s budget is consumed: the replay reports "$: fixture budget 960s spent". Because the budget is the fixture's and not the step's, the step never completes and every step after it in the circuit-breaker-rollback-vs-stop trajectory is abandoned unreplayed. The deployment the emulator exposes stays non-terminal (it never becomes FAILED with a circuit-breaker rolloutStateReason, and no rollback to the last completed revision is ever visible), so the fixture's rollback arm and its StopServiceDeployment arm can never be distinguished.

Why: a client that deploys behind the deployment circuit breaker relies on ECS marking an unlaunchable rollout FAILED and restoring the last good revision, and on DescribeServices reporting that within a bounded time. Standard deployment waiters (the CLI's services-stable waiter, CDK/CodeDeploy-style rollout gates, any poll on deployments[].rolloutState) therefore hang against the emulator until their own timeout and then report a false failure or a false success, rather than observing the documented FAILED + rollback transition. Any automation that must tell "the circuit breaker rolled this back" from "an operator stopped it" cannot do so at all.

Fix: the ECS service-deployment state machine in the ecs service crate - the deployment progression driven by CreateService/UpdateService must evaluate the circuit breaker against failing task launches, drive the deployment to the terminal rolloutState FAILED with a rolloutStateReason naming the circuit breaker, and, when rollback is enabled, make the prior completed revision PRIMARY again and converge runningCount on desiredCount; the DescribeServices handler's deployments projection must surface those fields so a poll can terminate. The matching ServiceDeployment record must reach a terminal status/statusReason on both the rollback and the StopServiceDeployment path. Adjacent open issues 20261106T115645Z-ecs-3-net-new-causes-a-cluster-scoped.md (cause 2, circuit-breaker rollback never happens) and 20261104T062603Z-ecs-rolling-deployment-controls-are-not-enforced-the.md (cause 3, the circuit breaker never fires) describe the same underlying gap, but both are behavioral-corpus issues and neither carries this wire fixture's replay key, so this divergence is not covered by either and will not be re-run by a fix to them.

Traces: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop#task#15 (DescribeServices).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, /__admin/health 200).

Divergence cluster service=ecs|DescribeServices|status=0->None|keys=$,status, verdict mismatch, affected_count 1:
  fixture_id: ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop
  step_key:   task#15
  operation:  DescribeServices
  expected_status: 0 (wait step - no status asserted); actual_status: null
  detail: "status: expected , got None" and "$: fixture budget 960s spent (expected None, got None)"

Expected (AWS): the DescribeServices poll terminates because the circuit-breaker deployment leaves IN_PROGRESS for a terminal rolloutState - FAILED with a rolloutStateReason attributing the failure to the deployment circuit breaker - and, with rollback enabled, the PRIMARY deployment returns to the last completed revision with runningCount == desiredCount. deployments[].rolloutState is constrained to COMPLETED | FAILED | IN_PROGRESS.

Actual (emulator): the polled state never appears. No response is ever accepted as final (actual_status null); the step is retried until the fixture's whole 960s budget is spent, so step task#15 never completes and the remainder of the fixture is never replayed. The absolute wall-clock figure (960s) is this corpus's budget, not a requirement; the defect is that the deployment never reaches a terminal state at all.

This cluster is the only one of the 95 reported by the replay with no owning open issue (owned_by empty) and no blocking stub (stub_path empty).

## Steps to Reproduce
Replay fixture ecs-ecsdep-traj-circuit-breaker-rollback-vs-stop from trace set simparity-wire-aws-v1 against the emulator and observe step task#15.

Equivalently, by hand:
1. CreateCluster.
2. RegisterTaskDefinition for a FARGATE/awsvpc task definition with a launchable image (revision 1).
3. CreateService desiredCount=1, launchType FARGATE, deploymentController ECS, deploymentConfiguration.deploymentCircuitBreaker {enable: true, rollback: true}; wait for the service to reach a steady state (deployments[0].rolloutState COMPLETED, runningCount == desiredCount).
4. RegisterTaskDefinition a second revision whose tasks cannot start (unpullable image).
5. UpdateService taskDefinition=<revision 2>.
6. Poll DescribeServices on that cluster/service for the new deployment to reach rolloutState FAILED with a rolloutStateReason naming the deployment circuit breaker, and then for the PRIMARY deployment to point back at revision 1 with runningCount == desiredCount.
   -> Against the emulator the poll never satisfies either condition; it keeps answering 200 with a non-terminal deployment until the caller's budget is exhausted (here the fixture's 960s), and the fixture's remaining steps (including its StopServiceDeployment comparison arm) never run.
7. For the contrast arm: ListServiceDeployments / DescribeServiceDeployments for the same service -> the service deployment record never reaches a terminal status/statusReason either, so the rollback outcome cannot be told apart from a StopServiceDeployment'd one.
