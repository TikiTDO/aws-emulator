Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__pipes__sqs.cfn-pipes-pipe-in-place-template-update-retained-replacement-both-deliver-then-not-found
Issue-Key: cloudformation__pipes__sqs.cfn-pipes-pipe-in-place-template-update-retained-replacement-both-deliver-then-not-found|_assert_replaced_and_retained#33|AssertionError|assert (
Issue-Commit: 56af4a38801370870cefc18c3ef22e7084182017

# cloudformation__pipes__sqs: a stack update that replaces a resource reports CREATE_*/DELETE_* events instead of UPDATE_IN_PROGRESS "Requested update requires the creation of a new physical resource"/UPDATE_COMPLETE, and ignores UpdateReplacePolicy: Retain by deleting the old physical resource

> _Note: this issue was identified against the code base at commit `56af4a38801370870cefc18c3ef22e7084182017`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause, grounded in a failing behavioral check the replay report left unmarked (no ALREADY-FILED marker, no OWNED marker).

Cause 1 of 1 — CloudFormation's replacement path reports a replacement as an unrelated create plus a delete, and the delete it performs ignores the resource's UpdateReplacePolicy: Retain.

AWS: when an UpdateStack changes a create-only property (here AWS::Pipes::Pipe.Name and .Source), CloudFormation replaces the resource and narrates it on the LOGICAL resource, never as a fresh create. The event sequence for that logical id is: `UPDATE_IN_PROGRESS` with ResourceStatusReason exactly "Requested update requires the creation of a new physical resource; hence creating one."; then `UPDATE_IN_PROGRESS` with reason "Resource creation Initiated" carrying the NEW physical id; then `UPDATE_COMPLETE` carrying the NEW physical id. The old physical resource is dealt with in the cleanup phase, and because `UpdateReplacePolicy: Retain` is declared, that phase emits `DELETE_SKIPPED` naming the OLD physical id and the old resource is left live — AWS does not delete it, which is the entire point of the policy (see the UpdateReplacePolicy attribute reference). Only the DELETE_SKIPPED event may appear among the resource's DELETE_* events.

Emulator: the logical resource's events for that update are, in order, `('CREATE_IN_PROGRESS', None)`, `('CREATE_IN_PROGRESS', 'Resource creation Initiated')` with the new physical id, `('CREATE_COMPLETE', ...)` with the new physical id, then `('DELETE_IN_PROGRESS', None)` and `('DELETE_COMPLETE', ...)` naming the OLD physical id. So: no UPDATE_IN_PROGRESS, no "Requested update requires the creation of a new physical resource; hence creating one." reason, no UPDATE_COMPLETE, and `UpdateReplacePolicy: Retain` is not honoured — the replaced pipe is DELETED rather than DELETE_SKIPPED.

Why: two client-visible consequences. (1) Stack-event consumers cannot tell a replacement from a brand-new resource: anything that watches DescribeStackEvents for UPDATE_COMPLETE on a logical id (deployment gates, change auditors, drift tooling, operators reading the console) sees a CREATE_COMPLETE and an unexplained DELETE and loses the "this was a replacement" signal and its documented reason string. (2) The retention guarantee is broken, which destroys data: UpdateReplacePolicy: Retain exists so the replaced resource outlives the stack's reference to it, and a client that relies on it to keep draining the old source, to keep the old queue/table/pipe's contents, or to hand the old physical resource to another owner finds it deleted. The emulator also emits DELETE_IN_PROGRESS/DELETE_COMPLETE inside the resource's own update events, where AWS puts only the cleanup-phase DELETE_SKIPPED.

Fix: the CloudFormation update executor's replacement path in the cloudformation service crate — a replacement must be recorded against the logical resource as UPDATE_IN_PROGRESS (with AWS's fixed reason "Requested update requires the creation of a new physical resource; hence creating one."), UPDATE_IN_PROGRESS "Resource creation Initiated" and UPDATE_COMPLETE on the new physical id, and the cleanup of the old physical resource must consult the resource's UpdateReplacePolicy: Retain (and Snapshot) and emit DELETE_SKIPPED without calling the provider's delete.

