Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.converse-stop-sequence-halts-generation
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.converse-stop-sequence-halts-generation|run#3|AssertionError|assert resp["stopReason"] == "end_turn", f"stopReason={resp['stopReason']!r} text={resp['output']['message']['content']!r}"
Issue-Commit: 1e62a847eb19bbf4a3d65677374795e0b4d56798

# bedrock-runtime Converse reports stopReason 'stop_sequence' where Nova reports 'end_turn' when a configured stop sequence halts the turn

> _Note: this issue was identified against the code base at commit `1e62a847eb19bbf4a3d65677374795e0b4d56798`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
ONE net-new cause on the bedrock-runtime Converse stop-reason mapping.

Cause 1 of 1: a turn whose generation ends at a configured inferenceConfig.stopSequences entry is reported with stopReason 'stop_sequence' instead of the value the Amazon Nova models report, 'end_turn'. Failing check: bedrock-runtime.converse-stop-sequence-halts-generation.

AWS: Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json) models Converse's required output member `stopReason` -> StopReason enum with values end_turn, tool_use, max_tokens, stop_sequence, guardrail_intervened, content_filtered, malformed_model_output, malformed_tool_use, model_context_window_exceeded, and InferenceConfiguration.stopSequences as 'A list of stop sequences. A stop sequence is a sequence of characters that causes the model to stop generating the response.' The model, not the service envelope, decides WHICH enum member a halted turn carries, and the model does not word it, so for the value itself this is recorded behavior only: a Converse call to amazon.nova-lite-v1:0 with inferenceConfig.stopSequences=['7'] returns HTTP 200 whose generated text stops at the stop sequence and whose stopReason is 'end_turn'. The requirement is that rule — for this model family a stop-sequence halt is reported as end_turn, and the text ends at (and does not include past) the stop sequence.

Emulator: the same call returns 200 with stopReason 'stop_sequence'. The halt is computed in the shared generation stage: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/state/generation.rs lines 105-117 set EmulatedCompletion.halted_at_stop_sequence=true whenever halt_at_stop_sequence() matches, and the Converse handler maps that flag straight onto StopReason::stop_sequence for every model id. Observed in the replay: stopReason='stop_sequence' with text=[{'text': 'No language model runs in this emulator. amazon.nova-lite-v1:0 generated no text for this request. The request carried 1 message(s) and 37'}] — the halt fired on the '7' of '37'.

Why: a client that branches on stopReason (retry/continue loops, agent drivers, 'was my turn cut short?' telemetry, assertions in SDK test suites) takes the wrong branch against this emulator for every Nova turn that ends on a stop sequence, and the divergence is invisible because the status is 200 either way. Code written and validated against the emulator then behaves differently against real Bedrock.

Fix: the stop-reason selection in the Converse/ConverseStream/InvokeModel generation path — the mapping from EmulatedCompletion.halted_at_stop_sequence onto the served StopReason (per-model, since the enum member a halted turn reports is a model-family property, not a protocol constant). The text content this check also asserts on (text ending exactly at the stop sequence, carrying the pre-stop output) cannot be satisfied while the generation stage serves the constant 'No language model runs in this emulator.' notice; that part is the platform blocker already filed as 20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md (that issue's keys are all content assertions and none of them is this stopReason assertion, so it does not cover this cause). This cause is the enum value alone and is fixable independently of it.

Traces: bedrock-runtime.converse-stop-sequence-halts-generation.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check bedrock-runtime.converse-stop-sequence-halts-generation [mismatch]:

  AssertionError: stopReason='stop_sequence' text=[{'text': 'No language model runs in this emulator. amazon.nova-lite-v1:0 generated no text for this request. The request carried 1 message(s) and 37'}]
  (assert resp["stopReason"] == "end_turn")

Request replayed: converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Output exactly the following, separated by single spaces and nothing else, no punctuation, no preamble: count from 1 to 100'}]}], inferenceConfig={'maxTokens':400,'temperature':0,'stopSequences':['7']}).

Expected (recorded AWS): HTTP 200, stopReason 'end_turn', generated text halted at the stop sequence '7' and never reaching 100.
Observed (emulator): HTTP 200, stopReason 'stop_sequence'.

Fixed text vs variable parts: the diverging value is the enum member itself — fixed text 'stop_sequence' served where 'end_turn' is required; the model id ('amazon.nova-lite-v1:0'), the stop sequence ('7') and the message/token counts in the served text are values taken from the request and the emulator's own state, not part of the requirement.

Model evidence (smithy_query, shape StopReason / InferenceConfiguration on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json): StopReason values = [end_turn, tool_use, max_tokens, stop_sequence, guardrail_intervened, content_filtered, malformed_model_output, malformed_tool_use, model_context_window_exceeded]; InferenceConfiguration.stopSequences -> NonEmptyStringList @length max 2500.

Emulator source: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/state/generation.rs:42-47 (halted_at_stop_sequence, documented as 'what a turn reports as the CAUSE of its stop') and :105-117 (the unconditional halt branch).

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay bedrock-runtime.converse-stop-sequence-halts-generation.
3. Directly: brt.converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Output exactly the following, separated by single spaces and nothing else, no punctuation, no preamble: count from 1 to 100'}]}], inferenceConfig={'maxTokens':400,'temperature':0,'stopSequences':['7']}).
4. Read resp['stopReason']. Expected 'end_turn' (AWS/Nova for a stop-sequence halt); observed 'stop_sequence'.
5. Any stopSequences value that occurs in the served text reproduces it — e.g. stopSequences=['emulator'] on the same call also yields stopReason='stop_sequence'.
