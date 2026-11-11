Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.alb-health-check-marks-ec2-instance-unhealthy autoscaling__ec2__elbv2.alb-listener-routes-traffic-to-registered-ec2-instance autoscaling__ec2__elbv2.alb-target-group-deregisters-ec2-instance-on-terminate autoscaling__ec2__elbv2.alb-target-group-registers-ec2-instance-healthy autoscaling__ec2__elbv2.nlb-target-group-registers-ec2-instance autoscaling__ec2__elbv2.target-group-deregistration-drains-connections-before-removal autoscaling__ec2__elbv2.target-group-register-fails-instance-wrong-vpc autoscaling__ec2__elbv2.target-group-registration-denied-without-elasticloadbalancing-permission
Issue-Key: ec2|RunInstances|NotImplemented
Issue-Commit: b4658095b9c4860859c50d7c6684da8fd2f8280e

# RunInstances with a NetworkInterfaces specification answers 501 NotImplemented, so no ALB/NLB target-group check can launch an instance

> _Note: this issue was identified against the code base at commit `b4658095b9c4860859c50d7c6684da8fd2f8280e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
REOPENS 20261022T235108Z-ec2-sql-ha-application-status-check-fast-launch.md: its fix left the call refused. That issue closed the LaunchTemplate branch of RunInstances; the NetworkInterfaces branch of the same operation still answers NotImplemented, so this is not a new defect but the un-fixed remainder of the same refusal.

One net-new root cause. Cause 1 of 1: RunInstances refuses any launch that carries an interface specification.

AWS: Smithy com.amazonaws.ec2#RunInstancesRequest models the member NetworkInterfaces as a list of InstanceNetworkInterfaceSpecification (DeviceIndex, SubnetId, Groups, AssociatePublicIpAddress, PrivateIpAddress/PrivateIpAddresses, DeleteOnTermination, Description, InterfaceType, Ipv6Addresses, …). A launch naming one eth0 interface answers HTTP 200 with a Reservation whose Instances carry that interface and its addresses; EC2 declares no error for this launch shape, and the recordings these checks were taken from show a 200.

Emulator: HTTP 501, error code NotImplemented, message "operation RunInstances with a network interface specification is scaffolded but not implemented in this emulator". The whole message is emulator-authored fixed text — nothing in it is taken from the request or from state — and neither the status nor the code is an EC2 refusal for this request.

Why: interface-level launch is how every VPC-aware client expresses a launch that pins a subnet, security groups and public-IP association on eth0 — which is exactly what an ALB/NLB target-group integration needs. No instance ever boots, so target registration, health transitions, drain-on-deregister, wrong-VPC refusal and permission-denied registration are all unreachable; the checks fail on the very first launch call.

Fix: the NetworkInterfaces branch of services/service-com-amazonaws-ec2-amazonec2/src/ops/run_instances.rs — resolve/create the described interfaces, attach them at their DeviceIndex and return them on the Reservation's Instances.

Traces: the eight checks listed in trace_ids, each failing on the first RunInstances call.

## Evidence
Replay output, identical for all eight checks:
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the RunInstances operation: operation RunInstances with a network interface specification is scaffolded but not implemented in this emulator

Expected: HTTP 200 and a Reservation whose Instances carry the requested eth0 interface (Smithy com.amazonaws.ec2#RunInstancesRequest.NetworkInterfaces -> InstanceNetworkInterfaceSpecification). Actual: HTTP 501 NotImplemented with emulator-authored prose.

## Steps to Reproduce
1. CreateVpc + CreateSubnet + CreateSecurityGroup.
2. Call ec2:RunInstances(MinCount=1, MaxCount=1, ImageId=<ami>, InstanceType=t3.micro, NetworkInterfaces=[{DeviceIndex:0, SubnetId:<subnet>, Groups:[<sg>], AssociatePublicIpAddress:true, DeleteOnTermination:true}]).
3. Expected 200 with Reservation.Instances[0].NetworkInterfaces; the emulator answers 501 NotImplemented.
4. Re-replay the check ids in trace_ids.
