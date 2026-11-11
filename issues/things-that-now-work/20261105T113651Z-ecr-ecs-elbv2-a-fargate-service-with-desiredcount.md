Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks
Issue-Key: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks|_poll_until#9|Timeout|last = probe()
Issue-Commit: 9fc0802c1fad78cd4c44ae6731253718bd889678

# ecr__ecs__elbv2: a Fargate service with desiredCount=1 never reaches runningCount=1, so the ALB target-registration scenario never reaches a verdict

> _Note: this issue was identified against the code base at commit `9fc0802c1fad78cd4c44ae6731253718bd889678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Cause 1 of 1: an ECS Fargate service created with desiredCount=1 and a loadBalancers entry never converges — its runningCount never reaches the desired count, so the task is never launched into a readable state and the ALB target group is never populated. The replay spends its entire budget polling DescribeServices and is killed.

AWS: CreateService with desiredCount=1, launchType=FARGATE, networkMode=awsvpc and a loadBalancers entry (targetGroupArn + containerName + containerPort) launches one task; Smithy ecs#Service.runningCount is "The number of tasks in the cluster that are in the RUNNING state" and reaches 1 (and ecs#Deployment.rolloutState moves IN_PROGRESS -> COMPLETED when the service reaches a steady state, or -> FAILED). ListTasks(desiredStatus="RUNNING") then returns that task, DescribeTasks exposes its awsvpc ENI attachment (Smithy ecs#Task.attachments: "The Elastic Network Adapter that's associated with the task if the task uses the awsvpc network mode"; ecs#Attachment.details: "For elastic network interfaces, this includes the network interface ID, the MAC address, the subnet ID, and the private IPv4 address"), and ELBv2 DescribeTargetHealth on the target group reports exactly one TargetHealthDescription whose Target.Id is that privateIPv4Address and whose Target.Port is the container port (80 here).

Emulator: DescribeServices keeps reporting a runningCount that never equals desiredCount, so the check never gets past the convergence poll: the only output is `Error: Check timed out after 60.0s`. No running task is enumerable, no ENI private IPv4 address is exposed, and the target group stays empty.

Why: a load-balanced ECS service is unusable and undiagnosable. A client that creates a service and then waits on runningCount == desiredCount (the services_stable waiter, CloudFormation's ECS service stabilization, any deploy script) blocks forever instead of succeeding or failing, and a client that reads DescribeTargetHealth to confirm the rollout sees an empty target group with no indication of why.

Fix: the ECS service scheduler / task lifecycle in services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/src must launch desiredCount tasks for a FARGATE service, advance them to lastStatus RUNNING (updating Service.runningCount/pendingCount and the primary Deployment's counts and rolloutState), synthesize the awsvpc ElasticNetworkInterface attachment with a privateIPv4Address from the task's subnet, and register that IP:containerPort in the service's loadBalancers target group so ELBv2 DescribeTargetHealth reports it. The open issue 20261104T060239Z-ecr-ecs-elbv2-untrusted-execution-roles-and-unpullable.md lists this check under a cause about the task ENI carrying no private IP, but its recorded replay key for this check is an assertion AFTER convergence (`assert len(ips) == 1`); the suite reports this failure as unowned because the replay now never reaches that assertion — the service does not converge at all — so this cause is not covered by it.

Traces: ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks.

## Evidence
Failing check ecr__ecs__elbv2.elbv2-target-group-registers-ecs-service-tasks ends with `Error: Check timed out after 60.0s`, while blocked in its convergence poll (`service <name> runningCount==1`).

Expected (AWS), after CreateService(desiredCount=1, launchType=FARGATE, loadBalancers=[{targetGroupArn, containerName="app", containerPort=80}], healthCheckGracePeriodSeconds=120):
  - DescribeServices -> services[0].runningCount == 1 (primary deployment reaches a terminal rolloutState)
  - ListTasks(desiredStatus="RUNNING") -> one taskArn; DescribeTasks -> attachments[].details contains name="privateIPv4Address" with the task ENI's address
  - DescribeTargetHealth(TargetGroupArn) -> exactly one TargetHealthDescription with Target.Id == that privateIPv4Address and Target.Port == 80

Actual (emulator): runningCount never equals 1, so no RUNNING task is listed, no privateIPv4Address detail exists, and DescribeTargetHealth returns no target descriptions for the lifetime of the replay.

## Steps to Reproduce
1. Create a VPC (10.99.0.0/16) with DNS support/hostnames, an internet gateway, two public subnets (MapPublicIpOnLaunch), a route table with 0.0.0.0/0 -> IGW associated to both, and a self-referencing security group.
2. Create an ECS cluster and RegisterTaskDefinition: FARGATE, networkMode=awsvpc, cpu=256/memory=512, one essential container "app" with portMappings containerPort=80 and a container healthCheck.
3. Create an internet-facing application load balancer across both subnets, a target group (Protocol=HTTP, Port=80, TargetType=ip, HealthCheckPath=/), and a listener forwarding to it.
4. CreateService with desiredCount=1, launchType=FARGATE, the two subnets, assignPublicIp=ENABLED, loadBalancers=[{targetGroupArn, containerName="app", containerPort=80}], healthCheckGracePeriodSeconds=120.
5. Poll DescribeServices for runningCount == 1 -> it never reaches 1; the replay is killed at the 60s budget before DescribeTasks / DescribeTargetHealth can be asserted.
