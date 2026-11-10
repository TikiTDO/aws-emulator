Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503 ecs__elbv2__logs.ecs-container-health-check-failure-makes-the-service-replace-the-task-and-the-alb-drop-the-old-target
Issue-Key: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: ecs__elbv2__logs.ecs-container-health-check-failure-makes-the-service-replace-the-task-and-the-alb-drop-the-old-target|_poll_until#22|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: b35089c281a7e84768d928c8c72034d27eee1ae8

# ecs__elbv2__logs: 2 net-new causes — the ALB fail-open / empty-group-503 behaviour never becomes observable, and a container's healthStatus never reaches HEALTHY so no task replacement is driven

> _Note: this issue was identified against the code base at commit `b35089c281a7e84768d928c8c72034d27eee1ae8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Two net-new root causes under the ecs__elbv2__logs heading (Cause 1 of 2 .. Cause 2 of 2).

Cause 1 of 2: the ALB's fail-open behaviour and its empty-target-group 503 are unobservable — the scenario never reaches a graded step and the check is killed by its ceiling.
AWS: Smithy (elasticloadbalancingv2) DescribeTargetHealth -> TargetHealthDescriptions[].TargetHealth {State, Reason, Description} (Reason documents Target.ResponseCodeMismatch for a health check that did not return an expected HTTP code); CreateListener DefaultActions[] {Type 'forward', TargetGroupArn}. AWS behaviour, per the target-group health-checks documentation the check cites: when EVERY target in the group is unhealthy the load balancer fails open and routes to them anyway, so the request is still served (HTTP 200 from the container); when the group has NO registered targets the load balancer answers its own HTTP 503; and repairing the health-check path returns the target to 'healthy' and the responses to healthy 200s. Those three outcomes are the requirement.
Emulator: "Check timed out after 60.0s" — no terminal target state and no served response is produced, so neither the fail-open 200, nor the empty-group 503, nor the recovery is reachable.
Why: fail-open is the behaviour that keeps a service reachable during a bad health-check rollout, and the empty-group 503 is how a client distinguishes "nothing registered" from "target refused". With neither observable, a client cannot predict what its load balancer does when health checks go wrong — the exact situation the behaviour exists for.
Fix: the ELBv2 target-health state machine (it must reach 'unhealthy' with Reason Target.ResponseCodeMismatch from a real probe) and the data-plane forwarding path (fail open when all targets are unhealthy; generate a 503 for an empty target group).
Traces: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503

Cause 2 of 2: a container with a failing HEALTHCHECK never reports healthStatus HEALTHY (or UNHEALTHY) — DescribeTasks keeps it at its initial value — so ECS never replaces the task and the ALB never drops the old target.
AWS: Smithy (ecs) RegisterTaskDefinition containerDefinitions[].healthCheck {command, interval, timeout, retries, startPeriod}; DescribeTasks -> tasks[].containers[].healthStatus with values HEALTHY / UNHEALTHY / UNKNOWN, and tasks[].{lastStatus,desiredStatus}. On AWS the container's healthStatus becomes HEALTHY once the command succeeds and UNHEALTHY after `retries` consecutive failures; a service whose task goes UNHEALTHY stops and replaces it, the stopped task's ip leaves the target group, the replacement registers and becomes 'healthy', and HTTP through the load balancer answers 200 before and after.
Emulator: the wait for healthStatus HEALTHY never completes and the run budget is consumed. Replay: TimeoutError "container healthStatus HEALTHY on arn:aws:ecs:<region>:<account>:task/<cluster>/<task id>: the run budget ended first; not graded" — the ARN is a per-run value; the defect is that healthStatus never reaches a terminal value, so the whole replacement chain out to the ALB target set never starts.
Why: container health is what makes an ECS service self-healing. A healthStatus that never settles means a broken container is never replaced, a client's deployment never converges or rolls back, and the ALB keeps an address that no longer serves — with no signal that anything is wrong.
Fix: the ECS container-health component — the task-definition healthCheck evaluator that must publish HEALTHY/UNHEALTHY on DescribeTasks containers[].healthStatus — and the service scheduler that must stop and replace an UNHEALTHY task and deregister its address from the attached target group.
Related open filings that do NOT carry these replay keys: 20261108T200250Z-ecs-elbv2-logs-an-alb-fronted-awsvpc-fargate.md, 20261107T222655Z-ecs-elbv2-logs-an-alb-attached-awsvpc-fargate.md, 20261109T143515Z-ecs-elbv2-logs-the-alb-dnsname-is-an.md and 20261109T141609Z-ecr-ecs-elbv2-a-container-with-a-healthcheck.md are adjacent; the suite did not mark either check below OWNED, so both survive as live failures.
Traces: ecs__elbv2__logs.ecs-container-health-check-failure-makes-the-service-replace-the-task-and-the-alb-drop-the-old-target

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the booted emulator; service heading ecs__elbv2__logs 0/6 (0%), 4 failed (plus 2 unloadable); 2 of the failures are OWNED, the two ids in trace_ids survive.

Cause 1: [mismatch] ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503
  Error: Check timed out after 60.0s
  Expected (AWS): the only target reaches 'unhealthy' with Reason Target.ResponseCodeMismatch and the load balancer fails open, still serving HTTP 200; with no registered targets the load balancer answers HTTP 503; fixing the health-check path restores 'healthy' 200s.
  Actual (emulator): no terminal state and no served response within the budget.

Cause 2: [error] ecs__elbv2__logs.ecs-container-health-check-failure-makes-the-service-replace-the-task-and-the-alb-drop-the-old-target
  File "check_ecs__elbv2__logs_ecs_container_health_check_failure_makes_the_service_replace_the_task_and_the_alb_drop_the_old_target.py", line 128, in run -> _poll_until(...)
  File "...", line 351, in _poll_until -> raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: container healthStatus HEALTHY on arn:aws:ecs:us-west-2:000000000000:task/sim2real-xsvc-ecs-cluster-59eae1550e1b/00000000000000000000000000000001: the run budget ended first; not graded
  Expected (AWS): healthStatus HEALTHY while the HEALTHCHECK passes, then UNHEALTHY after `retries` consecutive failures; the service stops and replaces the task; the old ip leaves the target group and the replacement becomes 'healthy'; HTTP through the load balancer answers 200 before and after.
  Actual (emulator): healthStatus never reaches HEALTHY, so no replacement and no target-set change ever occurs.

Smithy consulted: ecs RegisterTaskDefinition containerDefinitions[].healthCheck {command, interval, timeout, retries, startPeriod} and DescribeTasks output tasks[].containers[].healthStatus / tasks[].lastStatus; elasticloadbalancingv2 DescribeTargetHealth output TargetHealthDescriptions[].TargetHealth {State, Reason, Description} and CreateListener DefaultActions[] (model services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; both ids in trace_ids reproduce.
2. Cause 1: create an ALB with an HTTP:80 listener forwarding to an ip target group whose health-check path returns a non-matching code; register the single ECS task ip. Expect the target 'unhealthy' with Reason Target.ResponseCodeMismatch and the load balancer to fail open with HTTP 200. Then deregister all targets and expect the load balancer's own HTTP 503. Then repair the health-check path and expect 'healthy' 200s. The emulator never reaches any of these states.
3. Cause 2: register a Fargate task definition whose container has a healthCheck command that first passes, run it as a service with desiredCount=1 bound to a target group, and poll DescribeTasks for containers[].healthStatus. Expected: HEALTHY. Actual: it never reaches a terminal value.
4. Then make the health check fail and expect UNHEALTHY, a service-driven task replacement, the old ip leaving DescribeTargetHealth, the new ip becoming 'healthy', and HTTP 200 through the load balancer throughout.
5. Re-replay the two ids in trace_ids to confirm the fix.
