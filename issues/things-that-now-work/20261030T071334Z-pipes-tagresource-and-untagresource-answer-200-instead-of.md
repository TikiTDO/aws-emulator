Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-pipe-tag-management
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-pipe-tag-management pipes-tag-empty-value-vs-absent-preserved pipes-tag-value-unicode-preserved pipes-tagging-workflow
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|TagResource|2xx->2xx|status
Issue-Key: pipes|UntagResource|2xx->2xx|status
Issue-Commit: 533efece7fa11c0e537273065624bf5d01aa80f8

# pipes TagResource and UntagResource answer 200 instead of 204 No Content

> _Note: this issue was identified against the code base at commit `533efece7fa11c0e537273065624bf5d01aa80f8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
This bundle has 2 causes: the two tagging operations each return the wrong success status. They are separate handlers with separate replay bindings, so a fixer must close both before submitting.

Shared fault: both tagging operations have an EMPTY output shape and AWS answers them with 204 No Content. The Smithy model gives neither operation an explicit success code (http code 200 is marked codeDefaulted for both TagResource and UntagResource), so the recorded wire is the authority here, and it records 204.

Cause 1 of 2: TagResource.
  AWS: recorded behavior only — POST /tags/{resourceArn} (Smithy: TagResource, output TagResourceResponse with no members, http code 200 codeDefaulted) answers a successful tag write with HTTP 204 and no body. No error code or message is involved.
  Emulator: HTTP 200.
  Why: a client or proxy that branches on the exact status (204 => no body to read, 200 => parse a body) behaves differently against the emulator; status assertions in SDK/contract tests and any 204-aware caching or logging layer diverge.
  Fix: TagResource's response/status mapping (services/service-com-amazonaws-pipes-pipes/src/ops/tag_resource.rs) must emit 204 with an empty body.
  Traces: pipes-pipe-tag-management#task#10 (also pipes-tag-empty-value-vs-absent-preserved#task#9, pipes-tag-value-unicode-preserved#task#9, pipes-tagging-workflow#task#10).

Cause 2 of 2: UntagResource.
  AWS: recorded behavior only — DELETE /tags/{resourceArn}?tagKeys=... (Smithy: UntagResource, output UntagResourceResponse with no members, http code 200 codeDefaulted) answers a successful tag removal with HTTP 204 and no body.
  Emulator: HTTP 200.
  Why: same as cause 1.
  Fix: UntagResource's response/status mapping (src/ops/untag_resource.rs) must emit 204 with an empty body.
  Traces: pipes-pipe-tag-management#task#12 (also pipes-tag-empty-value-vs-absent-preserved#task#11, pipes-tagging-workflow#task#12).

## Evidence
Replay of simparity-wire-aws-v1, 2 mismatch clusters, 7 affected steps:
- TagResource, divergence_key service=pipes|TagResource|status=204->200|keys=status, rep pipes-pipe-tag-management#task#10, affected 4, detail "status: expected 204, got 200".
- UntagResource, divergence_key service=pipes|UntagResource|status=204->200|keys=status, rep pipes-pipe-tag-management#task#12, affected 3, detail "status: expected 204, got 200".
Expected (AWS, recorded): 204 No Content with an empty body on success. Actual (emulator): 200. Both operations' output shapes are empty, so nothing but the status line diverges.

## Steps to Reproduce
1. Replay fixture pipes-pipe-tag-management, step task#10: TagResource on an existing pipe ARN with a non-empty tag map. Emulator answers 200; AWS answers 204.
2. Same fixture, step task#12: UntagResource with tagKeys for the tag just written. Emulator answers 200; AWS answers 204.
3. Both must answer 204 with an empty body on success (error paths unchanged).

## Fix status

Cause 1 (TagResource) and cause 2 (UntagResource): FIXED. Both operations state the
no-content success status on the operation itself
(`SUCCESS_STATUS` in `services/service-com-amazonaws-pipes-pipes/src/ops/tag_resource.rs`
and `src/ops/untag_resource.rs`), and the body the serializer renders for a members-less
output structure is emptied at the one no-content seam every response of this service passes
through (`Service::override_protocol` in `src/service.rs`, through
`service_common::no_content_responses::without_content`). Replay reports both status keys
as `progressed`: the 204 matches on every affected step.

Remaining on those steps, and NOT a cause of this report:
`pipes|TagResource|2xx->2xx|$headers.content-length` and the same key for
`UntagResource`. The service states `content-length: 0` at that seam and every in-process
path serves it (the front door, the inter-service callback, the SDK connector — see
`set_framing_content_length` in `vendor/server/src/lib.rs`), but the HTTP/1.1 server writer
the vendored listener is built on strips `content-length` from a status that cannot carry a
body, which `vendor/server/src/lib.rs` records as its own behaviour
(`the_connector_keeps_a_handler_length_and_adds_none_to_a_status_without_content`). No seam
a service crate owns runs after that writer. The same blocker, with the same evidence, is
already filed for another service in
`issues/things-that-now-work/20261102T020000Z-notifications-mp-buyer-agreements-text-and-the-two-204-content-length-keys-remain-open.md`.

Also still diverging on `pipes-pipe-tag-management#task#8`, from before this change and
untouched by it: `pipes|DescribePipe|2xx->2xx|$.SourceParameters`,
`$.TargetParameters` and `$.EnrichmentParameters`.
