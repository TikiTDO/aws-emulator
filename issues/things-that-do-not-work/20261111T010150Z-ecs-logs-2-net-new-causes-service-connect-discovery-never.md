Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__logs.ecs-service-connect-client-task-reaches-the-server-service-by-its-discovery-name ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back
Issue-Key: ecs__logs.ecs-service-connect-client-task-reaches-the-server-service-by-its-discovery-name|_poll_until#21|Timeout|time.sleep(interval_s)
Issue-Key: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back|_poll_until#4|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# 2 net-new causes: Service Connect discovery never reachable, and a container that never passes its health probe never reaches RUNNING+HEALTHY

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Two distinct root causes, each grounded in its own failing check of this service.

Cause 1 of 2: ECS Service Connect does not deliver traffic: a client task cannot reach the server service by its discoveryName.
AWS: recorded behavior only for the data path (ECS's model exposes CreateService.serviceConnectConfiguration with services[].discoveryName/clientAliases). The recording shows the client task's HTTP request to the discoveryName returning 200 with a body carrying the server's per-run marker, and the server container's own start line present in its awslogs stream — i.e. the response travelled the ECS-injected Service Connect mesh.
Emulator: the check never reaches a graded state — 'Error: Check timed out after 60.0s'. Neither the server's log line nor a response through the discovery name is ever observable, so Service Connect's injected proxy/DNS is not emulated.
Why: a client whose microservices address each other only by Service Connect discovery names has no working service-to-service path at all; nothing in a mesh-based deployment can be exercised, and the failure is a silent hang rather than a refusal.
Fix: the ECS Service Connect component — the registration of discoveryName/clientAliases for a service and the injected proxy/DNS resolution that makes a sibling task reachable on that name inside the namespace.
Failing check: ecs__logs.ecs-service-connect-client-task-reaches-the-server-service-by-its-discovery-name.

Cause 2 of 2: a container's health probe is never evaluated, so even the healthy revision never reaches RUNNING with healthStatus HEALTHY and the circuit-breaker rollback cannot be exercised.
AWS: recorded behavior only for the timing (ECS's model exposes ContainerDefinition.healthCheck and Container.healthStatus; AWS documents the deployment circuit breaker). The recording shows revision 1's task reaching lastStatus RUNNING with its container healthStatus HEALTHY, then revision 2 — whose container runs but never passes its probe — being retried, failing, and the deployment rolled back to revision 1, whose container keeps serving.
Emulator: the first wait already fails — 'TimeoutError: the revision-1 task reaches RUNNING and its container health probe reports HEALTHY: the run budget ended first; not graded'. The container health probe is never run, so healthStatus never becomes HEALTHY and no rollback decision is ever taken.
Why: a client relying on container health checks to gate a rollout gets a deployment that neither succeeds nor rolls back: the bad revision is never detected and the good revision is never confirmed healthy, so the circuit breaker is inert.
Fix: the ECS container health-check evaluator (ContainerDefinition.healthCheck -> Container.healthStatus) and the service deployment controller's circuit-breaker rollback that consumes it.
Failing check: ecs__logs.svcreplace-deployment-whose-container-never-passes-its-healthcheck-is-rejected-and-rolled-back.

Traces: the two check ids above. Open issues 20261105T143800Z-ecs-logs-4-net-new-causes-awslogs-delivery.md and 20261104T062603Z-ecs-logs-the-awslogs-driver-never-delivers-awsvpc.md cover other checks of this service and carry neither replay key.

## Evidence
Cause 1 — Expected (AWS): the client task's HTTP GET to the server's Service Connect discoveryName returns 200 whose body contains the server's per-run marker, and the server's start line is in its awslogs stream. Actual (emulator): 'Error: Check timed out after 60.0s' — nothing observable.
Cause 2 — Expected (AWS): revision 1's task reaches lastStatus RUNNING with container healthStatus HEALTHY; revision 2 (probe never passes) is retried, fails and the deployment rolls back to revision 1. Actual (emulator): 'TimeoutError: the revision-1 task reaches RUNNING and its container health probe reports HEALTHY: the run budget ended first; not graded'.

## Steps to Reproduce
Cause 1: CreateCluster with a serviceConnectDefaults namespace; CreateService for a server with serviceConnectConfiguration services[].discoveryName and portName, and a client service in the same namespace; from the client container GET http://<discoveryName>:<port>/ and compare the body marker; also read the server's awslogs stream.
Cause 2: RegisterTaskDefinition revisions 1 (healthCheck passes) and 2 (healthCheck never passes) in one family; CreateService on revision 1 with deploymentCircuitBreaker enabled; poll DescribeTasks for lastStatus RUNNING and containers[].healthStatus HEALTHY; then UpdateService to revision 2 and poll for the rollback to revision 1.
