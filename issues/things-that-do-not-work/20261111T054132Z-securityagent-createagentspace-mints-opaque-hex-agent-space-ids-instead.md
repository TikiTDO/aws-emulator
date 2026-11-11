Issue-Source: trace-sweep-divergence
Issue-Trace: securityagent-agent-space-lifecycle
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securityagent-agent-space-lifecycle securityagent-artifact-batch-and-error-paths securityagent-artifact-lifecycle
Issue-Shape-Id: com.amazonaws.securityagent#SecurityAgent
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.agentSpaceId
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.id
Issue-Key: securityagent|CreateAgentSpace|2xx->2xx|$.arn
Issue-Commit: 0dd06decd64ca030d29e4dddabe070ef79046880

# CreateAgentSpace mints opaque hex agent space ids instead of the as-<uuid> form

> _Note: this issue was identified against the code base at commit `0dd06decd64ca030d29e4dddabe070ef79046880`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgentSpace

## Description
Cause 1 of 1: CreateAgentSpace returns an agent space identifier in the wrong lexical form, so every value derived from it (the response's id/agentSpaceId pair and the agent-instance ARN built from it) is shaped unlike AWS's.

AWS: recorded behavior only - Smithy com.amazonaws.securityagent#AgentSpaceId is an unconstrained string ("Unique identifier of the agent space.") with no @pattern or @length, and CreateAgentSpace (POST /CreateAgentSpace, HTTP 200) models agentSpaceId as a required output member, so the model does not state the format. The recording does: the CreateAgentSpace response in securityagent-agent-space-lifecycle carries agentSpaceId and id both set to the literal 'as-e951abe3-de7f-4d2b-b5a8-4a4d4c380cba' - the fixed text is the 'as-' prefix, and the variable part is a freshly generated RFC-4122 UUID (lowercase, hyphenated, 36 characters), one new value per created agent space. The same prefix shows up on the request side of the corpus, where a deliberately bogus id is written 'as-doesnotexist0000'.

Emulator: the same step returns a bare 32-character lowercase hex string with no prefix and no hyphens - '135dde59683453dc0000000000000001' in securityagent-agent-space-lifecycle#setup#1 and '037a00b5818400950000000000000001' in securityagent-artifact-batch-and-error-paths#task#1 - in BOTH the id and agentSpaceId members. Status is 200 in both the recording and the emulator, so only the identifier's shape diverges; the trailing-counter pattern of the actual values shows the id comes from a generic sequential-counter minter rather than an agent-space-specific one. Note that the concrete UUID and the concrete hex value are per-run values and are not themselves the requirement: the requirement is the 'as-' + UUID template.

Why: a client that validates, parses or pattern-matches an agent space id (for example routing on the 'as-' prefix to tell an agent space id apart from a target domain id or a pack id, or extracting the UUID) rejects or mis-classifies every id the emulator mints, and an id it stores from the emulator will not be accepted by code that was written against real AWS ids. Because the id is substituted into the agent-instance ARN, the ARN's resource segment is wrong too, so anything that re-derives the id from an ARN or compares an emulator ARN against an AWS-shaped one fails. Tests that assert on id shape pass against AWS and fail against the emulator.

Fix: the CreateAgentSpace handler's id generation in the securityagent service crate - the agent-space id minting step that populates the new record's identifier in state, which must emit 'as-' followed by a newly generated hyphenated lowercase UUID instead of borrowing the generic hex/counter id generator. The response serializer needs no change; it faithfully echoes whatever the state minted into id, agentSpaceId and the ARN.

Traces: securityagent-agent-space-lifecycle#setup#1 (expected 'as-e951abe3-de7f-4d2b-b5a8-4a4d4c380cba' for $.id and $.agentSpaceId, got '135dde59683453dc0000000000000001'); securityagent-artifact-batch-and-error-paths#task#1 (same operation, same fault, also propagated into $.arn as 'arn:aws:securityagent:us-west-2:000000000000:agent-instance/037a00b5818400950000000000000001'). Adjacent but distinct: open issue 20261030T135909Z-securityagent-updateagentspace-responses-omit-agent-instance-identity-fields-and.md covers UpdateAgentSpace omitting id/ownerId/arn entirely; it does not cover the format of the id CreateAgentSpace mints, so it does not own this.

## Evidence
Replay of fixture securityagent-agent-space-lifecycle, step setup#1, operation CreateAgentSpace, verdict mismatch, expected_status 200 / actual_status 200:
  $.id: value_mismatch (expected 'as-e951abe3-de7f-4d2b-b5a8-4a4d4c380cba', got '135dde59683453dc0000000000000001')
  $.agentSpaceId: value_mismatch (expected 'as-e951abe3-de7f-4d2b-b5a8-4a4d4c380cba', got '135dde59683453dc0000000000000001')

Second instance - fixture securityagent-artifact-batch-and-error-paths, step task#1, operation CreateAgentSpace, 200 vs 200:
  $.id / $.agentSpaceId: got '037a00b5818400950000000000000001'
  $.arn: got 'arn:aws:securityagent:us-west-2:000000000000:agent-instance/037a00b5818400950000000000000001'

Expected shape (template, not a literal): 'as-' + lowercase hyphenated UUID, e.g. as-<8-4-4-4-12 hex>. Actual shape: 32 lowercase hex characters, no prefix, no hyphens, with a monotonically increasing low-order counter.

Smithy (services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json): CreateAgentSpace input {name required, description, awsResources, targetDomainIds, codeReviewSettings, kmsKeyId, tags}, output {agentSpaceId required -> AgentSpaceId, name required, ...}, http POST /CreateAgentSpace code 200 (defaulted), no modeled errors; shape AgentSpaceId: plain string, doc 'Unique identifier of the agent space.', no pattern/length constraint - hence the format is grounded in the recording, not the model.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture securityagent-agent-space-lifecycle and stop at step setup#1: POST /CreateAgentSpace with a name (the fixture uses a sim2real-probe-securityagent-* name).
3. Read the 200 response body: $.agentSpaceId and $.id come back as a 32-char hex string such as '135dde59683453dc0000000000000001'; the recorded AWS response has both set to an 'as-'-prefixed UUID ('as-e951abe3-de7f-4d2b-b5a8-4a4d4c380cba').
4. Confirm the propagation with fixture securityagent-artifact-batch-and-error-paths step task#1: the same malformed id is embedded in $.arn as .../agent-instance/037a00b5818400950000000000000001.
5. Fix check: the new id must match 'as-' followed by a lowercase hyphenated UUID on every create (the UUID itself differs per run); re-replay both fixtures above.
