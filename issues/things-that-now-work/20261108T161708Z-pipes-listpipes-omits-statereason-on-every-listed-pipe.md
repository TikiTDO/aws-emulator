Issue-Source: trace-sweep-divergence
Issue-Trace: pipes-list-pipes-filtering
Issue-Step: task#15
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: pipes-list-pipes-filtering pipes-list-pipes-pagination pipes-sqs-to-sqs-lifecycle
Issue-Shape-Id: com.amazonaws.pipes#Pipes
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-a-4af7edb5"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-list-b-975dda47"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-1e168bce-0"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-1e168bce-1"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-probe-pipes-listp-1e168bce-2"].StateReason
Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn:aws:pipes:us-west-2:000000000000:pipe/sim2real-pipes-s2s-dfdaa29e"].StateReason
Issue-Commit: 3a2a2ae2b1ffef990857b0544f1895e5cc0c5b77

# pipes ListPipes omits StateReason on every listed pipe

> _Note: this issue was identified against the code base at commit `3a2a2ae2b1ffef990857b0544f1895e5cc0c5b77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipes

## Description
This bundle has 6 causes. Every one is a ListPipes 200 whose body is otherwise correct but whose `Pipes[]` entries carry NO `StateReason`, across three different fixtures and across filtered, paginated and post-lifecycle listings. A fixer must close all 6 before submitting; they share one root cause but are reported as six independent replay clusters (different pipes, different query shapes, different pages), so each must be re-replayed.

Shared fault: the stored pipe record's state-reason field is only ever written when a DELAYED state transition settles (src/state.rs, PipeRecord::settle writes `self.state_reason = settled.reason`). A pipe that reaches its state because the CALLER asked for it — CreatePipe with a DesiredState, StartPipe, StopPipe — leaves `state_reason` as None, so `PipeRecord::summary()` (src/state.rs:988-1001, which does copy `state_reason` through) projects an absent member and ListPipes reports a pipe with no reason at all. The defect is therefore in the state writes of the lifecycle operations, not in the ListPipes projection.

Note: open issue 20261030T071334Z-pipes-responses-that-describe-a-pipe-drop-modeled.md describes a ListPipes/StateReason cause (its "Cause 3 of 8") and attributes it to the summary projection, but its recorded replay key is truncated (`Issue-Key: pipes|ListPipes|2xx->2xx|$.Pipes[Arn="arn`), so these clusters replay as UNOWNED and are re-filed here with the intact bindings and with the actual root cause (the unset state-reason on user-initiated transitions, not the projection).

Cause 1 of 6: ListPipes with a NamePrefix filter returns the matching pipe without StateReason.
  AWS: ListPipes (Smithy: GET /v1/pipes, 200, output ListPipesResponse.Pipes: PipeList of Pipe) returns Pipe.StateReason (target PipeStateReason, string @length(0,512), @pattern ^.*$), documented "The reason the pipe is in its current state." For a pipe whose current state is the one the caller asked for, the recordings assert the fixed token "USER_INITIATED"; the token is the state-reason AWS reports for a caller-driven transition, it is not derived from the request.
  Emulator: the listed Pipe has StateReason absent (serialized as None/omitted).
  Why: a client that lists pipes to triage which ones are unhealthy or why one is stopped gets no reason on any entry and cannot distinguish a user-stopped pipe from a failed one; it must fall back to DescribePipe per pipe, which omits the member too.
  Fix: the state writes of the lifecycle/create handlers (CreatePipe, StartPipe, StopPipe) must set the pipe record's state-reason to the user-initiated reason token when the caller drives the transition, in the pipe record's state (services/service-com-amazonaws-pipes-pipes/src/state.rs) rather than only on settled delayed transitions.
  Traces: pipes-list-pipes-filtering#task#15.

Cause 2 of 6: the same ListPipes filtering fixture's second listed pipe also comes back without StateReason.
  AWS: as cause 1 — every Pipe entry carries StateReason, independent of which pipe it is.
  Emulator: StateReason absent on that entry as well.
  Why: as cause 1; the omission is not specific to one pipe, so no entry in a listing is usable for state triage.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-filtering#task#16.

Cause 3 of 6: the FIRST page of a paginated ListPipes returns its entries without StateReason.
  AWS: as cause 1 — a paged listing's entries are the same Pipe shape with StateReason populated.
  Emulator: every entry on the page is missing StateReason.
  Why: a paging client accumulating pipe state across pages ends with a complete inventory in which no pipe has a reason.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#13.

Cause 4 of 6: the FOLLOWING page of the same paginated ListPipes returns its entry without StateReason.
  AWS: as cause 1 — resuming with a NextToken does not change which members an entry carries.
  Emulator: StateReason absent on the continuation page's entry.
  Why: as cause 3; the gap is uniform over pages, so it cannot be worked around by re-reading a page.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#14.

Cause 5 of 6: an unpaged ListPipes over the same pipes returns all entries without StateReason.
  AWS: as cause 1.
  Emulator: StateReason absent on each of the listed pipes.
  Why: as cause 1; confirms the omission is in the record, not in the paging path.
  Fix: same component as cause 1.
  Traces: pipes-list-pipes-pagination#task#15.

Cause 6 of 6: ListPipes after a start/stop lifecycle run on an SQS-to-SQS pipe returns the pipe without StateReason.
  AWS: as cause 1 — after a caller-driven StartPipe/StopPipe the pipe's StateReason is the user-initiated reason token, which is exactly the case where AWS populates it.
  Emulator: StateReason absent, so the one transition that should have written a reason wrote none.
  Why: a client polling a start/stop by listing pipes never learns that the state it sees was caller-requested, and a lifecycle failure would be indistinguishable from success.
  Fix: same component as cause 1 — specifically the StartPipe/StopPipe state writes (src/ops/start_pipe.rs, src/ops/stop_pipe.rs) alongside the record's state-reason field.
  Traces: pipes-sqs-to-sqs-lifecycle#task#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout: 280 steps, 237 passed, 43 mismatched, score 84.6%; 6 of the 20 mismatch clusters are ListPipes with an empty owned_by and an empty stub_path. All six are expected_status == actual_status == 200, body-only:
- rep pipes-list-pipes-filtering#task#15 (affected 1): "$.Pipes[Arn=\"arn:aws:pipes:<region>:<account>:pipe/<name>\"].StateReason: missing (expected 'USER_INITIATED', got None)".
- rep pipes-list-pipes-filtering#task#16 (affected 1): same detail for the fixture's second pipe.
- rep pipes-list-pipes-pagination#task#13 (affected 1): the same missing-StateReason detail for two entries on one page.
- rep pipes-list-pipes-pagination#task#14 (affected 1): same, continuation page, one entry.
- rep pipes-list-pipes-pagination#task#15 (affected 1): same, three entries in one unpaged listing.
- rep pipes-sqs-to-sqs-lifecycle#task#11 (affected 1): same, after StartPipe/StopPipe.
Model evidence (services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json): ListPipes GET /v1/pipes -> 200, output member Pipes: PipeList of com.amazonaws.pipes#Pipe; Pipe.StateReason targets PipeStateReason (@length min 0 max 512, @pattern ^.*$) documented "The reason the pipe is in its current state."
Code evidence: src/ops/list_pipes.rs:284 projects each record through crate::state::PipeRecord::summary; src/state.rs:988-1001 summary() copies self.state_reason through unchanged, and src/state.rs:952-954 (PipeRecord::settle) is the only write of state_reason — it fires only for a delayed transition that became due, so a caller-driven create/start/stop leaves it None.
The ARNs, pipe names and per-run suffixes in the recorded details are per-run fillers and are not part of the requirement; the requirement is that Pipe.StateReason is PRESENT on every listed pipe and carries the state-reason token AWS reports for how the pipe reached its current state ("USER_INITIATED" for a caller-driven transition).

## Steps to Reproduce
1. Replay fixture pipes-list-pipes-filtering, step task#15: create two pipes with an SQS source, then call ListPipes with a NamePrefix filter. The response is 200 and matches on every other member, but the matching Pipes[] entry has no StateReason (AWS: "USER_INITIATED").
2. Replay pipes-list-pipes-filtering#task#16 — the fixture's second filtered listing: same omission on the other pipe.
3. Replay pipes-list-pipes-pagination#task#13 and #task#14 — ListPipes with Limit set and then the returned NextToken: every entry on both the first and the continuation page is missing StateReason.
4. Replay pipes-list-pipes-pagination#task#15 — the unpaged listing of the same three pipes: all three entries are missing StateReason.
5. Replay pipes-sqs-to-sqs-lifecycle#task#11 — create an SQS-to-SQS pipe, run StartPipe/StopPipe, then ListPipes: the pipe is listed with no StateReason even though the state it rests in was caller-requested.
