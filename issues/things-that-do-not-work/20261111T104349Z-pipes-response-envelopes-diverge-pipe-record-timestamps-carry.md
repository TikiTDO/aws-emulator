Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-error-conflict-and-source-notfound
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-error-conflict-and-source-notfound pipes-filter-criteria-nested-json-preserved pipes-list-pipes-filtering pipes-list-pipes-pagination pipes-pipe-error-paths pipes-pipe-tag-management pipes-tag-empty-value-vs-absent-preserved pipes-tag-value-unicode-preserved pipes-tagging-workflow
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|DescribePipe|2xx->2xx|$.CreationTime
Issue-Key: pipes|DescribePipe|2xx->2xx|$.LastModifiedTime
Issue-Key: pipes|DeletePipe|2xx->2xx|$.CreationTime
Issue-Key: pipes|DeletePipe|2xx->2xx|$.LastModifiedTime
Issue-Key: pipes|TagResource|2xx->2xx|$headers.content-length
Issue-Key: pipes|UntagResource|2xx->2xx|$headers.content-length
Issue-Commit: af64552852b9de70172a1fcfdec68b8a2bd1c743

# pipes response envelopes diverge: pipe-record timestamps carry a millisecond tail and tagging 204s omit Content-Length

> _Note: this issue was identified against the code base at commit `af64552852b9de70172a1fcfdec68b8a2bd1c743`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePipe

## Description
Every pipes response this emulator puts on the wire is shaped wrongly in its envelope, in two independent ways: the pipe record's CreationTime/LastModifiedTime are emitted with a sub-second (millisecond) tail the recorded AWS responses do not carry, and the two tag-mutating operations answer an empty 204 with no content-length framing header at all. 4 causes — numbered 1 of 4 through 4 of 4 below; ALL FOUR must be closed and all four fixture steps re-replayed before submitting.

Cause 1 of 4: DescribePipe reports CreationTime and LastModifiedTime in a timestamp shape AWS does not emit.
AWS: Smithy model services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json — DescribePipe GET /v1/pipes/{Name} -> 200 (errors NotFoundException 404, ValidationException 400, ThrottlingException 429, InternalException 500); output members CreationTime and LastModifiedTime both target com.amazonaws.pipes#Timestamp (type timestamp) and neither the member nor the shape carries @timestampFormat, and the service protocol is restJson1, so both are epoch-seconds NUMBERS on the wire. The INSTANT is a per-run value and is not a requirement — the recording asserts the SHAPE of the value, not the value — and what the recorded AWS responses fix is a value with no sub-second tail at that member. Recorded behavior only for the granularity: the model states no precision.
Emulator: both members are projected through crate::state::timestamp (services/service-com-amazonaws-pipes-pipes/src/state.rs lines 1296-1302), which converts the stored host SystemTime with DateTime::from_millis, and the DescribePipe handler emits them that way (services/service-com-amazonaws-pipes-pipes/src/ops/describe_pipe.rs lines 220-221). The body therefore carries a fractional epoch-seconds number with a millisecond tail. Replay verdict mismatch, expected_status 200, actual_status 200, value_mismatch at $.CreationTime and $.LastModifiedTime against the recording's shaped expectation (the run observed 1794393626.384 for both; that is that run's host clock and is NOT a requirement).
Why: a client that round-trips, snapshots or compares these timestamps — a golden test, a cache key, a diff of the record CreatePipe returned against the one DescribePipe later reports, or any consumer that assumes the granularity EventBridge Pipes reports — sees values of a kind the real service never returns, and any assertion written against AWS's shape fails against the emulator. Because the fault is in the shared projection helper, EVERY pipe read is affected, not this fixture's.
Fix: the timestamp projection in crate::state (state.rs around lines 1296-1302) and the DescribePipe output construction that calls it (ops/describe_pipe.rs around lines 220-221); the stamp must be normalised to the granularity AWS reports instead of passing the host clock's sub-second remainder through.
Traces: pipes-error-conflict-and-source-notfound#task#11 (representative, this issue's replay key); same cluster also at pipes-filter-criteria-nested-json-preserved#task#8, pipes-list-pipes-filtering#task#12 and #task#14, pipes-list-pipes-pagination#task#8/#task#10/#task#12, pipes-pipe-error-paths#task#8.

Cause 2 of 4: DeletePipe reports CreationTime and LastModifiedTime the same way.
AWS: same model — DeletePipe DELETE /v1/pipes/{Name} -> 200, @idempotent (errors ConflictException 409, NotFoundException 404, ValidationException 400, ThrottlingException 429, InternalException 500); output CreationTime and LastModifiedTime target the same Timestamp shape, restJson1, no @timestampFormat, so epoch-seconds numbers whose shape — not instant — the recording asserts, with no sub-second tail.
Emulator: the delete handler stamps LastModifiedTime with ::std::time::SystemTime::now() and projects both members through the same helper (services/service-com-amazonaws-pipes-pipes/src/ops/delete_pipe.rs lines 184 and 191-192), so the wound-down record is reported with the millisecond tail. Replay verdict mismatch, expected_status 200, actual_status 200, value_mismatch at $.CreationTime and $.LastModifiedTime (observed 1794393610.361 and 1794393625.453 in that run — per-run host clock, not requirements).
Why: same consequence as Cause 1, on the last record a caller ever sees for a pipe; it is listed separately because it is a different operation with its own output construction and the replay clusters it separately, so a fix applied only to the read path would leave the delete response diverging.
Fix: the DeletePipe output construction (ops/delete_pipe.rs around lines 186-193) and the shared crate::state::timestamp projection it calls.
Traces: pipes-list-pipes-pagination#teardown#17; same cluster also at pipes-list-pipes-pagination#teardown#18 and #teardown#19.

