Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__ecs__events__sqs.xsvc-ecs-2-container-instance-draining-event-reaches-draining-rule-not-active-rule
Issue-Key: ec2__ecs__events__sqs.xsvc-ecs-2-container-instance-draining-event-reaches-draining-rule-not-active-rule|_poll_until#11|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: f6476a64244d41dc0fd9adea93cb75255e1386be

# ec2__ecs__events__sqs: an EC2 instance launched from the ECS-optimized AMI with ECS_CLUSTER user data never registers as a container instance, so the container-instance-state-change routing scenario reaches no verdict

> _Note: this issue was identified against the code base at commit `f6476a64244d41dc0fd9adea93cb75255e1386be`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListContainerInstances

## Description
One net-new root cause under the ec2__ecs__events__sqs heading.

Cause 1 of 1: an EC2 instance launched from the ECS-optimized AMI with an instance profile carrying AmazonEC2ContainerServiceforEC2Role and user data setting ECS_CLUSTER=<cluster> never appears as a container instance of that cluster, so no 'ECS Container Instance State Change' event can be produced or routed.

AWS: Smithy (ecs) RegisterContainerInstance (the call the ECS agent makes on the instance, returning a ContainerInstance) and ListContainerInstances / DescribeContainerInstances (ContainerInstanceStatus filter, containerInstanceArns). Recorded behavior: minutes after RunInstances the agent on an ECS-optimized AMI reads /etc/ecs/ecs.config, registers with the named cluster, and the instance becomes listable with ec2InstanceId equal to the launched instance and agentConnected true; its status transitions then publish 'ECS Container Instance State Change' events on the aws.ecs source, which is what lets a DRAINING-scoped rule match while an ACTIVE-scoped rule does not.

Emulator: ListContainerInstances on the cluster returns no ARN whose DescribeContainerInstances row has ec2InstanceId = the launched instance, for the whole 420 s registration window; the check ends on its own time budget with 'EC2 instance i-... registers as a container instance: the run budget ended first; not graded'. Because registration never happens, nothing downstream of it (the DRAINING transition, the state-change event and its routing to exactly one of two rules) is exercised at all — so this issue reports the missing registration, which is the only observable the run produced.

Why: every EC2-launch-type ECS workflow starts with registration. A client that waits for its instance to join a cluster waits forever, capacity never exists for EC2-launch-type tasks, and the entire container-instance event family (including the DRAINING lifecycle operators use for safe scale-in) is unreachable.

Fix: the ECS container-instance registration path for EC2 instances (the agent-side RegisterContainerInstance the emulator must synthesize for an instance whose user data names a cluster) and the ListContainerInstances/DescribeContainerInstances state it feeds.

Related open issues that describe this cause under OTHER service headings and do NOT carry this check's replay key: 20261109T013300Z-ec2-ecs-servicediscovery-an-ec2-instance-launched-from.md, 20261109T083521Z-ecs-ec2-instances-launched-with-ecs-cluster-user.md, 20261109T061533Z-ecs-5-net-new-causes-ec2-hosts-never.md.

Traces: ec2__ecs__events__sqs.xsvc-ecs-2-container-instance-draining-event-reaches-draining-rule-not-active-rule.

## Evidence
ec2__ecs__events__sqs.xsvc-ecs-2-container-instance-draining-event-reaches-draining-rule-not-active-rule (_launch_real_container_instance line 356, _poll_until line 215):
  TimeoutError: EC2 instance i-00000000000000001 registers as a container instance: the run budget ended first; not graded

The polled condition is: ListContainerInstances(cluster=<cluster>) returns at least one ARN and DescribeContainerInstances on those ARNs yields a row with ec2InstanceId == the launched instance id. RunInstances had already succeeded (the instance id exists), the VPC/subnet/IGW/SG and the instance profile with AmazonEC2ContainerServiceforEC2Role were created, and user data was '#!/bin/bash\necho ECS_CLUSTER=<cluster> >> /etc/ecs/ecs.config'.

Expected: within the registration window the cluster lists one container instance whose ec2InstanceId is that instance (agentConnected true, status ACTIVE); the subsequent UpdateContainerInstancesState to DRAINING then publishes an 'ECS Container Instance State Change' event that reaches only the DRAINING-scoped rule's queue.
Actual: the cluster lists no container instance for 420 s, so the rest of the scenario never runs. The instance id in the message is a per-run value.

## Steps to Reproduce
1. CreateCluster. Create a VPC, public subnet, IGW + route and an egress-only security group.
2. Create a role trusting ec2.amazonaws.com with AmazonEC2ContainerServiceforEC2Role attached, plus an instance profile containing it.
3. GetParameter /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id and RunInstances that AMI (t3.micro) with the instance profile, a public IP and UserData '#!/bin/bash\necho ECS_CLUSTER=<cluster> >> /etc/ecs/ecs.config'.
4. Poll ListContainerInstances(cluster)/DescribeContainerInstances for a row with ec2InstanceId = the launched instance.
   Expected: the instance registers (agentConnected true, status ACTIVE).
   Emulator: the cluster lists no container instance for the whole window.
5. (Blocked by step 4) UpdateContainerInstancesState to DRAINING and confirm the 'ECS Container Instance State Change' event reaches a DRAINING-scoped rule's queue and not an ACTIVE-scoped sibling rule's queue.
