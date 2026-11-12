Issue-Source: trace-sweep-divergence
Issue-Trace: mq-tags-create-list-delete-roundtrip-on-configuration
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-tags-create-list-delete-roundtrip-on-configuration
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|CreateTags|2xx->2xx|$headers.content-length
Issue-Key: mq|DeleteTags|2xx->2xx|$headers.content-length
Issue-Commit: b92c95c734c83d384ee4236e8b1b66139a04e909

# mq tag mutations omit the Content-Length header on their empty 204 responses

> _Note: this issue was identified against the code base at commit `b92c95c734c83d384ee4236e8b1b66139a04e909`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTags

## Description
The emulator's Amazon MQ tag-mutation operations return the correct 204 status and an empty body, but they serve the response with NO Content-Length header at all. Real Amazon MQ frames these empty REST-JSON responses explicitly. Two causes, both in the same place (the mq response serializer / the empty-output path for operations modelled with an @http code of 204 and a Unit output):

Cause 1 of 2: CreateTags (fixture mq-tags-create-list-delete-roundtrip-on-configuration, step task#2).
AWS: Smithy: CreateTags input {ResourceArn @httpLabel @required, Tags (jsonName "tags")}, output shape Unit (no members), http {method POST, uri /v1/tags/{ResourceArn}, code 204}. The operation has no output members, so the response is an empty body under status 204; the recorded Amazon MQ response for that step carries a Content-Length response header (the recording asserts the header's PRESENCE only — its value is '<shaped-not-asserted>', so the requirement is that the header is emitted on an empty 204, not any one recorded number). Status 204 and the empty body are already correct.
Emulator: replay reports '$headers.content-length: missing (expected <shaped-not-asserted>, got None)' — the 204 is served with the header absent entirely.
Why: a client that frames the response from Content-Length rather than from the status sees an unframed reply. HTTP stacks and proxies that insist on explicit framing for a known-empty response (or that log/assert on the header) treat the emulator's reply as malformed or wait for framing that never arrives, so a tagging call that succeeds against AWS can fail or stall against the emulator even though the status matches.
Fix: the mq operation's serializer — the shared empty-output/204 response path that builds responses for operations whose output shape is Unit must set Content-Length (0) instead of emitting no entity headers. Not a handler or validation change: the status and body are already right.

Cause 2 of 2: DeleteTags (fixture mq-tags-create-list-delete-roundtrip-on-configuration, step task#4).
AWS: Smithy: DeleteTags input {ResourceArn @httpLabel @required, TagKeys @httpQuery("tagKeys") @required}, output shape Unit (no members), http {method DELETE, uri /v1/tags/{ResourceArn}, code 204}. Same shape of response as CreateTags: empty body, status 204, and the recorded response carries a Content-Length header whose value the fixture does not assert.
Emulator: replay reports '$headers.content-length: missing (expected <shaped-not-asserted>, got None)' on the DeleteTags step too.
Why: identical client impact to Cause 1, on the tag-removal half of the round trip — so a tag create/delete round trip is unframed at both of its mutating steps.
Fix: same component as Cause 1 (the mq serializer's empty-204 response path); fixing it there should close both causes, but BOTH steps must be re-replayed to confirm, since they are separate operations bound to the same serializer path.

No open issue covers these: the existing mq issues concern DescribeBroker endpoints/logGroups (20261111T005758Z-mq-describebroker-publishes-activemq-wire-endpoints-in-the.md), the instance-option catalog (20261107T120000Z-...), configuration-revision reads (20261031T013311Z-...), wrong refusal statuses/messages (20261103T045546Z-mq-refuses-invalid-requests-with-the-wrong-status.md) and revision state (20261103T045546Z-mq-response-bodies-carry-wrong-revision-state-and.md) — none of them mentions the tag operations or response framing.

## Evidence
Replay of trace set simparity-wire-aws-v1 (service mq):

Cause 1 — fixture mq-tags-create-list-delete-roundtrip-on-configuration, step task#2, operation CreateTags, verdict mismatch:
  expected_status 204, actual_status 204 (status correct)
  detail: $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=mq|CreateTags|status=204->204|keys=$headers.content-length

Cause 2 — fixture mq-tags-create-list-delete-roundtrip-on-configuration, step task#4, operation DeleteTags, verdict mismatch:
  expected_status 204, actual_status 204 (status correct)
  detail: $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=mq|DeleteTags|status=204->204|keys=$headers.content-length

Expected (AWS): an empty-bodied 204 response that carries a Content-Length response header (value 0 for an empty body; the fixture asserts presence, not a specific recorded value).
Actual (emulator): an empty-bodied 204 response with no Content-Length header.
Model citations: Smithy mq CreateTags -> output Unit, http {POST /v1/tags/{ResourceArn}, code 204}; DeleteTags -> output Unit, http {DELETE /v1/tags/{ResourceArn}, code 204}.

## Steps to Reproduce
1. git clone the repo and boot the emulator from that checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture mq-tags-create-list-delete-roundtrip-on-configuration step task#2 — CreateTags POST /v1/tags/{ResourceArn} with a tag map against an existing mq configuration ARN. Inspect the response headers: the status is 204 with an empty body, but no Content-Length header is present.
4. Cause 2: replay the same fixture's step task#4 — DeleteTags DELETE /v1/tags/{ResourceArn}?tagKeys=... for the keys added in step 2. Again status 204, empty body, and no Content-Length header.
5. Both steps must pass (header present on the empty 204) before this issue is closed.
