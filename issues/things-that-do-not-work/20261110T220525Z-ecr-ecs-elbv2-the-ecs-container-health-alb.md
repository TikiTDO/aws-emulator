Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status
Issue-Key: ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# ecr__ecs__elbv2: the ECS container-health / ALB target-health convergence check never reaches a verdict — neither signal settles within its budget

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: the two independently-computed health signals for one Fargate task — the ECS container healthStatus from the task definition's own healthCheck, and the ELBv2 target-group health for that task's ip address — never both reach a terminal value, so the convergence the check grades is unobservable and the check is killed by its ceiling.
AWS: Smithy (ecs) DescribeTasks -> tasks[].containers[].healthStatus (HEALTHY / UNHEALTHY / UNKNOWN) and tasks[].attachments[].details carrying the awsvpc ENI's privateIPv4Address; Smithy (elasticloadbalancingv2) DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth {State, Reason, Description}. On AWS, once the container's HEALTHCHECK passes, healthStatus becomes HEALTHY; independently, the load balancer probes the registered ip target on the target group's health-check protocol/port/path and, after HealthyThresholdCount successes, reports State 'healthy'. The requirement is that each signal reaches a terminal state derived from the real container/probe, and that both converge for a serving task.
Emulator: the check is killed by its ceiling — "Check timed out after 60.0s" — while waiting for runningCount, the container healthStatus, the task's private IP, or the target state. No terminal value is produced for the graded signals.
Why: a client that waits on either signal — an ECS deployment gate on container health, or an ALB wait-for-healthy before shifting traffic — hangs indefinitely rather than getting a healthy/unhealthy verdict or an error. Nothing distinguishes "still converging" from "will never converge".
Fix: the ECS task/service convergence path (running task, container health evaluation from the task definition's healthCheck, and the awsvpc attachment's privateIPv4Address) together with the ELBv2 target-health state machine behind DescribeTargetHealth that must probe that address and publish a terminal State/Reason.
Related open filings that do NOT carry this replay key: 20261109T141609Z-ecr-ecs-elbv2-a-container-with-a-healthcheck.md, 20261105T113651Z-ecr-ecs-elbv2-a-fargate-service-with-desiredcount.md and 20261110T143849Z-ecr-ecs-elbv2-an-ecs-service-created-with.md describe adjacent ECS/ALB convergence gaps under this heading; the suite did not mark this check OWNED by any of them, so it survives as a live failure.
Traces: ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading ecr__ecs__elbv2 5/17 (29%), 10 failed (plus 2 unloadable); 9 of the failures are OWNED, the id in trace_ids survives.

[mismatch] ecr__ecs__elbv2.elbv2-health-check-reflects-ecs-task-status
  Error: Check timed out after 60.0s

The check's own body: create network/cluster/task definition, create an ALB + ip target group, create the service with that target group binding and desiredCount=1, then
  _wait_running_count(..., 1, timeout_s=360)
  container_health = _wait_container_health(..., "HEALTHY", timeout_s=300); assert container_health == ["HEALTHY"]
  ips = _task_private_ips(...); assert len(ips) == 1
  state = _wait_target_state(..., ips[0], "healthy", timeout_s=300); assert state == "healthy"

Expected (AWS): runningCount reaches 1, DescribeTasks reports containers[].healthStatus 'HEALTHY', the task attachment exposes exactly one privateIPv4Address, and DescribeTargetHealth reports that address State 'healthy'.
Actual (emulator): none of those waits settles; the check exhausts its 60 s ceiling without a graded signal.

Smithy consulted: ecs DescribeTasks output tasks[].containers[].healthStatus and tasks[].attachments[].details (privateIPv4Address); elasticloadbalancingv2 DescribeTargetHealth output TargetHealthDescriptions[].TargetHealth {State, Reason, Description} (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; the check id in trace_ids reproduces.
2. Create a VPC/subnets/SG, an ECS cluster, and register a Fargate task definition with awsvpc networking and a container-level healthCheck.
3. Create an ALB and an ip-type target group, and create the service with desiredCount=1 bound to that target group.
4. Poll DescribeServices for runningCount==1, DescribeTasks for containers[].healthStatus=='HEALTHY', and the task attachment for its privateIPv4Address.
5. Poll elbv2.DescribeTargetHealth for that address. Expected: State 'healthy'. Actual: none of these waits reaches a terminal value.
6. Re-replay the id in trace_ids to confirm the fix.
