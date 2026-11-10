Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent.validate-flow-definition-structural-vs-semantic
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent.validate-flow-definition-structural-vs-semantic|run#8|AssertionError|assert message == _UNKNOWN_NODE_TYPE_MESSAGE, message
Issue-Commit: aa75f5d199f9816d8a358732bf51b7bd305abb42

# bedrock-agent ValidateFlowDefinition renders the FlowNodeType enum-value-set ValidationException message in model-declaration order, not AWS's order

> _Note: this issue was identified against the code base at commit `aa75f5d199f9816d8a358732bf51b7bd305abb42`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateFlowDefinition

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: the enum-constraint ValidationException message for FlowNode$type lists the allowed values in a different order than AWS does, so the message a client receives does not match AWS's message for that error class.

AWS: ValidateFlowDefinition takes `definition` (FlowDefinition, required), whose `nodes` members carry `type` targeting the enum `com.amazonaws.bedrockagent#FlowNodeType` (Smithy: FlowNodeType values = Input, Output, KnowledgeBase, Condition, Lex, Prompt, LambdaFunction, Storage, Agent, Retrieval, Iterator, Collector, InlineCode, Loop, LoopInput, LoopController; modelled error ValidationException, HTTP 400, "Input validation failed. Check your request parameters and retry the request."). A definition whose node declares a `type` outside that set is refused structurally, before any semantic graph analysis, with ValidationException and the standard AWS constraint message. Its FIXED text is `1 validation error detected: Value at '<path>' failed to satisfy constraint: Member must satisfy enum value set: [<allowed values>]`; the only REQUEST-derived part is `<path>` (here `definition.nodes.1.member.type`, taken from the offending member's position in the submitted definition). The bracketed value list is FIXED text of the message, and the recording shows AWS emitting it in this order: [LoopController, Output, Loop, Prompt, Agent, LoopInput, InlineCode, Input, Collector, Iterator, KnowledgeBase, Retrieval, Storage, Condition, LambdaFunction, Lex] (recorded behavior only — the model states the value set but not its rendering order).

Emulator: status and error code are correct (HTTP 400 / ValidationException) and the path segment is correct, but the value list is emitted in Smithy declaration order: `1 validation error detected: Value at 'definition.nodes.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [Input, Output, KnowledgeBase, Condition, Lex, Prompt, LambdaFunction, Storage, Agent, Retrieval, Iterator, Collector, InlineCode, Loop, LoopInput, LoopController]`. The set of values is the same; only the order of this fixed text diverges.

Note on prior work: the operation itself is now implemented — the check's positive control (a fully valid definition answered 200 with an EMPTY validations[]) and the semantic case both got past the assertion that fails, and the structural case is refused with the right code. So the fix claimed by the FIXED issue 20261030T000542Z-bedrock-agent-validateflowdefinition-stub.md did land and did NOT leave the call refused; this is a net-new message-rendering cause on top of it, and it is not covered by the open bedrock-agent issues (20261030T000542Z-bedrock-agent-5-net-new-causes-vector-knowledge.md covers VECTOR-knowledge-base/prompt refusal wording, 20261029T172151Z-bedrock-agent-prepareflow-leaves-the-flow-in-notprepared.md covers PrepareFlow state).

Why: a client (SDK, CLI, CFN provider or test harness) that matches on the ValidationException message — or a user reading it — sees a message AWS never produces for this error class. Any assertion or diff on the enum-value-set message text fails against the emulator even though the request is rejected for the right reason, so negative-path conformance of the whole structural-validation surface cannot be reproduced.

Fix: the shared constraint-validation message builder that renders the `Member must satisfy enum value set: [...]` text for this crate (the framework-level enum validation that runs in the deserialize → validate stage, before the ValidateFlowDefinition handler body) must render the enum's values in AWS's order for FlowNodeType rather than iterating the Smithy declaration order. The handler for the operation does not produce this message and needs no change.

Traces: bedrock-agent.validate-flow-definition-structural-vs-semantic (mismatch; AssertionError at the `message == _UNKNOWN_NODE_TYPE_MESSAGE` assertion). Handler/validation surface: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/validate_flow_definition.rs (handler, which only emits FlowValidation findings, lines ~200-580), with the enum message produced upstream of it in the service's generated constraint validation.

## Evidence
Check bedrock-agent.validate-flow-definition-structural-vs-semantic, [mismatch].

Expected (AWS recording), for a node whose type is 'NotARealFlowNodeType' at definition.nodes[1]:
  Code: ValidationException (HTTP 400)
  Message: "1 validation error detected: Value at 'definition.nodes.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [LoopController, Output, Loop, Prompt, Agent, LoopInput, InlineCode, Input, Collector, Iterator, KnowledgeBase, Retrieval, Storage, Condition, LambdaFunction, Lex]"

Actual (emulator):
  Code: ValidationException (HTTP 400)  <- matches
  Message: "1 validation error detected: Value at 'definition.nodes.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [Input, Output, KnowledgeBase, Condition, Lex, Prompt, LambdaFunction, Storage, Agent, Retrieval, Iterator, Collector, InlineCode, Loop, LoopInput, LoopController]"

AssertionError raised at check_bedrock_agent_validate_flow_definition_structural_vs_semantic.py line 61: `assert message == _UNKNOWN_NODE_TYPE_MESSAGE`.

Message anatomy: fixed text = "1 validation error detected: Value at '" ... "' failed to satisfy constraint: Member must satisfy enum value set: [" + the 16 FlowNodeType values in AWS's order + "]"; request-derived part = the member path 'definition.nodes.1.member.type'. The emulator's divergence is entirely in the fixed bracketed list, which it emits in Smithy declaration order (smithy_query shape=FlowNodeType returns exactly that order).

The other two legs of the same check agree with AWS: a fully valid definition answers 200 with validations == [], and the semantically broken but well-formed definition is not refused — so only the enum message text diverges.

## Steps to Reproduce
1. Call bedrock-agent ValidateFlowDefinition with a well-formed definition of two nodes and one Data connection, where nodes[0] is {"name": "FlowInputNode", "type": "NotARealFlowNodeType", "outputs": [{"name": "document", "type": "String"}]} and nodes[1] is a valid Output node.
2. Observe the ValidationException. Status 400 and code ValidationException are correct.
3. Compare Error.Message to AWS's: the bracketed FlowNodeType value list is in Smithy declaration order ([Input, Output, KnowledgeBase, ...]) instead of AWS's order ([LoopController, Output, Loop, Prompt, ...]).
4. Control (passes today, keep it passing): ValidateFlowDefinition with the same definition but nodes[0].type = "Input" answers 200 with validations == [].
5. Replay: bedrock-agent.validate-flow-definition-structural-vs-semantic.
