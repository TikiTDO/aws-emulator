Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-flow-validate-definition
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-flow-validate-definition
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|ValidateFlowDefinition|2xx->2xx|$.validations[*]
Issue-Commit: 87807d9b7479bf5c1eafe07a63e234f45d99140a

# bedrock-agent ValidateFlowDefinition suppresses graph-level findings and rewords UnknownConnectionTarget

> _Note: this issue was identified against the code base at commit `87807d9b7479bf5c1eafe07a63e234f45d99140a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateFlowDefinition

## Description
One cause (Cause 1 of 1).

Cause 1 of 1: for a definition whose connection names a target node the definition does not declare, ValidateFlowDefinition reports a SINGLE finding, with emulator-invented message text, where AWS reports the full set of findings for that definition — the unresolved-target Error plus the graph-level Warnings that the unresolved target implies.

AWS: ValidateFlowDefinition (Smithy: POST /flows/validate-definition, @readonly, input definition: FlowDefinition required/sensitive, output validations: FlowValidations required @length max 100) answers 200 with one FlowValidation per defect found in the submitted definition. FlowValidation carries message (NonBlankString, required), severity (FlowValidationSeverity, required, values Warning|Error), type (FlowValidationType) and the typed details union member for that type (FlowValidationDetails). The modelled FlowValidationType value set includes UnfulfilledNodeInput, UnknownConnectionTarget and UnreachableNode, and FlowValidationDetails declares the corresponding unfulfilledNodeInput / unknownConnectionTarget / unreachableNode members — so all three are findings AWS is modelled to report. The model does not state which findings co-occur, nor their rendering order or message text; the recording shows (recorded behavior only) that a definition with one unresolvable connection target produces THREE findings for the same definition, in this order: (1) type UnfulfilledNodeInput, severity Warning, details.unfulfilledNodeInput {node, input}, message "The input '<input>' in node '<node>' is undefined. Connect a source value to this input."; (2) type UnknownConnectionTarget, severity Error, details.unknownConnectionTarget {connection}, message "The connection between the nodes '<source>' and '<target>' references an unknown target node '<target>'. Update the connection to reference a valid node that exists in the flow, or add the missing node to the flow."; (3) type UnreachableNode, severity Warning, details.unreachableNode {node}, message "The node '<node>' isn't connected to any other nodes. Attach this node to the flow or delete it." In each of those messages the quoted node name, input name and connection name are the only REQUEST-derived parts (taken from the submitted definition's node/input/connection names); all the surrounding text, the severity and the type are fixed for the error class. Notably AWS does NOT treat an unresolved connection target as making the graph undecidable: it still reports the unfulfilled input and the unreachable node that follow from the connection not landing.

Emulator: the same request answers 200 but with validations of length 1 — only the UnknownConnectionTarget finding, and its message is rewritten as "Connection <connection> ends at node <target>, which the flow doesn't define." (its severity Error, type UnknownConnectionTarget and details.unknownConnectionTarget {connection} are correct). The UnfulfilledNodeInput and UnreachableNode findings are absent entirely. The cause is visible in the handler: the graph-property pass is gated on every connection resolving, so one unresolved reference suppresses reachability and node-input fulfilment for the whole definition, and the per-finding message builders use emulator wording rather than AWS's fixed text.

Why: ValidateFlowDefinition exists so a client can pre-flight a flow before CreateFlow/UpdateFlow/PrepareFlow. A client that gates on the finding set (count, types, severities, or details members) sees a definition with three defects reported as having one, so warnings AWS surfaces — an input left with no source, a node left detached — are invisible and the flow is treated as closer to valid than it is; and a client or user reading the Error finding gets message text AWS never emits, so any message match or diff on the unresolved-target class fails.

Fix: the ValidateFlowDefinition handler's semantic validator — the `validate_definition` pass and the graph pass it gates (services/.../src/ops/validate_flow_definition.rs). The gate that skips the graph checks when a connection does not resolve must not suppress the node-input-fulfilment and reachability findings, and the message strings those findings and the unresolved-target finding are built from must carry AWS's fixed text with only the node/input/connection names substituted from the request. This is NOT the cause already filed as 20261030T065936Z-bedrock-agent-validateflowdefinition-renders-the-flownodetype-enum-value.md, which is about the upstream ValidationException enum-value-set message for an unknown FlowNode$type; that one concerns a 400 refusal, this one concerns the 200 finding list.

Traces: bedrock-agent-flow-validate-definition#task#2 (mismatch, status 200 -> 200, keys $.validations[0], $.validations[1], $.validations[2]).

## Evidence
Fixture bedrock-agent-flow-validate-definition, step task#2, ValidateFlowDefinition, verdict mismatch. Status matches (expected 200, actual 200); the response body's validations list diverges.

Expected (AWS recording) — validations has 3 entries:
  [0] {"type": "UnfulfilledNodeInput", "severity": "Warning", "details": {"unfulfilledNodeInput": {"input": "document", "node": "FlowOutputNode"}}, "message": "The input 'document' in node 'FlowOutputNode' is undefined. Connect a source value to this input."}
  [1] {"type": "UnknownConnectionTarget", "severity": "Error", "details": {"unknownConnectionTarget": {"connection": "FlowInputNodeFlowOutputNodeDataSource"}}, "message": "The connection between the nodes 'FlowInputNode' and 'NoSuchNode' references an unknown target node 'NoSuchNode'. Update the connection to reference a valid node that exists in the flow, or add the missing node to the flow."}
  [2] {"type": "UnreachableNode", "severity": "Warning", "details": {"unreachableNode": {"node": "FlowOutputNode"}}, "message": "The node 'FlowOutputNode' isn't connected to any other nodes. Attach this node to the flow or delete it."}

Actual (emulator) — validations has 1 entry:
  [0] {"type": "UnknownConnectionTarget", "severity": "Error", "details": {"unknownConnectionTarget": {"connection": "FlowInputNodeFlowOutputNodeDataSource"}}, "message": "Connection FlowInputNodeFlowOutputNodeDataSource ends at node NoSuchNode, which the flow doesn't define."}

Replay detail reported: $.validations[0] missing / extra (UnfulfilledNodeInput expected, UnknownConnectionTarget got), $.validations[1] missing (UnknownConnectionTarget expected, got None), $.validations[2] missing (UnreachableNode expected, got None).

Message anatomy for the one finding the emulator does emit:
  fixed text (AWS) = "The connection between the nodes '" ... "' and '" ... "' references an unknown target node '" ... "'. Update the connection to reference a valid node that exists in the flow, or add the missing node to the flow."
  request-derived parts = the connection's source node name ('FlowInputNode' in this recording), its unresolved target name ('NoSuchNode'), both read from the submitted definition's connections[].source/target.
  The emulator substitutes the right values into entirely different fixed text, and additionally keys its sentence on the connection NAME rather than the source/target node pair.

Message anatomy for the two findings the emulator omits:
  UnfulfilledNodeInput: fixed = "The input '" + <input name> + "' in node '" + <node name> + "' is undefined. Connect a source value to this input."; request-derived = the node's name and the name of the input left with no incoming connection.
  UnreachableNode: fixed = "The node '" + <node name> + "' isn't connected to any other nodes. Attach this node to the flow or delete it."; request-derived = the detached node's name.

Model support (smithy_query on services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json): ValidateFlowDefinition output validations: FlowValidations required, @length max 100; FlowValidationType values include UnfulfilledNodeInput, UnknownConnectionTarget, UnreachableNode; FlowValidationSeverity values Warning, Error; FlowValidationDetails union declares unfulfilledNodeInput, unknownConnectionTarget, unreachableNode. Emulator source: services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/validate_flow_definition.rs — validate_definition at line 294, and the `if every_connection_resolves { check_graph(...) }` gate at lines 337-339 with the comment stating graph properties are withheld when a connection does not resolve, which is what drops the two findings AWS still reports.

## Steps to Reproduce
1. Call bedrock-agent ValidateFlowDefinition with a definition of two nodes — an Input node named FlowInputNode with output {name: document, type: String}, and an Output node named FlowOutputNode with input {name: document, type: String, expression: $.data} — and one Data connection named FlowInputNodeFlowOutputNodeDataSource whose source is FlowInputNode and whose target names a node the definition does NOT declare (NoSuchNode).
2. Observe HTTP 200 (correct) and read validations.
3. The emulator returns exactly one finding: type UnknownConnectionTarget, severity Error, message "Connection FlowInputNodeFlowOutputNodeDataSource ends at node NoSuchNode, which the flow doesn't define."
4. AWS returns three findings for the same definition, in order: UnfulfilledNodeInput (Warning, details.unfulfilledNodeInput {node: FlowOutputNode, input: document}), UnknownConnectionTarget (Error, details.unknownConnectionTarget {connection: FlowInputNodeFlowOutputNodeDataSource}) with the "The connection between the nodes '<source>' and '<target>' references an unknown target node '<target>'. Update the connection to reference a valid node that exists in the flow, or add the missing node to the flow." wording, and UnreachableNode (Warning, details.unreachableNode {node: FlowOutputNode}).
5. Confirm the two gaps independently: (a) the unresolved-target message text differs from AWS's fixed text for that class; (b) the graph-level findings are suppressed because the handler only runs its graph pass when every connection resolves — point the same connection at FlowOutputNode and the graph pass runs, showing the suppression rather than a missing check.
6. Control to keep passing: a fully valid definition (connection target = FlowOutputNode) must still answer 200 with validations == [].
7. Replay: fixture bedrock-agent-flow-validate-definition, step task#2.
