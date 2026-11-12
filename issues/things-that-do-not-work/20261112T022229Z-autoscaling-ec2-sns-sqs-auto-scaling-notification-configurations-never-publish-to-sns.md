Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-test-notification-reaches-only-the-configured-topic autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-launch-and-terminate-name-the-exact-instance
Issue-Key: autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-test-notification-reaches-only-the-configured-topic|_drain#3|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Key: autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-launch-and-terminate-name-the-exact-instance|_drain#3|Timeout|resp = sqs.receive_message(QueueUrl=queue_url, MaxNumberOfMessages=10,
Issue-Commit: 8a5c50c8a2e4f68c625b1ffaca3d8c020fa0b343

# Auto Scaling notification configurations never publish to SNS: no TEST_NOTIFICATION and no EC2_INSTANCE_LAUNCH/_TERMINATE reaches the configured topic (2 causes, 2 checks)

> _Note: this issue was identified against the code base at commit `8a5c50c8a2e4f68c625b1ffaca3d8c020fa0b343`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutNotificationConfiguration

## Description
Two net-new root causes on the Auto Scaling group notification surface. Both failing checks configure a real topic->queue pipe (proved live with a plain SNS Publish canary first), attach it to a group with PutNotificationConfiguration, and then wait on the subscriber queue; both burn the whole per-check budget with nothing delivered.

Model note: the autoscaling service crate and its Smithy model are not present in this finder checkout (services/service-com-amazonaws-autoscaling-autoscaling/... cannot be read here), so the AWS statements below are the Auto Scaling API/Developer Guide documentation for notifications plus recorded behavior, not a model citation, and the Fix names the component rather than a file and line.

