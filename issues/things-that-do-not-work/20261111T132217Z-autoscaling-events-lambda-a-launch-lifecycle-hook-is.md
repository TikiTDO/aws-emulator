Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__events__lambda.xsvc-autoscaling-3-launch-hook-lambda-error-times-out-to-abandon
Issue-Key: autoscaling__events__lambda.xsvc-autoscaling-3-launch-hook-lambda-error-times-out-to-abandon|_poll#17|AssertionError|raise AssertionError(f"{what} was not observed within {timeout_s}s; last saw {last!r}")
Issue-Commit: 3f07c9f1d60993a38a36a4f215d4651495d2c52f

# autoscaling__events__lambda: a launch lifecycle hook is bypassed entirely — the instance is announced 'EC2 Instance Launch Successful' with no 'EC2 Instance-launch Lifecycle Action' event and no DefaultResult=ABANDON timeout

> _Note: this issue was identified against the code base at commit `3f07c9f1d60993a38a36a4f215d4651495d2c52f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutLifecycleHook

## Description
Cause 1 of 1: a group carrying an autoscaling:EC2_INSTANCE_LAUNCHING hook with DefaultResult=ABANDON admits the instance immediately: the only event the emulator publishes for the launch is 'EC2 Instance Launch Successful', so the EventBridge-routed Lambda that should receive the hook's lifecycle-action event is never invoked and the ABANDON-on-timeout outcome never happens. Failing check id: autoscaling__events__lambda.xsvc-autoscaling-3-launch-hook-lambda-error-times-out-to-abandon.

AWS: recorded behavior — a launching instance held by a launch hook is announced with source 'aws.autoscaling' and detail-type 'EC2 Instance-launch Lifecycle Action' (detail carrying AutoScalingGroupName, EC2InstanceId, LifecycleHookName, LifecycleTransition 'autoscaling:EC2_INSTANCE_LAUNCHING' and a LifecycleActionToken), and it stays in the wait state until CompleteLifecycleAction or until HeartbeatTimeout elapses. Smithy (services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json) states the timeout rule for the hook: LifecycleHook.HeartbeatTimeout "The maximum time, in seconds, that can elapse before the lifecycle hook times out. If the lifecycle hook times out, Amazon EC2 Auto Scaling performs the action that you specified in the DefaultResult property", and LifecycleHook.DefaultResult "Valid values: CONTINUE | ABANDON". With DefaultResult=ABANDON and a handler that keeps failing, the instance is therefore terminated before InService and the launch is announced 'EC2 Instance Launch Unsuccessful' — never 'EC2 Instance Launch Successful'.

Emulator: the launch is announced only as 'EC2 Instance Launch Successful' for the instance, with Description 'Launching a new EC2 instance: <instance-id>' and Cause 'a user request explicitly set group desired capacity: …' — i.e. the instance went straight to success. No 'EC2 Instance-launch Lifecycle Action' event is ever published, so the hook's Lambda target never receives an event (the check waits 180 s and sees only the Launch-Successful envelope), the hook's HeartbeatTimeout never elapses, and DefaultResult=ABANDON is never applied.

Why: a client that gates admission on a launch hook gets the opposite outcome from AWS: a group whose hook handler always fails still reports a healthy, InService instance and a successful launch, instead of terminating the instance and reporting the launch unsuccessful. Nothing the hook handler would have done (validation, registration, bootstrap) runs, and no client can detect the failure from the event stream.

Fix: the group's launch reconciliation in the autoscaling service state must hold the instance for the launch hook, publish the lifecycle-action event (with the minted LifecycleActionToken) to the event bus, and on HeartbeatTimeout expiry apply DefaultResult — terminating the instance and emitting 'EC2 Instance Launch Unsuccessful' for ABANDON — instead of emitting 'EC2 Instance Launch Successful' at launch time.

Traces: autoscaling__events__lambda.xsvc-autoscaling-3-launch-hook-lambda-error-times-out-to-abandon.

## Evidence
Expected (AWS): with PutLifecycleHook(LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING, HeartbeatTimeout=30, DefaultResult=ABANDON) on group G and an EventBridge rule routing 'EC2 Instance-launch Lifecycle Action' to a Lambda that always raises, the Lambda is invoked with that event for the launching instance; the hook then times out and ABANDON terminates the instance before InService, and the group's launch is announced as detail-type 'EC2 Instance Launch Unsuccessful'.
Actual (emulator): the Lambda is never invoked for a lifecycle-action event; within 180 s the only envelope recorded for the launch is {source: 'aws.autoscaling', detail-type: 'EC2 Instance Launch Successful', detail: {StatusCode: 'Successful', Description: 'Launching a new EC2 instance: <instance-id>', AutoScalingGroupName: <group>, EC2InstanceId: <instance-id>, Cause: 'a user request explicitly set group desired capacity: changing the desired capacity from 0 to 1.'}}. Replay detail: AssertionError: the Lambda receiving the launch hook event for <instance-id> was not observed within 180.0s. (Instance ids, group names, timestamps and request ids in that envelope are per-run values; the divergence is the detail-type served and the missing lifecycle-action event.)

## Steps to Reproduce
1. Create an ASG G (launch template, MinSize 0, MaxSize 1, DesiredCapacity 0) and autoscaling:PutLifecycleHook(LifecycleHookName=H, AutoScalingGroupName=G, LifecycleTransition=autoscaling:EC2_INSTANCE_LAUNCHING, HeartbeatTimeout=30, DefaultResult=ABANDON).
2. Create a Lambda that always raises and an EventBridge rule matching {"source": ["aws.autoscaling"], "detail-type": ["EC2 Instance-launch Lifecycle Action"], "detail": {"AutoScalingGroupName": [G]}} targeting it.
3. autoscaling:SetDesiredCapacity(G, 1) and watch the rule's target: the Lambda is never invoked, and the bus carries only 'EC2 Instance Launch Successful'.
4. Wait past HeartbeatTimeout and read DescribeAutoScalingGroups: the instance is InService instead of having been terminated by DefaultResult=ABANDON, and no 'EC2 Instance Launch Unsuccessful' event is published.
5. Re-replay autoscaling__events__lambda.xsvc-autoscaling-3-launch-hook-lambda-error-times-out-to-abandon.
