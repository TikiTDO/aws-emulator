Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100
Issue-Key: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100|_poll_until#22|AssertionError|raise AssertionError(f"{description} did not happen within {timeout_s}s; last read {last!r}")
Issue-Commit: 4315fe29349a8ccf086f6caac897ebb4dc04700d

# ECS managed scaling never creates the target-tracking policy on the capacity provider's Auto Scaling group (1 cause)

> _Note: this issue was identified against the code base at commit `4315fe29349a8ccf086f6caac897ebb4dc04700d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityProvider

## Description
Cause 1 of 1: CreateCapacityProvider with autoScalingGroupProvider.managedScaling.status=ENABLED is accepted, but ECS never creates the managed target-tracking scaling policy on the referenced Auto Scaling group, so the group holds no TargetTrackingScaling policy at all.

AWS: Smithy: ecs CreateCapacityProvider takes AutoScalingGroupProvider{autoScalingGroupArn, managedScaling{status, targetCapacity, minimumScalingStepSize, maximumScalingStepSize, instanceWarmupPeriod}}. AWS documents (https://docs.aws.amazon.com/AmazonECS/latest/developerguide/asg-capacity-providers.html) that enabling managed scaling makes ECS create and own a TargetTrackingScaling policy on that ASG, whose CustomizedMetricSpecification is Namespace AWS/ECS/ManagedScaling, MetricName CapacityProviderReservation, Dimensions exactly {CapacityProviderName: <the provider's name>, ClusterName: <the cluster it is associated with>}, and whose TargetValue equals the managedScaling targetCapacity given in the request (100 here). Recorded behavior: autoscaling DescribePolicies on the group returns exactly one policy of PolicyType TargetTrackingScaling with that specification shortly after the provider is associated with the cluster.

Emulator: CreateCapacityProvider and PutClusterCapacityProviders succeed, but autoscaling DescribePolicies(AutoScalingGroupName=<asg>) lists no TargetTrackingScaling policy for the full 300 s budget (last read []), so the managed policy, its AWS/ECS/ManagedScaling metric specification and its TargetValue are all absent.

Why: a client that enables managed scaling gets a cluster whose capacity never scales and whose managed policy it cannot inspect. Code that reads the generated policy (to verify targetCapacity, to look up the CapacityProviderReservation specification, or to assert that ECS — not the caller — owns the policy) finds nothing, and no CapacityProviderReservation-driven capacity change can ever happen, so capacity-provider behaviour cannot be exercised.

Fix: the ECS capacity-provider component (CreateCapacityProvider / the cluster-association path) — when managedScaling.status is ENABLED it must create the ECS-owned TargetTrackingScaling policy on the referenced ASG with the AWS/ECS/ManagedScaling CapacityProviderReservation customized metric specification, the CapacityProviderName/ClusterName dimensions and TargetValue taken from targetCapacity, so autoscaling DescribePolicies reports it.

Traces: cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100.

## Evidence
Check cloudwatch__ec2__ecs.obs-ecs-3-reservation-metrics-match-registered-resources-and-pressure-exceeds-100 [mismatch]:

AssertionError: ECS creating its target-tracking policy on sim2real-xsvc-obs3-asg-<id> did not happen within 300.0s; last read []

Expected: autoscaling DescribePolicies lists exactly one PolicyType=TargetTrackingScaling policy whose TargetTrackingConfiguration.CustomizedMetricSpecification is {Namespace: AWS/ECS/ManagedScaling, MetricName: CapacityProviderReservation, Dimensions: {CapacityProviderName: <provider>, ClusterName: <cluster>}} with TargetValue 100.0 (the names/ids are per-run values).
Actual: the policy list is empty for the whole 300 s budget.

## Steps to Reproduce
1. Create a VPC/subnet, an ECS cluster, a launch template joining that cluster and an Auto Scaling group from it.
2. ecs CreateCapacityProvider with autoScalingGroupProvider={autoScalingGroupArn: <asg arn>, managedScaling: {status: ENABLED, targetCapacity: 100, minimumScalingStepSize: 1, maximumScalingStepSize: 1, instanceWarmupPeriod: 60}, managedTerminationProtection: DISABLED}.
3. Associate the provider with the cluster (PutClusterCapacityProviders).
4. Poll autoscaling DescribePolicies(AutoScalingGroupName=<asg>) for a TargetTrackingScaling policy.
5. AWS reports the ECS-created policy with the AWS/ECS/ManagedScaling CapacityProviderReservation specification; the emulator reports an empty list.
