Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other
Issue-Key: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other|_drain_once#25|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# ecs__events__sqs: the exit-code-scoped Task State Change routing scenario never reaches a verdict — two Fargate tasks exiting 7 and 0 never settle within the harness window

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunTask

## Description
One net-new root cause under the ecs__events__sqs heading, reported honestly as an unreached verdict: the check produced no response at all and was cut at the harness's 60 s cap, so what is grounded here is that the scenario does not complete, not a specific wrong value.

Cause 1 of 1: the ECS Task State Change scenario that differentiates two Fargate tasks by containers[].exitCode makes no observable progress — the emulator never carries it to the point where either task's STOPPED event could be read.

AWS: recorded behavior only (the EventBridge event envelope and ECS's detail are not modelled shapes). Two Fargate tasks from one task definition, one exiting 7 and one exiting 0, each reach STOPPED within a few minutes; each STOPPED 'ECS Task State Change' event (source aws.ecs) is delivered only to the rule whose pattern matches its detail.containers[].exitCode and is withheld from the sibling rule, with detail exitCode/stopCode/stoppedReason equal to DescribeTasks, and each task's earlier RUNNING event carries a lower detail.version and no exitCode.

Emulator: the check is terminated with 'Check timed out after 60.0s' having produced no assertion and no partial result. Its own internal budgets (420 s for a task to reach STOPPED, 180 s for event delivery) are far above that cap, so nothing in the trajectory — task settle, event emission, pattern routing — is observable. Sibling checks under this heading show the related owned defect (ECS publishes no Task State Change events at all), which is the likely family this belongs to; this check adds the exit-code-scoped routing and version/exitCode projection contract, which no other check carries.

Why: the exit-code routing contract is how clients split failed from successful task completions into different consumers. With the scenario unreachable, that contract is unverified and a client cannot tell whether its pattern would match.

Fix: the ECS task lifecycle and its Task State Change event emission (the path that must settle a Fargate task to STOPPED and publish an event whose detail carries containers[].exitCode, stopCode, stoppedReason and an incrementing version), and whatever in that path currently makes no progress.

Related open issue in this family that does NOT carry this replay key: 20261104T062603Z-ecs-events-sqs-ecs-never-publishes-deployment-or.md.

Traces: ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other.

## Evidence
ecs__events__sqs.events-workload-state-change-ecs-exit-code-rules-route-each-stopped-task-and-not-delivered-to-the-other:
  Error: Check timed out after 60.0s
No assertion was reached and no partial response was recorded. The check creates a cluster, a VPC/subnet/SG, a Fargate task definition on public.ecr.aws/docker/library/busybox:latest, two rules whose patterns differ only in detail.containers.exitCode (7 vs 0) with an SQS target each, then RunTask twice (exit 7 and exit 0) and waits up to 420 s for STOPPED and 180 s for delivery.

Expected: each STOPPED 'ECS Task State Change' event reaches only the rule matching its containers[].exitCode (the other queue stays empty), with detail exitCode/stopCode/stoppedReason equal to DescribeTasks, and each RUNNING event carrying a lower detail.version and no exitCode.
Actual: no verdict — the scenario does not complete inside the cap.

## Steps to Reproduce
1. CreateCluster; create a VPC with a public subnet and a security group usable by Fargate.
2. RegisterTaskDefinition (FARGATE, awsvpc) on public.ecr.aws/docker/library/busybox:latest with a command that exits with a given code.
3. PutRule twice with patterns {source: ['aws.ecs'], detail-type: ['ECS Task State Change'], detail: {clusterArn: [<cluster>], lastStatus: ['STOPPED'], containers: {exitCode: [7]}}} and the same with exitCode 0; PutTargets an SQS queue on each (queue policies granting events.amazonaws.com).
4. RunTask twice — one task exiting 7, one exiting 0 — and poll DescribeTasks until both are STOPPED.
5. ReceiveMessage on both queues.
   Expected: the exit-7 event only on the exit-7 queue and the exit-0 event only on the exit-0 queue, each detail matching DescribeTasks; each task's RUNNING event has a lower detail.version and no exitCode.
   Emulator: the run produces no result at all — the check is cut at the 60 s harness cap.
