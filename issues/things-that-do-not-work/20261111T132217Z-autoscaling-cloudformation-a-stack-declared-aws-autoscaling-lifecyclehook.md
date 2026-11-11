Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted
Issue-Key: autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted|run#21|AssertionError|assert [h["LifecycleHookName"] for h in hooks] == [hook_name], (
Issue-Commit: 3f07c9f1d60993a38a36a4f215d4651495d2c52f

# autoscaling__cloudformation: a stack-declared AWS::AutoScaling::LifecycleHook is never registered on the group — DescribeLifecycleHooks lists nothing after CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `3f07c9f1d60993a38a36a4f215d4651495d2c52f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeLifecycleHooks

## Description
Cause 1 of 1: a CloudFormation stack that declares an AWS::AutoScaling::LifecycleHook child of an AWS::AutoScaling::AutoScalingGroup reaches CREATE_COMPLETE and resolves the resource's Ref to the hook name, but the hook is never added to the group's hook collection, so the Auto Scaling read path reports no hook at all. Failing check id: autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted.

AWS: an AWS::AutoScaling::LifecycleHook resource is materialized by calling PutLifecycleHook on the referenced group with the template's properties, so after the stack reaches CREATE_COMPLETE the hook is an attribute of the group. Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json): DescribeLifecycleHooks takes AutoScalingGroupName (@required, XmlStringMaxLen255 1..255) and an optional LifecycleHookNames list (0..50) and returns LifecycleHooks[] of LifecycleHook{LifecycleHookName, AutoScalingGroupName, LifecycleTransition, HeartbeatTimeout, GlobalTimeout, DefaultResult, NotificationTargetARN, RoleARN, NotificationMetadata}; the only modeled error is ResourceContentionFault (HTTP 500). So an unfiltered DescribeLifecycleHooks on the stack's group must list exactly the hook the template declared, with LifecycleTransition, HeartbeatTimeout and DefaultResult equal to the template's property values; after the child-removing stack update it must list nothing for that hook name.

Emulator: the stack create succeeds and the hook's Ref is correct, but DescribeLifecycleHooks on the created group returns an empty LifecycleHooks list — the hook was never written into the group's state. The check fails at its first hook assertion: "DescribeLifecycleHooks on '<asg-name>' lists [], expected only '<hook-name>'". The group name and hook name are per-run values; the requirement is that the list contains exactly the declared hook.

Why: a client that provisions lifecycle hooks declaratively (the normal way, and what CDK's L2 ASG emits) gets a stack that reports success while the group has no hook: drain/warm-up/bootstrap actions silently never run, and the stack is not a faithful description of the account. It also makes the matching removal assertion vacuous, so a client cannot tell a removed hook from one that was never created.

Fix: the CloudFormation resource provider for AWS::AutoScaling::LifecycleHook must perform the create/update/delete against the autoscaling service state (the same upsert PutLifecycleHook performs on the group's hook collection) rather than only minting a physical id; the group's hook collection is what DescribeLifecycleHooks projects.

Traces: autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted.

## Evidence
Expected (AWS): after the stack with resources {ASG: AWS::AutoScaling::AutoScalingGroup, LifecycleHook: AWS::AutoScaling::LifecycleHook with Properties {AutoScalingGroupName: {Ref: ASG}, LifecycleHookName: <hook>, LifecycleTransition: autoscaling:EC2_INSTANCE_LAUNCHING, HeartbeatTimeout: 60, DefaultResult: ABANDON}} reaches CREATE_COMPLETE, autoscaling:DescribeLifecycleHooks(AutoScalingGroupName=<asg>) returns LifecycleHooks with exactly one entry whose LifecycleHookName == <hook> and whose LifecycleTransition/HeartbeatTimeout/DefaultResult equal the template's values; after the update that removes the child resource, DescribeLifecycleHooks(AutoScalingGroupName=<asg>, LifecycleHookNames=[<hook>]) returns [].
Actual (emulator): the stack reaches CREATE_COMPLETE and Ref LifecycleHook == <hook>, but DescribeLifecycleHooks(AutoScalingGroupName=<asg>) returns LifecycleHooks == []. Replay detail: AssertionError: DescribeLifecycleHooks on '<asg-name>' lists [], expected only '<hook-name>'.

## Steps to Reproduce
1. cloudformation:CreateStack with an AWS::EC2::LaunchTemplate, an AWS::AutoScaling::AutoScalingGroup (MinSize 0, MaxSize 1, DesiredCapacity 0) and an AWS::AutoScaling::LifecycleHook child with Properties {AutoScalingGroupName: {Ref: ASG}, LifecycleHookName: <hook>, LifecycleTransition: autoscaling:EC2_INSTANCE_LAUNCHING, HeartbeatTimeout: 60, DefaultResult: ABANDON}; wait for CREATE_COMPLETE.
2. autoscaling:DescribeLifecycleHooks(AutoScalingGroupName=<the stack's group>) -> emulator returns LifecycleHooks: [] instead of the single declared hook.
3. cloudformation:UpdateStack removing the child resource, then autoscaling:DescribeLifecycleHooks(AutoScalingGroupName=<asg>, LifecycleHookNames=[<hook>]) -> must be [] (unobservable today, since step 2 was already empty).
4. Re-replay autoscaling__cloudformation.deploy-asg-lifecyclehook-scheduledaction-warmpool-provisioned-and-deleted.
