Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling.describe-scaling-activities-server-recorded autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-keeps-instance-outside-desired
Issue-Shape-Id: com.amazonaws.autoscaling#AutoScaling_2011_01_01
Issue-Key: autoscaling.describe-scaling-activities-server-recorded|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-keeps-instance-outside-desired|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: c3de02674d16064d9f5c8a9be8af17d3d848d40e

# autoscaling: a DesiredCapacity>0 group never reaches an InService instance, so the retained-terminate-hook and server-recorded scaling-activity scenarios never reach an observable step (1 cause)

> _Note: this issue was identified against the code base at commit `c3de02674d16064d9f5c8a9be8af17d3d848d40e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoScalingGroups

## Description
Cause 1 of 1: an Auto Scaling group whose capacity is raised above 0 never converges to an instance in LifecycleState=InService, so every scenario that must observe a launched instance (and the scaling activities the launch records) stalls until the suite budget runs out.

AWS: Smithy models DescribeAutoScalingGroups returning AutoScalingGroups[].Instances[] whose LifecycleState walks Pending -> Pending:Wait/Pending:Proceed -> InService, and SetDesiredCapacity (input AutoScalingGroupName @required, DesiredCapacity @required, HonorCooldown; errors ScalingActivityInProgressFault 400 / ResourceContentionFault 500) is documented to drive that convergence asynchronously. The convergence is also what makes a server-recorded activity exist: Smithy's Activity shape marks ActivityId, AutoScalingGroupName, Cause, StartTime and StatusCode (enum ScalingActivityStatusCode) all @required, so AWS must mint one activity per scale-out/terminate with a unique ActivityId, a Cause sentence, a valid StatusCode and a StartTime. For the retained-terminate-hook scenario the model says nothing beyond the hook shapes, so: recorded behavior only — the recording shows an ABANDON on an autoscaling:EC2_INSTANCE_TERMINATING hook under TerminateHookAbandon=retain parking the instance in LifecycleState=Terminating:Retained, still running and outside DesiredCapacity, while an unretained twin terminates.

Emulator: both checks never get past their first poll for an instance. autoscaling.describe-scaling-activities-server-recorded calls CreateAutoScalingGroup(MinSize=0,MaxSize=1,DesiredCapacity=0) then SetDesiredCapacity(DesiredCapacity=1,HonorCooldown=False) and polls DescribeAutoScalingGroups for exactly one instance with LifecycleState=='InService' (its own budget is 300s); the replay ends with 'Check timed out after 60.0s', i.e. no instance ever reports InService, so the SetDesiredCapacity and TerminateInstanceInAutoScalingGroup activities it then wanted to read are never reachable. autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-keeps-instance-outside-desired builds a control group and a subject group each carrying a terminating lifecycle hook (DefaultResult=ABANDON, HeartbeatTimeout=900) and likewise ends with 'Check timed out after 60.0s' before any instance exists to park in Terminating:Retained.

Why: a client that raises capacity and then waits for DescribeAutoScalingGroups to report InService — the standard pattern every waiter, CDK/CFN CreationPolicy and ECS capacity provider uses — waits forever against the emulator. Downstream, DescribeScalingActivities can never return an activity with the @required ActivityId/Cause/StartTime/StatusCode, and no terminate-hook policy (ABANDON, retain, Terminating:Retained) is observable at all, so correctness of the whole instance-lifecycle surface is untestable.

Fix: the Auto Scaling group reconciliation loop that turns DesiredCapacity into instances and advances LifecycleState (the group/instance state machine in the autoscaling service state, driving the EC2 RunInstances call and the Pending->InService transition), plus the activity recorder that must mint an Activity per capacity change. This is the same convergence gap already open as 20261107T124656Z-autoscaling-a-group-with-desiredcapacity-0-never-reports-an.md, which does NOT carry these two checks' replay keys; it does not cover them.

Traces: autoscaling.describe-scaling-activities-server-recorded, autoscaling.deep-autoscaling-2-asg-enforce-terminate-hook-abandon-retain-keeps-instance-outside-desired (both 'Check timed out after 60.0s').

## Evidence
Expected (AWS): after SetDesiredCapacity(DesiredCapacity=1), DescribeAutoScalingGroups eventually reports Instances=[{InstanceId, LifecycleState: 'InService', ...}]; DescribeScalingActivities then reports an Activity with a unique ActivityId, a non-empty Cause, a StatusCode from ScalingActivityStatusCode and a StartTime (all @required in the model). With a terminating hook at DefaultResult=ABANDON and TerminateHookAbandon=retain, the abandoned instance reports LifecycleState='Terminating:Retained' and stays running outside DesiredCapacity.

Actual (emulator): no instance ever reports LifecycleState='InService'. Both checks end with the replay verdict 'Error: Check timed out after 60.0s' while polling DescribeAutoScalingGroups()['AutoScalingGroups'][0]['Instances'], so neither the activity records nor the Terminating:Retained state is ever produced.

## Steps to Reproduce
1. ec2:CreateLaunchTemplate with a valid ImageId and InstanceType=t3.micro.
2. autoscaling:CreateAutoScalingGroup(AutoScalingGroupName=<g>, LaunchTemplate={LaunchTemplateName:<lt>}, MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet>).
3. autoscaling:SetDesiredCapacity(AutoScalingGroupName=<g>, DesiredCapacity=1, HonorCooldown=False).
4. Poll autoscaling:DescribeAutoScalingGroups(AutoScalingGroupNames=[<g>]) for an instance with LifecycleState=='InService' — it never appears.
5. (Second check) repeat with LifecycleHookSpecificationList entry {LifecycleTransition: 'autoscaling:EC2_INSTANCE_TERMINATING', DefaultResult: 'ABANDON', HeartbeatTimeout: 900} and TerminateHookAbandon=retain; no instance ever exists to be parked in Terminating:Retained.
