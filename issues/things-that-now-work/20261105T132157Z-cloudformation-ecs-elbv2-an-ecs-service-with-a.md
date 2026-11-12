Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative
Issue-Key: cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive|run#15|StopIteration|victim_before = next(d for d in descs if d["Target"]["Id"] == victim_ip)
Issue-Key: cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event|_poll_for_event#25|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: 269019b5b42949e66be9fce8117dfdac4ac9abf5

# cloudformation__ecs__elbv2: an ECS service with a loadBalancers attachment never registers its tasks as ELBv2 targets, so target health is empty and six ECS/ALB target-lifecycle checks never reach a verdict

> _Note: this issue was identified against the code base at commit `269019b5b42949e66be9fce8117dfdac4ac9abf5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Two net-new behavioral root causes of the `cloudformation__ecs__elbv2` suite, grounded in seven failing behavioral checks. A fixer must close both before submitting.

Cause 1 of 2: a running ECS service that declares `loadBalancers` is never reflected in its ELBv2 target group — DescribeTargetHealth reports no target for the task's ENI IP.
AWS: recorded behavior only for the timing; the shapes are modeled (ECS CreateService.loadBalancers -> targetGroupArn/containerName/containerPort; ELBv2 DescribeTargetHealth -> TargetHealthDescriptions[].Target.Id / TargetHealth.State). On AWS, once a service with `awsvpc` network mode and a `loadBalancers` entry reaches a stable deployment, ECS registers EACH running task's ENI private IP as a target in that target group; DescribeTargetHealth then returns one TargetHealthDescription per task whose `Target.Id` is that private IP and whose `TargetHealth.State` becomes `healthy` after the health-check grace period. Stopping a task moves its target to `draining` and then removes it entirely (after the target group's deregistration delay).
Emulator: after the service reports stable and two tasks are running with known private IPs, DescribeTargetHealth on the attached target group contains NO description whose `Target.Id` is a task IP at all — the check dies on `StopIteration` while selecting its victim target, so the registration never happened and the draining/deregistration transition that depends on it can never be observed.
Why: the ECS-to-ALB attachment is the whole point of a load-balanced service. A client cannot tell whether its tasks are in service, cannot drain one before stopping it, and any rollout, scale or health-based automation driven off DescribeTargetHealth sees an empty target group while traffic is supposedly being served.
Fix: the ECS service/task-placement state that must register and deregister targets in ELBv2 — the loadBalancers handling in the ECS CreateService/UpdateService and task lifecycle (start/stop) paths, and the target-group membership and TargetHealth state it must write for ELBv2 DescribeTargetHealth (including the draining transition and the deregistration delay).
Traces: cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive.

Cause 2 of 2: six ECS/ALB target-lifecycle scenarios produce no verdict at all — the surfaces they poll (service stability, task running state, target health, and HTTP through the listener) never settle inside the suite's per-check budget.
AWS: recorded behavior only — each of these sequences converges well inside the budget: CreateService with `--load-balancers` reaches a stable deployment and its task IP appears healthy; awsvpc IP-target registration appears in the target group; stopping a task produces the deregistration transition; a listener rule routes by path to the right service; and a security group that blocks the health-check port drives the target `unhealthy` rather than leaving it unknown.
Emulator: all six are cut off with `Error: Check timed out after 60.0s` and no response to compare. The 60 s figure is the suite's budget, not a requirement — the requirement is that the polled surfaces (ECS DescribeServices deployment/stability, DescribeTasks lastStatus, ELBv2 DescribeTargetHealth state) reach a terminal value. Hypothesis: the same missing target registration as Cause 1 is what these checks wait on; they are reported as a separate cause because no response was observed for them, so closing Cause 1 must be confirmed by re-replaying them.
Why: a client polling for a healthy target or a stable service hangs instead of being told what happened, so no load-balanced ECS deployment can be scripted against the emulator the way it is against AWS.
Fix: whichever of those polled surfaces does not settle — the ECS deployment/stability reporting and task lifecycle, and the ELBv2 target-health state machine (initial -> healthy -> draining -> removed, and unhealthy when the health check cannot reach the target).
Traces: cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets, cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive, cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive, cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event, cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive, cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative.

## Evidence
Cause 1 — cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive [error]:
  File "check_cloudformation__ecs__elbv2_elbv2_deregisters_targets_on_ecs_task_stop_positive.py", line 39, in run
    victim_before = next(d for d in descs if d["Target"]["Id"] == victim_ip)
  StopIteration
  The check had already: created the VPC/subnets/SG, an execution role, a cluster, a task definition, an ALB, a target group (deregistration_delay=20) and a listener; created the service with desired_count=2, tg_arn, containerName 'web', containerPort 80, grace_period 120; waited for the service to report stable; listed 2 running task ARNs and read the victim task's private IP. DescribeTargetHealth on that target group then contained no description for that IP.
  Expected on AWS: one TargetHealthDescription per running task, Target.Id = the task ENI private IP, TargetHealth.State 'healthy'; after StopTask the state becomes 'draining' and then the description disappears.

Cause 2 — six checks, same terminal diagnosis, no response to diff:
  cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets [mismatch] — Error: Check timed out after 60.0s
  cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive [mismatch] — Error: Check timed out after 60.0s
  cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive [mismatch] — Error: Check timed out after 60.0s
  cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event [mismatch] — Error: Check timed out after 60.0s
  cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive [mismatch] — Error: Check timed out after 60.0s
  cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative [mismatch] — Error: Check timed out after 60.0s

Smithy consulted: ECS CreateService.loadBalancers (LoadBalancer: targetGroupArn, containerName, containerPort) and networkConfiguration/awsvpcConfiguration; ELBv2 DescribeTargetHealth -> TargetHealthDescriptions[].Target.Id and TargetHealth.State (TargetHealthStateEnum includes initial/healthy/unhealthy/draining). The convergence timing is recorded behavior only; the model carries the states but no timing.

## Steps to Reproduce
1. Create a VPC with two subnets and a security group; create an ECS cluster and register an awsvpc Fargate task definition exposing container port 80.
2. Create an ELBv2 application load balancer in those subnets, a target group in that VPC with TargetType 'ip' (deregistration delay 20s) and a listener forwarding to it.
3. CreateService with desiredCount=2, networkConfiguration.awsvpcConfiguration on those subnets, and loadBalancers=[{targetGroupArn, containerName:'web', containerPort:80}], healthCheckGracePeriodSeconds=120. Wait for the service to report a stable deployment.
4. ListTasks/DescribeTasks to read each running task's ENI private IP, then call ELBv2 DescribeTargetHealth on the target group: expect one TargetHealthDescription per task with Target.Id equal to that private IP and TargetHealth.State 'healthy'. Observe no such description.
5. StopTask one of them and poll DescribeTargetHealth: expect the target to go 'draining' and then disappear.
6. Re-replay: cloudformation__ecs__elbv2.elbv2-deregisters-targets-on-ecs-task-stop-positive, cloudformation__ecs__elbv2.cli-ecs-create-service-with-alb-attach-and-verify-targets, cloudformation__ecs__elbv2.ecs-service-registers-tasks-as-alb-targets-positive, cloudformation__ecs__elbv2.ecs-task-network-mode-awsvpc-ip-target-registration-positive, cloudformation__ecs__elbv2.ecs-task-stopped-event-triggers-target-deregistration-event, cloudformation__ecs__elbv2.elbv2-listener-rule-path-based-routing-to-multiple-ecs-services-positive, cloudformation__ecs__elbv2.elbv2-security-group-mismatch-blocks-health-checks-negative.
