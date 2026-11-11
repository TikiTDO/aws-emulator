Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed
Issue-Key: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed|run#20|AssertionError|assert cancelled["StatusCode"] == "Cancelled", f"the scale-out activity answered {cancelled!r}"
Issue-Commit: f456950adb04cb9e7696776dd0db02907fd8875a

# autoscaling__ec2: a scale-out whose launch template has been deleted in EC2 launches an instance anyway — the activity reports Successful instead of Cancelled with EC2's not-found StatusMessage

> _Note: this issue was identified against the code base at commit `f456950adb04cb9e7696776dd0db02907fd8875a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
One net-new root cause: the launch path never resolves the group's launch template against EC2, so a stale reference is honoured instead of cancelling the launch.

Cause 1 of 1: after EC2 deletes the launch template an Auto Scaling group points at, the group's next launch must fail at launch time and be reported as a Cancelled scaling activity; the emulator launches an instance and reports the activity Successful. Failing check id: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed.
  AWS: the group keeps its stale LaunchTemplate reference after DeleteLaunchTemplate (DescribeAutoScalingGroups still shows the deleted LaunchTemplateId, and SetDesiredCapacity is ACCEPTED — this is not a request refusal). The resulting launch attempt then fails against EC2 and the scaling activity settles with StatusCode=`Cancelled`, Progress=100 and StatusMessage carrying EC2's own not-found sentence: fixed text `The specified launch template, with template ID ` + `, does not exist.` with the template id taken from the group's reference. DesiredCapacity stays at the requested value, unmet, with Instances empty and no live EC2 instance; only UpdateAutoScalingGroup repointing the group at a real template lets the next scale-out settle Successful. Smithy (autoscaling): Activity.StatusCode -> ScalingActivityStatusCode enum includes `Cancelled`, Activity.StatusMessage (XmlStringMaxLen255) is the verbose status; SetDesiredCapacity models only ScalingActivityInProgress/ResourceContention. The exact sentence is recorded behavior (it mirrors EC2's InvalidLaunchTemplateId.NotFound text).
  Emulator: the launch succeeds. The settled activity is `{'StatusCode': 'Successful', 'Progress': 100, 'Description': 'Launching a new EC2 instance: i-...', 'Cause': 'a user request explicitly set group desired capacity: changing the desired capacity from 0 to 1.', 'AutoScalingGroupState': 'InService', ...}` with no StatusMessage at all — `AssertionError: the scale-out activity answered {...'StatusCode': 'Successful'...}`. An instance is created from a template EC2 has already deleted (EC2's own DescribeLaunchTemplates correctly answers InvalidLaunchTemplateId.NotFound for the same id), so the launch path is reading a cached/own copy of the template rather than EC2's state.
  Why: the emulator reports a fleet that cannot exist. A client that deletes or rotates a launch template relies on the Cancelled activity and its StatusMessage to detect the broken reference (that is the only signal AWS gives, since the capacity request itself is accepted); here it gets a healthy-looking group, so template-deletion drift, blue/green template swaps and the repair-then-relaunch workflow are all silently wrong, and the remediation step cannot be distinguished from the broken state.
  Fix: the Auto Scaling launch path/reconciler for the group (it must resolve LaunchTemplateId/Name+Version against EC2 at launch time and, on not-found, record a Cancelled activity with Progress=100 and EC2's not-found StatusMessage, launching nothing and leaving DesiredCapacity unmet) — not the describe serializer, which already reports the stale reference correctly.
  Traces: autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed

## Evidence
Expected: with a group at MinSize=0/MaxSize=2/DesiredCapacity=0 referencing LaunchTemplateId=<lt> ($Latest), after ec2.DeleteLaunchTemplate(<lt>) (and ec2.DescribeLaunchTemplates answering InvalidLaunchTemplateId.NotFound / 'At least one of the launch templates specified in the request does not exist.'), SetDesiredCapacity(DesiredCapacity=1) is accepted and the settled activity reads StatusCode='Cancelled', Progress=100, StatusMessage='The specified launch template, with template ID <lt>, does not exist.'; DescribeAutoScalingGroups then shows DesiredCapacity=1 with Instances=[] and EC2 shows no live instance for the group.
Actual: the settled activity reads StatusCode='Successful', Progress=100, Description='Launching a new EC2 instance: i-...', and no StatusMessage; an instance exists. (ActivityId, instance id, group name, ARNs and timestamps in the dump are per-run values; the StatusCode/Progress/StatusMessage and the absence of a launch are the requirement.)

## Steps to Reproduce
1. CreateLaunchTemplate in EC2; CreateAutoScalingGroup(MinSize=0, MaxSize=2, DesiredCapacity=0, VPCZoneIdentifier=<subnet>, LaunchTemplate={LaunchTemplateId:<lt>, Version:'$Latest'}).
2. ec2.DeleteLaunchTemplate(LaunchTemplateId=<lt>) and confirm ec2.DescribeLaunchTemplates refuses with InvalidLaunchTemplateId.NotFound.
3. Confirm DescribeAutoScalingGroups still reports the stale LaunchTemplateId.
4. SetDesiredCapacity(AutoScalingGroupName=<group>, DesiredCapacity=1, HonorCooldown=False) and poll DescribeScalingActivities for the settled activity.
5. Observe StatusCode='Successful' and a launched instance instead of StatusCode='Cancelled' / Progress=100 / StatusMessage='The specified launch template, with template ID <lt>, does not exist.' and an unmet DesiredCapacity.
6. Replay autoscaling__ec2.asghc-asg-scale-out-after-template-deleted-cancels-until-repointed.
