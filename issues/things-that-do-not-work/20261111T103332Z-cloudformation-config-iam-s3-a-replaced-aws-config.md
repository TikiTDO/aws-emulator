Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__iam__s3.cfn-config-rule-create-update-in-place-replace-retained-then-delete
Issue-Key: cloudformation__config__iam__s3.cfn-config-rule-create-update-in-place-replace-retained-then-delete|_assert_replaced_and_retained#28|AssertionError|assert _in_order(
Issue-Commit: 94350cf775acbc496950503935930c48964de18f

# cloudformation__config__iam__s3: a replaced AWS::Config::ConfigRule is reported with CREATE_*/DELETE_* events and the retained old rule is deleted despite UpdateReplacePolicy Retain

> _Note: this issue was identified against the code base at commit `94350cf775acbc496950503935930c48964de18f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: an update that must REPLACE a resource is reported with the create/delete vocabulary instead of the replacement vocabulary, and UpdateReplacePolicy: Retain is ignored so the old physical resource is deleted.
AWS: when UpdateStack changes a property that forces replacement of a resource (here the ConfigRuleName of an AWS::Config::ConfigRule), CloudFormation reports, for that logical id, in order: UPDATE_IN_PROGRESS with ResourceStatusReason "Requested update requires the creation of a new physical resource; hence creating one." (the replacement marker), then the new physical resource being created, then UPDATE_COMPLETE carrying the NEW PhysicalResourceId, and finally — because the resource carries UpdateReplacePolicy: Retain — DELETE_SKIPPED carrying the OLD PhysicalResourceId in the cleanup phase. No DELETE_IN_PROGRESS or DELETE_COMPLETE event is emitted for that logical id, and the old rule survives in Config and is still describable after the update. (Recorded behavior; the fixed parts are the status values and the replacement reason text, the variable parts are the logical id and the two physical ids, taken from the template and the stack's state.)
Emulator: the stack update succeeds but the event stream for the logical id is `CREATE_IN_PROGRESS` (no reason), `CREATE_IN_PROGRESS` "Resource creation Initiated" with the new physical id, `CREATE_COMPLETE` with the new physical id, then `DELETE_IN_PROGRESS` and `DELETE_COMPLETE` with the OLD physical id. There is no UPDATE_IN_PROGRESS replacement event, no UPDATE_COMPLETE on the new id, no DELETE_SKIPPED, and the retained old rule is actually deleted.
Why: a client that watches stack events to learn whether an update replaced a resource cannot tell a replacement from a fresh create, and the deliberate data-retention contract is broken — a resource the template told CloudFormation to keep is destroyed, which is unrecoverable for the caller. Drift/ change-set tooling that keys on UPDATE_* + DELETE_SKIPPED sees nothing it recognises.
Fix: the CloudFormation stack-update engine's replacement path — the resource-event vocabulary it emits for a replacement (UPDATE_IN_PROGRESS with the replacement reason, UPDATE_COMPLETE on the new physical id) and its honouring of UpdateReplacePolicy Retain in the cleanup phase (emit DELETE_SKIPPED for the old physical id and do not call the provider's delete).
Related open issues report the identical engine fault from other service sections — 20261110T232635Z-cloudformation-ssm-a-replacement-update-reports-create-delete-resource-events.md, 20261111T003921Z-cloudformation-iam-a-replaced-aws-iam-managedpolicy-is.md, 20261111T001055Z-cloudformation-secretsmanager-a-replacement-update-of-aws-secretsmanager.md — none of them carries this check id, so this check is not covered by any of them.
Traces: cloudformation__config__iam__s3.cfn-config-rule-create-update-in-place-replace-retained-then-delete.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1), `### cloudformation__config__iam__s3` section (0/1 passed).

[mismatch] cloudformation__config__iam__s3.cfn-config-rule-create-update-in-place-replace-retained-then-delete
  AssertionError: the replacement must report creating a new resource, complete as 'sim2real-cfnreal-rule-b-<id>' and skip deleting 'sim2real-cfnreal-rule-a-<id>' (UpdateReplacePolicy Retain), in that order:
  [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', 'sim2real-cfnreal-rule-b-<id>'), ('CREATE_COMPLETE', None, 'sim2real-cfnreal-rule-b-<id>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, 'sim2real-cfnreal-rule-a-<id>')]

The check's assertion helper requires, in order, (ResourceStatus, ResourceStatusReason, PhysicalResourceId) rows: ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', any), ('UPDATE_COMPLETE', any, <new physical id>), ('DELETE_SKIPPED', any, <old physical id>), and additionally asserts that no DELETE_IN_PROGRESS/DELETE_COMPLETE row exists at all for the logical id ("UpdateReplacePolicy Retain must delete nothing"). Rule names above are per-run values.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout.
2. CreateStack with an AWS::Config::ConfigRule carrying UpdateReplacePolicy: Retain and an explicit ConfigRuleName.
3. UpdateStack changing ConfigRuleName (a replacement-forcing property), then DescribeStackEvents and filter on that logical id.
4. AWS emits UPDATE_IN_PROGRESS (replacement reason) -> UPDATE_COMPLETE on the new physical id -> DELETE_SKIPPED on the old physical id, and the old rule is still describable through Config. The emulator emits CREATE_IN_PROGRESS/CREATE_COMPLETE then DELETE_IN_PROGRESS/DELETE_COMPLETE and removes the old rule.
5. Replay cloudformation__config__iam__s3.cfn-config-rule-create-update-in-place-replace-retained-then-delete.
