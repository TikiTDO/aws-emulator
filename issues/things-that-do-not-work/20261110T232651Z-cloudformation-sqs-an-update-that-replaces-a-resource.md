Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cfn-sqs-queue-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__sqs.cfn-sqs-queue-in-place-update-retained-replacement-then-not-found|_assert_replaced_and_retained#35|AssertionError|assert (
Issue-Commit: cd6d73564e5f4cf53934bca1327f08de31f2ee5f

# cloudformation__sqs: an update that replaces a resource is reported with CREATE_* events instead of the UPDATE_* replacement vocabulary, and UpdateReplacePolicy Retain still deletes the replaced resource (2 causes)

> _Note: this issue was identified against the code base at commit `cd6d73564e5f4cf53934bca1327f08de31f2ee5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes of service `cloudformation__sqs`, both grounded in the same failing behavioral check. A fixer must close both before submitting.

Cause 1 of 2 — a replacement is announced with CREATE_* resource events instead of AWS's UPDATE_* replacement events.
AWS: recorded behavior only (stack-event vocabulary). When an UpdateStack must replace a resource, the resource's events on that update are, in order, `UPDATE_IN_PROGRESS` with no reason, `UPDATE_IN_PROGRESS` with the fixed reason `Requested update requires the creation of a new physical resource; hence creating one.`, `UPDATE_IN_PROGRESS` with reason `Resource creation Initiated`, and `UPDATE_COMPLETE` carrying the NEW physical id. The reasons are fixed AWS text; the physical ids are the values the run produced.
Emulator: the replaced AWS::SQS::Queue's events for that update are `CREATE_IN_PROGRESS` (no reason), `CREATE_IN_PROGRESS` (`Resource creation Initiated`), `CREATE_COMPLETE` — the replacement announcement (`Requested update requires the creation of a new physical resource; hence creating one.`) never appears and no UPDATE_* status is emitted for the logical resource.
Why: a client that watches stack events to detect that an update replaced a resource (every drift/replacement detector, and anything that distinguishes a create from a replacement) sees a fresh create inside an update and never sees the replacement announcement.

Cause 2 of 2 — `UpdateReplacePolicy: Retain` is ignored: the replaced physical resource is deleted.
AWS: recorded behavior only. With `UpdateReplacePolicy: Retain` on the replaced resource, CloudFormation does NOT delete the old physical resource during the update's cleanup phase: the only delete-family event for that logical id is `DELETE_SKIPPED` carrying the OLD physical id, and the old queue keeps existing (and keeps its messages) after the stack update.
Emulator: the logical resource's delete-family events are `DELETE_IN_PROGRESS` followed by `DELETE_COMPLETE` naming the OLD queue url — the replaced queue is actually deleted, and no DELETE_SKIPPED event is ever emitted.
Why: data a stack was told to retain is destroyed by a routine update — the retained resource and any messages on it are lost, and a caller auditing the events cannot see that a resource was intentionally kept.

Fix: the cloudformation crate's resource-provisioner update/replacement path — the replacement must run as an update of the logical resource (emitting the UPDATE_* events above) and its cleanup phase must honour UpdateReplacePolicy Retain by emitting DELETE_SKIPPED for the old physical id instead of deleting it.
Traces (both causes): cloudformation__sqs.cfn-sqs-queue-in-place-update-retained-replacement-then-not-found

## Evidence
Check cloudformation__sqs.cfn-sqs-queue-in-place-update-retained-replacement-then-not-found
AssertionError: Queue must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new queue url>'), ('CREATE_COMPLETE', None, '<new queue url>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old queue url>')]
The check asserts, for the replacing update: ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.') present; ('UPDATE_COMPLETE', <new physical id>) present; and the delete-family events exactly [('DELETE_SKIPPED', <old physical id>)].
Actual: CREATE_* statuses for the new resource, and DELETE_IN_PROGRESS/DELETE_COMPLETE on the old physical id despite UpdateReplacePolicy Retain. (The queue urls are per-run values; the required events and reasons are the fixed part.)

## Steps to Reproduce
1. Boot the emulator and replay cloudformation__sqs.cfn-sqs-queue-in-place-update-retained-replacement-then-not-found.
2. CreateStack with an AWS::SQS::Queue that has an explicit QueueName and UpdateReplacePolicy: Retain; wait for CREATE_COMPLETE and note the queue url.
3. UpdateStack changing the QueueName (a create-only property), forcing a replacement.
4. DescribeStackEvents for that logical id on this update: expect UPDATE_IN_PROGRESS with 'Requested update requires the creation of a new physical resource; hence creating one.', UPDATE_COMPLETE on the new queue url, and exactly one delete-family event DELETE_SKIPPED naming the old queue url.
5. GetQueueAttributes on the old queue url: it must still exist.
