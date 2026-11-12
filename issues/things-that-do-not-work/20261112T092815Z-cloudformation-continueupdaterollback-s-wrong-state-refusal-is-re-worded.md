Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-stack-failure-rollback-and-cancel-recovery
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-stack-failure-rollback-and-cancel-recovery cloudformation-stack-recovery-operation-errors cloudformation-stack-rollback-and-signals stack-rollback-operations-wrong-state
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ContinueUpdateRollback|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 7d841236d87a4a0bdd7ae60e4f2e3cff95974158

# ContinueUpdateRollback's wrong-state refusal is re-worded: it names the invoked operation and interpolates the stack status instead of AWS's invariant RollbackUpdatedStack sentence

> _Note: this issue was identified against the code base at commit `7d841236d87a4a0bdd7ae60e4f2e3cff95974158`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ContinueUpdateRollback

## Description
One cause.

Cause 1 of 1: ContinueUpdateRollback invoked on a stack that is NOT in UPDATE_ROLLBACK_FAILED is refused with the correct HTTP status and error code but with an emulator-authored message instead of CloudFormation's invariant refusal sentence.

AWS: Smithy (services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json): ContinueUpdateRollback(StackName: StackNameOrId, required; RoleARN; ResourcesToSkip; ClientRequestToken; ForceRollback) models ONLY TokenAlreadyExistsException (client, HTTP 400, awsQueryError code TokenAlreadyExistsException), so a wrong-state refusal is CloudFormation's service-common ValidationError at HTTP 400; the operation's documentation scopes it to a single state - "Continues rolling back a stack from UPDATE_ROLLBACK_FAILED to UPDATE_ROLLBACK_COMPLETE state". The model does not state the message text, so for the wording: recorded behavior. The recording shows HTTP 400 with the awsQuery envelope ErrorResponse.Error (Type 'Sender', Code 'ValidationError') and Error.Message exactly:

  RollbackUpdatedStack cannot be called from current stack status

That whole sentence is FIXED text with NO variable part: AWS names its internal verb 'RollbackUpdatedStack' rather than the public operation the caller invoked, uses the constant phrase 'current stack status' rather than interpolating the stack's StackStatus, and ends WITHOUT a trailing period. Four independent recordings of the stack-recovery path agree on it, from different blocking statuses (CREATE_IN_PROGRESS in the representative step, and the other three fixtures' own states), which is itself the evidence that no part of the sentence is derived from state. The refusal must not perturb the stack: its StackStatus is unchanged afterwards.

Emulator: HTTP 400, Error.Type 'Sender' and Error.Code 'ValidationError' all agree; only $.ErrorResponse.Error.Message diverges. The emulator answers

  ContinueUpdateRollback cannot be called from stack with <StackStatus> status.

observed in the representative step as 'ContinueUpdateRollback cannot be called from stack with CREATE_IN_PROGRESS status.'. Three defects in one string: the subject is the invoked public operation name instead of AWS's internal 'RollbackUpdatedStack'; the constant phrase 'current stack status' is replaced by 'stack with <StackStatus> status', interpolating state into text AWS keeps invariant; and a trailing period is appended. CREATE_IN_PROGRESS there is this run's stack state filling the emulator's own template, not part of any requirement.

Why: this message is the only thing distinguishing "this stack is not stuck, there is nothing to continue" from every other ValidationError on the recovery path. A client or deploy wrapper that matches CloudFormation's sentence to choose between ContinueUpdateRollback, RollbackStack and UpdateStack --disable-rollback never recognizes the refusal; and because the emulator's text varies with the stack's status where AWS's is constant, no exact match is possible at all - a matcher cannot even be written for the emulator's output.

