Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ecs__elbv2.autoscaling-lifecycle-hook-event-delivered-to-eventbridge
Issue-Key: autoscaling__ecs__elbv2.autoscaling-lifecycle-hook-event-delivered-to-eventbridge|_wait_for_lifecycle_event#28|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: 3f07c9f1d60993a38a36a4f215d4651495d2c52f

# autoscaling__ecs__elbv2: scaling out a group with a launch lifecycle hook publishes no 'EC2 Instance-launch Lifecycle Action' event, so an EventBridge-targeted queue never receives the hook's transition and action token

> _Note: this issue was identified against the code base at commit `3f07c9f1d60993a38a36a4f215d4651495d2c52f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
Cause 1 of 1: with an autoscaling:EC2_INSTANCE_LAUNCHING lifecycle hook on the group, a scale-out emits no 'EC2 Instance-launch Lifecycle Action' event, so an EventBridge rule routing that event to an SQS queue delivers nothing. Failing check id: autoscaling__ecs__elbv2.autoscaling-lifecycle-hook-event-delivered-to-eventbridge.

AWS: recorded behavior — when a launching instance is held by a launch lifecycle hook, Auto Scaling publishes an event with source 'aws.autoscaling', detail-type 'EC2 Instance-launch Lifecycle Action' and a detail carrying AutoScalingGroupName, EC2InstanceId, LifecycleHookName, LifecycleTransition == 'autoscaling:EC2_INSTANCE_LAUNCHING' and the LifecycleActionToken the client must pass back. The Smithy model (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json) states no event schema, but it does require the token path the event carries: CompleteLifecycleAction.LifecycleActionToken is @length 36..36 and RecordLifecycleActionHeartbeat accepts the same token.

Emulator: no event matching {source: aws.autoscaling, detail-type: 'EC2 Instance-launch Lifecycle Action', detail.AutoScalingGroupName: <group>} is ever delivered to the rule's target; the check polls the queue and is killed by the harness at 60 s with no verdict. The group does scale out (sibling traces show launch activities being emitted), so the launch is simply not announced as a lifecycle action and no token is minted.

Why: event-driven hook handlers — the documented pattern for ECS capacity providers, instance warm-up and bootstrap automation — are never invoked, and because the token is only ever delivered in that event, a client has no way to complete or heartbeat the hook; instances are admitted without the hook's work ever running.

Fix: the group's launch reconciliation in the autoscaling service state must publish the lifecycle-action event (hook name, transition, minted LifecycleActionToken, instance id) to the event bus when an instance is held by a launch hook, alongside the existing 'EC2 Instance Launch Successful' activity notification.

Note: open issue 20261107T080429Z-autoscaling-ecs-elbv2-6-net-new-causes-asg.md mentions Auto Scaling events not reaching EventBridge for this service but does not carry this check's replay key, and the replay does not report this check as owned by it; it does not cover this cause.

## Evidence
Expected (AWS): after autoscaling:PutLifecycleHook(LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING) on group G and a scale-out, an EventBridge rule matching {"source": ["aws.autoscaling"], "detail-type": ["EC2 Instance-launch Lifecycle Action"], "detail": {"AutoScalingGroupName": [G]}} delivers an envelope whose source == 'aws.autoscaling', detail-type == 'EC2 Instance-launch Lifecycle Action' and whose detail carries LifecycleTransition == 'autoscaling:EC2_INSTANCE_LAUNCHING', the LifecycleHookName and a 36-character LifecycleActionToken.
Actual (emulator): the target queue receives no message matching that rule; the replay records 'Error: Check timed out after 60.0s' while polling for it.

## Steps to Reproduce
1. Create an ASG G with a launch template, then autoscaling:PutLifecycleHook(LifecycleHookName=H, AutoScalingGroupName=G, LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING, HeartbeatTimeout=60).
2. Create an SQS queue and an EventBridge rule with pattern {"source": ["aws.autoscaling"], "detail-type": ["EC2 Instance-launch Lifecycle Action"], "detail": {"AutoScalingGroupName": [G]}} targeting the queue.
3. autoscaling:SetDesiredCapacity(G, 1) (or UpdateAutoScalingGroup raising capacity).
4. Poll the queue for the lifecycle-action event -> nothing arrives.
5. Re-replay autoscaling__ecs__elbv2.autoscaling-lifecycle-hook-event-delivered-to-eventbridge.
