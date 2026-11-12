Issue-Source: trace-sweep-divergence
Issue-Trace: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ContinueUpdateRollback|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Commit: 3ab6a566a5e8009a4810936ef4b3a5f4bdba04b2

# ContinueUpdateRollback's wrong-state refusal names RollbackUpdatedStack and omits the stack status

> _Note: this issue was identified against the code base at commit `3ab6a566a5e8009a4810936ef4b3a5f4bdba04b2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ContinueUpdateRollback

## Description
One cause.

Cause 1 of 1: ContinueUpdateRollback called on a stack that is NOT in UPDATE_ROLLBACK_FAILED is refused with the correct status and error code but with an emulator-authored message that names a different operation and drops the offending stack status.

AWS: ContinueUpdateRollback is only valid for a stack in UPDATE_ROLLBACK_FAILED (Smithy: ContinueUpdateRollback documentation - "Continues rolling back a stack from UPDATE_ROLLBACK_FAILED to UPDATE_ROLLBACK_COMPLETE state"). For any other stack state AWS refuses with HTTP 400 and the awsQuery error envelope ErrorResponse.Error with Type 'Sender' and Code 'ValidationError' (ContinueUpdateRollback models only TokenAlreadyExistsException, so the state refusal is the service-common ValidationError), and Message:

  '<OperationName> cannot be called from stack with <StackStatus> status.'

Fixed text: the operation name as the caller invoked it ('ContinueUpdateRollback'), then the literal ' cannot be called from stack with ', then the literal ' status.' including the trailing period. Variable part: <StackStatus>, the stack's current StackStatus taken from the emulator's own stack state (UPDATE_FAILED in the recorded step; UPDATE_COMPLETE, CREATE_COMPLETE, ROLLBACK_COMPLETE etc. in the general case). The recorded response is 'ContinueUpdateRollback cannot be called from stack with UPDATE_FAILED status.' - UPDATE_FAILED is that run's state value, not part of the requirement. The sibling DetectStackDrift guard in the same fixture shows CloudFormation consistently quotes the blocking status back to the caller.

Emulator: HTTP 400 with ErrorResponse.Error.Message 'RollbackUpdatedStack cannot be called from current stack status'. Two defects in one string: the subject is the internal verb name 'RollbackUpdatedStack' rather than the invoked operation 'ContinueUpdateRollback', and the status is replaced by the constant phrase 'current stack status' so the actual StackStatus is never interpolated (the trailing period is also missing). Status and error code match.

Why: a client recovering a stuck stack branches on this refusal. It cannot match the message because the subject is an operation it never called, and - more damaging - it cannot learn WHICH state blocked it, because the status is not in the message. The documented recovery flow (read the blocking status, then pick ContinueUpdateRollback vs RollbackStack vs UpdateStack --disable-rollback) therefore cannot be driven from the emulator's response, and any log scraping or assertion keyed on the AWS sentence fails.

Fix: the ContinueUpdateRollback handler's stack-status pre-condition in the cloudformation service - it must emit the invoked operation name and format the stack's current StackStatus into the ValidationError message instead of using a shared constant written for the internal RollbackUpdatedStack verb. Related open issue 20261103T171935Z-cloudformation-state-guards-misfire-operations-accepted-or-refused.md bundles ten other cloudformation state-guard causes but carries no replay key for this one (owned_by was empty for this cluster) and does not mention ContinueUpdateRollback, so it does not cover this cause.

Traces: cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward#task#5.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout. One divergence cluster, verdict=mismatch, stub_path='', owned_by=[].

Fixture cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward, step task#5, operation ContinueUpdateRollback, service cloudformation.
Expected status 400, actual status 400 (status and Error.Type/Error.Code agree).
Divergent key $.ErrorResponse.Error.Message:
  expected 'ContinueUpdateRollback cannot be called from stack with UPDATE_FAILED status.'
  actual   'RollbackUpdatedStack cannot be called from current stack status'

Message-template breakdown - fixed text that MUST appear: '<invoked operation> cannot be called from stack with ' + '<status>' + ' status.'; the only value taken from emulator state is the stack's current StackStatus (UPDATE_FAILED in this recording). The emulator substitutes the internal verb name for the operation and the literal phrase 'current stack status' for the status value, and drops the terminating period.

## Steps to Reproduce
1. Replay the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout.
2. Replay fixture cloudformation-cfnuf-wrong-recovery-verbs-then-fix-forward up to step task#5: the fixture creates a stack, drives an update that leaves it in UPDATE_FAILED (not UPDATE_ROLLBACK_FAILED), then tries the wrong recovery verb by calling ContinueUpdateRollback on that stack.
3. Read the 400 response body's ErrorResponse.Error.Message.
4. It reads 'RollbackUpdatedStack cannot be called from current stack status'; AWS answers 'ContinueUpdateRollback cannot be called from stack with <StackStatus> status.' with the stack's current StackStatus interpolated (UPDATE_FAILED for this fixture). Confirm Error.Type 'Sender' and Error.Code 'ValidationError' are unchanged - only the message diverges.