Cause 3 of 4: TagResource's empty 204 response carries no content-length header.
AWS: same model — TagResource POST /tags/{resourceArn}, @idempotent, output shape TagResourceResponse with NO members (the @http code is the protocol default, code 200 codeDefaulted), so the response carries no content. Recorded behavior only for the framing: the recorded AWS response for this call is a 204 whose body length is stated explicitly with a content-length header (the literal 0 for an always-empty body — a fixed value for this response class, not a per-run quantity), and the replay asserts that header's presence by shape.
Emulator: answers 204 with the header absent entirely — replay reports $headers.content-length: missing (expected <shaped-not-asserted>, got None), verdict mismatch, expected_status 204, actual_status 204. The status and the empty body are correct; only the framing header is gone. The service's own notes say the framing is applied for every no-content answer through service_common::no_content_responses::without_content from the override_protocol seam (services/service-com-amazonaws-pipes-pipes/src/service.rs lines 79-100) and that the HTTP writer the vendored listener is built on then DROPS content-length from a status that cannot carry a body — so the header never reaches the client, which is exactly what the replay observes.
Why: a client or intermediary that reads framing strictly sees a response whose body length is unstated instead of explicitly zero. Proxies, caching layers, SDK response-length validation and any HTTP-level assertion that an empty mutation response is explicitly zero-length behave differently against the emulator than against AWS, and the difference is invisible in the parsed API result, which makes it hard to diagnose.
Fix: the no-content response framing this service's responses pass through — the override_protocol seam in crate::service (service.rs around lines 89-100) together with the shared service_common::no_content_responses::without_content builder and the vendored HTTP/1.1 response writer that currently strips content-length from a 204; the empty 204 must reach the client carrying content-length: 0.
Traces: pipes-pipe-tag-management#task#10; same cluster also at pipes-tag-empty-value-vs-absent-preserved#task#9, pipes-tag-value-unicode-preserved#task#9, pipes-tagging-workflow#task#10.

Cause 4 of 4: UntagResource's empty 204 response carries no content-length header either.
AWS: same model — UntagResource DELETE /tags/{resourceArn}?tagKeys=..., @idempotent, output shape UntagResourceResponse with no members; same framing contract as Cause 3, an empty response whose length is stated explicitly.
Emulator: answers 204 with the header absent — $headers.content-length: missing (expected <shaped-not-asserted>, got None), verdict mismatch, expected_status 204, actual_status 204.
Why: as Cause 3. It is its own cause because it is a separate operation on a different HTTP method (DELETE) and the replay clusters it separately; if one shared no-content builder serves both, a single fix closes Causes 3 and 4, but BOTH steps must still be re-replayed.
Fix: the same no-content framing path (crate::service override_protocol / service_common::no_content_responses::without_content / the vendored HTTP writer).
Traces: pipes-pipe-tag-management#task#12; same cluster also at pipes-tag-empty-value-vs-absent-preserved#task#11, pipes-tagging-workflow#task#12.

Relation to open issues: 20261030T071334Z-pipes-conflictexception-reports-resourcetype-pipe-instead-of-pipe.md owns the four ConflictException resourceType clusters this replay also reported (CreatePipe/UpdatePipe/StartPipe/StopPipe), which are skipped here. No open pipes issue touches the pipe record's timestamp shape or the 204 framing of the tagging operations (the open pipes issues are about delivery, StateReason, filter criteria, templates and CloudFormation); the Content-Length-on-204 family is filed per service elsewhere (e.g. 20261109T115329Z-mediapackagev2-..., 20261109T161717Z-batch-...), and none of those covers pipes.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, bootstrap_signal=false, 18 fixtures, 295 steps, 267 passed, 28 mismatched, 0 errored, 0 blocked, score 90.5%, 8 divergence clusters — all service=pipes. The four clusters below all report empty stub_path and empty owned_by. (The other four clusters are the ConflictException resourceType 'pipe' vs 'Pipe' mismatches, each reporting owned_by 20261030T071334Z-pipes-conflictexception-reports-resourcetype-pipe-instead-of-pipe.md, and are not re-filed here.)

