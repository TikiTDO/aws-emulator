Issue-Source: trace-sweep-divergence
Issue-Trace: mediatailor-channel-tags-and-schedule-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediatailor-channel-tags-and-schedule-lifecycle
Issue-Shape-Id: com.amazonaws.mediatailor#MediaTailor
Issue-Key: mediatailor|TagResource|2xx->4xx|$
Issue-Key: mediatailor|TagResource|2xx->4xx|status
Issue-Key: mediatailor|ListTagsForResource|2xx->2xx|$.tags.sim2real-probe
Issue-Key: mediatailor|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: 841ccf5bd961485229918da026bcdc910fdfcb94

# MediaTailor tagging path broken: valid TagResource refused, tags unreadable, 204 lacks content-length

> _Note: this issue was identified against the code base at commit `841ccf5bd961485229918da026bcdc910fdfcb94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Three root causes (Cause 1 of 3 .. Cause 3 of 3), all on the tagging surface of mediatailor. Replay fixture mediatailor-channel-tags-and-schedule-lifecycle.

Cause 1 of 3 - TagResource refuses a well-formed request as if Tags were null (fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#3).

AWS: Smithy model services/service-com-amazonaws-mediatailor-mediatailor/smithy-model/com-amazonaws-mediatailor-mediatailor.json - TagResource: POST /tags/{ResourceArn}, success code 204 with an EMPTY body (output shape Unit), errors [BadRequestException @httpError 400]. Input TagResourceRequest$Tags targets __mapOf__string, is @required, and carries jsonName "tags" - so the member travels in the JSON body under the key `tags`, which is exactly what every AWS SDK and the CLI send. A request that carries that member for an ARN the account holds succeeds with 204 and no body.

Emulator: HTTP 400 with body {"__type": "ValidationException", "message": "1 validation error detected: Value null at 'tags' failed to satisfy constraint: Member must not be null"} where AWS answers 204 and {}. The refusal comes from the framework's boundary validation walk, not from the handler (services/service-com-amazonaws-mediatailor-mediatailor/src/ops/tag_resource.rs is implemented and its own refusals are BadRequestException, not ValidationException), so the request's body member never reached the input struct. Hypothesis: the request deserializer binds body members by Smithy member name (`Tags`) instead of by the member's jsonName (`tags`), so the SDK-shaped body leaves input.tags unset and the @required check then reports it null. The correct behavior for a valid request is 204 with an empty body and the tag set merged into the resource; a genuinely absent Tags member is a client error, but it must be refused with this service's modeled BadRequestException (HTTP 400, code BadRequestException), not with a ValidationException the service does not model.

Why: tagging is completely unusable through the emulator. Every SDK/CLI tag-resource call fails with a 400 claiming the caller sent null tags, so tag-driven provisioning, cost-allocation setup and any create-then-tag workflow aborts; worse, the error code ValidationException is not a code this service can return, so a client's error matching on BadRequestException does not even recognize the failure.

Fix: the TagResource request deserialization/validation path - the body member must be read under its modeled jsonName so input.tags is populated, after which the existing handler (src/ops/tag_resource.rs lines 155-205) stores it and answers 204. Audit the sibling operations that carry the same jsonName "tags" member (CreateChannel, PutPlaybackConfiguration) for the same binding defect.

Traces: mediatailor-channel-tags-and-schedule-lifecycle#task#3.

Cause 2 of 3 - ListTagsForResource reports no tags for a resource that was tagged (fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#4).

AWS: Smithy model - ListTagsForResource: GET /tags/{ResourceArn}, success 200, errors [BadRequestException 400]; output ListTagsForResourceResponse$Tags targets __mapOf__string with jsonName "tags", so the response body carries the resource's tags under the key `tags`. A read immediately after a successful TagResource reports every key that call wrote, with its value.

Emulator: HTTP 200 (status matches) but $.tags.<key> is absent - the tag the preceding TagResource was supposed to write is not reported. Hypothesis: this is the read side of Cause 1 - the tag was never stored because the write was refused - but it is a distinct replay step with its own assertion and must be re-verified separately, because if the key is still absent once Cause 1 is fixed then the response serializer is dropping the jsonName-bound `tags` map as well.

Why: a client cannot read back tags it just wrote, so tag-based discovery and any read-after-write assertion on tags sees an untagged resource and takes the wrong branch (re-tagging in a loop, or treating the resource as unmanaged).

Fix: follows Cause 1 (the write must land in state.tags_by_arn); then confirm the ListTagsForResource handler's response projection emits the stored map under the modeled jsonName. src/state.rs tags_of() is the single source of truth and already returns an empty map for an untagged resource, so no change belongs there.

Traces: mediatailor-channel-tags-and-schedule-lifecycle#task#4.

Cause 3 of 3 - UntagResource's empty 204 response omits the Content-Length header (fixture mediatailor-channel-tags-and-schedule-lifecycle, step task#5).

AWS: Smithy model - UntagResource: DELETE /tags/{ResourceArn}, success code 204, output shape Unit (no body), TagKeys bound to the httpQuery parameter tagKeys. Recorded behavior: AWS returns the 204 with the header content-length: 0 - the fixed value 0 for a response with no body, not a per-run value.

Emulator: HTTP 204 with the correct empty body, but no content-length header at all ($headers.content-length missing, expected '0'). Every other aspect of the step matches.

Why: HTTP clients and proxies that require an explicit framing header on a bodyless response - and strict test harnesses asserting AWS's response headers - see a differently framed response; a client that reads content-length to decide whether to attempt a body parse has no value to read. The same omission applies to every bodyless 204 this service serves (TagResource shares the output shape Unit and code 204).

Fix: the response serializer for operations whose output is Unit with a 2xx no-body status - it must emit content-length: 0 on the empty response. Not a handler change: src/ops/tag_resource.rs already models the 204 correctly via with_http(..., Some(204)).

Traces: mediatailor-channel-tags-and-schedule-lifecycle#task#5.

## Evidence
Fixture: mediatailor-channel-tags-and-schedule-lifecycle (trace set simparity-wire-aws-v1)

Cause 1 - step task#3, operation TagResource, verdict mismatch:
  status: expected 204, got 400
  $: value_mismatch (expected {}, got {'__type': 'ValidationException', 'message': "1 validation error detected: Value null at 'tags' failed to satisfy constraint: Member must not be null"})
  Expected (AWS): HTTP 204, empty body, tag set merged onto the ARN.
  Actual (emulator): HTTP 400, error code ValidationException - a code this service's model does not declare. Fixed text of the emulator message: "1 validation error detected: Value null at '" + "' failed to satisfy constraint: Member must not be null"; variable part: the member path 'tags'.

Cause 2 - step task#4, operation ListTagsForResource, verdict mismatch, expected status 200, actual status 200:
  $.tags.sim2real-probe: missing (expected 'mediatailor-fixture', got None)
  (the tag key and value are the fixture's own per-run request values; the requirement is that the key written by the preceding TagResource is reported with its value, not these literals)

Cause 3 - step task#5, operation UntagResource, verdict mismatch, expected status 204, actual status 204:
  $headers.content-length: missing (expected '0', got None)

Model evidence (smithy_query query="operation" operation="TagResource,UntagResource,ListTagsForResource"):
  TagResource:         POST /tags/{ResourceArn}, code 204, output Unit, errors [BadRequestException/client/400]
                       input Tags -> __mapOf__string, jsonName "tags", REQUIRED
  UntagResource:       DELETE /tags/{ResourceArn}, code 204, output Unit, idempotent, errors [BadRequestException/client/400]
                       input TagKeys -> httpQuery "tagKeys", REQUIRED
  ListTagsForResource: GET /tags/{ResourceArn}, code 200, output Tags -> __mapOf__string jsonName "tags", errors [BadRequestException/client/400]

Code citations:
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/tag_resource.rs:155-205 (handler is implemented; its refusals are BadRequestException, so the ValidationException is produced upstream by the boundary validator)
  services/service-com-amazonaws-mediatailor-mediatailor/src/ops/tag_resource.rs:80-89 (output schema carries the modeled 204)
  services/service-com-amazonaws-mediatailor-mediatailor/src/state.rs:146-202 (tags_by_arn is the single tag store; tags_of returns {} for an untagged resource)

## Steps to Reproduce
1. git_clone() the repo and boot the emulator.
2. Create a channel and note its ARN:
   aws mediatailor create-channel --channel-name tag-probe \
     --outputs '[{"ManifestName":"default","SourceGroup":"default","HlsPlaylistSettings":{"ManifestWindowSeconds":30}}]' \
     --playback-mode LOOP --tier BASIC
3. Tag it with an SDK-shaped body (Tags travels as the JSON key `tags`):
   aws mediatailor tag-resource --resource-arn <arn> --tags probe=fixture
   -> must be HTTP 204 with an empty body; the emulator answers HTTP 400 {"__type":"ValidationException","message":"1 validation error detected: Value null at 'tags' failed to satisfy constraint: Member must not be null"}.
4. aws mediatailor list-tags-for-resource --resource-arn <arn>
   -> tags must contain probe=fixture; the emulator reports no such key.
5. aws mediatailor untag-resource --resource-arn <arn> --tag-keys probe
   -> 204 with header content-length: 0; the emulator sends the 204 with no content-length header (inspect with: curl -i -X DELETE 'http://<host>:<port>/tags/<url-encoded-arn>?tagKeys=probe').
6. Re-replay with replay_wire_corpus: fixture mediatailor-channel-tags-and-schedule-lifecycle steps task#3, task#4 and task#5 must all pass.

## Resolution

Cause 1 and Cause 2 are FIXED. The body member is now read under either spelling — the
`@jsonName` an SDK writes or the member name the model declares — by
`services/service-com-amazonaws-mediatailor-mediatailor/src/service/body_member_names.rs`,
which re-keys the request body from each operation's own input schema and then runs the
modelled operation unchanged, so `TagResource` answers the modelled 204 and the following
`ListTagsForResource` reports every key it wrote. A request whose required member is absent
under both spellings is now refused under the one client-input error this service's model
declares (`BadRequestException`, HTTP 400) rather than under a shape the model does not
carry.

Cause 3 is implemented at the service boundary and BLOCKED below it. The service now answers
a no-content success with an empty body and `content-length: 0`
(`src/service/wire.rs`, through `services/common/src/no_content_responses.rs`, asserted by
`tests/all/wire_diagnostic.rs`), but the vendored HTTP writer does not put that header on
the socket: `vendor/server/src/lib.rs:239-256` states that the listeners' HTTP stack writes
no `Content-Length` for a status that carries no content, its own test at
`vendor/server/src/lib.rs:2157-2175` pins that, and
`services/common/src/no_content_responses.rs:31-33` records the same thing. RFC 9110 § 8.6
forbids the header on a 204, so reproducing AWS's recorded framing needs a writer that
departs from the RFC — a change to the read-only `vendor/` serve path
(`vendor/server/src/lib.rs:896`), not to any service crate. The replay therefore still
reports `$headers.content-length` on the two 204 steps; every other difference on them is
gone.
