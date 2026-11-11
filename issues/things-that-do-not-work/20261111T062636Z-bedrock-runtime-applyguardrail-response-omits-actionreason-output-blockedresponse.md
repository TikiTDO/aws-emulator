Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-guardrail-version-applies-word-and-regex-policies
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-guardrail-version-applies-word-and-regex-policies
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.assessments[*].invocationMetrics.usage.automatedReasoningPolicies
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.assessments[*].invocationMetrics.usage.automatedReasoningPolicyUnits
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.assessments[*].invocationMetrics.usage.contentPolicyImageUnits
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.assessments[*].invocationMetrics.usage.sensitiveInformationPolicyFreeUnits
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.assessments[*].wordPolicy.managedWordLists
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.blockedResponse
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.output
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.usage.automatedReasoningPolicies
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.usage.automatedReasoningPolicyUnits
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.usage.contentPolicyImageUnits
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.usage.sensitiveInformationPolicyFreeUnits
Issue-Key: bedrock-runtime|ApplyGuardrail|2xx->2xx|$.actionReason
Issue-Commit: 92a32530ab8850c08aaa0896436d0dde582b0aa5

# bedrock-runtime ApplyGuardrail response omits actionReason, output/blockedResponse and the new usage counters

> _Note: this issue was identified against the code base at commit `92a32530ab8850c08aaa0896436d0dde582b0aa5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyGuardrail

## Description
ApplyGuardrail against a published guardrail version returns HTTP 200, but the emulator's success body is structurally incomplete in the same way across every guardrail verdict (intervened/blocked, masked, and no-action): the members that tell a client WHAT the guardrail did and WHAT text to use are absent, the three newer GuardrailUsage counters are never serialized, sensitiveInformationPolicyFreeUnits is mis-counted, and an empty managedWordLists list is emitted where AWS omits it. All three causes are one defect family in the ApplyGuardrail response builder / usage accounting, filed as one issue; a fixer must close ALL THREE before submitting.

None of the open bedrock-runtime guardrail issues covers this: 20261024T031932Z-bedrock-runtime-inline-guardrail-error-taxonomy-applyguardrail-and.md and 20261024T094806Z-bedrock-runtime-applyguardrail-answers-the-unmodeled-code-notimplemented.md are about ApplyGuardrail ERROR codes, and 20261023T232022Z / 20261029T171601Z / 20261025T164541Z are about Converse/Invoke guardrail enforcement and trace. This issue is about the shape and values of the ApplyGuardrail 200 body.

Cause 1 of 3: GUARDRAIL_INTERVENED (blocking) evaluation returns no blocked text and no usage counters. Fixture bedrock-guardrail-version-applies-word-and-regex-policies, step task#5.
AWS: Smithy (bedrock-runtime) ApplyGuardrailResponse members: usage (GuardrailUsage, @required), action (GuardrailAction @required, enum NONE|GUARDRAIL_INTERVENED), actionReason (String), outputs (@required), assessments (@required), guardrailCoverage. GuardrailUsage models contentPolicyImageUnits, automatedReasoningPolicyUnits and automatedReasoningPolicies as optional integers, and sensitiveInformationPolicyFreeUnits as an @required integer. Recorded behavior only (not in the checked-in model): when a guardrail blocks, AWS additionally serializes `blockedResponse` carrying the guardrail's configured blocked-messaging text and an `output` list of {text} blocks holding that same text. The recording shows AWS emitting contentPolicyImageUnits=0, automatedReasoningPolicyUnits=0 and automatedReasoningPolicies=0 (present, zero-valued, not omitted) and sensitiveInformationPolicyFreeUnits=0 for a request whose sensitive-information policy matched nothing by a managed/free PII entity. The recording also shows GuardrailWordPolicyAssessment.managedWordLists absent from the body when no managed word list matched, even though the model marks it @required.
Emulator: `blockedResponse` is absent (expected the guardrail's blocked-messaging text, got None); `output` is absent (expected a one-element list of {text: <blocked-messaging text>}, got None); usage.automatedReasoningPolicies, usage.automatedReasoningPolicyUnits and usage.contentPolicyImageUnits are absent at both $.usage and $.assessments[0].invocationMetrics.usage (expected 0, got None); usage.sensitiveInformationPolicyFreeUnits is 1 where AWS reports 0, again at both $.usage and $.assessments[0].invocationMetrics.usage; and $.assessments[0].wordPolicy.managedWordLists is serialized as [] where AWS omits the member.
Why: a client that blocks a prompt and then shows the user the guardrail's own blocked message reads blockedResponse/output, finds nothing, and either shows an empty reply or crashes on a null. A client that bills or meters on GuardrailUsage sees three counters as missing rather than zero (a typed SDK may surface them as null instead of 0) and mis-bills free sensitive-information units by one per call. An extra empty managedWordLists changes the assessment's JSON shape for consumers that distinguish 'absent' from 'no matches'.
Fix: the ApplyGuardrail handler's response serializer and its usage accounting (populate blockedResponse/output on an intervened verdict from the guardrail's configured blocked messaging; always serialize the three optional usage counters as 0; count sensitive-information free units from actual free-entity matches; stop emitting an empty managedWordLists).
Traces: bedrock-guardrail-version-applies-word-and-regex-policies#task#5.

Cause 2 of 3: masking evaluation returns neither actionReason nor the masked output text. Fixture bedrock-guardrail-version-applies-word-and-regex-policies, step task#6.
AWS: Smithy ApplyGuardrailResponse.actionReason is a String member; recorded behavior shows AWS setting it to the fixed text "Guardrail masked." when the guardrail anonymised/masked content rather than blocking it. The fixed text is the whole value here — it carries no request- or state-derived variable part. AWS also serializes `output` (recorded behavior only) as the list of {text} blocks holding the request's content with each matched entity replaced by its configured mask placeholder, e.g. the request text with the matched regex/PII span replaced by its {entity-name} tag. usage again carries contentPolicyImageUnits=0, automatedReasoningPolicyUnits=0 and automatedReasoningPolicies=0.
Emulator: $.actionReason is absent (expected 'Guardrail masked.', got None); $.output is absent (expected the masked text blocks, got None); usage.automatedReasoningPolicies, usage.automatedReasoningPolicyUnits and usage.contentPolicyImageUnits are absent at both $.usage and $.assessments[0].invocationMetrics.usage (expected 0, got None).
Why: masking is only useful if the caller can retrieve the masked text; with `output` absent the caller has no sanitized content to forward, and with actionReason absent it cannot distinguish 'masked' from 'blocked' or 'intervened for another reason' without re-deriving it from the assessment.
Fix: the ApplyGuardrail handler's response builder — set actionReason for the masking verdict and emit the masked content in `output`; same usage-counter serialization gap as Cause 1.
Traces: bedrock-guardrail-version-applies-word-and-regex-policies#task#6.

Cause 3 of 3: no-action evaluation omits actionReason and the empty output list, and still over-counts free sensitive-information units. Fixture bedrock-guardrail-version-applies-word-and-regex-policies, step task#7.
AWS: recorded behavior shows that when the guardrail takes no action AWS still serializes actionReason with the fixed text "No action." and still serializes `output` as an empty list (present-and-empty, not omitted), with usage.sensitiveInformationPolicyFreeUnits=0 and the three optional counters present as 0.
Emulator: $.actionReason is absent (expected 'No action.', got None); $.output is absent (expected [], got None); usage.automatedReasoningPolicies, usage.automatedReasoningPolicyUnits and usage.contentPolicyImageUnits are absent (expected 0, got None) at both $.usage and $.assessments[0].invocationMetrics.usage; usage.sensitiveInformationPolicyFreeUnits is 1 where AWS reports 0, at both those paths.
Why: a client that branches on actionReason to decide whether to pass content through gets a null on the pass-through path, and a client that reads `output` unconditionally must special-case absent-vs-empty. The free-unit over-count means every ApplyGuardrail call is metered as having consumed a free sensitive-information unit it did not.
Fix: same component as Causes 1 and 2 — the ApplyGuardrail response builder and its usage accounting, on the NONE/no-action path.
Traces: bedrock-guardrail-version-applies-word-and-regex-policies#task#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 (31 fixtures, 172 steps, 128 passed, score 74.4%). Fixture bedrock-guardrail-version-applies-word-and-regex-policies, three clusters, all verdict=mismatch, all expected_status 200 / actual_status 200, all stub_path empty and owned_by empty (no existing issue carries their replay key).

Step task#5 (ApplyGuardrail, blocking verdict):
  $.blockedResponse: missing (expected the guardrail's blocked-messaging text, got None)
  $.output: missing (expected [{'text': <same blocked-messaging text>}], got None)
  $.usage.automatedReasoningPolicies: missing (expected 0, got None)
  $.usage.automatedReasoningPolicyUnits: missing (expected 0, got None)
  $.usage.contentPolicyImageUnits: missing (expected 0, got None)
  $.usage.sensitiveInformationPolicyFreeUnits: value_mismatch (expected 0, got 1)
  $.assessments[0].invocationMetrics.usage.{automatedReasoningPolicies,automatedReasoningPolicyUnits,contentPolicyImageUnits}: missing (expected 0, got None)
  $.assessments[0].invocationMetrics.usage.sensitiveInformationPolicyFreeUnits: value_mismatch (expected 0, got 1)
  $.assessments[0].wordPolicy.managedWordLists: type_mismatch (expected None/absent, got [])

Step task#6 (ApplyGuardrail, masking verdict):
  $.actionReason: missing (expected 'Guardrail masked.', got None)
  $.output: missing (expected [{'text': <request text with matched entity replaced by its mask placeholder>}], got None)
  $.usage.{automatedReasoningPolicies,automatedReasoningPolicyUnits,contentPolicyImageUnits}: missing (expected 0, got None)
  $.assessments[0].invocationMetrics.usage.{automatedReasoningPolicies,automatedReasoningPolicyUnits,contentPolicyImageUnits}: missing (expected 0, got None)

Step task#7 (ApplyGuardrail, no-action verdict):
  $.actionReason: missing (expected 'No action.', got None)
  $.output: missing (expected [], got None)
  $.usage.{automatedReasoningPolicies,automatedReasoningPolicyUnits,contentPolicyImageUnits}: missing (expected 0, got None)
  $.usage.sensitiveInformationPolicyFreeUnits: value_mismatch (expected 0, got 1)
  $.assessments[0].invocationMetrics.usage.{automatedReasoningPolicies,automatedReasoningPolicyUnits,contentPolicyImageUnits}: missing (expected 0, got None)
  $.assessments[0].invocationMetrics.usage.sensitiveInformationPolicyFreeUnits: value_mismatch (expected 0, got 1)

Model citation: Smithy com.amazonaws.bedrockruntime ApplyGuardrail -> ApplyGuardrailResponse{usage @required, action @required, actionReason:String, outputs @required, assessments @required, guardrailCoverage}; GuardrailUsage{..., sensitiveInformationPolicyFreeUnits @required, contentPolicyImageUnits, automatedReasoningPolicyUnits, automatedReasoningPolicies}; GuardrailWordPolicyAssessment{customWords @required, managedWordLists @required}. `output` and `blockedResponse` are recorded behavior only (not present in the checked-in model). The fixed texts 'Guardrail masked.' and 'No action.' are whole-value constants; the blocked/masked text content is derived from the guardrail configuration and the request content, not from any one recording.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1); or replay fixture bedrock-guardrail-version-applies-word-and-regex-policies on its own.
3. The fixture creates a guardrail with word (custom words) and sensitive-information (regex) policies, publishes a version, then calls ApplyGuardrail against that guardrail version three times:
   - task#5: content that matches a blocking custom-word/topic rule -> inspect the 200 body for `blockedResponse`, `output`, the three GuardrailUsage counters and `assessments[0].wordPolicy.managedWordLists`.
   - task#6: content that matches a masking regex rule -> inspect `actionReason` and `output`.
   - task#7: content matching nothing -> inspect `actionReason`, `output` and `usage.sensitiveInformationPolicyFreeUnits`.
4. Compare each 200 body against the fixture's recorded response: the members listed in Evidence are absent or wrong in the emulator's body.
