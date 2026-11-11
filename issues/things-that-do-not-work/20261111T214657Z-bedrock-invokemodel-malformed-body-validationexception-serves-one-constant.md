Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.error-validation-malformed-body-invoke
Issue-Key: bedrock.error-validation-malformed-body-invoke|run#6|AssertionError|assert message == expected, f"Expected message {expected!r}, got {message!r}"
Issue-Commit: 83a487fe0db4ce5c414ce564b50d725b494461fa

# bedrock InvokeModel malformed-body ValidationException serves one constant sentence for every model: moonshotai.kimi-k2.5 gets the provider-agnostic text instead of AWS's JSON-object-malformed sentence

> _Note: this issue was identified against the code base at commit `83a487fe0db4ce5c414ce564b50d725b494461fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModel

## Description
One net-new root cause in the bedrock data-plane body-parse refusal path.

Cause 1 of 1: InvokeModel is called with contentType=application/json and a payload that is not parseable JSON, against a model whose provider request-validator words the refusal differently (here moonshotai.kimi-k2.5). The HTTP status (400) and the error code (ValidationException) are correct, but the `message` member carries the emulator's single hard-coded malformed-input sentence regardless of which model the request targets, so the sentence AWS emits for this model family is never served.

AWS: Smithy com.amazonaws.bedrockruntime#InvokeModel (POST /model/{modelId}/invoke, success 200) models ValidationException with errorKind=client, @httpError 400; `body` is the @httpPayload blob (@length max 25000000, @sensitive) and `modelId` is the required @httpLabel. A body that cannot be parsed as JSON is refused at the API boundary and never forwarded to the model. The exact sentence is recorded behavior only, and it is NOT one constant across the catalogue: the message is produced by the target model provider's input validator, so it varies by the model named in `modelId`. For moonshotai.kimi-k2.5 the recorded refusal is HTTP 400 / ValidationException with message 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' That text is entirely FIXED for this class — it names no member, value, byte offset, model id or request id, so there is no variable part to fill from the request or from emulator state, and no per-run value (request id, clock time, generated id) participates in the comparison. The other recorded wording, 'Malformed input request, please reformat your input and try again.', belongs to the model families exercised by the already-filed checks (amazon.nova-lite-v1:0 and the Anthropic-shaped bodies), not to this one.

Emulator: status 400 and code ValidationException are both right, but message = 'Malformed input request, please reformat your input and try again.' for moonshotai.kimi-k2.5 — the provider-agnostic sentence, emitted from one shared parse-failure site that is keyed on nothing about `modelId`. The positive control in the same check passes (a well-formed body for the same model is accepted and returns a non-empty JSON payload), so the operation and the model resolve correctly and only the refusal wording diverges.

Why: the error message is a requirement. A client, SDK recovery policy, negative test or support tool keyed on the model provider's malformed-JSON sentence cannot distinguish 'the body could not be parsed for this provider's schema' from any other 400 ValidationException on the same operation (unknown modelId, out-of-range inference parameters, oversized payload, bad guardrail header), and a message assertion or golden file that passes against the emulator fails against AWS. Collapsing every provider onto one sentence also hides that the refusal is produced per model family.

Fix: the InvokeModel handler's body-parse / input-validation stage in the bedrock-runtime service crate — the site that turns a JSON deserialization failure of the @httpPayload `body` into a ValidationException. It must select the refusal sentence from the resolved target model's provider validator rather than returning one hard-coded string for every modelId; the status and code paths already behave.

NOTE on neighbouring open issues (neither carries this check's replay key, and the replay report did not mark this check as owned by either, so neither covers it): 20261109T043951Z-bedrock-runtime-refusal-messages-are-re-worded-by.md and 20261109T201949Z-bedrock-runtime-invokemodel-malformed-body-refusal-carries-an.md both file the SAME parse-failure site for the OTHER model families, and both demand the 'Malformed input request, please reformat your input and try again.' sentence. They are named here because a fix that simply hard-codes that string (which is what the emulator now serves) satisfies those two and still fails this one: the fix must be model-provider-aware, and all three replay keys must pass together.

Traces: bedrock.error-validation-malformed-body-invoke.

## Evidence
Behavioral replay, check bedrock.error-validation-malformed-body-invoke, verdict=mismatch.

Request sequence (same client, same model throughout):
1. Positive control — InvokeModel(modelId='moonshotai.kimi-k2.5', contentType='application/json', body=json of {anthropic_version, max_tokens:1, messages:[{role:user, content:'hi'}]}) succeeded and returned a non-empty JSON body. So the model resolves and the happy path works.
2. InvokeModel(modelId='moonshotai.kimi-k2.5', contentType='application/json', body=b'this is not json{{{').

Observed: error code ValidationException (as expected, so status 400 / code are correct) with
  expected message: 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.'
  actual message:   'Malformed input request, please reformat your input and try again.'
AssertionError at check_bedrock_error_validation_malformed_body_invoke.py line 71: "Expected message 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.', got 'Malformed input request, please reformat your input and try again.'"

Message anatomy: the expected string is 100% fixed text — there is no slot in it for a member name, value, limit, id or timestamp, so nothing in it is drawn from the request or from emulator state and nothing in it is per-run. The comparison is an exact-string requirement, not a per-run value difference.

Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json): InvokeModel http {method: POST, uri: '/model/{modelId}/invoke', code: 200}; errors include ValidationException (errorKind=client, httpError=400); input member `body` target Body, httpPayload=true, @length max 25000000, @sensitive; `modelId` target InvokeModelIdentifier, httpLabel=true, required, @length 1..2048.

## Steps to Reproduce
1. git_clone() the repo and let run_behavioral_suite() build and boot the emulator.
2. Call InvokeModel with modelId='moonshotai.kimi-k2.5', contentType='application/json' and a well-formed JSON body (e.g. {"anthropic_version":"bedrock-2023-05-31","max_tokens":1,"messages":[{"role":"user","content":"hi"}]}) — it is accepted and returns a non-empty JSON payload (positive control).
3. Call InvokeModel again with the same modelId and contentType but body=b'this is not json{{{'.
4. Observe HTTP 400 with x-amzn-errortype ValidationException and $.message = 'Malformed input request, please reformat your input and try again.' where AWS returns $.message = 'The JSON object in the request body is malformed. Fix the syntax errors and retry your request.' for this model.
5. Re-replay behavioral check bedrock.error-validation-malformed-body-invoke to confirm the fix. Also re-replay bedrock-runtime.invoke-model-malformed-json-body-rejected and bedrock-runtime.malformed-request-body-not-forwarded-to-model (the other model families, filed separately) — a fix must keep those passing while making this one pass, i.e. the sentence must be chosen per target model provider rather than hard-coded.
