Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.streaming-collects-full-response bedrock.error-validation-wrong-role-order
Issue-Key: bedrock.streaming-collects-full-response|run#7|AssertionError|assert len(text_parts) > 1, (
Issue-Key: bedrock-runtime|Converse|ValidationException
Issue-Commit: 016c7defff76bf6b624ee66d2926fa081d59376e

# bedrock: 8 net-new data-plane causes — Converse/InvokeModel/ConverseStream generation ignores the prompt, the system prompt, stopSequences, temperature and prior turns; ConverseStream emits one delta; toolUse.input is a placeholder; consecutive user-role messages are refused ValidationException

> _Note: this issue was identified against the code base at commit `016c7defff76bf6b624ee66d2926fa081d59376e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Resolution

Causes 6 and 8 are FIXED, and the half of Cause 4 that needs no model with them; both of their replay keys
pass (`bedrock.streaming-collects-full-response`, `bedrock-runtime|Converse|ValidationException`).

* Cause 8: `Converse` and `ConverseStream` no longer apply a role-ORDER precondition. `Messages` is a plain
  list, `Message.role` a required `ConversationRole` (`user`/`assistant`/`system`) and neither shape
  constrains the sequence the roles may form, so consecutive user turns, a repeated assistant turn and a
  conversation opening on a non-`user` role are all served; the one precondition left is that there is a turn
  to answer (`crate::state::validate_conversation`).
* Cause 6: a streamed content block is delivered as the INCREMENTS a client appends — several
  `contentBlockDelta` events, each carrying that block's own `contentBlockIndex`, which reassemble to exactly
  what `Converse` answers with (`src/state/stream_framing.rs`, used for both text blocks and a tool
  request's input document).
* Cause 4, in part: a turn whose generation ended because a configured stop sequence completed now reports
  the modelled `stopReason` for that cause, `stop_sequence`, instead of `end_turn`.

Causes 1, 2, 3, 5, 7 and the generation half of Cause 4 are NOT fixed and are NOT closed: they are the
platform blocker this suite keeps rediscovering — NOTHING in this emulator can run a language model (no
in-process model runtime, no inference engine crate in the tree, no weights and no external process), and a
generation stage that instead COMPUTES the text from the request was implemented once before and refused at
the merge gate as output synthesised to look computed. Their seven replay keys now live on the open report
that owns that defect,
`issues/things-that-do-not-work/20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md`
(whose Cause 1 is the constant completion and whose Cause 2 is the self-referential `toolUse.input`), which
this change added them to. They stay FILED there until the platform has a model runtime.

## Operation
Converse

## Description
This issue bundles 8 DISTINCT net-new root causes under the suite's `bedrock` service heading, all on the bedrock-runtime inference surface (Converse / ConverseStream / InvokeModel). A fixer must close ALL EIGHT before submitting; each cause names its own failing check ids. Model consulted: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json. The emulator answered every call below (HTTP 200, or a modeled 400 for Cause 8), so the service is up and answering wrongly, not unbootstrapped.

RELATED OPEN ISSUES (named, not covering these checks): 20261029T171601Z-bedrock-converse-serves-a-constant-notice-string-inline.md (open) and 20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md (open) describe the constant-notice generation defect for OTHER check ids; neither carries any replay key listed here, so Causes 1-6 add those keys. Causes 4 and 7 are regressions of the shape earlier fixed issues claimed: 20261027T220631Z-bedrock-runtime-converse-conversestream-ignore-inferenceconfig-stopsequences.md and 20261026T054901Z-bedrock-runtime-converse-conversestream-tooluse-block-carries-an-empty-input.md. Those checks fail on a 200 response with wrong content, not on a refusal, so they are filed here as current divergences rather than as reopened refusals.

---
Cause 1 of 8: the generated completion is a constant notice string, so a single-turn instruction is never followed.
AWS: Smithy InvokeModel POST /model/{modelId}/invoke -> 200 with @required `body` (Body, @httpPayload, @sensitive) and `contentType`; the generated text itself is not described by the model, so recorded behavior only: InvokeModel on moonshotai.kimi-k2.5 with a prompt instructing the model to say exactly one word returns a body whose assistant text contains that word (the word is a per-request value, never a literal).
Emulator: HTTP 200 whose text is 'No language model runs in this emulator. <modelId> generated no text for this request. The request carried N message(s) and M input token(s).' — the fixed sentence is the whole string; only <modelId>, N and M vary, all taken from the request.
Why: every client that reads the completion (echo, extraction, instruction-following, agent loops) cannot be exercised at all: a correct client and one that drops the prompt produce identical bytes here.
Fix: the generation stage behind InvokeModel/Converse/ConverseStream in the bedrock-runtime frontend handler — the response text must be derived from the request's own messages.
Traces: bedrock.invoke-model-basic.

---
Cause 2 of 8: prior conversation turns carried in `messages` are ignored, so a value stated in an earlier turn is never recalled.
AWS: Smithy Converse.messages -> Messages (list of Message, each with @required role/content); recorded behavior only: a 3-message conversation whose first user turn states a value and whose last turn asks for it back returns an assistant text containing that value (a per-request value, not a literal).
Emulator: HTTP 200 with the Cause-1 notice text, which reports only the message and token counts ('... carried 3 message(s) and 68 input token(s).'); the content of the prior turns is never used.
Why: multi-turn context assembly (the core of every chat client) is untestable: a client that silently drops prior turns gets the same response as a correct one.
Fix: same generation stage — it must consume the full `messages` list, not just its length.
Traces: bedrock.multi-turn-conversation.

---
Cause 3 of 8: Converse's `system` block has no effect on the completion.
AWS: Smithy Converse.system -> SystemContentBlocks (list of SystemContentBlock, a union whose `text` member is NonEmptyString @length min 1) and is documented as 'instructions to provide the model for how to handle input'. Recorded behavior only: a system prompt instructing the model to include a marker token yields a completion containing that marker (the marker is a per-request value).
Emulator: HTTP 200 with the Cause-1 notice text; the marker never appears and the system block is not reflected anywhere in the response.
Why: a client whose behavior depends on system-prompt steering (persona, format contracts, guard instructions) cannot verify that the system block is even transmitted.
Fix: same generation stage — it must take the `system` content blocks into account when producing the text.
Traces: bedrock.converse-system-prompt.

---
Cause 4 of 8: inferenceConfig.stopSequences never truncates the completion — there is no text to truncate.
AWS: Smithy InferenceConfiguration.stopSequences -> NonEmptyStringList @length max 2500, documented as 'a sequence of characters that causes the model to stop generating the response'; StopReason enum includes `stop_sequence`. Recorded behavior only: on us.meta.llama4-scout-17b-instruct-v1:0 a prompt that would emit an enumerated sequence, with a stop sequence set, returns the text generated BEFORE that sequence (and nothing after it), so content preceding the stop sequence is present in the completion.
Emulator: HTTP 200 with the Cause-1 notice text, i.e. no generated content at all, so the check cannot observe a truncation. A fixed issue (20261027T220631Z) claimed this behavior; it is currently not observable.
Why: a client relying on stop sequences to delimit generated output (JSON fences, tool delimiters, turn boundaries) cannot distinguish 'stop sequence honored' from 'nothing generated'.
Fix: the generation stage plus the stop-sequence post-processing in the Converse/ConverseStream handlers — truncate the derived text at the first configured stop sequence and report stopReason='stop_sequence'.
Traces: bedrock.bedrock-converse-stop-sequence-truncates-the-generated-text, bedrock.inference-params-stop-sequences.

---
Cause 5 of 8: inferenceConfig.temperature has no effect: the response is byte-identical at temperature 0.0 and 1.0.
AWS: Smithy InferenceConfiguration.temperature -> Float @range {min:0,max:1}, documented as the likelihood of the model selecting higher-probability options. Recorded behavior only: repeated identical Converse calls at temperature=1.0 produce MORE distinct completions than the same prompt at temperature=0.0 (which is near-deterministic); the specific texts are per-run values.
Emulator: 5 calls at temperature=0.0 and 5 at temperature=1.0 all return the same Cause-1 notice string — 1 distinct output in each arm, so sampling is unobservable.
Why: a client that tunes temperature, or a test that asserts determinism at temperature 0 versus variety above it, gets no signal from the emulator.
Fix: the generation stage — the derived text must vary with the sampling parameters it is given (deterministic at 0.0, varying at 1.0).
Traces: bedrock.inference-params-temperature.

---
Cause 6 of 8: ConverseStream delivers the whole response as a single contentBlockDelta event.
AWS: Smithy ConverseStream POST /model/{modelId}/converse-stream -> 200 with an event stream (`stream`, @httpPayload @streaming) carrying ContentBlockDeltaEvent (@required delta + contentBlockIndex). Recorded behavior only: a Converse-stream call returns the completion as MULTIPLE contentBlockDelta events whose concatenation is the full text.
Emulator: exactly one contentBlockDelta is emitted, carrying the Cause-1 notice string in one chunk.
Why: a streaming client's incremental-assembly path (partial render, chunk accounting, cancellation mid-stream) is never exercised; a client that only ever reads the first delta passes here and truncates in production.
Fix: the ConverseStream handler's event-stream emitter — chunk the generated text across several ContentBlockDeltaEvents with increasing indexes.
Traces: bedrock.streaming-collects-full-response.

---
Cause 7 of 8: the toolUse block's `input` document is a placeholder that maps each declared property name to its own name instead of to the value from the prompt.
AWS: Smithy ToolUseBlock -> @required toolUseId, name and `input` (Document), documented as 'the input to pass to the tool'; StopReason includes `tool_use`. Recorded behavior only: a Converse call with a toolConfig declaring a one-property tool and a prompt naming a concrete value returns a toolUse block whose input maps that property to the value taken FROM THE PROMPT (the property name is from the tool schema, the value is from the request).
Emulator: the input document is {'<property>': '<property>'} — the member name echoed as its own value (observed: {'symbol': 'symbol'}), so the tool is invoked with a nonsense argument and the round-trip answer cannot carry a tool result.
Why: an agent loop built on the emulator runs its tool with the schema's own key as the argument; argument extraction, validation and the tool-result round trip are all untestable, and a client that never reads the value passes here.
Fix: the tool-use synthesis stage of the Converse/ConverseStream handlers — populate ToolUseBlock.input from the request's messages against the declared inputSchema rather than from the schema's property names.
Traces: bedrock.bedrock-tool-use-round-trip-returns-the-tool-result-in-the-final-answer.

---
Cause 8 of 8: Converse REFUSES two consecutive user-role messages with 400 ValidationException, where AWS accepts them.
AWS: Smithy Converse.messages -> Messages, Message.role -> ConversationRole @required; the model states NO alternation constraint, and Message's documentation constrains only content composition (images/documents per role), not role ordering. Recorded behavior: Converse on moonshotai.kimi-k2.5 with messages=[{user,'first'},{user,'second'}] is ACCEPTED with HTTP 200 and returns output.message.role='assistant' with non-empty content and a stopReason — role alternation is not enforced by the service.
Emulator: the call is refused with Error.Code='ValidationException' (the modeled 400 client error); the refusal message text is not surfaced by the replay harness, which reports only the code, so the whole response is wrong in status and code regardless of wording.
Why: this is an over-strict validation gate: a client that legitimately appends consecutive user turns (user sends two messages before the assistant replies, or a retry re-appends the pending turn) works in production and is hard-failed by the emulator.
Fix: the Converse/ConverseStream handler's input-validation stage — drop the role-alternation precondition and serve the request.
Traces: bedrock.error-validation-wrong-role-order.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 142/196 passed; service heading `bedrock` 142/191 (74%), 49 failed. bedrock is bootstrapped (get_bootstrap_status().bootstrapped contains service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice and service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice, and `pending` is empty), and every call below was answered.

Cause 1 — bedrock.invoke-model-basic [mismatch]
  Expected (AWS): the completion contains the single word the prompt asked for.
  Actual: AssertionError: Asked the model to say exactly 'hello', got: 'No language model runs in this emulator. moonshotai.kimi-k2.5 generated no text for this request. The request carried 1 message(s) and 6 input token(s).'

Cause 2 — bedrock.multi-turn-conversation [mismatch]
  Expected (AWS): the final reply recalls the value stated in the first turn.
  Actual: AssertionError: Expected 'banana' in response, got: 'no language model runs in this emulator. moonshotai.kimi-k2.5 generated no text for this request. the request carried 3 message(s) and 68 input token(s).'

Cause 3 — bedrock.converse-system-prompt [mismatch]
  Expected (AWS): the completion contains the marker the system prompt required.
  Actual: AssertionError: system prompt did not influence the response: expected marker SP-MARKER-<nonce> in 'No language model runs in this emulator. moonshotai.kimi-k2.5 generated no text for this request. The request carried 1 message(s) and 46 input token(s).'

Cause 4 — bedrock.bedrock-converse-stop-sequence-truncates-the-generated-text, bedrock.inference-params-stop-sequences [mismatch]
  Expected (AWS): the text generated before the stop sequence is present (the check requires the enumerated items preceding it).
  Actual (both checks): AssertionError: the model must have generated the text BEFORE the stop sequence -- an empty or immediately-stopped completion is not a truncation; got 'No language model runs in this emulator. meta.llama4-scout-17b-instruct-v1:0 generated no text for this request. The request carried 1 message(s) and 31 input token(s).'

Cause 5 — bedrock.inference-params-temperature [mismatch]
  Expected (AWS): temperature=1.0 yields more distinct completions across 5 identical calls than temperature=0.0.
  Actual: AssertionError: temperature=1.0 ... gave 1 distinct outputs across 5 calls versus 1 distinct at temperature=0.0 -- temperature appears to have no effect on generation. Both arms returned 'No language model runs in this emulator. moonshotai.kimi-k2.5 generated no text for this request.' five times.

Cause 6 — bedrock.streaming-collects-full-response [mismatch]
  Expected (AWS): the response arrives as multiple contentBlockDelta chunks.
  Actual: AssertionError: expected the response to arrive as multiple contentBlockDelta chunks, got 1: ['No language model runs in this emulator. moonshotai.kimi-k2.5 generated no text for this request. The request carried 1 message(s) and 18 input token(s).']

Cause 7 — bedrock.bedrock-tool-use-round-trip-returns-the-tool-result-in-the-final-answer [mismatch]
  Expected (AWS): toolUse.input maps the tool's declared property to the value named in the prompt.
  Actual: AssertionError: the toolUse input must carry the symbol from the prompt -- an emulator returning an empty input dict fails here; got {'symbol': 'symbol'} (the property name used as its own value).

Cause 8 — bedrock.error-validation-wrong-role-order [mismatch]
  Request: Converse(modelId='moonshotai.kimi-k2.5', messages=[{'role':'user','content':[{'text':'first'}]},{'role':'user','content':[{'text':'second'}]}]). The check's positive control (single-turn Converse with maxTokens=1 on the same model) succeeded first, so the model is usable and the refusal is specific to the message sequence.
  Expected (AWS): HTTP 200 with output.message.role='assistant', non-empty content and a stopReason.
  Actual: AssertionError: Expected consecutive user-role messages to be accepted, got 'ValidationException' (400).

Model evidence: smithy_query query=operation Converse,ConverseStream,InvokeModel and query=shape InferenceConfiguration,ToolUseBlock,SystemContentBlock,ContentBlockDeltaEvent,Message,StopReason on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json — Converse POST /model/{modelId}/converse -> 200 (@required output/stopReason/usage/metrics, ValidationException errorKind client httpError 400); ConverseStream POST /model/{modelId}/converse-stream -> 200 event stream; InvokeModel POST /model/{modelId}/invoke -> 200 with @required body/contentType; InferenceConfiguration.temperature/topP Float @range {min:0,max:1}, stopSequences NonEmptyStringList @length max 2500; ToolUseBlock.input Document @required; SystemContentBlock union with text NonEmptyString; ContentBlockDeltaEvent @required delta+contentBlockIndex; Message.role ConversationRole @required with no alternation constraint; StopReason includes stop_sequence and tool_use.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite so the emulator is built and booted.
2. Re-replay exactly the nine check ids in trace_ids.
3. Cause 1: bedrock-runtime.invoke_model(modelId='moonshotai.kimi-k2.5', body=json({...'Say exactly hello'...})) — expected 'hello' in the decoded body text; observed the constant 'No language model runs in this emulator. ...' notice.
4. Cause 2: send a 3-message Converse conversation whose first user turn states a value and whose final turn asks for it back — expected the value recalled; observed the same notice, which reports only message and token counts.
5. Cause 3: Converse with system=[{'text':'<instruction to include marker X>'}] — expected X in the completion; observed the notice.
6. Cause 4: Converse on 'us.meta.llama4-scout-17b-instruct-v1:0' with inferenceConfig.stopSequences set and a prompt that would enumerate items — expected the items preceding the stop sequence in the text and stopReason='stop_sequence'; observed the notice.
7. Cause 5: issue 5 identical Converse calls at temperature=0.0 and 5 at temperature=1.0 — expected more distinct texts in the 1.0 arm; observed 1 distinct text in each.
8. Cause 6: bedrock-runtime.converse_stream(...) and count contentBlockDelta events — expected >1; observed exactly 1.
9. Cause 7: Converse with a toolConfig declaring a one-property tool and a prompt naming a concrete value — expected toolUse.input to carry that value under the declared property; observed {'<property>': '<property>'}.
10. Cause 8: Converse(modelId='moonshotai.kimi-k2.5', messages=[{user,'first'},{user,'second'}]) — expected HTTP 200 with an assistant message and a stopReason; observed 400 ValidationException. Confirm the single-turn control on the same model still succeeds.
