Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration
Issue-Key: autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command|_run_shell#8|AssertionError|raise AssertionError(
Issue-Key: autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration|_run_shell#8|AssertionError|raise AssertionError(
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# Auto Scaling CreateAutoScalingGroup refuses a VPCZoneIdentifier-only group with 501 NotImplemented, breaking the ASG/ELBv2 CLI workflows

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutoScalingGroup

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: CreateAutoScalingGroup demands AvailabilityZones when only VPCZoneIdentifier is given, and answers 501 NotImplemented when it is absent.

AWS: CreateAutoScalingGroup accepts a group that names only VPCZoneIdentifier (a comma-separated subnet list); Auto Scaling resolves each subnet's Availability Zone itself and DescribeAutoScalingGroups then reports those AZs. AvailabilityZones is optional for a VPC group — `aws autoscaling create-auto-scaling-group --vpc-zone-identifier subnet-a,subnet-b --launch-template ... --min-size/--max-size/--desired-capacity` is the documented form and returns 200 with an empty body. Recorded behavior only for the emulator's refusal text.

Emulator: the call is answered 501 `NotImplemented` with "launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well". Through the CLI this is an rc=254 failure before any instance is launched, so both ASG+ELBv2 CLI workflows stop on their first create.

Why: the standard way to create a VPC-based Auto Scaling group fails. Nothing downstream is reachable: no instances launch, --target-group-arns never registers anything, and attach-load-balancer-target-groups can never be exercised. NotImplemented is also not a modeled Auto Scaling error, so clients see an unretriable, unbranchable failure.

Fix: the CreateAutoScalingGroup handler must resolve each subnet in VPCZoneIdentifier to its AvailabilityZone through the EC2 subnet state (the same state DescribeSubnets serves) instead of requiring the caller to supply AvailabilityZones.

Related open issues that record this refusal but do not carry these two replay keys: 20261022T221142Z-autoscaling-ec2-elbv2-auto-scaling-elb-cli-workflows-createautoscalinggroup-refuses-a.md, 20261023T025808Z-autoscaling-ec2-elbv2-auto-scaling-ec2-elbv2-integration-blocked-elbv2-createtargetgroup.md.

Traces: autoscaling__ec2__elbv2.cli-asg-attach-load-balancer-target-groups-command, autoscaling__ec2__elbv2.cli-create-asg-with-target-group-and-verify-ec2-registration.

## Evidence
Expected: `aws autoscaling create-auto-scaling-group --auto-scaling-group-name <n> --launch-template LaunchTemplateId=lt-...,Version=$Latest --min-size 1 --max-size 1 --desired-capacity 1 --vpc-zone-identifier subnet-a,subnet-b` -> rc=0, HTTP 200, group created with the AZs of those subnets.
Actual: rc=254 and `aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateAutoScalingGroup operation: launching instances for a group that names only VPCZoneIdentifier needs each subnet's Availability Zone, which this emulator does not hold: pass AvailabilityZones as well` (HTTP 501).

## Steps to Reproduce
1. Create a VPC with two subnets and a launch template referencing a security group in that VPC.
2. Create an ELBv2 target group in the same VPC.
3. Run `aws autoscaling create-auto-scaling-group --auto-scaling-group-name <n> --launch-template LaunchTemplateId=<lt>,Version=$Latest --min-size 2 --max-size 2 --desired-capacity 2 --vpc-zone-identifier <subnet-a>,<subnet-b> --target-group-arns <tg arn>` (no --availability-zones).
4. Observe rc=254 / HTTP 501 NotImplemented instead of a created group whose instances register into the target group.