Fix: the stack-status pre-condition of the ContinueUpdateRollback handler in the cloudformation service and the shared message builder it calls - emit the invariant sentence and stop formatting the stack's status and the invoked operation name into it. Leave the neighbouring guards' wordings alone (CancelUpdateStack's 'CancelUpdateStack cannot be called from current stack status' and RollbackStack's 'RollbackStack can only be called on any of the [UPDATE_FAILED, CREATE_FAILED] statuses.' are the family's reference wording and pass today). File/line citation for orientation: services/service-com-amazonaws-cloudformation-cloudformation/src/ops/continue_update_rollback.rs, the pre-condition that calls crate::state::cannot_continue_update_rollback(&previous.status).

IMPORTANT - contradicting open issues, neither of which the replay report named as this cluster's owner (owned_by was empty):
- open WIRE issue 20261109T061652Z-cloudformation-continueupdaterollback-s-wrong-state-refusal-names-rollbackupdatedstack-and.md asks for the OPPOSITE wording ('<InvokedOperation> cannot be called from stack with <StackStatus> status.'), i.e. exactly what the emulator emits today, keyed to fixture cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#5. It therefore does not cover this cause, and the handler now satisfies it while diverging from the four fixtures and three behavioral checks cited here.
- open BEHAVIORAL issue 20261112T002904Z-cloudformation-2-net-new-causes-continueupdaterollback-s-wrong.md describes the same root cause from the behavioral suite (checks cloudformation.continue-update-rollback-on-stable-stack-fails, cloudformation.continue-update-rollback-wrong-state-rejected, cloudformation.rollback-cancel-continue-flow) and asks for the invariant sentence filed here; it carries no wire replay key, so the wire corpus is unguarded without this issue.
Reconcile the two before editing; the weight of evidence (4 wire fixtures + 3 behavioral checks) is on the invariant 'RollbackUpdatedStack cannot be called from current stack status'.

Traces: cloudformation-stack-failure-rollback-and-cancel-recovery#task#5 (representative), cloudformation-stack-recovery-operation-errors#task#9, cloudformation-stack-rollback-and-signals#task#4, stack-rollback-operations-wrong-state#task#3.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. One divergence cluster, verdict=mismatch, stub_path='', owned_by=[], affected_count=4.

divergence_key: service=cloudformation|ContinueUpdateRollback|status=400->400|keys=$.ErrorResponse.Error.Message
Representative: fixture cloudformation-stack-failure-rollback-and-cancel-recovery, step task#5, operation ContinueUpdateRollback, service cloudformation.
Expected status 400, actual status 400 (status, Error.Type and Error.Code all agree).
Divergent key $.ErrorResponse.Error.Message - value_mismatch:
  expected 'RollbackUpdatedStack cannot be called from current stack status'
  actual   'ContinueUpdateRollback cannot be called from stack with CREATE_IN_PROGRESS status.'

Other members of the cluster (same divergence_key, same expected message):
  cloudformation-stack-recovery-operation-errors#task#9
  cloudformation-stack-rollback-and-signals#task#4
  stack-rollback-operations-wrong-state#task#3

Message anatomy: AWS's string is 100% fixed text - 'RollbackUpdatedStack cannot be called from current stack status', no interpolated value, no trailing period. Nothing in it comes from the request or from emulator state. The emulator's string replaces the subject with the invoked operation name, replaces the constant phrase 'current stack status' with 'stack with <StackStatus> status' and appends '.'; CREATE_IN_PROGRESS above is this run's stack state filling the emulator's template and is not part of the requirement. Smithy: ContinueUpdateRollback declares only TokenAlreadyExistsException (client/400), so the wrong-state refusal is the service-common ValidationError at HTTP 400 - which the emulator already gets right.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Replay fixture cloudformation-stack-failure-rollback-and-cancel-recovery to step task#5: it creates a stack and, while the stack is not in UPDATE_ROLLBACK_FAILED, calls ContinueUpdateRollback(StackName=<that stack>) - the wrong recovery verb for the stack's current state.
3. Read the 400 response body's $.ErrorResponse.Error.Message. Error.Type 'Sender' and Error.Code 'ValidationError' are correct; the message must be exactly 'RollbackUpdatedStack cannot be called from current stack status' (no status interpolated, no trailing period) and is instead 'ContinueUpdateRollback cannot be called from stack with <StackStatus> status.'.
4. Confirm DescribeStacks still reports the stack's prior StackStatus (the refusal must not perturb the stack).
5. Repeat from other blocking statuses via the sibling fixtures cloudformation-stack-recovery-operation-errors#task#9, cloudformation-stack-rollback-and-signals#task#4 and stack-rollback-operations-wrong-state#task#3: the expected message is the same invariant sentence in every one, while the emulator's varies with the status.
6. Compare with the neighbouring guards that already pass: CancelUpdateStack from an invalid status -> 'CancelUpdateStack cannot be called from current stack status'; RollbackStack on a stable stack -> 'RollbackStack can only be called on any of the [UPDATE_FAILED, CREATE_FAILED] statuses.'
