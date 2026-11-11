Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation
Issue-Key: bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation|run#17|AssertionError|assert blocked["stopReason"] == "guardrail_intervened", (
Issue-Commit: f0242a1930b25bb78bdea7a3bf1c4ec4cb6cc2ae

# bedrock__bedrock-runtime Converse: an output-side guardrail never intervenes on the completion — stopReason stays end_turn, blockedOutputsMessaging is never substituted and trace.guardrail reports no modelOutput or word finding

> _Note: this issue was identified against the code base at commit `f0242a1930b25bb78bdea7a3bf1c4ec4cb6cc2ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
Cause 1 of 1: Converse applies no guardrail policy to the text it generates, because the generation stage serves a fixed emulator notice instead of a model completion; an output-side word policy therefore can never match.
AWS: Smithy (com.amazonaws.bedrockruntime) Converse -> ConverseResponse.stopReason is a StopReason enum whose values include 'end_turn', 'guardrail_intervened', 'content_filtered'. When guardrailConfig={guardrailIdentifier, guardrailVersion, trace:'enabled'} names a guardrail whose wordPolicyConfig.wordsConfig blocks a word, and the MODEL's completion contains that word, AWS evaluates the completion after generation and returns stopReason='guardrail_intervened' with output.message.content replaced byte-for-byte by the guardrail's configured blockedOutputsMessaging (not blockedInputMessaging), with usage.inputTokens>0 and usage.outputTokens>0 because the model did run, and with trace.guardrail carrying modelOutput (the suppressed completion) and outputAssessments[<guardrailId>] whose wordPolicy.customWords entry has match=<the configured blocked word>, action='BLOCKED', detected=true, while the inputAssessment for the same guardrail reports no word finding because the prompt never contained the word (recorded behavior for the trace members; the member names and stopReason value are modeled).
Emulator: the same Converse call returns stopReason='end_turn' and output text 'No language model runs in this emulator.' — a constant string that contains neither the model's real answer nor the blocked word, so the output-side word policy never fires: no substitution of blockedOutputsMessaging, no 'guardrail_intervened', and no modelOutput/word finding in trace.guardrail for the blocked turn. The preceding control in the same check (a prompt whose answer does not contain the blocked word) passes, including a non-empty trace.guardrail.outputAssessments, so the guardrail is wired into the response envelope while the content it is supposed to police is synthetic.
Why: a client that relies on an output-side guardrail to suppress a forbidden completion sees the turn complete normally; its 'is this answer safe' branch keyed on stopReason=='guardrail_intervened' is dead code, and the text it forwards to its user was never policed. Nothing in the response distinguishes 'the guardrail passed the completion' from 'the completion was never generated or evaluated'.
Fix: the bedrock-runtime Converse handler's generation stage must produce a completion derived from the request (so content policies have real text to evaluate), and its guardrail stage must evaluate that completion on the OUTPUT side, set stopReason='guardrail_intervened', substitute the guardrail's blockedOutputsMessaging into output.message.content, and populate trace.guardrail.modelOutput and outputAssessments[<guardrailId>].wordPolicy.customWords for the matched word.
Traces: bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation.

No open issue carries this check's replay key. The constant-generation root cause is described from other recordings by open issues 20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md and 20261029T171601Z-bedrock-converse-serves-a-constant-notice-string-inline.md (inline guardrails never applied), and 20261025T164541Z-bedrock-bedrock-runtime-converse-ignores-guardrailconfig-trace-enabled-the-200-response.md covers a missing `trace` member — none of them is grounded in this output-side-after-generation check, and the trace member is present here.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replay: 79/153 passed. Listed under '### bedrock__bedrock-runtime', marked neither 'ALREADY FILED' nor 'OWNED by':

[mismatch] bedrock__bedrock-runtime.bedrock-runtime-guardrail-blocks-the-model-output-after-generation
  AssertionError: a completion containing the blocked word must stop with 'guardrail_intervened'; got 'end_turn' text='No language model runs in this emulator.'

Setup in the check: create_guardrail(wordPolicyConfig={'wordsConfig': [{'text': <blocked word>}]}, blockedInputMessaging=<in-msg>, blockedOutputsMessaging=<out-msg>), wait READY, create_guardrail_version, wait READY; converse(modelId=<a Nova text model>, guardrailConfig={guardrailIdentifier, guardrailVersion, trace:'enabled'}, inferenceConfig={'maxTokens':20,'temperature':0.0}) with a prompt whose correct answer IS the blocked word (the word appears nowhere in the prompt, so only the OUTPUT side can trigger).

Expected (AWS): stopReason='guardrail_intervened'; the concatenated output.message.content text equals the configured blockedOutputsMessaging exactly and does not contain the blocked word; usage.inputTokens>0 and usage.outputTokens>0; trace.guardrail.modelOutput contains the suppressed completion (which contains the blocked word); trace.guardrail.outputAssessments[<guardrailId>][*].wordPolicy.customWords contains {match: <blocked word>, action: 'BLOCKED', detected: true}; trace.guardrail.inputAssessment[<guardrailId>].wordPolicy has no customWords finding.
Got (emulator): stopReason='end_turn' and output text 'No language model runs in this emulator.' — generated text is a constant, so no output-side policy can match. The check's earlier control turn (answer without the blocked word) passed with stopReason='end_turn' and a non-empty outputAssessments, so the divergence is the generated content and its output-side evaluation, not the trace envelope.

Model citation: com.amazonaws.bedrockruntime#StopReason = [end_turn, tool_use, max_tokens, stop_sequence, guardrail_intervened, content_filtered, malformed_model_output, malformed_tool_use, model_context_window_exceeded].

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. bedrock.create_guardrail(name=..., blockedInputMessaging=<IN>, blockedOutputsMessaging=<OUT>, wordPolicyConfig={'wordsConfig': [{'text': <WORD>}]}); poll get_guardrail until status=='READY'; create_guardrail_version and poll that version until READY.
3. bedrock-runtime.converse(modelId=<a Nova text model>, guardrailConfig={'guardrailIdentifier': <id>, 'guardrailVersion': <version>, 'trace': 'enabled'}, messages=[{'role':'user','content':[{'text': <a prompt whose only correct answer is <WORD>, with <WORD> absent from the prompt>}]}], inferenceConfig={'maxTokens': 20, 'temperature': 0.0}).
4. Observe stopReason='end_turn' and output text 'No language model runs in this emulator.': AWS returns stopReason='guardrail_intervened' with the content replaced by <OUT>, usage.outputTokens>0, trace.guardrail.modelOutput holding the suppressed completion and outputAssessments[<id>] naming <WORD> with action='BLOCKED', detected=true.
5. Control: repeat step 3 with a prompt whose answer does not contain <WORD> (passes today) and without guardrailConfig (AWS answers with <WORD>; the emulator serves the same constant).
