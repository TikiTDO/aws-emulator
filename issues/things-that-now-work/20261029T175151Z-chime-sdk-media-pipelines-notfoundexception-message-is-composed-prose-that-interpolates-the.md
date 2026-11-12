Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found
Issue-Shape-Id: com.amazonaws.chimesdkmediapipelines#ChimeSDKMediaPipelinesService
Issue-Key: chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|4xx->4xx|$.Message
Issue-Commit: 646959a788c6809f2c0136ca28665b584f5bac57

# NotFoundException message is composed prose that interpolates the requested identifier instead of AWS's fixed not-found text

> _Note: this issue was identified against the code base at commit `646959a788c6809f2c0136ca28665b584f5bac57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteMediaInsightsPipelineConfiguration

## Description
One net-new wire root cause in the chime-sdk-media-pipelines emulator, found by replaying the wire fixture corpus (simparity-wire-aws-v1). The replay reported it as NOT owned by any open issue (owned_by is empty) and NOT blocked by a stub (stub_path is empty).

Related but non-covering issue: the open behavioral issue "chime-sdk-media-pipelines: 7 net-new root causes - NotFoundException messages are bespoke prose instead of AWS's fixed text, duplicate PoolName misclassified ConflictException, ..." (20261029T171129Z-chime-sdk-media-pipelines-7-net-new-root.md) describes the same emulator defect in its Cause 1, but it is filed against the BEHAVIORAL corpus (Issue-Corpus: simparity-behavioral-aws-v1) and carries only behavioral check ids as its replay keys. It carries NO Issue-Trace/Issue-Step for this wire fixture step, so this wire divergence has no replay key on any open issue and is filed here with its own.

=== Cause 1 of 1: DeleteMediaInsightsPipelineConfiguration's 404 body carries a per-resource sentence with the requested Identifier substituted into it, instead of this service's one fixed not-found message ===
AWS: Smithy: DeleteMediaInsightsPipelineConfiguration, DELETE /media-insights-pipeline-configurations/{Identifier}, success 204, models NotFoundException with httpError 404 (documented "One or more of the resources in the request does not exist in the system."); its Identifier member is an @required httpLabel NonEmptyString, @length max 1024, @pattern \\S. NotFoundException's members are Code, Message (String, @length max 4096) and RequestId. The model does not state the Message text, so for the wording: recorded behavior only - when the Identifier names no existing media insights pipeline configuration, the response is HTTP 404 with Code NotFoundException and Message exactly the fixed string "Unable to find the requested identifier". That message is ENTIRELY fixed text: it has no variable parts at all - no resource-kind noun, no configuration name, no id, no ARN - so nothing from the request or from emulator state is interpolated into it. The only per-run value in the 404 body is RequestId, which is not part of this finding.
Emulator: answers the correct HTTP 404 and the correct Code NotFoundException, but composes its own sentence in the handler and substitutes the caller's Identifier into it: Message is "No media insights pipeline configuration identified by <Identifier> exists." - fixed prose "No media insights pipeline configuration identified by " + the Identifier copied verbatim from the request path + " exists." (in the replayed step the Identifier was a nonexistent configuration name, and it appeared in full inside the message). The divergence detail is exactly $.Message: value_mismatch, expected 'Unable to find the requested identifier', got the composed sentence; expected_status 404 and actual_status 404 agree, so status and error code are right and only the message text is wrong.
Why: a client that branches on, matches, or logs this service's not-found wording - and any SDK/test harness asserting the documented error text - behaves differently against the emulator than against AWS. AWS's text deliberately echoes NO caller-supplied identifier; the emulator's echoes the whole Identifier (up to the member's 1024-character bound) back into the error body, so an emulator-based test of an error-redaction or log-scrubbing path sees identifier material AWS never emits, and error-message equality checks fail.
Fix: the service's not-found refusal wording - the shared not-found error builder this service's handlers call (crate::state::not_found) must own the one fixed message, and DeleteMediaInsightsPipelineConfiguration's handler must stop passing it a format!-composed, Identifier-interpolated string. This belongs in the error construction/serialization path, not in the lookup logic: the 404 and the NotFoundException code are already correct.
Traces: fixture chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found, step task#1 (cluster of 1; its only affected member is that same step).

## Evidence
Wire replay, trace set simparity-wire-aws-v1: steps_total 1, steps_passed 0, steps_mismatched 1, score_pct 0.0; emulator_up true (/__admin/health answered 200 and the data listener accepts), bootstrap_signal false.

Divergence cluster (verdict mismatch, stub_path empty, owned_by empty):
  service: chime-sdk-media-pipelines
  operation: DeleteMediaInsightsPipelineConfiguration
  fixture_id: chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found
  step_key: task#1
  divergence_key: service=chime-sdk-media-pipelines|DeleteMediaInsightsPipelineConfiguration|status=404->404|keys=$.Message
  expected_status: 404   actual_status: 404
  detail: $.Message: value_mismatch (expected 'Unable to find the requested identifier', got 'No media insights pipeline configuration identified by sim2real-no-such-config-7e0c739f-0ee7-4586-bc56-1102c84c2108 exists.')
  affected (1): chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found#task#1

Requirement vs observed, with the fixed/variable split spelled out:
  Required: HTTP 404, Code "NotFoundException", Message "Unable to find the requested identifier" - 100% fixed text, no variable parts.
  Observed: HTTP 404, Code "NotFoundException", Message "No media insights pipeline configuration identified by <Identifier> exists." - fixed parts "No media insights pipeline configuration identified by " and " exists."; variable part the Identifier taken from the request path. The configuration name quoted in the detail above (sim2real-no-such-config-<uuid>) is a per-run value from this one recording and is NOT part of the requirement - the defect is that the template interpolates any identifier at all, and that the surrounding prose is not AWS's text.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture chime-sdk-media-pipelines-delete-media-insights-pipeline-configuration-not-found, step task#1.
2. Equivalently by hand: call DeleteMediaInsightsPipelineConfiguration (DELETE /media-insights-pipeline-configurations/{Identifier}) with an Identifier naming a media insights pipeline configuration that does not exist in the account (any non-empty name or ARN that was never created, or one already deleted).
3. Expected: HTTP 404, body Code "NotFoundException" and Message exactly "Unable to find the requested identifier", with the requested Identifier appearing nowhere in the body.
4. Observed: HTTP 404 and Code "NotFoundException" (both correct), but Message "No media insights pipeline configuration identified by <the Identifier you passed> exists."
5. Re-replay the same fixture step to confirm the fix: the only difference must be the Message, which must equal the fixed text and must contain no part of the request's Identifier.
