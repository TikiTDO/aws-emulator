Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.get-execution-preview-nonexistent-rejected ssm.execution-preview-result-expires-after-window ssm.start-execution-preview-malformed-input-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.get-execution-preview-nonexistent-rejected|run#5|AssertionError|assert error["Message"] == (
Issue-Key: ssm.execution-preview-result-expires-after-window|run#15|AssertionError|assert error["Message"] == (
Issue-Key: ssm.start-execution-preview-malformed-input-rejected|run#8|AssertionError|assert message == "Targets and TargetMaps cannot both be provided.", message
Issue-Commit: 55ffe08148b0f5da621dea606ca05d7920bfd4b8

# SSM execution-preview refusal messages are emulator-authored: GetExecutionPreview's ResourceNotFoundException and StartExecutionPreview's Targets/TargetMaps ValidationException both diverge from AWS's wording

> _Note: this issue was identified against the code base at commit `55ffe08148b0f5da621dea606ca05d7920bfd4b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetExecutionPreview

## Description
Two net-new root causes in the SSM execution-preview surface. Both operations reach the right error SHAPE and HTTP status, but each serves an emulator-authored message instead of the message AWS words for that error class. The error message is part of the contract, so both must be corrected.

Cause 1 of 2: GetExecutionPreview's ResourceNotFoundException message for an unknown or expired preview id.
  AWS: Smithy (services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json) models GetExecutionPreview with input member ExecutionPreviewId (required, @length min 36 max 36, pattern ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$) and error ResourceNotFoundException (client error, HTTP 400). The model does not state the message text, so this is recorded behavior only: for a syntactically valid but never-issued preview id AWS answers HTTP 400 / ResourceNotFoundException with the message `Execution preview '<ExecutionPreviewId>' does not exist or is expired`. Fixed text: `Execution preview '` ... `' does not exist or is expired` — single-quoted id, no trailing period, and the clause `or is expired` present because an expired preview is refused identically. Variable part: the ExecutionPreviewId copied verbatim from the request (the UUID in the recordings, e.g. 89092927-98c8-47d6-b469-b80d697f3320 or 35486199-2876-40f5-8c1c-477def021dbd, is only a filled-in slot, not part of the requirement).
  Emulator: answers ResourceNotFoundException with `Execution preview <id> does not exist.` — the id is unquoted, the `or is expired` clause is dropped, and a trailing period is added.
  Why: a client (or a human operator reading the error) cannot distinguish "never existed" from "expired", which is the whole point of AWS's combined wording; any client, test or runbook that matches the refusal text — including the two behavioral checks here — treats the emulator's refusal as an unknown error class and fails, even though the code and status are right.
  Fix: the message builder behind GetExecutionPreview's not-found branch — crate::state::execution_previews::execution_preview_not_found_message, called from the handler's miss branch; the handler itself (control flow, region scoping, store lookup) is correct.
  Traces: ssm.get-execution-preview-nonexistent-rejected (asserts the message directly), ssm.execution-preview-result-expires-after-window (same assertion, reached after a real preview has been started and polled to a terminal status).

Cause 2 of 2: StartExecutionPreview's ValidationException message when Targets and TargetMaps are both supplied.
  AWS: Smithy models StartExecutionPreview with input members DocumentName (required, pattern ^[a-zA-Z0-9_\-.]{3,128}$), DocumentVersion and ExecutionInputs, and error ValidationException (client error, HTTP 400, documented as "The request isn't valid. Verify that you entered valid contents for the command and try again."). The mutual exclusivity of ExecutionInputs.Automation.Targets and ExecutionInputs.Automation.TargetMaps is not expressible in the model, so the message is recorded behavior only: AWS answers HTTP 400 / ValidationException with the message `Targets and TargetMaps cannot both be provided.` This message has no variable parts — it is fixed text, including the final period and the word `provided`.
  Emulator: answers ValidationException (correct code and status) with `Targets and TargetMaps can't both be specified` — contracted `can't` instead of `cannot`, `specified` instead of `provided`, and no terminating period.
  Why: a client that branches on the refusal text to tell a mutually-exclusive-input mistake apart from the other ValidationExceptions StartExecutionPreview raises (unknown runbook, unknown version, unknown parameter, unresolvable target key) mis-classifies the failure and cannot surface the right remediation to its caller.
  Fix: the validation message builder for the target members — crate::state::automation::target_rejection, invoked from the StartExecutionPreview handler's target_rejection branch (services/service-com-amazonaws-ssm-amazonssm/src/ops/start_execution_preview.rs around the `target_rejection` call). The surrounding validation order and the chosen error shape are correct; only the rendered text diverges. Note this builder is shared with the automation start path, so the wording AWS uses for each caller must be preserved when it is corrected.
  Traces: ssm.start-execution-preview-malformed-input-rejected (also exercises the positive control — the same document with neither member set must succeed and the preview must reach Status Success, which it does).

## Evidence
Cause 1 — ssm.get-execution-preview-nonexistent-rejected:
  expected: error Code == ResourceNotFoundException AND Message == "Execution preview '<bogus-uuid>' does not exist or is expired"
  actual:   Code ResourceNotFoundException (correct), Message "Execution preview 89092927-98c8-47d6-b469-b80d697f3320 does not exist."
  AssertionError raised at check_ssm_get_execution_preview_nonexistent_rejected.py line 37 (`assert error["Message"] == ...`).

Cause 1 — ssm.execution-preview-result-expires-after-window:
  same assertion, reached after StartExecutionPreview + GetExecutionPreview polled to a terminal Status:
  actual Message "Execution preview 35486199-2876-40f5-8c1c-477def021dbd does not exist."
  AssertionError raised at check_ssm_execution_preview_result_expires_after_window.py line 75.

Cause 2 — ssm.start-execution-preview-malformed-input-rejected:
  request: StartExecutionPreview(DocumentName=<automation runbook>, ExecutionInputs={"Automation": {"Targets": [{"Key": "InstanceIds", "Values": ["i-..."]}], "TargetMaps": [{"InstanceIds": ["i-..."]}], "TargetParameterName": "InstanceIds"}})
  expected: Code == ValidationException AND Message == "Targets and TargetMaps cannot both be provided."
  actual:   Code ValidationException (correct), Message "Targets and TargetMaps can't both be specified"
  AssertionError raised at check_ssm_start_execution_preview_malformed_input_rejected.py line 54.

## Steps to Reproduce
Cause 1:
1. Call ssm:GetExecutionPreview with a syntactically valid but never-issued ExecutionPreviewId (any UUID matching ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$).
2. Observe HTTP 400 / ResourceNotFoundException. The message reads "Execution preview <id> does not exist." instead of "Execution preview '<id>' does not exist or is expired".
3. Positive control that must keep working: CreateDocument (DocumentType=Automation, a single aws:sleep step) -> StartExecutionPreview -> GetExecutionPreview on the returned id, which resolves and reaches a terminal Status.

Cause 2:
1. CreateDocument with DocumentType=Automation, DocumentFormat=JSON, schemaVersion 0.3 and one aws:sleep mainStep; wait for DescribeDocument Status == Active.
2. Call ssm:StartExecutionPreview on that document with ExecutionInputs.Automation carrying BOTH Targets and TargetMaps (plus TargetParameterName).
3. Observe HTTP 400 / ValidationException with message "Targets and TargetMaps can't both be specified" instead of "Targets and TargetMaps cannot both be provided."
4. Positive control: the same StartExecutionPreview with neither Targets nor TargetMaps succeeds and the preview reaches Status Success.

Replay: run_behavioral_suite and re-run the three checks listed in trace_ids.
