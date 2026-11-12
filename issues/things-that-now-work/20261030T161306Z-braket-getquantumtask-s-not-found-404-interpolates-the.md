Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: braket.braket-resource-not-found-error-semantics-consistent
Issue-Shape-Id: com.amazonaws.braket#Braket
Issue-Key: braket.braket-resource-not-found-error-semantics-consistent|run#19|AssertionError|assert _quantum_task_message == "Quantum task not found", (
Issue-Commit: 47f4067126cea87038b612b9d5cde693ce54e59e

# Braket GetQuantumTask's not-found 404 interpolates the ARN instead of AWS's fixed "Quantum task not found" sentence (1 cause)

> _Note: this issue was identified against the code base at commit `47f4067126cea87038b612b9d5cde693ce54e59e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQuantumTask

## Description
ONE net-new root cause in the braket crate. The refusal the emulator serves for a well-formed but never-issued quantum-task ARN carries the right HTTP status and the right error code, but the message is the handler's own interpolated prose instead of the fixed sentence Amazon Braket words for that error class. An error message is a requirement, so this is a fidelity defect. braket is bootstrapped and answering (bootstrap issue 20261021T211836Z-bootstrap-service-com-amazonaws-braket-braket is resolved), so this is not a bootstrap gap. A fixer must close ONE cause before submitting.

Related-but-not-covering issues: the open 20261029T220739Z-braket-refusal-messages-diverge-from-aws-s-wording carries this same check id, but its two causes are CreateSpendingLimit's simulator refusal and GetDevice's not-found template — the GetDevice assertion now PASSES in this replay and the check has advanced past it to the GetQuantumTask assertion below, which that issue explicitly records as "never exercised and NOT filed here". The open 20261022T134201Z-braket-quantum-task-lifecycle-createquantumtask-validation-and-not also carries this check id, but at the GetJob assertion (Cause 3, 'NotImplemented'), which now PASSES too. The resolved 20261029T170807Z-braket-client-error-messages-are-the-emulator-s covers GetQuantumTask's MALFORMED-ARN ValidationException wording ('quantumTaskArn is invalid.', its Cause 3) — a different error class on the same operation; its 400 path is not this 404 path, and a resolved issue never covers a live failure.

=== Cause 1 of 1: GetQuantumTask's ResourceNotFoundException message echoes the requested ARN where AWS returns a constant sentence ===
AWS: Smithy (services/service-com-amazonaws-braket-braket/smithy-model/com-amazonaws-braket-braket.json): GetQuantumTask GET /quantum-task/{quantumTaskArn}, success 200, input quantumTaskArn (QuantumTaskArn, @httpLabel, @required, len 0..256); modeled errors ValidationException (httpError 400), ResourceNotFoundException (httpError 404, documented 'The specified resource was not found.'), AccessDeniedException (403), ThrottlingException (429), InternalServiceException (500). The model carries no message text — recorded behavior only: a syntactically valid quantum-task ARN (an ARN whose id part is a well-formed UUID) that names no existing task is refused 404 ResourceNotFoundException whose message is the FIXED text "Quantum task not found". The whole sentence is fixed: it interpolates NOTHING from the request — not the ARN, not the task id, not the account or region — and carries no trailing period. CancelQuantumTask answers the same fixed sentence for the same class on a never-issued ARN (same recorded corpus; this check asserts it too, but aborted before reaching it).
Emulator: status 404 and code ResourceNotFoundException match, but the message is the handler's `format!("Quantum task {} not found", input.quantum_task_arn)` — AWS's words wrapped around an echo of the request's ARN. Observed in replay: 'Quantum task arn:aws:braket:us-west-2:000000000000:quantum-task/50f1f85f-4329-4981-8688-cd23b549beae not found'. The ARN, region, account id and UUID inside that string are this run's per-request values filling the emulator's template; they are not part of any requirement — the requirement is the constant sentence with nothing interpolated.
Why: a client that classifies Braket not-found failures by message text, or a test that asserts the service's documented sentence, does not recognise the error; the emulator's variant also leaks caller-supplied input of unbounded length into an error body where AWS returns a constant, so any caller that logs or compares the message sees a value that differs per request where AWS's is stable.
Fix: the missing-task branch of GetQuantumTask's handler (services/service-com-amazonaws-braket-braket/src/ops/get_quantum_task.rs), which must emit the service's fixed not-found sentence instead of an interpolated one; the wording belongs in that operation's not-found path, not in a shared single-homed prose helper, because neighbouring braket operations word their own refusals differently (GetQuantumTask's and CancelQuantumTask's malformed-ARN sentences already differ). CancelQuantumTask's identical not-found branch (.../src/ops/cancel_quantum_task.rs) must answer the same fixed sentence; the check asserts it immediately after the ones below, so it will be exercised once this cause is fixed.
Traces: braket.braket-resource-not-found-error-semantics-consistent

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator: 5/16 checks passed (31.2%), all 16 in braket. Of the 8 reported braket failures, 5 are OWNED by open issues (4 by 20261022T134201Z-braket-quantum-task-lifecycle-..., 1 by 20261029T220739Z-braket-refusal-messages-...) and 2 are ALREADY FILED stub blocks (UpdateSpendingLimit, ListTagsForResource); the single surviving failure is the cause below.

Cause 1 — check braket.braket-resource-not-found-error-semantics-consistent (mismatch), at check line 60 / run#assertion on _quantum_task_message:
  braket.get_quantum_task(quantumTaskArn='arn:aws:braket:<region>:<account>:quantum-task/<fresh uuid4>')
  The preceding assertions now PASS: GetDevice's code AND its full not-found template, and GetJob's code and 'Job <account>#<name> not found' message. The failure is the next one:
  AssertionError: expected the exact ResourceNotFoundException message AWS returns, got 'Quantum task arn:aws:braket:us-west-2:000000000000:quantum-task/50f1f85f-4329-4981-8688-cd23b549beae not found'
  expected (fixed text, nothing interpolated): 'Quantum task not found'
  The error Code assertion immediately above it PASSES (ResourceNotFoundException, 404), so status and code are correct and only the message diverges.

Code site observed in the checkout: services/service-com-amazonaws-braket-braket/src/ops/get_quantum_task.rs lines 262-272 — the `guard.quantum_tasks.get(...)` miss builds ResourceNotFoundException with `format!("Quantum task {} not found", input.quantum_task_arn)`.

The check aborted at this assertion, so its remaining expectations were never reached and are NOT filed here: ListTagsForResource answering the distinct NotFoundException 'Resource was not found' (that operation is an unimplemented stub, already filed as 20261029T220739Z-braket-listtagsforresource-stub), DeleteSpendingLimit's 'Spending limit <arn> not found', CancelJob's 'Job <account>#<name> not found', and CancelQuantumTask's 'Quantum task not found'. They may or may not hold once this cause is fixed, so the check must be run to completion after the fix.

## Steps to Reproduce
Boot the emulator and point a botocore braket client at it in us-west-2.

Cause 1 (check braket.braket-resource-not-found-error-semantics-consistent):
  import uuid
  arn = f"arn:aws:braket:{region}:{account_id}:quantum-task/{uuid.uuid4()}"   # well-formed, never issued
  braket.get_quantum_task(quantumTaskArn=arn)
  Expect: 404 ResourceNotFoundException with message exactly 'Quantum task not found' — the complete sentence, with no part of the request (ARN, account, region, task id) interpolated.
  Observe: 404 ResourceNotFoundException with message 'Quantum task <the ARN just passed> not found'.
  Then, with the same never-issued ARN: braket.cancel_quantum_task(quantumTaskArn=arn, clientToken=<uniq>) -> must also answer 404 ResourceNotFoundException 'Quantum task not found'.

Then re-replay exactly this check: braket.braket-resource-not-found-error-semantics-consistent. Note it continues past the fixed assertion into further not-found expectations (ListTagsForResource's NotFoundException 'Resource was not found', DeleteSpendingLimit, CancelJob, CancelQuantumTask) that this run never reached, so it must be run to completion.