Traces: cloudformation__pipes__sqs.cfn-pipes-pipe-in-place-template-update-retained-replacement-both-deliver-then-not-found. The legs after the event assertion — GetAtt Pipe.Arn on the new pipe, the new pipe delivering with its new InputTemplate, and the RETAINED old pipe still delivering from the old source — were not reached and are not claimed here; they must be re-asserted once Retain is honoured. Related but NOT covering it: 20261110T232651Z-cloudformation-sqs-an-update-that-replaces-a-resource.md concerns an AWS::SQS::Queue replacement and does not carry this check's replay key.

## Evidence
cloudformation__pipes__sqs.cfn-pipes-pipe-in-place-template-update-retained-replacement-both-deliver-then-not-found — verdict mismatch.

Legs 1 and 2 passed: the stack (two source queues SourceA/SourceB, a target queue, a role, and an AWS::Pipes::Pipe with `UpdateReplacePolicy: Retain`, SourceParameters.SqsQueueParameters.BatchSize 1, DesiredState RUNNING) created, and an InputTemplate-only update was correctly applied in place — the pipe kept its physical id and its events were exactly [UPDATE_IN_PROGRESS, UPDATE_COMPLETE].

Leg 3 diverges. The template is updated to a new pipe Name and Source (both create-only), so the pipe must be replaced while the old one is retained.
  Expected, among the logical resource `Pipe`'s new events: ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.'); ('UPDATE_COMPLETE', <new physical id>); and exactly one DELETE_* event, ('DELETE_SKIPPED', <old physical id>).
  Actual (AssertionError from the check's _assert_replaced_and_retained):
    Pipe must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new pipe name>'), ('CREATE_COMPLETE', None, '<new pipe name>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old pipe name>')]

The fixed text at issue is the ResourceStatusReason "Requested update requires the creation of a new physical resource; hence creating one.", the statuses UPDATE_IN_PROGRESS/UPDATE_COMPLETE/DELETE_SKIPPED, and the pairing of DELETE_SKIPPED with the OLD physical id; the pipe names above are this run's generated ids and are not part of the requirement. The DELETE_COMPLETE on the old physical id is the second, independent symptom: the retained resource was destroyed.

Model note: the Smithy model for pipes constrains the pipe's own API, not stack-event narration or UpdateReplacePolicy, so the expectation above is AWS's documented CloudFormation behaviour plus the recorded sequence the check encodes (reference: the UpdateReplacePolicy attribute and "Update behaviors of stack resources" in the CloudFormation User Guide).

## Steps to Reproduce
1. cloudformation:CreateStack with a template containing two AWS::SQS::Queue sources (SourceA, SourceB), a target AWS::SQS::Queue, a role Pipes can assume, and an `AWS::Pipes::Pipe` carrying `UpdateReplacePolicy: Retain` with Properties {Name: <name-a>, RoleArn: !GetAtt Role.Arn, Source: !GetAtt SourceA.Arn, SourceParameters: {SqsQueueParameters: {BatchSize: 1}}, Target: !GetAtt Target.Arn, TargetParameters: {InputTemplate: <t1>}, DesiredState: RUNNING}. Wait for CREATE_COMPLETE.
2. Record the current DescribeStackEvents EventIds.
3. cloudformation:UpdateStack with the same template but Name: <name-b> and Source: !GetAtt SourceB.Arn (both create-only, so a replacement). Wait for UPDATE_COMPLETE.
4. Read the stack events newer than step 2 for logical id `Pipe`. Expected ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.'), then ('UPDATE_IN_PROGRESS', 'Resource creation Initiated', <name-b>), then ('UPDATE_COMPLETE', <name-b>), and exactly one DELETE_* event: ('DELETE_SKIPPED', <name-a>). Observed CREATE_IN_PROGRESS/CREATE_IN_PROGRESS/CREATE_COMPLETE followed by DELETE_IN_PROGRESS/DELETE_COMPLETE on <name-a>.
5. Confirm the retention guarantee: pipes:DescribePipe(Name=<name-a>) must still succeed after the update and the old pipe must still forward a message sent to SourceA to the target. Observed: it was deleted.
