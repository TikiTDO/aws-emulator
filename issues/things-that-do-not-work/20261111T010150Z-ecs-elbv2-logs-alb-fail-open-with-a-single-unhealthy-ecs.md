Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503
Issue-Key: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# ALB fail-open with a single unhealthy ECS target and the 503 of an empty target group never materialise

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 1 of 1: the ALB's fail-open behaviour over ECS targets is not reproduced, and the check exercising it never reaches a graded state (killed at its 60 s budget).

AWS: recorded behavior only for the routing outcomes (elbv2's model exposes DescribeTargetHealth -> TargetHealth.State/Reason, e.g. 'Target.ResponseCodeMismatch'); AWS documents target-group health checks and fail-open. The recording shows three distinct outcomes: (a) a target group whose ONLY target is unhealthy with Reason 'Target.ResponseCodeMismatch' is still sent the request and the listener answers HTTP 200 (fail open); (b) a target group with NO targets makes the listener answer HTTP 503; (c) fixing the health-check path brings the target to 'healthy' and the listener keeps answering 200.

Emulator: the check never gets to compare any of the three — 'Error: Check timed out after 60.0s'. The states it polls for (an ECS task registered as a target, that target reporting unhealthy with Target.ResponseCodeMismatch, then healthy) are never reached, so the ALB's fail-open/503 distinction is unobservable.

Why: fail-open versus 503 is the difference between a brief degradation and a hard outage in a client's resilience tests. A client cannot verify that a single unhealthy task still serves traffic, nor that an empty group returns 503, so capacity-loss runbooks and chaos tests built on AWS's semantics cannot run.

Fix: the elbv2 listener's routing/health-evaluation component (fail-open when every target of the group is unhealthy, HTTP 503 when the group is empty) together with the ECS-to-target-group registration and health-check evaluation that must make a task's health observable through DescribeTargetHealth.

Traces: ecs__elbv2__logs.alb-unhealthy-only-target-fails-open-empty-group-returns-503. Open issues 20261110T220525Z-ecs-elbv2-logs-2-net-new-causes-the.md and 20261108T200250Z-ecs-elbv2-logs-an-alb-fronted-awsvpc-fargate.md cover other checks of this service and carry neither this replay key nor this behaviour.

## Evidence
Expected (AWS): with one target whose TargetHealth.State='unhealthy', Reason='Target.ResponseCodeMismatch', an HTTP request to the listener returns 200 (fail open); with the target group emptied the listener returns 503; after repairing the health-check path the target reports 'healthy' and requests return 200.
Actual (emulator): none of these are reached — 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
1. CreateLoadBalancer + CreateTargetGroup (TargetType=ip) with a health-check path the container does not serve.
2. Run an ECS awsvpc task serving HTTP and register/attach it via the service's loadBalancers config.
3. Poll DescribeTargetHealth for State='unhealthy', Reason='Target.ResponseCodeMismatch', then GET the listener: AWS answers 200 (fail open).
4. Deregister the only target and GET again: AWS answers 503.
5. Fix the health-check path: the target becomes 'healthy' and GETs stay 200. Against the emulator the check never gets past step 3 and is killed at 60 s.
