Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected
Issue-Key: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal|run#21|AssertionError|assert len(samples) >= min_samples, (
Issue-Key: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected|run#26|AssertionError|assert asg_a2 != asg_a1, (
Issue-Commit: 1814b3996c94d6e0656759171876e93c26063e2d

# autoscaling__cloudformation__ec2: 2 net-new causes — an AutoScalingRollingUpdate performs no observable batched replacement, and AutoScalingReplacingUpdate WillReplace=true never gives the group a new physical id

> _Note: this issue was identified against the code base at commit `1814b3996c94d6e0656759171876e93c26063e2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two independent root causes in the CloudFormation UpdatePolicy path of the AWS::AutoScaling::AutoScalingGroup resource handler. Both checks now get past the fixed issue 20261030T154944Z-autoscaling-cloudformation-ec2-a-cloudformation-stack-whose-aws.md (the signal-gated stacks do reach a terminal status now) and fail further along, on the UpdatePolicy semantics themselves; neither failure is a refusal, so this is not a reopen of that issue.

Cause 1 of 2: UpdatePolicy AutoScalingRollingUpdate is not executed as a batched replacement — the update completes before any batch boundary exists.
AWS: with UpdatePolicy.AutoScalingRollingUpdate {MaxBatchSize: N, MinInstancesInService: M, MinSuccessfulInstancesPercent: 100, PauseTime: PT15M, WaitOnResourceSignals: true}, an UpdateStack that changes the group's LaunchTemplate property replaces the group's instances one batch at a time: CloudFormation terminates at most MaxBatchSize instances, launches their replacements, and holds the batch until a SignalResource call with Status=SUCCESS arrives for the group's LogicalResourceId from each replacement before starting the next batch (Smithy, cloudformation: SignalResource input StackName (required), LogicalResourceId (required), UniqueId (required, @length 1..64), Status (required, enum ResourceSignalStatus SUCCESS/FAILURE)). Two consequences are observable through DescribeAutoScalingGroups while the update runs: the InService count dips (to DesiredCapacity - MaxBatchSize, never below MinInstancesInService, never above DesiredCapacity), and the update spans at least one instance-launch + signal round-trip per batch, i.e. it cannot finish in the time it takes to poll the group twice. For the exact stack-event wording there is recorded behavior only; the requirement asserted here is only that the batched replacement happens at all and is observable.
Emulator: the UpdateStack reaches its terminal status essentially instantly. Sampling DescribeAutoScalingGroups every 10 s for the whole update of a 3-instance group with MaxBatchSize=1 (3 batches) yielded 2 samples — the sampling-adequacy precondition the check asserts before trusting any minimum ("only 2 samples were taken across a 3-batch rolling update; at least 9 are needed before the observed minimum in-service count means anything. The update finished faster than expected -- widen the sampling, do not trust this run.") fails. Nothing is ever taken out of service and no replacement is waited on, so MinInstancesInService / MaxBatchSize / WaitOnResourceSignals / PauseTime are all unenforced, and the FAILURE-signal leg (a replacement that signals FAILURE must cancel the update and roll the stack back to the launch-template version it started from) can never be reached.
Why: a client that relies on a rolling update for a zero-downtime deploy gets an atomic, unvalidated swap instead — capacity guarantees it declared are not honoured, and a bad AMI/UserData revision is never caught by the signal gate because no signal is awaited.
Fix: the UpdatePolicy.AutoScalingRollingUpdate path of the AWS::AutoScaling::AutoScalingGroup resource handler in the CloudFormation provisioner — batch scheduling (terminate up to MaxBatchSize, respect MinInstancesInService and DesiredCapacity), per-batch WaitOnResourceSignals accounting against SignalResource, PauseTime, and MinSuccessfulInstancesPercent-driven cancellation plus rollback.
Failing check: autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal.

Cause 2 of 2: UpdatePolicy AutoScalingReplacingUpdate with WillReplace=true updates the group in place instead of replacing it.
AWS: with UpdatePolicy.AutoScalingReplacingUpdate {WillReplace: true}, an update to the group (here: the group's LaunchTemplate property, the group carrying no AutoScalingGroupName so that it is replaceable) makes CloudFormation create a NEW Auto Scaling group and delete the old one once the new one's instances have signalled, so the logical resource's PhysicalResourceId changes to the new group's name; if the replacement's instances signal FAILURE the new group is deleted and the original group survives the rollback. Recorded behavior only for the exact event wording.
Emulator: after the update the AWS::AutoScaling::AutoScalingGroup's PhysicalResourceId is unchanged — the same group name is reported before and after (AssertionError: "WillReplace=true must give the group a NEW PhysicalResourceId, but it is still '<stack>-AutoScalingGroup-<suffix>'"; the group name is a per-run generated value, the requirement is only that the id differs from the pre-update one). The group was mutated in place, so no replacement group exists.
Why: WillReplace is the all-or-nothing swap: the client expects a second group to exist, to be validated by signals, and to be discarded on failure while the original keeps serving. With an in-place mutation there is no original left to roll back to, and the documented rejection of a failed replacement cannot occur.
Fix: the UpdatePolicy.AutoScalingReplacingUpdate path of the same handler — update classification (WillReplace forces replacement), minting the new group's physical resource id, and the delete-after-success / discard-on-failure ordering in the CloudFormation update engine.
Failing check: autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected.

Traces: the two failing check ids in trace_ids.

## Evidence
Cause 1 — autoscaling__cloudformation__ec2.cfn-rolling-update-honours-max-batch-size-then-rolls-back-on-a-failed-signal [mismatch]:
  AssertionError: only 2 samples were taken across a 3-batch rolling update; at least 9 are needed before the observed minimum in-service count means anything. The update finished faster than expected -- widen the sampling, do not trust this run.
  Setup (inlined deployments/asg-rolling-update-signals.yaml): DesiredCapacity=3, MinSize=1, MaxSize=4; UpdatePolicy.AutoScalingRollingUpdate {MaxBatchSize: 1, MinInstancesInService: 1, MinSuccessfulInstancesPercent: 100, PauseTime: PT15M, WaitOnResourceSignals: true, SuspendProcesses: [HealthCheck, ReplaceUnhealthy, AZRebalance, AlarmNotification, ScheduledActions]}; CreationPolicy.ResourceSignal {Count: 3, Timeout: PT15M}; instances signal with `aws cloudformation signal-resource` from UserData. The update bumps a Generation parameter, which rewrites UserData -> new launch-template version -> the group's LaunchTemplate property changes (a documented rolling-update trigger).
  Expected (AWS): the update spans 3 signal-gated batches, so DescribeAutoScalingGroups sampled every 10 s yields >= 9 samples whose InService counts dip to exactly DesiredCapacity - MaxBatchSize = 2, never below MinInstancesInService = 1 and never above DesiredCapacity = 3.
  Actual (emulator): the stack reached a terminal update status within two 10 s polls — only 2 samples exist, so no dip, no batch boundary and no awaited signal is observable.

Cause 2 — autoscaling__cloudformation__ec2.deploy-autoscaling-1-replacing-update-swaps-group-failed-signal-replacement-rejected [mismatch]:
  AssertionError: WillReplace=true must give the group a NEW PhysicalResourceId, but it is still 'sim2real-xsvc-rpa-0aea55279289-AutoScalingGroup-947F2D60FF0E'
  (the quoted name is one run's generated value; the requirement is that the post-update PhysicalResourceId differs from the pre-update one.)
  Setup: the replacing-update twin of the template above — same CreationPolicy.ResourceSignal signal path, DESIRED=2, UpdatePolicy.AutoScalingReplacingUpdate {WillReplace: true}, and no AutoScalingGroupName (CloudFormation cannot replace a custom-named resource).
  Expected (AWS): DescribeStackResources/DescribeStackEvents report a new group name as the resource's PhysicalResourceId after the update.
  Actual (emulator): the pre-update group name is still the PhysicalResourceId.

## Steps to Reproduce
Cause 1:
1. Build and boot the emulator from this checkout.
2. CreateStack a template whose AWS::AutoScaling::AutoScalingGroup has DesiredCapacity 3, MinSize 1, MaxSize 4, a launch template, CreationPolicy.ResourceSignal {Count: 3, Timeout: PT15M} and UpdatePolicy.AutoScalingRollingUpdate {MaxBatchSize: 1, MinInstancesInService: 1, MinSuccessfulInstancesPercent: 100, PauseTime: PT15M, WaitOnResourceSignals: true}; wait for CREATE_COMPLETE.
3. UpdateStack changing only the launch template's UserData (so the group's LaunchTemplate property points at a new version), and while the update runs poll `aws autoscaling describe-auto-scaling-groups --auto-scaling-group-names <name>` every 10 s, recording the instances in LifecycleState InService.
   Observed: the stack reaches UPDATE_COMPLETE after at most two polls; the InService set never changes.
   Expected: at least one poll per batch shows the count dipped to 2 (DesiredCapacity - MaxBatchSize) and never below MinInstancesInService, and the update does not finish until each batch's replacement has called SignalResource with Status=SUCCESS.
4. Second leg: set the UserData so replacements signal FAILURE and confirm the update is cancelled and the stack rolls back to the original launch-template version.

Cause 2:
1. CreateStack the same estate but with no AutoScalingGroupName, DesiredCapacity 2, and UpdatePolicy.AutoScalingReplacingUpdate {WillReplace: true}; wait for CREATE_COMPLETE and record the group's PhysicalResourceId from `aws cloudformation describe-stack-resources`.
2. UpdateStack so the group's LaunchTemplate property changes; wait for the stack to settle and read the PhysicalResourceId again.
   Observed: unchanged.
   Expected: a new Auto Scaling group name.
3. Replay the two checks in trace_ids to confirm the fix.
