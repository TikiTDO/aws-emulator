Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codepipeline.put-webhook-invalid-filter-pattern-rejected
Issue-Shape-Id: com.amazonaws.codepipeline#CodePipeline_20150709
Issue-Key: codepipeline.put-webhook-invalid-filter-pattern-rejected|run#15|AssertionError|assert error.get("Message") == expected_message, (
Issue-Commit: f0b886657eea5773ff1aa71c94acbc6bb0f20b91

# CodePipeline PutWebhook's InvalidWebhookFilterPatternException message is emulator-worded (member path + quoted value) instead of AWS's 'Request contains invalid filter pattern: <jsonPath>'

> _Note: this issue was identified against the code base at commit `f0b886657eea5773ff1aa71c94acbc6bb0f20b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutWebhook

## Description
One net-new root cause in the codepipeline behavioral sweep. The failing check is NOT marked ALREADY FILED or OWNED by the replay report. Related open issue: 20261023T080113Z-codepipeline-control-plane-declaration-validation-stage-transition-state.md — its Cause 6 covers PutWebhook resolving webhook.targetPipeline and refusing an absent one with PipelineNotFoundException (replay key `codepipeline|PutWebhook|PipelineNotFoundException`), and it happens to carry this check id in its trace list, but that precondition no longer fires: PutWebhook now accepts an absent targetPipeline, validates the definition, and classifies the bad jsonPath correctly. What remains, and what that issue does not describe anywhere, is the WORDING of the refusal. This is the only cause here.

=== Cause 1 of 1: the InvalidWebhookFilterPatternException message is invented wording, not CodePipeline's filter-pattern sentence ===
AWS: Smithy: PutWebhook.errors includes InvalidWebhookFilterPatternException (errorKind client, httpError 400) — "The specified event filter rule is in an invalid format"; WebhookFilterRule$jsonPath is "a JsonPath expression that is applied to the body/payload of the webhook" (WebhookFilters is @length(min 0, max 5), JsonPath is @length(min 1, max 150)). The model states the error class but not its message text, so the message is recorded behavior only: AWS refuses a filter whose jsonPath is not a parseable JSONPath with HTTP 400, error code InvalidWebhookFilterPatternException, and the message `Request contains invalid filter pattern: <jsonPath>`. Message template: the FIXED text is `Request contains invalid filter pattern: ` (one sentence, no trailing period); the only VARIABLE part is the offending jsonPath value, taken verbatim from the request's webhook.filters[].jsonPath — unquoted, with no member path, no list index and no parser diagnostic. (In the recording that value happened to be `$.[[not-a-json-path`; that string is request data filling the template, not part of it.)
Emulator: the status (400) and the error code (InvalidWebhookFilterPatternException) are right, but the message is the emulator's own diagnostic: `filters[0].jsonPath "$.[[not-a-json-path" is not a JsonPath expression` — it prefixes a member path with a list index, wraps the value in Rust `{:?}` quotes, and replaces AWS's sentence with "is not a JsonPath expression". None of that fixed text appears in AWS's message.
Why: the error message is part of the contract. A client (or a CI assertion, a log scraper, a console surface) that reads or matches CodePipeline's `Request contains invalid filter pattern: ` sentence — the only wording AWS emits for this class — finds a sentence it does not recognise, and a caller extracting the offending expression from the message gets a quoted, index-prefixed fragment rather than the jsonPath it sent. Tests that pass against AWS fail here even though the classification is correct.
Fix: the message formatter behind this refusal, not the classification: `WebhookProblem::FilterPattern` is constructed with an invented `format!` in the webhook-definition validator (services/service-com-amazonaws-codepipeline-codepipeline-20150709/src/state.rs, validate_webhook_definition, around lines 1839-1845), and the PutWebhook handler passes `problem.message()` straight through into InvalidWebhookFilterPatternException.message (src/ops/put_webhook.rs, lines 185-200). It should emit AWS's sentence from the request's jsonPath, the same way this crate's sibling webhook refusal already does — authentication_configuration_single_property_message (state.rs ~line 4411) already produces AWS's exact `Optional['authenticationConfig' should contain only one property: '<Property>']` wording, so the filter-pattern path is the one that is off-contract.
Note on scope: because this assertion fails first, the check's later assertions (the GITHUB_HMAC-with-empty-authenticationConfiguration refusal and the ListWebhooks absence/control-undisturbed assertions) are never reached in this run, so nothing is claimed about them here; re-replaying this check after the message is fixed is what confirms the rest.
Traces: codepipeline.put-webhook-invalid-filter-pattern-rejected.

## Evidence
Cause 1 — check codepipeline.put-webhook-invalid-filter-pattern-rejected, [mismatch].
Setup that succeeded (so the divergence is isolated to the message): PutWebhook with filters=[{jsonPath: "$.ref", matchEquals: "refs/heads/main"}] and an absent targetPipeline was accepted, round-tripped its filter, and appeared in ListWebhooks.
Then PutWebhook with filters=[{jsonPath: <unparseable JSONPath>, matchEquals: "refs/heads/main"}]:
  expected: HTTP 400, Code InvalidWebhookFilterPatternException, Message 'Request contains invalid filter pattern: $.[[not-a-json-path' (fixed text 'Request contains invalid filter pattern: ' + the request's jsonPath verbatim).
  actual:   HTTP 400, Code InvalidWebhookFilterPatternException (correct), Message 'filters[0].jsonPath "$.[[not-a-json-path" is not a JsonPath expression'.
Replay assertion verbatim: `AssertionError: expected message 'Request contains invalid filter pattern: $.[[not-a-json-path', got 'filters[0].jsonPath "$.[[not-a-json-path" is not a JsonPath expression'` (check_codepipeline_put_webhook_invalid_filter_pattern_rejected.py line 120).
Source of the emulator text: state.rs validate_webhook_definition -> `WebhookProblem::FilterPattern(format!("filters[{index}].jsonPath {:?} is not a JsonPath expression", filter.json_path))`.

## Steps to Reproduce
1. Boot the emulator and replay codepipeline.put-webhook-invalid-filter-pattern-rejected.
2. Or by hand: call PutWebhook with webhook={name: <any unused name>, targetPipeline: <any name>, targetAction: 'SourceAction', authentication: 'UNAUTHENTICATED', authenticationConfiguration: {}, filters: [{jsonPath: '$.ref', matchEquals: 'refs/heads/main'}]} — it is accepted (control).
3. Call PutWebhook again with a new name and filters: [{jsonPath: '$.[[not-a-json-path', matchEquals: 'refs/heads/main'}] (any value that is not a parseable JSONPath, e.g. '((' , works).
4. Read the error: the status is 400 and the code is InvalidWebhookFilterPatternException as AWS has it, but Message is 'filters[0].jsonPath "<value>" is not a JsonPath expression' instead of AWS's 'Request contains invalid filter pattern: <value>'.
