Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found
Issue-Key: cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found|_assert_replaced_and_retained#41|AssertionError|assert (
Issue-Commit: 83a487fe0db4ce5c414ce564b50d725b494461fa

# cloudformation__scheduler__sqs: a stack update that REPLACES an AWS::Scheduler::Schedule reports CREATE_* events instead of the UPDATE_* replacement vocabulary, and UpdateReplacePolicy Retain still deletes the replaced schedule (2 causes)

> _Note: this issue was identified against the code base at commit `83a487fe0db4ce5c414ce564b50d725b494461fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes of service `cloudformation__scheduler__sqs`, both grounded in the one failing behavioral check the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker). A fixer must close BOTH before submitting.

Cause 1 of 2 — a replacing update is announced with CREATE_* resource events instead of AWS's UPDATE_* replacement events.
AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation) models StackEvent.ResourceStatus as the enum ResourceStatus, whose members include UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED; the event vocabulary for a replacement is recorded behavior. When UpdateStack must replace a resource because a create-only property changed (here AWS::Scheduler::Schedule.Name), the events CloudFormation emits for that LOGICAL resource during the update are, in order: UPDATE_IN_PROGRESS with no reason; UPDATE_IN_PROGRESS carrying the fixed reason text `Requested update requires the creation of a new physical resource; hence creating one.`; UPDATE_IN_PROGRESS with reason `Resource creation Initiated`; and UPDATE_COMPLETE carrying the NEW physical id. The reason strings are AWS's fixed text; the physical ids are the names the request supplied (the schedule's Name property) and are the variable part.
Emulator: for that same update the logical resource `Schedule` emitted CREATE_IN_PROGRESS (no reason, empty physical id), CREATE_IN_PROGRESS (`Resource creation Initiated`, new schedule name), CREATE_COMPLETE (new schedule name). No UPDATE_* status is ever emitted for the logical id and the replacement announcement reason never appears, so an update is indistinguishable from a first create.
Why: every client that watches DescribeStackEvents to learn that an update replaced a resource — replacement/drift detectors, deployment gates, CI log scrapers, CDK/SAM progress reporters — sees a brand-new create inside an update and never sees the replacement announcement, so it cannot tell a replacement from a create and cannot attribute the new physical id to the update.
Fix: the cloudformation crate's resource-provisioner update path — a replacement must be driven as an UPDATE of the logical resource and emit the UPDATE_* events above, not a fresh CREATE_* sequence.
Traces: cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found

Cause 2 of 2 — `UpdateReplacePolicy: Retain` is ignored: the replaced schedule is actually deleted and no DELETE_SKIPPED event is emitted.
AWS: recorded behavior, plus the modeled ResourceStatus member DELETE_SKIPPED. With `UpdateReplacePolicy: Retain` on the replaced resource, CloudFormation does NOT delete the old physical resource in the update's cleanup phase: the only delete-family event for that logical id on the update is DELETE_SKIPPED carrying the OLD physical id, and the old schedule keeps existing afterwards (scheduler:GetSchedule on the old Name still returns it, unchanged, with its own expression/Input/Description).
Emulator: the logical resource's delete-family events for the update are DELETE_IN_PROGRESS (no physical id) followed by DELETE_COMPLETE naming the OLD schedule name — the retained schedule is destroyed and DELETE_SKIPPED is never emitted.
Why: a resource the template explicitly told CloudFormation to keep across a replacement is destroyed by a routine update. Anything parked on the retained resource (here a pending one-time schedule, in general a queue and its messages, a table, a bucket) is silently lost, and an auditor reading the events cannot see that a resource was intentionally kept.
Fix: the cloudformation crate's update cleanup phase — honour UpdateReplacePolicy Retain for the replaced physical resource: leave it in place and record DELETE_SKIPPED for the old physical id instead of deleting it.
Traces: cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found

Not covered by, but related to: 20261110T232651Z-cloudformation-sqs-an-update-that-replaces-a-resource.md reports the same two causes for AWS::SQS::Queue under the service heading `cloudformation__sqs` and does not carry this check's replay key. 20261111T110051Z-cloudformation-scheduler-sqs-a-one-time-eventbridge-scheduler.md is keyed to an EARLIER leg of this same check (the at() delivery instant, run#34) which this replay got past; it does not cover the replacement legs and says so explicitly.

## Evidence
Check cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found — verdict mismatch.

The create leg and the in-place update leg passed (stack settled UPDATE_COMPLETE, physical id unchanged, resource events exactly [UPDATE_IN_PROGRESS, UPDATE_COMPLETE], GetSchedule read back the updated expression and Input, and the delivery arrived at/after the at() instant). The failure is on the third leg, the Name change under `UpdateReplacePolicy: Retain`:

AssertionError: Schedule must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', 'sim2real-cfnsch-b-ef16f7803374'), ('CREATE_COMPLETE', None, 'sim2real-cfnsch-b-ef16f7803374'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, 'sim2real-cfnsch-a-03f4936fc4c4')]

Expected for that update, on logical id `Schedule` (tuples are (ResourceStatus, ResourceStatusReason, PhysicalResourceId)):
  ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <old or new id>) present
  ('UPDATE_COMPLETE', ..., <NEW schedule name>) present
  delete-family events exactly [('DELETE_SKIPPED', <OLD schedule name>)]
Actual: CREATE_IN_PROGRESS / CREATE_IN_PROGRESS('Resource creation Initiated') / CREATE_COMPLETE for the new schedule, then DELETE_IN_PROGRESS / DELETE_COMPLETE naming the old schedule.

`sim2real-cfnsch-a-03f4936fc4c4` and `sim2real-cfnsch-b-ef16f7803374` are this run's generated schedule names (the template's Name property) and are the variable part; the fixed part of the requirement is the UPDATE_* statuses, the quoted replacement reason text, and the single DELETE_SKIPPED on the old physical id.

Model note: StackEvent.ResourceStatus is the ResourceStatus enum and does contain UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED, so the emulator is not limited by the model here — it simply never produces them for a replacement.

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__scheduler__sqs.cfn-scheduler-schedule-in-place-at-update-delivers-retained-replacement-then-not-found.
2. CreateStack with an AWS::SQS::Queue, an IAM role scheduler.amazonaws.com may assume with sqs:SendMessage on it, and an AWS::Scheduler::Schedule that carries `UpdateReplacePolicy: Retain` and an explicit Name (Name is create-only), FlexibleTimeWindow {Mode: OFF}, Target {Arn: <queue arn>, RoleArn: <role arn>, Input: <payload>}. Wait for CREATE_COMPLETE.
3. UpdateStack with the SAME template except a DIFFERENT schedule Name (forcing a replacement). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents and filter to the events of logical id `Schedule` produced by this update. Expect UPDATE_IN_PROGRESS with reason 'Requested update requires the creation of a new physical resource; hence creating one.', then UPDATE_COMPLETE on the NEW schedule name. Observed: CREATE_IN_PROGRESS / CREATE_IN_PROGRESS('Resource creation Initiated') / CREATE_COMPLETE.
5. In the same event slice, look at the delete-family events for that logical id. Expect exactly one: DELETE_SKIPPED naming the OLD schedule name. Observed: DELETE_IN_PROGRESS then DELETE_COMPLETE naming the old name.
6. Call scheduler:GetSchedule(Name=<old name>). Expect it to still exist (retained); observed it is gone.
