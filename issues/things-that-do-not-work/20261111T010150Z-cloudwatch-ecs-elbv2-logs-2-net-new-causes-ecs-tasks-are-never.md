Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks cloudwatch__ecs__elbv2__logs.ecs-log-retention-policy-applied-to-loggroup
Issue-Key: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks|_poll_until#24|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last state={last!r}")
Issue-Key: cloudwatch__ecs__elbv2__logs.ecs-log-retention-policy-applied-to-loggroup|_wait_for_task_stopped#13|Timeout|return last
Issue-Commit: dff6e291e5e6489707888b0049fa49b367909ca3

# 2 net-new causes: ECS tasks are never registered as ALB targets, and the ECS/awslogs retention round trip never completes

> _Note: this issue was identified against the code base at commit `dff6e291e5e6489707888b0049fa49b367909ca3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeTargetHealth

## Description
Two distinct root causes, both grounded in failing checks of this service.

Cause 1 of 2: an ECS service created with a loadBalancers entry never puts its tasks into the target group.
AWS: recorded behavior only for the registration timing (elbv2's Smithy model exposes DescribeTargetHealth -> TargetHealthDescriptions). The recording shows that once the service's awsvpc task reaches RUNNING, DescribeTargetHealth on the configured target group returns one TargetHealthDescription whose Target.Id is the task ENI's private IP and Target.Port is the container port, progressing to State 'healthy'.
Emulator: DescribeTargetHealth keeps returning an EMPTY list for the whole poll window — 'AssertionError: condition not met within 24.541234207999878s; last state=[]'. No target is ever registered for the service's task.
Why: every client that reaches its container through the load balancer, and every deploy gate that waits for a healthy target, is stuck forever: the ALB has nothing to route to and target-health based rollout checks can never pass.
Fix: the ECS service's load-balancer attachment component — on task transition to RUNNING it must call the elbv2 target registration for each loadBalancers entry (IP target for awsvpc, instance:port for bridge) and remove it on task stop.
Failing check: cloudwatch__ecs__elbv2__logs.alb-target-group-registers-ecs-tasks.

Cause 2 of 2: the "pre-existing log group keeps its retention after an ECS task delivers into it" round trip never reaches a graded state.
AWS: recorded behavior only. The recording shows a log group created with retentionInDays=1 still reporting retentionInDays=1 after an ECS task has delivered events into it (ECS does not reset retention on an existing group), and the task's events being readable from that group.
Emulator: the check is killed at its budget — 'Error: Check timed out after 60.0s' — nothing it polls for (the task's delivery into the group) ever happens.
Why: a client cannot confirm that running a task preserves the retention it configured, and log-based assertions on the task's output hang.
Fix: the ECS awslogs driver delivery path (log-stream creation + PutLogEvents for container stdout) and the logs service's retention bookkeeping across ingestion.
Failing check: cloudwatch__ecs__elbv2__logs.ecs-log-retention-policy-applied-to-loggroup.

Traces: both check ids above. The open issue 20261104T062603Z-cloudwatch-ecs-elbv2-logs-task-attachments-expose-no.md covers other checks of this service and carries neither replay key.

## Evidence
Cause 1 — Expected (AWS): DescribeTargetHealth(TargetGroupArn) returns one TargetHealthDescription for the service task (Target.Id = task ENI private IP, Target.Port = container port), reaching State 'healthy'. Actual (emulator): 'AssertionError: condition not met within 24.541234207999878s; last state=[]' — the list stays empty.
Cause 2 — Expected (AWS): DescribeLogGroups still reports the pre-set retentionInDays after the ECS task delivers events, and the task's log events are readable. Actual (emulator): 'Error: Check timed out after 60.0s' — no delivery, no graded result.

## Steps to Reproduce
Cause 1: CreateLoadBalancer + CreateTargetGroup (TargetType=ip) in the VPC, CreateService with loadBalancers=[{targetGroupArn, containerName, containerPort}] and desiredCount=1, wait for the task to reach RUNNING, then poll DescribeTargetHealth — AWS registers the task IP, the emulator returns [].
Cause 2: CreateLogGroup + PutRetentionPolicy(retentionInDays=1); RegisterTaskDefinition with awslogs pointing at that group; RunTask; poll for the container's log events and re-read DescribeLogGroups — AWS resolves both, the emulator times out at 60 s.