Cause 1 — DescribePipe, fixture pipes-error-conflict-and-source-notfound step task#11, verdict mismatch, expected_status 200, actual_status 200, divergence_key service=pipes|DescribePipe|status=200->200|keys=$.CreationTime,$.LastModifiedTime, affected_count 8:
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794393626.384)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794393626.384)
  affected: pipes-error-conflict-and-source-notfound#task#11, pipes-filter-criteria-nested-json-preserved#task#8, pipes-list-pipes-filtering#task#12, pipes-list-pipes-filtering#task#14, pipes-list-pipes-pagination#task#8, pipes-list-pipes-pagination#task#10, pipes-list-pipes-pagination#task#12, pipes-pipe-error-paths#task#8

Cause 2 — DeletePipe, fixture pipes-list-pipes-pagination step teardown#17, verdict mismatch, expected_status 200, actual_status 200, divergence_key service=pipes|DeletePipe|status=200->200|keys=$.CreationTime,$.LastModifiedTime, affected_count 3:
  $.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1794393610.361)
  $.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1794393625.453)
  affected: pipes-list-pipes-pagination#teardown#17, #teardown#18, #teardown#19

Cause 3 — TagResource, fixture pipes-pipe-tag-management step task#10, verdict mismatch, expected_status 204, actual_status 204, divergence_key service=pipes|TagResource|status=204->204|keys=$headers.content-length, affected_count 4:
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  affected: pipes-pipe-tag-management#task#10, pipes-tag-empty-value-vs-absent-preserved#task#9, pipes-tag-value-unicode-preserved#task#9, pipes-tagging-workflow#task#10

Cause 4 — UntagResource, fixture pipes-pipe-tag-management step task#12, verdict mismatch, expected_status 204, actual_status 204, divergence_key service=pipes|UntagResource|status=204->204|keys=$headers.content-length, affected_count 3:
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  affected: pipes-pipe-tag-management#task#12, pipes-tag-empty-value-vs-absent-preserved#task#11, pipes-tagging-workflow#task#12

The epoch values quoted above are that run's host clock and are NOT requirements; the requirement is the timestamp shape AWS emits for those members. The content-length value for an always-empty 204 is the fixed literal 0, not a per-run quantity, so neither cause is a request-id/clock-style difference.

Model consulted (smithy_query query=operation/shape/service_info, services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json):
  service Pipes, version 2015-10-07, protocols [restJson1], sigv4Name pipes
  DescribePipe GET /v1/pipes/{Name} -> 200; output CreationTime/LastModifiedTime -> com.amazonaws.pipes#Timestamp (timestamp), no @timestampFormat anywhere on the member or the shape
  DeletePipe DELETE /v1/pipes/{Name} -> 200, @idempotent; output CreationTime/LastModifiedTime -> same Timestamp shape
  TagResource POST /tags/{resourceArn} (code 200, codeDefaulted), output TagResourceResponse: no members
  UntagResource DELETE /tags/{resourceArn} (code 200, codeDefaulted), output UntagResourceResponse: no members

Emulator code consulted: services/service-com-amazonaws-pipes-pipes/src/state.rs lines 1296-1302 (pub fn timestamp -> DateTime::from_millis of the stored SystemTime), src/ops/describe_pipe.rs lines 220-221, src/ops/delete_pipe.rs lines 184 and 191-192, src/ops/tag_resource.rs lines 148-156 (SUCCESS_STATUS 204, 'the empty body such an answer carries is framed once for the whole service, in crate::service'), src/service.rs lines 79-100 (no-content framing through service_common::no_content_responses::without_content, with the note that the vendored listener's HTTP writer drops content-length from a status that cannot carry a body).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout; replay the simparity-wire-aws-v1 wire corpus, or reproduce at the HTTP level against the emulator endpoint (inspect the RAW responses, not the CLI/SDK-formatted result, which reformats timestamps and hides headers).
2. Cause 1: CreatePipe(Name=p, ...) then GET /v1/pipes/p (DescribePipe). In the raw JSON body, $.CreationTime and $.LastModifiedTime are epoch-seconds numbers carrying a millisecond fraction (e.g. ...626.384) instead of the timestamp shape AWS reports for these members. Fixture: pipes-error-conflict-and-source-notfound#task#11.
3. Cause 2: DELETE /v1/pipes/p (DeletePipe). The same two members in the delete response carry the same millisecond fraction. Fixture: pipes-list-pipes-pagination#teardown#17.
4. Cause 3: POST /tags/<url-encoded pipe ARN> with body {"tags":{"k":"v"}} (TagResource). The response is 204 with an empty body and NO content-length header, where AWS sends content-length: 0. Fixture: pipes-pipe-tag-management#task#10.
5. Cause 4: DELETE /tags/<url-encoded pipe ARN>?tagKeys=k (UntagResource). Same observation: 204 with no content-length header. Fixture: pipes-pipe-tag-management#task#12.
6. Pass criteria: (Causes 1-2) CreationTime/LastModifiedTime are emitted in the timestamp shape the recorded AWS responses carry, with no host sub-second remainder, on both the read and the delete path; (Causes 3-4) both empty 204 responses reach the client carrying content-length: 0, with the status still 204 and the body still empty. Re-replay pipes-error-conflict-and-source-notfound#task#11, pipes-list-pipes-pagination#teardown#17, pipes-pipe-tag-management#task#10 AND pipes-pipe-tag-management#task#12; all four must pass.
