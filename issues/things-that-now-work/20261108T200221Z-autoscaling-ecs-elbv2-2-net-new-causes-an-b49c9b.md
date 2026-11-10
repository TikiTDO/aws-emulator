Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets
Issue-Key: autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets|_poll_pair#17|AssertionError|assert mismatch_state != "healthy", "target with a mismatched health-check port must never report healthy"
Issue-Commit: 99cd58a9732d3abbd4ae376bbbd8ca320e363114

# autoscaling__ecs__elbv2: a health check pointed at a port nothing listens on still reported healthy (cause 2 of the two-cause report)

> _RESOLUTION: this cause is FIXED. A health check now CONNECTS to the target group's
> effective health-check port — `HealthCheckPort` when it names one, otherwise the port the
> target receives traffic on — and the reported state is the one those checks reached: a
> registration whose workload does not listen on that port is `initial`
> (`Elb.InitialHealthChecking`) while its failures accumulate and settles on `unhealthy` with
> `Target.Timeout` and a description, never `healthy`; one whose workload does listen on it
> converges to `healthy` with no reason code and no description
> (`services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/src/state/target_health.rs`,
> `.../src/state/host_counts.rs`). What a check can be answered on is now carried on the
> registration: an Amazon ECS task registration declares the container ports its revision
> declares — every `containerPort` and every port of a `containerPortRange`, which for an
> `awsvpc` task are the ports its own interface's address is reached on
> (`services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src/state/load_balancer_targets.rs`)
> — while a registration nothing declares ports for (every `RegisterTargets` caller's own) is
> still judged on what the account's EC2 inventory holds. Pinned by
> `services/service-com-amazonaws-elasticloadbalancingv2-elasticloadbalancing-v10/tests/all/scheduled_target_health_check_port.rs`,
> `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/tests/all/load_balanced_task_health_check_port.rs`
> and `core/router/tests/all/ecs_service_target_group_health_check_port.rs`._

> _Split out of the two-cause report this file was: cause 1 (an ALB answering 502 for a
> `healthy` ECS awsvpc ip target) is NOT fixed and is re-filed, with its own trace id and
> replay key, as
> `issues/things-that-do-not-work/20261108T200221Z-autoscaling-ecs-elbv2-an-alb-answers-502-for-a-healthy-ecs-awsvpc-ip.md`,
> so it stays filed and replayable. This report keeps the original numbering, trace id and
> replay key of cause 2. It was identified against the code base at commit
> `99cd58a9732d3abbd4ae376bbbd8ca320e363114`, so trace it back to the current code base._

## Operation
DescribeTargetHealth

## Description
Cause 2 of 2: a target group whose HealthCheckPort is a port the container never listens on reports its target `healthy`; the health check is never performed.
Failing check ids: autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets.
AWS: Smithy — `TargetHealth.State` is `TargetHealthStateEnum` {initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable}; `TargetHealth.Reason` is `TargetHealthReasonEnum` and its documentation states that for State='unhealthy' the Reason is one of `Target.ResponseCodeMismatch`, `Target.Timeout`, `Target.FailedHealthChecks`, `Elb.InternalError`, that for State='healthy' "a reason code is not provided", and that `Description` carries additional detail for any non-healthy state. Beyond the model this is recorded behavior only: with two identical awsvpc tasks on container port 80, the target group whose health check uses the traffic port converges to `healthy`, while the sibling target group created with HealthCheckPort explicitly set to a port nothing listens on (8888 in this recording) never reports `healthy` — it settles on `unhealthy` with Reason `Target.Timeout` or `Target.FailedHealthChecks` and a non-empty Description. The state and reason names are the fixed required text; the ports, interval and threshold of any one run are request values filling the template.
Emulator: both target groups report State='healthy'. Replay: `AssertionError: target with a mismatched health-check port must never report healthy`. Because the verdict is a constant, the required Reason and Description for the failing half are never emitted either.
Why: health is the only signal a client has that a target is actually serving the configured health-check port. Reported as a constant, a misconfigured HealthCheckPort — one of the most common real ALB/ECS misconfigurations — is indistinguishable from a correct one: negative checks silently 'pass', health-based rollback never trips, and ECS/Auto Scaling replacement driven by ELB health never fires.
Fix: the ELBv2 target-health state machine and its probe (the target-health state the DescribeTargetHealth handler reads) must derive the terminal verdict from an actual check against the effective health-check port (HealthCheckPort when set, otherwise the traffic port) instead of defaulting to `healthy` after the settling span, and must emit the matching Reason/Description. A constant `unhealthy` is the same defect with another literal: the verdict must follow whether the configured port is served.
Traces: autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets.

Related open issues that do NOT carry this replay key and so do not cover it: 20261107T080429Z-autoscaling-ecs-elbv2-6-net-new-causes-asg.md (ASG/target-group convergence) and 20261105T050431Z-elbv2-describetargethealth-reports-a-constant-healthy-for-an.md (the same probe defect, but scoped to the plain `elbv2` service heading and to one elbv2-only check).

## Evidence
Failing check: [mismatch] autoscaling__ecs__elbv2.target-group-port-mismatch-container-port-no-healthy-targets
  File "check_autoscaling__ecs__elbv2_target_group_port_mismatch_container_port_no_healthy_targets.py", line 58, in _poll_pair
    assert mismatch_state != "healthy", "target with a mismatched health-check port must never report healthy"
  AssertionError: target with a mismatched health-check port must never report healthy
Expected (AWS): the control target group (health check on the traffic port 80) converges to TargetHealth={State:'healthy'}; the sibling target group created with HealthCheckPort=<a port the container does not listen on> never reports 'healthy' and settles on {State:'unhealthy', Reason:'Target.Timeout'|'Target.FailedHealthChecks', Description:<non-empty>}.
Smithy: TargetHealthStateEnum = [initial, healthy, unhealthy, unhealthy.draining, unused, draining, unavailable]; TargetHealth.Reason doc — for 'unhealthy' the reason is Target.ResponseCodeMismatch | Target.Timeout | Target.FailedHealthChecks | Elb.InternalError; for 'healthy' no reason code is provided; Description is not provided for 'healthy'.
Actual (emulator): both target groups report {State:'healthy'} with no Reason and no Description.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the check in trace_ids, or by hand:
1. In the shared-network VPC, RegisterTaskDefinition with container port 80.
2. elbv2 CreateTargetGroup #1 with no HealthCheckPort override (traffic-port health check) and CreateTargetGroup #2 with HealthCheckPort='8888' (nothing listens there); attach each to its own listener on the same ALB (ports 80 and 8080).
3. CreateService for each target group with desiredCount=1; wait runningCount=1 for both.
4. Poll DescribeTargetHealth on both. EXPECT target group #1 -> State='healthy'; target group #2 -> never 'healthy', converging to State='unhealthy' with Reason 'Target.Timeout' or 'Target.FailedHealthChecks' and a non-empty Description. OBSERVED (before the fix): both report State='healthy'.
