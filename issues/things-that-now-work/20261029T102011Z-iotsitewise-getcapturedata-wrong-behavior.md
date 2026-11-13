Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 99c19747b30de4fba88ebdfb7af58fcb882c179f

# GetCaptureData is unreachable over the wire: no operation of this service ever records a capture source, so every request 404s on the time series it names

_Filed: 2026-10-29T10:20:11Z_
_Operation: GetCaptureData | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `99c19747b30de4fba88ebdfb7af58fcb882c179f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

GetCaptureData is fully implemented against `store.capture_sources` — it resolves the source by `timeSeriesId` or by the `propertyAlias` bound to it, selects the stored segments overlapping the requested window, pages them one segment per answer and mints a resume token. But nothing in `src/ops/` ever writes a capture source: `put_capture_segment` — which `src/state.rs` names as the writer of that map, and whose own doc comment says its typed refusals exist "so the producer handlers this serves can map one refusal to one modelled error" — has no caller anywhere in the crate's handlers. Its only callers are test harnesses that reach into `crate::state` directly, one of which calls it "the producer entry point on this service's state — the one writer of a workspace's video, which is what `GetCaptureData` reads back".

The consequence is that every GetCaptureData request over HTTP falls into the not-found arm (404 ResourceNotFoundException naming the time series), because `capture_source_by_id` / `capture_source_by_alias` can only ever answer `None`. The same map is, per its own field doc, what the workspace-scoped forms of the time-series operations read, so those are empty for the same reason — i.e. the fix for the (already closed) workspace-scoped ListTimeSeries gap reads state that no request can create.

This is a handler implemented against state the service's own API surface cannot produce: the stated producer side of the feature is missing, not the reader.

## Evidence

The state module naming the writer and the readers — services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/state.rs:4030-4037:
  "/// The capture sources (video streams) of every workspace ... Written by [`Self::put_capture_segment`], read by `GetCaptureData` and by the workspace-scoped forms of the time-series operations, whose series these are, and removed by [`Self::forget_capture_source`].
     pub capture_sources: HashMap<String, HashMap<String, CaptureSourceRecord>>,"

The writer's own claim that handlers call it — src/state.rs:4999-5002:
  "/// The `Err` names WHICH rule refused the write ([`CaptureWriteRefusal`]), so the producer handlers this serves can map one refusal to one modelled error instead of re-deriving the checks — and nothing is written on any of them."
  followed by src/state.rs:5008 "pub fn put_capture_segment(".

No such producer handler exists: grep for `put_capture_segment` across the crate matches only src/state.rs (its definition and doc links) and three test files — tests/all/capture_data.rs:161, tests/all/workspace_time_series.rs:175, tests/all/delete_time_series_data_segments.rs:353. Not one file under src/ops/ calls it. (grep for `capture_source` under src/ops/ matches only the two READ lookups in get_capture_data.rs:231,234 and the removal in delete_time_series.rs:270.)

The handler that therefore always refuses — src/ops/get_capture_data.rs:229-242:
  "let source = match identifier { Identifier::TimeSeriesId(time_series_id) => store.capture_source_by_id(&input.workspace_name, time_series_id), Identifier::Alias(alias) => store.capture_source_by_alias(&input.workspace_name, alias) }
   .ok_or_else(|| not_found(crate::state::not_found_message(crate::state::RESOURCE_TIME_SERIES, identifier.requested())))?;"
  on the strength of its own header, src/ops/get_capture_data.rs:8-10 — "A capture source is a time series of a workspace ... and its video is kept as the segments the producer ingested ([`crate::state::CaptureSegment`])."

The test harness stating that the producer is only reachable in-process — tests/all/capture_data.rs:148-151:
  "/// Seed `segments` on the `(workspace, time_series_id)` capture source, through the producer entry point on this service's state — the one writer of a workspace's video, which is what `GetCaptureData` reads back."

Test coverage: every GetCaptureData assertion runs against state seeded by that in-process helper (tests/all/capture_data.rs:151-180 and the suites below it), so no test exercises — or pins — what an HTTP-only client sees.

## Steps to Reproduce

1. POST /workspaces with workspaceName=press-floor -> 200.
2. Exercise every operation this service exposes for that workspace (CreateBulkImportJob into it, BatchPutAssetPropertyValue, CreateDataset/BatchAssociateDataSegmentsToDataset, AssociateTimeSeriesToAssetProperty, ...). None of them calls `put_capture_segment`, so `store.capture_sources` stays empty.
3. GET /timeseries?workspaceName=press-floor -> 200 with an empty timeSeriesSummaries list, since the workspace-scoped form reads the same map (src/state.rs:4034-4036).
4. POST /workspaces/press-floor/get-capture-data with {"startTime":{...},"endTime":{...},"timeSeriesId":"<any id>"} (or with propertyAlias).
5. Observed: 404 ResourceNotFoundException naming the time series, for every identifier, always — the 200 path of src/ops/get_capture_data.rs:280-304 is unreachable without an in-process `store.put_capture_segment` call.
6. Expected: the state GetCaptureData reads is producible through this service's API, as src/state.rs:4999-5002 says ("the producer handlers this serves"); then a window covering ingested video answers 200 with that segment's bytes, span and dataType.
