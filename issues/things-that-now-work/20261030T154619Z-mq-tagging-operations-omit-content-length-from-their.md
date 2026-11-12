Issue-Source: trace-sweep-divergence
Issue-Trace: mq-tags-create-list-delete-roundtrip-on-configuration
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mq-tags-create-list-delete-roundtrip-on-configuration
Issue-Shape-Id: com.amazonaws.mq#mq
Issue-Key: mq|CreateTags|2xx->2xx|$headers.content-length
Issue-Key: mq|DeleteTags|2xx->2xx|$headers.content-length
Issue-Commit: 0da124688ced2f47ce1b8859b8dabf10bd34dedc

# mq tagging operations omit Content-Length from their 204 responses

> _Note: this issue was identified against the code base at commit `0da124688ced2f47ce1b8859b8dabf10bd34dedc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTags

## Description
Two causes, same fault on two operations. A fixer must close BOTH and re-replay both fixture#steps before submitting.

Cause 1 of 2: CreateTags answers 204 without a Content-Length header (fixture mq-tags-create-list-delete-roundtrip-on-configuration, step task#2).
AWS: Smithy models CreateTags as POST /v1/tags/{ResourceArn} with success code 204 and an empty output (output shape Unit), so there is no body. The recording shows AWS nevertheless sending a Content-Length header on that 204 response (recorded behavior only — the header's value is not asserted, its presence is).
Emulator: the 204 response carries no Content-Length header at all.
Why: a client or intermediary that requires a framing header on every response — HTTP/1.1 proxies and some SDK transports treat a response with neither Content-Length nor Transfer-Encoding as needing connection-close framing — behaves differently against the emulator than against AWS: connection reuse is lost or the response is read as unframed, which is exactly the class of difference a wire-fidelity emulator exists to hide.
Fix: the empty-output response serialisation for this operation — services/service-com-amazonaws-mq-mq/src/ops/create_tags.rs and the shared REST-JSON response writer that emits a no-body 204 (it must still set Content-Length for an empty body).
Traces: mq-tags-create-list-delete-roundtrip-on-configuration#task#2.

Cause 2 of 2: DeleteTags answers 204 without a Content-Length header (same fixture, step task#4).
AWS: Smithy models DeleteTags as DELETE /v1/tags/{ResourceArn} with success code 204 and an empty output (Unit); the recording again shows a Content-Length header present on the 204 (recorded behavior only).
Emulator: the 204 response carries no Content-Length header.
Why: same as Cause 1 — the tag write succeeds (the surrounding ListTags steps in the fixture pass), but the response framing differs from AWS for every tag mutation a client makes.
Fix: the same empty-output response serialisation, for this operation — services/service-com-amazonaws-mq-mq/src/ops/delete_tags.rs and the shared no-body response writer.
Traces: mq-tags-create-list-delete-roundtrip-on-configuration#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1.

Cause 1 — mq-tags-create-list-delete-roundtrip-on-configuration#task#2, CreateTags, expected 204 / actual 204:
  $headers.content-length: expected present (value not asserted), got None

Cause 2 — mq-tags-create-list-delete-roundtrip-on-configuration#task#4, DeleteTags, expected 204 / actual 204:
  $headers.content-length: expected present (value not asserted), got None

Model grounding: CreateTags POST /v1/tags/{ResourceArn}, code 204, output Unit; DeleteTags DELETE /v1/tags/{ResourceArn}, code 204, output Unit (services/service-com-amazonaws-mq-mq/smithy-model/com-amazonaws-mq-mq.json). Status code and body are correct; only the response framing header diverges.

## Steps to Reproduce
1. git clone the project repo and build/boot the emulator from the checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture mq-tags-create-list-delete-roundtrip-on-configuration, step task#2 — POST /v1/tags/{configuration-arn} with a tag map and inspect the 204 response headers: no content-length is sent.
4. Cause 2: same fixture, step task#4 — DELETE /v1/tags/{configuration-arn}?tagKeys=... and inspect the 204 response headers: no content-length is sent.

## Resolution

Causes 1 and 2 are the same fault and are closed together, at the one seam a service owns
for this: the protocol this service hands every modelled response through
(`src/service/iso8601_timestamps.rs`, taken at `Service::override_protocol`) now frames an
answer at the no-content status as one — no body, and the length it has,
`content-length: 0` — through the shared reading of that framing in
`services/common/src/no_content_responses.rs`. The rule is the status's, not the
operation's, so it covers every operation of this model whose success code is 204
(`CreateTags`, `DeleteTags`) and leaves every answer that carries content exactly as its
renderer wrote it. `tests/all/no_content_framing.rs` pins both halves.

CAVEAT: a client reached over HTTP/1.1 still does not see the header. The HTTP writer the
vendored transport serves on (`vendor/server/src/lib.rs`, hyper's connection builder)
removes `content-length` from a `204` — RFC 9110 §15.3.5 gives that status no body — and
that writer is under the read-only vendored tree, so the wire-level half of this cause is a
platform limitation rather than something a service crate can state its way out of. The
suite's wire probe pins what the socket really delivers, so the boundary is a fact in the
tests rather than a claim in a comment.
