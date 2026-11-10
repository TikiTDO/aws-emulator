Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule
Issue-Key: ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule|_build#5|Timeout|time.sleep(_WIRING_SETTLE_S)
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# ec2__events__sqs: the per-state EC2 Instance State-change Notification flow never reaches a verdict — the instance lifecycle/event delivery does not progress within the check's budget

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
One net-new root cause of service `ec2__events__sqs`, grounded in one failing behavioral check.

Cause 1 of 1 — driving one instance through its lifecycle produces no per-state EventBridge delivery the check can grade; the whole check burns its budget without a verdict.
AWS: recorded behavior only (EC2's EventBridge integration is not modelled in Smithy). EC2 publishes an `EC2 Instance State-change Notification` event (source `aws.ec2`) on the default bus for each state an instance enters — `pending`, `running`, `stopping`, `stopped`, `shutting-down`, `terminated` — with `detail.instance-id` and `detail.state`. A rule whose pattern names one state receives only that state's envelope, and a rule scoped to a different state receives nothing for it.
Emulator: the check does not get far enough to compare anything — `Error: Check timed out after 60.0s`. No per-state envelope was observed on any of the rules' queues.
Why: a client that reacts to instance state transitions (scaling, cleanup, cost control, drain-on-terminate) is never notified, and a state-scoped rule cannot be distinguished from a broken one.
Fix: the EC2 instance state machine's event-emission path (it must publish one `EC2 Instance State-change Notification` per state transition to the EventBridge default bus), and whatever in the instance lifecycle keeps the run/stop/terminate sequence from progressing in time. Open issues 20261107T075601Z-ec2-events-sqs-ec2-eventbridge-volume-instance-lifecycle-notifications-never-arrive.md and 20261110T160612Z-ec2-events-sqs-ec2-instance-state-change-notifications.md cover sibling checks of this family and do not carry this check's replay key.
Traces: ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule

## Evidence
Check ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule
Error: Check timed out after 60.0s — no state-change envelope was ever observed, so the per-state routing (one rule per state, a 'shutting-down'-scoped rule receiving only that state) could not be graded at all.
Expected: for each of pending/running/stopping/stopped/shutting-down/terminated, one envelope with source 'aws.ec2', detail-type 'EC2 Instance State-change Notification' and detail {instance-id, state} delivered to the rule scoped to that state, and nothing to the rules scoped to the other states.

## Steps to Reproduce
1. Boot the emulator and replay ec2__events__sqs.instance-state-change-notifications-are-delivered-per-state-to-their-own-rule.
2. Create a VPC/subnet/security group, resolve the AL2023 AMI from SSM, and RunInstances one t3.micro.
3. Create EventBridge rules on {"source":["aws.ec2"],"detail-type":["EC2 Instance State-change Notification"],"detail":{"state":["<state>"]}} for each state, each targeting its own queue (queue policies granting events.amazonaws.com SendMessage).
4. Stop, start and terminate the instance and ReceiveMessage on each queue: expect each state's envelope only on its own rule's queue.