Note on existing issues: several open autoscaling issues cover NEIGHBOURING causes and do not cover these — 20261107T081710Z-autoscaling-describenotificationconfigurations-stub.md (DescribeNotificationConfigurations is an unimplemented stub, which is about the READBACK, not the publish), 20261107T075601Z-autoscaling-ec2-events-sqs-asg-lifecycle-events-never-reach-eventbridge-sqs-and.md (the EVENTBRIDGE lifecycle-event path, a different delivery path from a group's own SNS notification configuration), and 20261107T124656Z-autoscaling-a-group-with-desiredcapacity-0-never-reports-an.md / 20261111T093414Z-autoscaling-a-desiredcapacity-0-group-never-reaches-an.md (a DesiredCapacity>0 group never reports an InService instance). That last pair is relevant to Cause 2: the same check also waits for an InService instance, so a fixer must re-replay the check after fixing both the capacity convergence and the publish path.

Cause 1 of 2: PutNotificationConfiguration stores the configuration but publishes no autoscaling:TEST_NOTIFICATION to the named topic.
AWS: Auto Scaling documents that configuring notifications for a group sends a test notification to the topic — a message whose JSON body carries Event='autoscaling:TEST_NOTIFICATION', AutoScalingGroupName = the configured group and AutoScalingGroupARN = that group's ARN — delivered to that topic ALONE; a topic that no notification configuration of the group names receives nothing. DescribeNotificationConfigurations then lists exactly the (TopicARN, NotificationType) pairs configured, and DeleteNotificationConfiguration removes one pair at a time. Fixed parts of the requirement: the notification type string 'autoscaling:TEST_NOTIFICATION' and the member names Event / AutoScalingGroupName / AutoScalingGroupARN inside the published message. Variable parts, from the request and state: the topic ARN, the group name, the group ARN and the timestamps.
Emulator: nothing is ever delivered. The check configures ONE of two live, independently proved topic->queue pipes and waits up to 300s for the TEST_NOTIFICATION on the configured queue; the wait never terminates and the harness kills the check at its 60s ceiling, so the later arms (the second topic becoming live after a second PutNotificationConfiguration, and DeleteNotificationConfiguration leaving exactly the remaining pair) are never reached either.
Why: a client that wires a group's notifications to SNS uses the test notification as the proof the wiring works. Here it gets silence, cannot distinguish "my topic policy is wrong" from "the target publishes nothing", and every downstream alarm/automation subscribed to that topic stays dark.
Traces: autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-test-notification-reaches-only-the-configured-topic.

Cause 2 of 2: scaling a group up and back down publishes no autoscaling:EC2_INSTANCE_LAUNCH or autoscaling:EC2_INSTANCE_TERMINATE notification to the group's configured topic.
AWS: with a notification configuration for those two types, each scaling transition publishes a message naming the exact instance the group moved (EC2InstanceId), the group (AutoScalingGroupName and AutoScalingGroupARN) and the ActivityId of the scaling activity that moved it — an id that DescribeScalingActivities also reports for the group, with launch and terminate carrying DIFFERENT activity ids; the terminated instance is really shutting-down/terminated in EC2. Fixed parts: the type strings 'autoscaling:EC2_INSTANCE_LAUNCH' / 'autoscaling:EC2_INSTANCE_TERMINATE' and the member names Event / EC2InstanceId / AutoScalingGroupName / AutoScalingGroupARN / ActivityId. Variable parts, from state: the instance id, group name/ARN, the activity ids and the timestamps.
Emulator: the check sets DesiredCapacity 0 -> 1, waits for an InService instance and then for the launch notification, then sets it back to 0 and waits for the terminate notification; the check never reaches an observable step inside its budget and is killed at 60s. No notification for either transition is delivered to the configured topic.
Why: launch/terminate notifications are how a client reconciles its own inventory with the group (and how it correlates a notification with a scaling activity). With none published, scale-out and scale-in are invisible to every SNS subscriber, and the per-instance ActivityId correlation AWS guarantees cannot be observed at all.
Traces: autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-launch-and-terminate-name-the-exact-instance.

Fix (both causes): the Auto Scaling notification component — the handler for PutNotificationConfiguration (which owes the topic a TEST_NOTIFICATION at configuration time) and the group's scaling-transition path that records a scaling activity (which owes the configured topics an EC2_INSTANCE_LAUNCH / EC2_INSTANCE_TERMINATE message naming the instance, the group and that activity id) must publish to every topic the group's stored notification configurations name for that type, and to no other topic. The configuration store those handlers read is the same one DescribeNotificationConfigurations must report.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against this checkout: 451/602 passed (74.9%). Service autoscaling__ec2__sns__sqs 0/2 (0%), 2 failed. Neither check is marked ALREADY FILED or OWNED in this report.

  [mismatch] autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-test-notification-reaches-only-the-configured-topic
  Error: Check timed out after 60.0s
  [mismatch] autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-launch-and-terminate-name-the-exact-instance
  Error: Check timed out after 60.0s

Both checks first prove their SNS topic -> SQS queue pipes end to end with a plain sns Publish canary (_publish_until_delivered), so an absence afterwards is the Auto Scaling side, not the SNS/SQS side.

Expected (AWS), test-notification check: after PutNotificationConfiguration(AutoScalingGroupName=<group>, TopicARN=<lit topic>, NotificationTypes=['autoscaling:TEST_NOTIFICATION']), the queue subscribed to <lit topic> receives an SNS envelope (TopicArn=<lit topic>) whose Message JSON has Event='autoscaling:TEST_NOTIFICATION', AutoScalingGroupName=<group>, AutoScalingGroupARN=<group arn>, while the second, UNCONFIGURED topic's queue receives nothing; configuring the second topic then delivers to its queue too, and deleting the configurations one at a time leaves exactly the remaining pair, then none.
Actual: no message ever arrives on the configured queue; the 300s _await_event wait is cut off by the suite's 60s ceiling.

Expected (AWS), launch/terminate check: with NotificationTypes=['autoscaling:EC2_INSTANCE_LAUNCH','autoscaling:EC2_INSTANCE_TERMINATE'], SetDesiredCapacity 1 then 0 delivers two messages naming the same EC2InstanceId, the group name and ARN, and two DIFFERENT ActivityIds both present in DescribeScalingActivities, with the instance reported shutting-down/terminated by EC2 DescribeInstances.
Actual: the check never reaches an observable step inside its budget and is killed at 60s; no launch or terminate notification is delivered.

The report gives only the timeout verdict, so the exact statement each check is parked on is not isolated by the replay; for the launch/terminate check the InService wait covered by the two open DesiredCapacity>0 issues named in the description can park it before the notification wait is reached. Re-replay BOTH check ids listed in trace_ids to confirm a fix.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Cause 1: sns CreateTopic x2 (call them lit and dark); sqs CreateQueue x2; set each queue's Policy to allow sns.amazonaws.com sqs:SendMessage for its topic; sns Subscribe(Protocol='sqs') each queue to its topic; prove BOTH pipes with a plain sns Publish that arrives on the matching queue.
3. ec2 CreateVpc + CreateSubnet + CreateLaunchTemplate (ImageId from ssm GetParameter /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-x86_64, InstanceType t3.micro); autoscaling CreateAutoScalingGroup(MinSize=0, MaxSize=1, DesiredCapacity=0, VPCZoneIdentifier=<subnet>).
4. autoscaling PutNotificationConfiguration(AutoScalingGroupName=<group>, TopicARN=<lit topic>, NotificationTypes=['autoscaling:TEST_NOTIFICATION']).
5. sqs ReceiveMessage on the lit queue -> expect an SNS Notification envelope from <lit topic> whose Message JSON has Event='autoscaling:TEST_NOTIFICATION', AutoScalingGroupName=<group> and AutoScalingGroupARN=<group arn>; the dark queue must stay empty. Observe nothing arrives on either queue.
6. autoscaling PutNotificationConfiguration for the dark topic as well -> expect its queue to receive a TEST_NOTIFICATION too; then DescribeNotificationConfigurations -> expect both (TopicARN, NotificationType) pairs; then DeleteNotificationConfiguration for one -> expect exactly the other to remain, and for both -> none.
7. Cause 2: on a second group with NotificationTypes=['autoscaling:EC2_INSTANCE_LAUNCH','autoscaling:EC2_INSTANCE_TERMINATE'], autoscaling SetDesiredCapacity(DesiredCapacity=1), wait for one InService instance, then SetDesiredCapacity(DesiredCapacity=0).
8. sqs ReceiveMessage -> expect one EC2_INSTANCE_LAUNCH and one EC2_INSTANCE_TERMINATE message, both naming the launched instance id, the group name and ARN, each with an ActivityId present in autoscaling DescribeScalingActivities and the two ActivityIds different; ec2 DescribeInstances must report that instance shutting-down/terminated. Observe no notification is delivered.
9. Re-replay autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-test-notification-reaches-only-the-configured-topic and autoscaling__ec2__sns__sqs.deep-autoscaling-3-asg-notify-launch-and-terminate-name-the-exact-instance.
