Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-runtime-converse-error-paths
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-runtime-converse-error-paths
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime|Converse|4xx->2xx|$.message
Issue-Key: bedrock-runtime|Converse|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: bedrock-runtime|Converse|4xx->2xx|status
Issue-Commit: f5be510438d0c6d7c76de043cac8125e7ef395f2

# bedrock-runtime Converse accepts a conversation whose first message is not from the user and answers 200 instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `f5be510438d0c6d7c76de043cac8125e7ef395f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
One net-new cause in bedrock-runtime's Converse validation: a message list that does not begin with a `user` turn is a request AWS refuses, and the emulator runs inference on it and returns a normal 200 inference envelope.

Cause 1 of 1: Converse does not enforce the conversation-ordering rule on ConverseRequest.messages, so a request whose first Message.role is `assistant` is accepted (fixture bedrock-runtime-converse-error-paths, step task#2).

AWS: Smithy com.amazonaws.bedrockruntime#Converse -- POST /model/{modelId}/converse, input ConverseRequest.messages targets the list Messages of Message, whose `role` member is @required and targets the enum ConversationRole (values user, assistant, system). Converse models ValidationException with errorKind=client and @httpError 400 ("The input fails to satisfy the constraints specified by Amazon Bedrock"); its `message` member targets NonBlankString. The ordering requirement -- a Converse conversation must open with a `user` turn -- is not expressible as a Smithy constraint, so it is enforced in the service's request validation; for that rule the recording is the authority (recorded behavior only). Recorded response: HTTP 400, header x-amzn-errortype = "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/", body $.message = "A conversation must start with a user message. Try again with a conversation that starts with a user message." That sentence is entirely fixed text -- it names no member path, index, role or model id -- so there is no variable part to fill from the request or from emulator state, and any deviation in wording is itself the defect.

Emulator: HTTP 200 with a full successful Converse envelope. No error is raised at all: $.message is absent and the x-amzn-errortype header is absent, i.e. the ordering rule is simply not checked and the request proceeds to the generation stage.

Why: a client relying on AWS's behavior treats a 200 from Converse as "the model saw this conversation and replied". Because the emulator answers 200, an agent loop that has mis-assembled its turn history (a dropped first user turn, an assistant turn replayed at the head of the list, a reordered transcript) gets a plausible-looking reply instead of the service's refusal, so the bug is invisible in the emulator and surfaces only against real Bedrock. Conversely a negative test that asserts the 400/ValidationException for this ordering rule passes against AWS and fails against the emulator, and error handlers that branch on x-amzn-errortype never see the header. This is a silent-acceptance divergence (400 -> 200), the most damaging direction, which is why it is HIGH rather than a message-wording defect.

Fix: Converse's input-validation stage -- the ordering/role check over ConverseRequest.messages in the operation's validation, which must run before any generation or state work and raise ValidationException (400) with the service's fixed sentence and the ValidationException error type on the x-amzn-errortype header. The open issue 20261029T213834Z-bedrock-runtime-error-paths-diverge-wrong-status-and.md covers sibling 4xx->2xx and re-wording causes on async-invoke and guardrail paths, and 20261027T061233Z-bedrock-runtime-synchronous-and-async-inference-input-validation.md covers over-permissive empty Message.content / out-of-range inference parameters; neither carries this replay key or the message-ordering rule, so this cause is filed separately and will not be closed by either.

Traces: bedrock-runtime-converse-error-paths#task#2 (Converse, verdict=mismatch).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout, verdict=mismatch:

fixture bedrock-runtime-converse-error-paths, step task#2, operation Converse, service bedrock-runtime
  divergence_key: service=bedrock-runtime|Converse|status=400->200|keys=$.message,$headers.x-amzn-errortype,status
  status: expected 400, got 200
  $.message: missing (expected "A conversation must start with a user message. Try again with a conversation that starts with a user message.", got None)
  $headers.x-amzn-errortype: missing (expected "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/", got None)
  stub_path: (empty -- the operation is implemented, it is the validation that is missing)
  owned_by: (empty -- no open issue carries this replay key)

Expected (AWS, from the recording): 400 / ValidationException / fixed message text above.
Actual (emulator): 200 with a successful Converse response envelope and no error type header.

Message breakdown: fixed text = the whole sentence "A conversation must start with a user message. Try again with a conversation that starts with a user message."; variable parts = none.

Smithy (services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json):
  Converse: http POST /model/{modelId}/converse, code 200; errors include ValidationException (client, httpError 400), AccessDeniedException(403), ModelErrorException(424), ResourceNotFoundException(404), ThrottlingException(429), InternalServerException(500), ServiceUnavailableException(503).
  ConverseRequest.messages -> Messages (list of Message); Message.role @required -> ConversationRole enum [user, assistant, system]; Message.content @required.
  ValidationException.message -> NonBlankString, pattern ^[\s\S]*$.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from that checkout.
2. Replay the wire fixture bedrock-runtime-converse-error-paths and inspect step task#2: it POSTs to /model/{modelId}/converse with a `messages` list whose first element has role `assistant` (no leading `user` turn).
3. Observe the emulator answers HTTP 200 with a complete Converse envelope (output/stopReason/usage/metrics) and no x-amzn-errortype header.
4. Expected, per the recording: HTTP 400 with header x-amzn-errortype = "ValidationException:http://internal.amazon.com/coral/com.amazon.bedrock/" and body $.message = "A conversation must start with a user message. Try again with a conversation that starts with a user message."
5. Equivalent by hand: send any Converse request whose messages[0].role is `assistant` (or `system`) to the emulator's data listener and compare the status, the x-amzn-errortype header and $.message against the above.
