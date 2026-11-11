Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__secretsmanager.cfn-secretsmanager-secret-in-place-update-retained-replacement-then-not-found
Issue-Key: cloudformation__secretsmanager.cfn-secretsmanager-secret-in-place-update-retained-replacement-then-not-found|_assert_replaced_and_retained#28|AssertionError|assert (
Issue-Commit: add629af7d6117f223747ffc9fa4e155dbf41c42

# cloudformation__secretsmanager: a replacement update of AWS::SecretsManager::Secret is reported with CREATE_*/DELETE_* events and deletes the old secret despite UpdateReplacePolicy Retain (2 causes)

> _Note: this issue was identified against the code base at commit `add629af7d6117f223747ffc9fa4e155dbf41c42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
UpdateStack on a stack whose AWS::SecretsManager::Secret changes its create-only `Name` property must replace the secret. The emulator gets both halves of that replacement wrong: the resource events use the create/delete vocabulary instead of CloudFormation's replacement vocabulary, and the `UpdateReplacePolicy: Retain` on the resource is ignored so the replaced secret is destroyed. Two net-new root causes, both observed in the same recorded event list.

Note: three open issues describe the same replacement defect for OTHER resource types and service groups — 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md, 20261110T232651Z-cloudformation-sqs-an-update-that-replaces-a-resource.md, and 20261107T060626Z-s3-cloudformation-updatereplacepolicy-retain-is-ignored-when-an.md. None of them carries this check's replay key and none covers the AWS::SecretsManager::Secret handler, so this is filed separately; if the fix is made in the shared update engine these may close together.

Cause 1 of 2: a replacement update emits CREATE_*/DELETE_* resource events instead of the UPDATE_* replacement vocabulary.
  AWS:      When an update requires replacing a resource, CloudFormation records the replacement against the SAME logical resource id using the update vocabulary: an `UPDATE_IN_PROGRESS` event whose `ResourceStatusReason` is the fixed sentence `Requested update requires the creation of a new physical resource; hence creating one.`, then an `UPDATE_COMPLETE` event whose `PhysicalResourceId` is the NEW physical id, and finally a cleanup event for the OLD physical id. Smithy: cloudformation#ResourceStatus is an enum whose values include `UPDATE_IN_PROGRESS`, `UPDATE_COMPLETE`, `DELETE_SKIPPED`, `DELETE_IN_PROGRESS` and `DELETE_COMPLETE`; cloudformation#StackEvent carries `LogicalResourceId`, `PhysicalResourceId`, `ResourceStatus` and `ResourceStatusReason`, so the reason sentence is a modeled, client-visible field. The fixed text of that reason is the whole sentence above — it has no variable parts; the variable parts of the surrounding events are only `PhysicalResourceId` (the old and new secret ARNs minted by the emulator) and `LogicalResourceId` (`Secret`, from the template).
  Emulator: the replacement is reported as a fresh create followed by a delete. For logical resource `Secret` the update emitted, in order, `CREATE_IN_PROGRESS` (no reason, empty physical id), `CREATE_IN_PROGRESS` with reason `Resource creation Initiated` on the new secret ARN, `CREATE_COMPLETE` on the new secret ARN, then `DELETE_IN_PROGRESS` and `DELETE_COMPLETE`. No `UPDATE_IN_PROGRESS` event and no `Requested update requires the creation of a new physical resource; hence creating one.` reason is ever recorded, and there is no `UPDATE_COMPLETE` naming the new physical id.
  Why:      a client cannot tell a replacement from a brand-new resource. Deploy tooling and operators that scan DescribeStackEvents for the replacement reason (to warn that a resource is about to be recreated, or to correlate an update to its new physical id) see a create of a resource that was never newly added, and see an `UPDATE_COMPLETE` for the logical id nowhere, so change auditing and replacement detection both fail against the emulator while they work against AWS.
  Fix:      the stack-update engine's replacement path and the event writer it feeds — the component that decides which ResourceStatus/ResourceStatusReason pair to record when a create-only property changes, shared by the AWS::SecretsManager::Secret resource handler.
  Traces:   cloudformation__secretsmanager.cfn-secretsmanager-secret-in-place-update-retained-replacement-then-not-found

Cause 2 of 2: `UpdateReplacePolicy: Retain` is ignored — the replaced secret is deleted instead of retained and reported DELETE_SKIPPED.
  AWS:      a resource declaring `UpdateReplacePolicy: Retain` is kept when an update replaces it. CloudFormation does not delete the old physical resource; it records a `DELETE_SKIPPED` event for the OLD physical id, and the old resource stays fully usable outside the stack. Smithy: cloudformation#ResourceStatus includes `DELETE_SKIPPED` precisely for this outcome. For an AWS::SecretsManager::Secret that means the old secret remains readable: DescribeSecret and GetSecretValue on the old name/ARN keep answering 200 with no `DeletedDate`, and the stack's remaining lifetime does not affect it.
  Emulator: the old secret is destroyed by the replacement. The only delete-family events for logical resource `Secret` are `DELETE_IN_PROGRESS` (no physical id) and `DELETE_COMPLETE` carrying the OLD secret ARN; no `DELETE_SKIPPED` event is emitted for it. The retained secret therefore does not survive the update, so the subsequent reads of the old name cannot succeed.
  Why:      this is silent data loss. UpdateReplacePolicy Retain is the documented way to preserve credential material across a replacement — a template that renames a secret deliberately keeps the old secret so consumers still holding the old ARN keep working and the operator can roll back. Against the emulator the old secret and every version of its material are deleted, and the event log claims a completed delete rather than the skipped delete AWS reports, so a client has no signal that its retention request was dropped.
  Fix:      the stack-update engine's resource-disposition step — the code that reads DeletionPolicy/UpdateReplacePolicy for the replaced resource and chooses between deleting the old physical resource and skipping it, plus the state it drives in the AWS::SecretsManager::Secret handler (the old secret must be released from stack ownership, not deleted) and the DELETE_SKIPPED event it must record.
  Traces:   cloudformation__secretsmanager.cfn-secretsmanager-secret-in-place-update-retained-replacement-then-not-found

## Evidence
Failing check: [mismatch] cloudformation__secretsmanager.cfn-secretsmanager-secret-in-place-update-retained-replacement-then-not-found

Assertion that fired (step 3 of the check, the replacement under UpdateReplacePolicy Retain):

  AssertionError: Secret must announce its replacement, its events were
  [('CREATE_IN_PROGRESS', None, ''),
   ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new-secret-arn>'),
   ('CREATE_COMPLETE', None, '<new-secret-arn>'),
   ('DELETE_IN_PROGRESS', None, None),
   ('DELETE_COMPLETE', None, '<old-secret-arn>')]

(rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for LogicalResourceId 'Secret', oldest first, restricted to the events of the replacing UpdateStack; the two ARNs are the emulator-minted physical ids of the before/after secrets and are per-run values, not requirements.)

Expected, per AWS:
  - ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <old-or-new physical id>) present in the event rows
  - ('UPDATE_COMPLETE', <new physical id>) present in the event rows
  - the ONLY DELETE_* row for the logical resource is ('DELETE_SKIPPED', <old physical id>)

Actual, per the emulator:
  - no UPDATE_IN_PROGRESS row at all, so the replacement reason sentence never appears
  - no UPDATE_COMPLETE row naming the new physical id
  - the DELETE_* rows are ('DELETE_IN_PROGRESS', None) and ('DELETE_COMPLETE', <old physical id>) — the retained secret was deleted, and DELETE_SKIPPED was never emitted

The stack itself did reach UPDATE_COMPLETE and did mint a new physical id distinct from the old one, so replacement was performed; only its event vocabulary and the Retain disposition diverge.

Model evidence: smithy_query(shape='ResourceStatus') on services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json lists DELETE_SKIPPED, UPDATE_IN_PROGRESS and UPDATE_COMPLETE among ResourceStatus values; smithy_query(shape='StackEvent') shows ResourceStatus and ResourceStatusReason as members of every event, i.e. both diverging fields are modeled and client-visible.

## Steps to Reproduce
1. CreateStack with a template containing one resource:
     Secret:
       Type: AWS::SecretsManager::Secret
       UpdateReplacePolicy: Retain
       Properties: { Name: <first-name>, Description: first, SecretString: <v1> }
   and Outputs.SecretArn = { Ref: Secret }. Wait for CREATE_COMPLETE.
2. Record the current event ids via DescribeStackEvents, then UpdateStack with the SAME Name but Description 'second' and SecretString <v2>. Wait for UPDATE_COMPLETE (this in-place update behaves correctly).
3. Record the current event ids again, then UpdateStack changing ONLY Properties.Name to <second-name> (Name is create-only, so this forces a replacement). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents (paginated) and keep the rows for LogicalResourceId 'Secret' whose EventId was not seen in step 3.
   Cause 1: there is no row with ResourceStatus UPDATE_IN_PROGRESS and ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.', and no UPDATE_COMPLETE row carrying the new physical id; the emulator emits CREATE_IN_PROGRESS / CREATE_IN_PROGRESS 'Resource creation Initiated' / CREATE_COMPLETE instead.
   Cause 2: the DELETE_* rows are DELETE_IN_PROGRESS and DELETE_COMPLETE on the OLD secret ARN rather than the single DELETE_SKIPPED row on the OLD secret ARN that UpdateReplacePolicy Retain requires.
5. Confirm the data loss of cause 2 directly: call DescribeSecret and GetSecretValue with SecretId=<first-name>. AWS serves the retained secret (200, Description 'second', SecretString <v2>, VersionStages ['AWSCURRENT'], no DeletedDate); the emulator has deleted it.
