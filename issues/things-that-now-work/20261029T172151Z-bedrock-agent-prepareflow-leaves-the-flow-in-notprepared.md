Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-agent-flow-prepare-and-version
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-agent-flow-prepare-and-version
Issue-Shape-Id: com.amazonaws.bedrockagent#AmazonBedrockAgentBuildTimeLambda
Issue-Key: bedrock-agent|GetFlow|2xx->2xx|$
Issue-Key: bedrock-agent|GetFlow|2xx->2xx|$.status
Issue-Key: bedrock-agent|GetFlow|2xx->2xx|$.validations
Issue-Related: 20261029T172025Z-bedrock-agent-serializes-createdat-updatedat-as-epoch-numbers.md
Issue-Commit: 9cbe597425894ecc7b6940c236dd920b40ebc4b7

# bedrock-agent PrepareFlow leaves the flow in NotPrepared and never populates validations

> _Note: this issue was identified against the code base at commit `9cbe597425894ecc7b6940c236dd920b40ebc4b7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PrepareFlow

## Description
1 cause.

Cause 1 of 1: GetFlow after PrepareFlow reports NotPrepared forever (fixture bedrock-agent-flow-prepare-and-version, step task#4).
AWS: Smithy: PrepareFlow (POST /flows/{flowIdentifier}/, success 202) returns {id: FlowId, status: FlowStatus} and asynchronously prepares the DRAFT flow; FlowStatus is an enum with values Failed | Prepared | Preparing | NotPrepared, and GetFlow's status member is @required FlowStatus. After PrepareFlow a client polls GetFlow until status leaves the transient state and settles on Prepared (or Failed), with validations: FlowValidations (@length max 100) reporting any problems — an empty list when the definition is valid. Only a Prepared flow can be versioned (CreateFlowVersion) or invoked.
Emulator: PrepareFlow is accepted, but GetFlow keeps reporting status 'NotPrepared' indefinitely — the replay poll capped at 60s / 61 attempts without ever observing Prepared, and the response carries no validations member at all. The flow's definition, id, arn, executionRoleArn and version (DRAFT) are otherwise correct.
Why: a client that prepares a flow and waits for it to become Prepared before versioning or invoking it blocks forever; because the status never moves off the pre-prepare value, the client cannot distinguish "not prepared yet", "preparing" and "preparation failed", and the validations list that AWS uses to report definition errors is never available.
Fix: the PrepareFlow handler's state transition (the flow state machine driving FlowStatus NotPrepared -> Preparing -> Prepared/Failed) and the GetFlow response projection for the validations member.

The flow id, name, role ARN and timestamps in the recording are per-run values; the requirement is that PrepareFlow drives status to Prepared and that GetFlow then exposes validations.

## Evidence
Replay of trace set simparity-wire-aws-v1 — GetFlow, expected 200 / actual 200 (bedrock-agent-flow-prepare-and-version#task#4), after PrepareFlow:
  $.status: value_mismatch (expected 'Prepared', got 'NotPrepared')
  $.validations: missing (expected [], got None)
  $: poll: status never satisfied, poll capped at 60s (61 attempts)
  expected body also carried 'validations': [] alongside status 'Prepared'; actual body carried status 'NotPrepared' and no validations member
  (the same step also exhibits the epoch-number createdAt/updatedAt defect filed separately)

Smithy (services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json): PrepareFlow http {method: POST, uri: /flows/{flowIdentifier}/, code: 202}, output {id: FlowId required, status: FlowStatus required}; FlowStatus = [Failed, Prepared, Preparing, NotPrepared]; GetFlow output status @required FlowStatus, validations: FlowValidations (length max 100).

## Steps to Reproduce
1. Replay fixture bedrock-agent-flow-prepare-and-version through task#4: CreateFlow with a valid definition, call PrepareFlow, then poll GetFlow for status == 'Prepared'.
2. The emulator's GetFlow reports 'NotPrepared' on every poll (61 attempts / 60s cap) and the response has no validations member, so the wait never completes and downstream CreateFlowVersion/flow invocation cannot proceed.
