Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__elbv2.asg-lifecycle-hook-event-not-delivered-without-notification-permission autoscaling__ec2__elbv2.asg-lifecycle-hook-event-triggers-on-instance-terminate
Issue-Key: autoscaling__ec2__elbv2.asg-lifecycle-hook-event-not-delivered-without-notification-permission|run#11|AssertionError|raise AssertionError(
Issue-Key: autoscaling__ec2__elbv2.asg-lifecycle-hook-event-triggers-on-instance-terminate|_poll_sqs_for_message#15|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=10)
Issue-Commit: 3f07c9f1d60993a38a36a4f215d4651495d2c52f

# autoscaling__ec2__elbv2: PutLifecycleHook accepts a notification target its RoleARN cannot publish to, and no 'EC2 Instance-terminate Lifecycle Action' event is ever published (2 causes)

> _Note: this issue was identified against the code base at commit `3f07c9f1d60993a38a36a4f215d4651495d2c52f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
Two net-new root causes on the lifecycle-hook notification path. A fixer must close BOTH before submitting.

Cause 1 of 2: PutLifecycleHook does not publish the synchronous test notification, so a hook whose RoleARN cannot publish to its NotificationTargetARN is accepted with HTTP 200 instead of being refused. Failing check id: autoscaling__ec2__elbv2.asg-lifecycle-hook-event-not-delivered-without-notification-permission.
AWS: specifying a notification target makes Auto Scaling send a test message to it as part of the call — Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json) documents LifecycleHook.RoleARN as "The ARN of the IAM role that allows the Auto Scaling group to publish to the specified notification target (an Amazon SNS topic or an Amazon SQS queue)" and NotificationTargetARN as the target Auto Scaling sends notifications to. When that publish fails the call is refused synchronously with error code ValidationError and the message "Unable to publish test message to notification target <NotificationTargetARN> using IAM role <RoleARN>. Please check your target and role configuration and try to put lifecycle hook again." — the two ARNs are values from the request and the rest is fixed text — and no hook is created. (The message is recorded behavior for this error class; ValidationError is not in PutLifecycleHook's modeled error list, which carries only LimitExceededFault 400 and ResourceContentionFault 500.)
Emulator: the same PutLifecycleHook with a role holding no sqs:SendMessage on the target queue returns success and creates the hook; no test message is published and no refusal is produced. Replay detail: "PutLifecycleHook '<hook>' with a role that cannot publish to <queue arn> was accepted; AWS refuses it synchronously with ValidationError".
Why: a misconfigured role/target pair is accepted, so the operator learns nothing at configuration time and instead discovers at scale time that hook notifications go nowhere while instances sit in a wait state until DefaultResult fires. The positive control is affected too: nothing proves the target is reachable.
Fix: the PutLifecycleHook handler (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/src/ops/put_lifecycle_hook.rs) must attempt the autoscaling:TEST_NOTIFICATION publish to NotificationTargetARN under RoleARN before committing the hook, and on failure refuse with ValidationError carrying AWS's message template and leave the hook collection unchanged.

Cause 2 of 2: a terminating lifecycle hook produces no 'EC2 Instance-terminate Lifecycle Action' EventBridge event on scale-in. Failing check id: autoscaling__ec2__elbv2.asg-lifecycle-hook-event-triggers-on-instance-terminate.
AWS: recorded behavior — when an instance enters the terminate wait state of an autoscaling:EC2_INSTANCE_TERMINATING hook, Auto Scaling publishes an event with source 'aws.autoscaling', detail-type 'EC2 Instance-terminate Lifecycle Action' and a detail carrying AutoScalingGroupName, EC2InstanceId, LifecycleTransition == 'autoscaling:EC2_INSTANCE_TERMINATING', LifecycleHookName and LifecycleActionToken; an EventBridge rule matching that source/detail-type delivers it to its target.
Emulator: no such event is ever delivered — the check polls its rule target for a matching event and is killed by the harness at 60 s with no event seen. This is the EventBridge face of the same gap as the wait state itself: with no terminate wait state there is no lifecycle-action event or token.
Why: event-driven drain/backup automation triggered by the terminate hook never runs, and the client gets no action token with which to complete the hook.
Fix: the group's termination reconciliation in the autoscaling service state must emit the lifecycle-action event (with the hook name and the minted LifecycleActionToken) to the event bus when an instance is held by a terminating hook, alongside the existing activity notifications.

Note: open issue 20261111T030957Z-autoscaling-ec2-elbv2-4-net-new-causes-target.md covers target-health/registration causes for this service and does not carry either of these replay keys; it does not cover these two causes.

## Evidence
Cause 1 — Expected (AWS): autoscaling:PutLifecycleHook(LifecycleHookName=<hook>, AutoScalingGroupName=<asg>, LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING, RoleARN=<role without publish permission>, NotificationTargetARN=<queue arn>, HeartbeatTimeout=60) is refused with Error.Code == 'ValidationError' and Error.Message == "Unable to publish test message to notification target <queue arn> using IAM role <role arn>. Please check your target and role configuration and try to put lifecycle hook again." (the two ARNs come from the request; the surrounding sentence is fixed), and no hook is created. With a role that CAN publish, the call succeeds and an autoscaling:TEST_NOTIFICATION message for that group arrives at the target.
Actual (emulator): the call returns HTTP 200 and creates the hook. Replay detail: AssertionError: PutLifecycleHook 'sim2real-hook-neg-…' with a role that cannot publish to arn:aws:sqs:…:sim2real-q-… was accepted; AWS refuses it synchronously with ValidationError.

Cause 2 — Expected (AWS): with a terminating hook on the group, scaling in publishes an event {source: 'aws.autoscaling', detail-type: 'EC2 Instance-terminate Lifecycle Action', detail: {AutoScalingGroupName: <asg>, EC2InstanceId: <instance>, LifecycleTransition: 'autoscaling:EC2_INSTANCE_TERMINATING', LifecycleHookName: <hook>, LifecycleActionToken: <token>}} which an EventBridge rule matching that source and detail-type delivers to its target.
Actual (emulator): nothing matching that pattern is ever delivered; the check is recorded as 'Error: Check timed out after 60.0s' while waiting for it.

## Steps to Reproduce
1. Create an SQS queue Q and an ASG G (launch template, MinSize 0, MaxSize 0/1) ; create IAM role R1 trusted by autoscaling.amazonaws.com WITHOUT permission to publish to Q, and R2 WITH it.
2. autoscaling:PutLifecycleHook(LifecycleHookName=Hneg, AutoScalingGroupName=G, LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING, RoleARN=R1, NotificationTargetARN=<Q arn>, HeartbeatTimeout=60) -> emulator answers 200; AWS refuses with ValidationError and the publish-failure message (cause 1).
3. Repeat with R2 -> expect 200 plus an autoscaling:TEST_NOTIFICATION message for G in Q.
4. Put an autoscaling:EC2_INSTANCE_TERMINATING hook on a group with a running instance, add an EventBridge rule matching {source: [aws.autoscaling], detail-type: ['EC2 Instance-terminate Lifecycle Action'], detail: {AutoScalingGroupName: [G]}} with a queue target, then scale the group in and poll the target -> no event arrives (cause 2).
5. Re-replay autoscaling__ec2__elbv2.asg-lifecycle-hook-event-not-delivered-without-notification-permission and autoscaling__ec2__elbv2.asg-lifecycle-hook-event-triggers-on-instance-terminate.
