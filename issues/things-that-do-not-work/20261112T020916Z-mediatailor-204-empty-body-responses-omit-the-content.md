Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-channel-tags-and-schedule-lifecycle
Issue-Step: task#3
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-channel-tags-and-schedule-lifecycle
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|TagResource|2xx->2xx|$headers.content-length
Issue-Key: mediatailor|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 270dc0c197f465d3081495464e266854d6a4e1a6

# MediaTailor 204 empty-body responses omit the Content-Length header

> _Note: this issue was identified against the code base at commit `270dc0c197f465d3081495464e266854d6a4e1a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Two MediaTailor operations whose modeled response is an empty 204 serialize the response without a Content-Length header. AWS sends `Content-Length: 0` on these responses. This is one shared fault in how the service's restJson1 response serializer emits a response for an operation with an empty (Unit) output shape, reproduced by 2 causes below.

Cause 1 of 2: TagResource (fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#3).
  AWS:      Smithy: TagResource http {method: POST, uri: "/tags/{ResourceArn}", code: 204}; output_shape Unit (no members); only modeled error BadRequestException (400). The operation has no response payload, and the recorded AWS response for the success case is HTTP 204 with the header `content-length: 0` and an empty body. The header's value is fixed text (`0`) for this operation class, not a per-run value.
  Emulator: answers HTTP 204 with the correct empty body, but the `content-length` response header is absent entirely (replay detail: `$headers.content-length: missing (expected '0', got None)`).
  Why:      a client that relies on AWS's framing of an empty-body response does not see a declared zero-length body. HTTP clients and proxies that key message framing off Content-Length must instead fall back to the 204 status or to connection-close semantics; strict clients and request-recording middleware that assert on the header see a response shape no AWS endpoint returns, and any code path that reads the declared body length to decide whether to attempt a parse behaves differently against the emulator than against AWS.
  Fix:      the response serializer for MediaTailor operations with an empty/Unit output shape (the restJson1 empty-response path in this service's serializer layer) must set `Content-Length: 0` on the 204 it emits, rather than emitting the status and body with no length header.
  Traces:   mediatailor-channel-tags-and-schedule-lifecycle#task#3.

Cause 2 of 2: UntagResource (fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#5).
  AWS:      Smithy: UntagResource http {method: DELETE, uri: "/tags/{ResourceArn}", code: 204}, @idempotent; output_shape Unit (no members); only modeled error BadRequestException (400). As with TagResource the recorded AWS success response is HTTP 204 carrying `content-length: 0` with an empty body; `0` is the fixed text of the header for an empty response.
  Emulator: answers HTTP 204 with an empty body but no `content-length` header at all (replay detail: `$headers.content-length: missing (expected '0', got None)`).
  Why:      same client-visible consequence as cause 1 — the zero-length body is never declared, so framing-sensitive clients, proxies and header assertions diverge from what AWS returns for a successful untag.
  Fix:      same component as cause 1 — the empty-response branch of the MediaTailor restJson1 response serializer. Fixing it in one place should close both causes, but BOTH steps must be re-replayed before closing, since UntagResource is a DELETE with a query-bound input and may take a different serializer branch than the POST.
  Traces:   mediatailor-channel-tags-and-schedule-lifecycle#task#5.

Not covered by any open issue: the open MediaTailor issues concern StartChannel/ConfigureLogsForChannel error status and code (20261103T024708Z-..., 20261104T201408Z-...), channel playback URL hostnames (20261109T142128Z-mediatailor-channel-playback-urls-omit-the-channel-assembly.md) and the GetChannelSchedule stub (20261110T213848Z-mediatailor-getchannelschedule-stub.md). None of them touches response header serialization for empty 204 responses.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, score 76.9%).

Cause 1 — TagResource, fixture mediatailor-channel-tags-and-schedule-lifecycle step task#3:
  verdict: mismatch; expected_status 204, actual_status 204
  detail: $headers.content-length: missing (expected '0', got None)
  Expected: HTTP 204, header `content-length: 0`, empty body.
  Actual:   HTTP 204, empty body, no `content-length` header.

Cause 2 — UntagResource, fixture mediatailor-channel-tags-and-schedule-lifecycle step task#5:
  verdict: mismatch; expected_status 204, actual_status 204
  detail: $headers.content-length: missing (expected '0', got None)
  Expected: HTTP 204, header `content-length: 0`, empty body.
  Actual:   HTTP 204, empty body, no `content-length` header.

Model support (services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json):
  TagResource   -> POST /tags/{ResourceArn}, http code 204, output_shape Unit
  UntagResource -> DELETE /tags/{ResourceArn}, http code 204, output_shape Unit, @idempotent
Both therefore return a body-less success response, for which AWS declares `Content-Length: 0`.

Only the header differs; status code and (empty) body match in both cases. No per-run value (request id, timestamp, generated name) is involved — the expected header value is the constant `0`.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture `mediatailor-channel-tags-and-schedule-lifecycle`.
2. Cause 1: at step `task#3`, call MediaTailor TagResource (POST /tags/{ResourceArn} with a `tags` map) against an existing channel ARN. Inspect the raw response headers (e.g. `curl -i`). Observe HTTP 204 with no `content-length` header; AWS returns HTTP 204 with `content-length: 0`.
3. Cause 2: at step `task#5`, call MediaTailor UntagResource (DELETE /tags/{ResourceArn}?tagKeys=<key>) on the same ARN. Inspect the raw response headers. Observe HTTP 204 with no `content-length` header; AWS returns HTTP 204 with `content-length: 0`.
4. After fixing the empty-response serializer, re-replay BOTH mediatailor-channel-tags-and-schedule-lifecycle#task#3 and mediatailor-channel-tags-and-schedule-lifecycle#task#5 and confirm each reports `content-length: 0`.
