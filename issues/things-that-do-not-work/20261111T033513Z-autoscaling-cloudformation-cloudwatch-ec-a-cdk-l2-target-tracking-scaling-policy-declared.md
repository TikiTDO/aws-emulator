Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts
Issue-Key: autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts|run#38|AssertionError|assert len(policies) == 1, (
Issue-Commit: a5271544d134d49a9bef1d276b891cd8aa450a13

# A CDK L2 target-tracking scaling policy declared by the stack is never created: DescribePolicies reports no scaling policies for the provisioned ASG

> _Note: this issue was identified against the code base at commit `a5271544d134d49a9bef1d276b891cd8aa450a13`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePolicies

## Description
One net-new root cause, reached by deploying a CDK L2 AutoScalingGroup with a target-tracking scaling policy and reading the policies back through Auto Scaling.

Cause 1 of 1: an AWS::AutoScaling::ScalingPolicy that CloudFormation reports as provisioned does not exist in Auto Scaling state, so the group has no scaling policies at all.
AWS:      Smithy (autoscaling) DescribePolicies -> ScalingPolicies: list<ScalingPolicy>, filtered by AutoScalingGroupName; PutScalingPolicy accepts PolicyType "TargetTrackingScaling" with TargetTrackingConfiguration, and CloudFormation's AWS::AutoScaling::ScalingPolicy creates exactly that policy and also mints the CloudWatch alarms that drive it. For a group whose template declares one target-tracking policy, DescribePolicies(AutoScalingGroupName=<group>) answers exactly one ScalingPolicy whose PolicyName/PolicyARN are the stack's, PolicyType "TargetTrackingScaling", and whose Alarms list names the generated alarms. Recorded behavior only for the CDK synthesis step: the deployed app declares a single target-tracking policy.
Emulator: the stack reaches its terminal success status and the ASG exists, but DescribePolicies for that group answers an empty ScalingPolicies list (HTTP 200, no error). The check fails with `AssertionError: ASG '<group>' has 0 scaling policies ([]), expected the one CDK declared`.
Why:      a client that deploys autoscaling through CloudFormation/CDK believes its group will scale on the declared metric. Nothing scales, and because the read succeeds with an empty list rather than refusing, drift detection and any "is my policy there?" probe reports a healthy, policy-less group instead of surfacing the missing resource.
Fix:      the CloudFormation AWS::AutoScaling::ScalingPolicy resource handler's create path and its mapping onto Auto Scaling's scaling-policy state (plus the alarms it must mint), not DescribePolicies' serializer - the listing is well-formed, the policy was never written.
Traces:   autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts.

## Evidence
Check autoscaling__cloudformation__cloudwatch__ec2.deploy-autoscaling-3-cdk-l2-asg-target-tracking-deploys-runs-and-drifts [mismatch]:

AssertionError: ASG '<stack-generated-asg-name>' has 0 scaling policies ([]), expected the one CDK declared
  at check_autoscaling__cloudformation__cloudwatch__ec2_deploy_autoscaling_3_cdk_l2_asg_target_tracking_deploys_runs_and_drifts.py line 184: assert len(policies) == 1

Expected (AWS): DescribePolicies(AutoScalingGroupName=<group>) -> ScalingPolicies == [ one policy, PolicyType 'TargetTrackingScaling', with its TargetTrackingConfiguration and Alarms ].
Actual (emulator): HTTP 200 with ScalingPolicies == [].
(The ASG name in the message is a per-run generated value, not part of the requirement.)

## Steps to Reproduce
1. Synthesize and deploy a CDK app with an autoscaling.AutoScalingGroup and `asg.scaleOnCpuUtilization(...)` (an AWS::AutoScaling::ScalingPolicy of type TargetTrackingScaling) against the emulator.
2. Wait for the stack to reach CREATE_COMPLETE and resolve the group's physical name.
3. Call autoscaling:DescribePolicies(AutoScalingGroupName=<group>).
4. Observe an empty ScalingPolicies list instead of the one policy the template declared.
