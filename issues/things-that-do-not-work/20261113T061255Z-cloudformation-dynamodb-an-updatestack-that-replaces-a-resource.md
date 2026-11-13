Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found|_assert_replaced_and_retained#31|AssertionError|assert (
Issue-Commit: bd012b1aa51096c7ce0d935c21037429ca5dabd2

# cloudformation__dynamodb: an UpdateStack that replaces a resource emits no UPDATE_* events for it and ignores UpdateReplacePolicy Retain, deleting the replaced table (2 causes)

> _Note: this issue was identified against the code base at commit `bd012b1aa51096c7ce0d935c21037429ca5dabd2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes in CloudFormation's resource-replacement path, both observed on one UpdateStack that changes the create-only property AWS::DynamoDB::Table.TableName on a resource declared with `UpdateReplacePolicy: Retain`. Both are grounded in check cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found.

Cause 1 of 2: the replaced logical resource gets no UPDATE_IN_PROGRESS replacement announcement and no terminal UPDATE_COMPLETE event.
AWS: when an update changes a create-only property, CloudFormation replaces the resource and the logical resource's events for that update are, oldest first: UPDATE_IN_PROGRESS with ResourceStatusReason exactly `Requested update requires the creation of a new physical resource; hence creating one.`, then CREATE_IN_PROGRESS / CREATE_COMPLETE carrying the NEW physical id, then the disposal event for the old physical id, then UPDATE_COMPLETE carrying the NEW physical id. Smithy (services/service-com-amazonaws-cloudformation-cloudformation): ResourceStatus enumerates UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED, and StackEvent carries LogicalResourceId, PhysicalResourceId, ResourceStatus and ResourceStatusReason; the reason prose is not in the model, so the fixed reason text above is recorded behavior only (it is fixed text — it names no resource, id or value from the request). The stack itself reached UPDATE_COMPLETE and DescribeStackResources already reports the new physical id, so only the event stream diverges.
Emulator: for logical id `Table`, the update's events were [(CREATE_IN_PROGRESS, None, ''), (CREATE_IN_PROGRESS, 'Resource creation Initiated', <new table name>), (CREATE_COMPLETE, None, <new table name>), (DELETE_IN_PROGRESS, None, None), (DELETE_COMPLETE, None, <old table name>)] — i.e. it records a bare create+delete pair with no UPDATE_IN_PROGRESS, no replacement reason and no UPDATE_COMPLETE for the logical resource.
Why: a client that reads DescribeStackEvents to learn whether an update replaced a resource — a deploy pipeline gating on replacement, a drift/audit tool, or `cdk deploy`/`sam deploy` progress rendering — sees no replacement announcement and no UPDATE_COMPLETE, so it cannot distinguish a replacement from an unrelated create and cannot tell when the logical resource's update finished.
Fix: the CloudFormation stack-update engine's replacement path and its stack-event recorder (the component that appends StackEvents for a resource whose update requires replacement).

Cause 2 of 2: `UpdateReplacePolicy: Retain` is ignored — the replaced physical resource is deleted instead of skipped.
AWS: with UpdateReplacePolicy Retain, the replaced resource is kept; the only DELETE_* event the logical resource gets for that update is DELETE_SKIPPED carrying the OLD physical id (Smithy: ResourceStatus includes DELETE_SKIPPED), and the old DynamoDB table survives the update and the subsequent stack delete — DescribeTable on the old name keeps answering an ACTIVE table with its items, while DescribeTable on the stack's current table after DeleteStack is refused with ResourceNotFoundException and the message `Requested resource not found: Table: <table name> not found` (fixed text `Requested resource not found: Table: ` + the requested table name from the request + ` not found`).
Emulator: the logical resource's DELETE_* events for the update were DELETE_IN_PROGRESS followed by DELETE_COMPLETE on the old physical id, so the retained table is destroyed rather than adopted out of the stack; no DELETE_SKIPPED event is ever emitted.
Why: this is silent data loss. A template author relies on UpdateReplacePolicy Retain precisely so that a replacement preserves the old table and its items for migration or rollback; against the emulator the old table and every item in it are deleted, and a client that then reads the retained table gets ResourceNotFoundException where AWS serves the data.
Fix: the CloudFormation resource-disposition logic for a replaced resource — it must consult the resource's UpdateReplacePolicy (Retain / Snapshot / Delete) before disposing of the old physical resource, retain it for Retain, and record DELETE_SKIPPED with the old physical id instead of a delete pair.

Traces: cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found (step 3 of the check: UpdateStack changing TableName under UpdateReplacePolicy Retain). Table names, stack names and ids in the recorded event list are per-run values, not requirements.

## Evidence
Check cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found:
  AssertionError: Table must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', 'sim2real-cfntbl-b-4f7e81a558ac'), ('CREATE_COMPLETE', None, 'sim2real-cfntbl-b-4f7e81a558ac'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, 'sim2real-cfntbl-a-d039d68d016a')]
  (tuples are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for LogicalResourceId 'Table', restricted to the events of the replacing UpdateStack; 'sim2real-cfntbl-b-...' is the new table name and 'sim2real-cfntbl-a-...' the replaced one — both per-run values.)

Cause 1 expected vs actual:
  expected to contain ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.') and ('UPDATE_COMPLETE', <new physical id>)
  actual: neither UPDATE_IN_PROGRESS nor UPDATE_COMPLETE appears for the logical resource.

Cause 2 expected vs actual:
  expected DELETE_* events for the logical resource == [('DELETE_SKIPPED', <old physical id>)]
  actual: [('DELETE_IN_PROGRESS', None), ('DELETE_COMPLETE', <old physical id>)] — the replaced table is deleted although the template declares UpdateReplacePolicy: Retain.

The prior steps of the check passed: the create reached CREATE_COMPLETE, the physical id equalled the TableName, Fn::GetAtt Table.Arn matched, the item round-tripped, and the in-place ReadCapacityUnits update emitted exactly UPDATE_IN_PROGRESS + UPDATE_COMPLETE on the same physical id — so only the REPLACEMENT path diverges.

## Steps to Reproduce
1. CreateStack with one AWS::DynamoDB::Table carrying `UpdateReplacePolicy: Retain`, TableName=<name-a>, pk (HASH, S), BillingMode PROVISIONED, ReadCapacityUnits 1 / WriteCapacityUnits 1, and Output TableArn = Fn::GetAtt [Table, Arn]; wait for CREATE_COMPLETE and PutItem one item.
2. UpdateStack with the same template but ReadCapacityUnits 2 — this updates in place and is already correct.
3. UpdateStack with TableName changed to <name-b> (a create-only property, so the resource must be replaced); wait for UPDATE_COMPLETE.
4. DescribeStackEvents for the stack and keep the events of LogicalResourceId 'Table' added by step 3.
   Cause 1: no ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.') event and no ('UPDATE_COMPLETE', <name-b>) event is present — only CREATE_* on <name-b> and DELETE_* on <name-a>.
   Cause 2: the DELETE_* events are DELETE_IN_PROGRESS + DELETE_COMPLETE on <name-a> instead of the single DELETE_SKIPPED on <name-a> that UpdateReplacePolicy Retain requires; DescribeTable on <name-a> and GetItem of the item written in step 1 then fail where AWS still serves them.
5. Re-replay check cloudformation__dynamodb.cfn-dynamodb-table-in-place-update-retained-replacement-then-not-found to confirm the fix.
