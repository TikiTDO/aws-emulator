Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue.cfn-glue-database-in-place-update-retained-replacement-keeps-tables-then-not-found
Issue-Key: cloudformation__glue.cfn-glue-database-in-place-update-retained-replacement-keeps-tables-then-not-found|_assert_replaced_and_retained#30|AssertionError|assert (
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudformation__glue: a replaced AWS::Glue::Database is reported with CREATE_*/DELETE_* events and the retained old database (with its tables) is deleted despite UpdateReplacePolicy Retain (2 causes)

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Two net-new root causes, both observed on one AWS::Glue::Database whose create-only DatabaseName changes during an update while the resource declares "UpdateReplacePolicy": "Retain". The failing check is marked neither ALREADY FILED nor OWNED by any open issue.

Cause 1 of 2: the resource events of a replacement are reported as a fresh CREATE, not as an UPDATE, and the replacement reason is missing.
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvent{ResourceStatus, ResourceStatusReason, PhysicalResourceId}; ResourceStatus is a modeled enum that includes UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED. An update that changes a create-only property replaces the resource INSIDE the update: AWS emits UPDATE_IN_PROGRESS whose ResourceStatusReason is the fixed text 'Requested update requires the creation of a new physical resource; hence creating one.', then UPDATE_IN_PROGRESS 'Resource creation Initiated', then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (for AWS::Glue::Database the database name, a request value).
Emulator: for that logical id the update emits CREATE_IN_PROGRESS (reason absent, PhysicalResourceId empty string), CREATE_IN_PROGRESS 'Resource creation Initiated', CREATE_COMPLETE — no UPDATE_* event at all and no replacement reason.
Why: a client reading DescribeStackEvents cannot distinguish a replacement from a first-time create, and never sees the reason that announces a catalog database (and everything hanging off it) is being re-created; change-review tooling that greps for the replacement reason sees none.
Fix: the CloudFormation update engine's resource-event emission on the replacement path (not the Glue provisioner).
Traces: cloudformation__glue.cfn-glue-database-in-place-update-retained-replacement-keeps-tables-then-not-found.

Cause 2 of 2: UpdateReplacePolicy Retain is ignored — the replaced database is deleted instead of skipped.
AWS: with UpdateReplacePolicy Retain the old physical resource is NOT deleted in the replacement's cleanup phase. CloudFormation emits exactly one DELETE_SKIPPED (a modeled ResourceStatus value) carrying the OLD PhysicalResourceId and no DELETE_IN_PROGRESS/DELETE_COMPLETE for that logical id; the old Glue database and the tables inside it survive the update and the later DeleteStack, so glue GetDatabase/GetTables on the old name keep answering while GetDatabase on the stack-owned name refuses with EntityNotFoundException and the message 'Database {name} not found.' ({name} is the requested database name; the rest is fixed text).
Emulator: DELETE_IN_PROGRESS (PhysicalResourceId absent) then DELETE_COMPLETE carrying the OLD database name — no DELETE_SKIPPED — and the retained database together with its table is destroyed.
Why: silent metadata loss on a documented retention guarantee. Retaining a catalog database across a replacement is exactly why operators set UpdateReplacePolicy Retain, and because the event stream reports a successful delete rather than a skip, the loss is invisible in the stack's own record.
Fix: the CloudFormation update engine's cleanup phase — honour UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain.
Traces: cloudformation__glue.cfn-glue-database-in-place-update-retained-replacement-keeps-tables-then-not-found (the check fails at Cause 1; the event rows it prints also prove Cause 2).

The same engine-level behaviour is already recorded for other resource types in the open issues 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md, 20261111T003921Z-cloudformation-logs-a-replaced-aws-logs-loggroup-is.md and 20261110T232651Z-cloudformation-sqs-an-update-that-replaces-a-resource.md; none of their replay sets names this check, so this recorded unit is not re-replayed by any of them.

## Evidence
Replay output (behavioral suite, section ### cloudformation__glue):
  [mismatch] cloudformation__glue.cfn-glue-database-in-place-update-retained-replacement-keeps-tables-then-not-found
  AssertionError: Database must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new db>'), ('CREATE_COMPLETE', None, '<new db>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old db>')]

Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced logical id 'Database', oldest first, taken from DescribeStackEvents; the database names are per-run request values.

Expected (AWS), in order, for that logical id:
  ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>)
  ('UPDATE_COMPLETE', None, '<new db>')
  ('DELETE_SKIPPED', None, '<old db>')
and NO DELETE_IN_PROGRESS / DELETE_COMPLETE event for it; afterwards glue GetDatabase('<old db>') and GetTables('<old db>') still answer with the database and its 'probe' table.

Actual (emulator): the CREATE_* triple above with no reason and an empty PhysicalResourceId on the first row, then DELETE_IN_PROGRESS and DELETE_COMPLETE on the old database, which is gone along with its table.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and glue both serve).
2. CreateStack with one AWS::Glue::Database (CatalogId = AWS::AccountId, DatabaseInput with a pinned Name, a Description and Parameters) declaring "UpdateReplacePolicy": "Retain". Wait for CREATE_COMPLETE, then glue CreateTable a table in that database.
3. UpdateStack with the same template but a different DatabaseName (create-only, so the update must replace). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents for the 'Database' logical id: expect UPDATE_IN_PROGRESS with ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.' followed by UPDATE_COMPLETE carrying the new database name; observe CREATE_IN_PROGRESS/CREATE_COMPLETE with no reason instead.
5. Expect exactly one DELETE_SKIPPED carrying the OLD database name and no DELETE_* progress events; observe DELETE_IN_PROGRESS + DELETE_COMPLETE.
6. glue GetDatabase / GetTables on the OLD name: expect the database and its table to survive (both before and after DeleteStack); observe EntityNotFoundException because the retained database was deleted.
7. Equivalently: re-replay cloudformation__glue.cfn-glue-database-in-place-update-retained-replacement-keeps-tables-then-not-found.
