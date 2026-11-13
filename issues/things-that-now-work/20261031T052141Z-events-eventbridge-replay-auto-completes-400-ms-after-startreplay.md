Issue-Source: trace-sweep-divergence
Issue-Trace: events-replay-archived-events-start-monitor-cancel
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: events-replay-archived-events-start-monitor-cancel
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CancelReplay|2xx->4xx|$.ReplayArn
Issue-Key: events|CancelReplay|2xx->4xx|$.State
Issue-Key: events|CancelReplay|2xx->4xx|status
Issue-Key: events|DescribeReplay|2xx->2xx|$.State
Issue-Commit: 2e22400542df9bfa6316774997ff300306f2d2d8

# EventBridge replay auto-completes 400 ms after StartReplay, so the CANCELLING/CANCELLED branch is unreachable

> _Note: this issue was identified against the code base at commit `2e22400542df9bfa6316774997ff300306f2d2d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelReplay

## Description
The EventBridge replay state machine settles every started replay to COMPLETED unconditionally, a fixed 400 ms after StartReplay, and nothing else can move it. Any client that does what AWS documents — start a replay, poll DescribeReplay to watch its progress, then stop it — finds the replay already terminal by the time the cancel is issued, so the whole cancellation branch of the replay lifecycle (CANCELLING, then terminal CANCELLED with a ReplayEndTime) can never be observed. The two causes below are the two wire steps that fall out of that single state machine, and BOTH must be closed and re-replayed.

Cause 1 of 2: CancelReplay of a replay the client is still monitoring is refused IllegalStatusException (HTTP 400) instead of being accepted HTTP 200 in CANCELLING.
AWS: Smithy (com.amazonaws.eventbridge#CancelReplay) models input {ReplayName required, @length 1..64, @pattern ^[\.\-_A-Za-z0-9]+$} and output {ReplayArn, State (ReplayState: STARTING|RUNNING|COMPLETED|CANCELLING|CANCELLED|FAILED), StateReason}; modeled errors are exactly ConcurrentModificationException (400), IllegalStatusException (400), InternalException (500) and ResourceNotFoundException (400). A replay re-publishing an archive window is asynchronous and runs for as long as its window takes ("You can use DescribeReplay to determine the progress of a replay", StartReplay), so a replay that has been started and is still STARTING/RUNNING is cancellable: AWS answers HTTP 200 with the replay's ReplayArn and State=CANCELLING. IllegalStatusException is reserved for a replay whose state this call cannot change — AWS words that condition "An error occurred because a replay can be canceled only when the state is Running or Starting" (the IllegalStatusException doc), i.e. one already terminal or already cancelling. Recorded behavior for this fixture: HTTP 200, ReplayArn = the replay's ARN (arn:aws:events:<region>:<account>:replay/<ReplayName>, values from the request and the emulator's own account/region), State = 'CANCELLING'.
Emulator: HTTP 400 with the modeled IllegalStatusException, message 'Replay <ReplayName> is not in a valid state for this operation.' (fixed text 'Replay ' + the request's ReplayName + ' is not in a valid state for this operation.', from crate::state::replay_not_cancellable). No ReplayArn and no State are returned at all. The refusal is produced because services/service-com-amazonaws-eventbridge-awsevents/src/ops/cancel_replay.rs calls store.settle_replays(now) BEFORE it reads the record and guards on ReplayRecord::is_cancellable(), and settle_replays has by then flipped the replay to the COMPLETED state ReplayRecord::new seeded as due at started + REPLAY_TRANSITION (400 ms). So the guard is evaluated against a state the clock manufactured, not against the state the client is monitoring.
Why: a client that must stop a backfill flooding its consumers has no way to do it — the operation that exists for exactly that answers a client error, and because the error is a modeled 400 an SDK treats it as a permanent refusal and does not retry. Worse, the replay the client tried to stop is reported as having run to completion, so the client believes its whole window was re-published.
Fix: the replay state machine in services/service-com-amazonaws-eventbridge-awsevents/src/state.rs — a started replay must remain in a cancellable state (STARTING, then RUNNING) until its window is actually drained or a cancel arrives, instead of being seeded with an unconditional clock-owed COMPLETED; and the CancelReplay handler must decide cancellability from the replay's own progress rather than from a pre-call settle. Identically scaffolded sibling under services/service-com-amazonaws-cloudwatchevents-awsevents/src/ops/.
Traces: events-replay-archived-events-start-monitor-cancel#task#6.

Cause 2 of 2: after the cancel, DescribeReplay reports the replay terminal in COMPLETED, where AWS reports CANCELLED.
AWS: Smithy (com.amazonaws.eventbridge#DescribeReplay) output carries State (ReplayState) plus ReplayStartTime/ReplayEndTime/EventLastReplayedTime; errors are InternalException (500) and ResourceNotFoundException (400). A replay that was cancelled settles in CANCELLED and stays there — a stopped replay did not finish its window, so COMPLETED is the wrong terminal state for it. Recorded behavior: polling DescribeReplay after the accepted cancel converges on State='CANCELLED' (and COMPLETED is a terminal state the poll explicitly treats as a failure, because it can no longer become CANCELLED).
Emulator: HTTP 200 with State='COMPLETED' on the first poll attempt, so the poll fails immediately rather than converging. The emulator's own state.rs says the cancelled state 'REPLACES the COMPLETED the clock owed the replay' (ReplayRecord::cancel), but because Cause 1 refuses the cancel, ReplayRecord::cancel is never reached and the COMPLETED transition stands.
Why: a client cannot distinguish a replay it successfully stopped from one that re-published its entire window — the two have opposite meanings for data correctness (a cancelled replay left part of its window un-replayed). Any automation that waits for CANCELLED after a cancel hangs or fails, and anything keying off ReplayEndTime attributes a completed run to a replay that was stopped.
Fix: same component — once the cancellation is accepted, the terminal state settle_replays materialises for that replay must be CANCELLED (with its ReplayEndTime), not the previously owed COMPLETED, in services/service-com-amazonaws-eventbridge-awsevents/src/state.rs (ReplayRecord::cancel / settle_replays) and reported unchanged by the DescribeReplay handler.
Traces: events-replay-archived-events-start-monitor-cancel#task#7.

Not covered by any existing issue: 20261025T115007Z-events-eventbridge-replay-cancelreplay-is-a-501-stub-and.md (fixed) described CancelReplay answering 501 NotImplemented — the handler is now implemented, and this is a different signature (a modeled 400 on the success path). 20261027T000839Z-events-sqs-an-accepted-eventbridge-replay-still-re.md (open) is about an accepted replay not re-emitting events to the destination bus, not about its lifecycle states. Neither carries these replay keys; the replay report marks both of these clusters unowned.

## Evidence
Fixture events-replay-archived-events-start-monitor-cancel (bus + ENABLED archive + PutEvents + StartReplay + DescribeReplay all pass; steps 1-5 pass).

Cause 1 — step task#6, CancelReplay [mismatch]:
  status: expected 200, got 400
  $.ReplayArn: missing (expected the replay's ARN 'arn:aws:events:<region>:<account>:replay/<ReplayName>', got None)
  $.State: missing (expected 'CANCELLING', got None)
Expected: HTTP 200, {ReplayArn: <arn of the replay named in the request>, State: 'CANCELLING'}.
Actual: HTTP 400, modeled IllegalStatusException with message 'Replay <ReplayName> is not in a valid state for this operation.' — fixed text 'Replay ' + request ReplayName + ' is not in a valid state for this operation.'
Mechanism: state.rs REPLAY_TRANSITION = Duration::from_millis(400) (line 1665); ReplayRecord::new seeds pending = {due: started + REPLAY_TRANSITION, state: REPLAY_STATE_COMPLETED} (lines 1749-1752); cancel_replay.rs calls store.settle_replays(now) (line 179) before the is_cancellable() guard (line 195), so the replay is COMPLETED and is_cancellable() (state.rs 1762-1767, STARTING|RUNNING only) is false.

Cause 2 — step task#7, DescribeReplay [mismatch]:
  $.State: value_mismatch (expected 'CANCELLED', got 'COMPLETED')
  $.State: poll: 'COMPLETED' is in fail_on after 1 attempt(s) (expected 'CANCELLED', got 'COMPLETED')
Expected: the poll converges on State='CANCELLED' (terminal), with a ReplayEndTime.
Actual: State='COMPLETED' on the first attempt — a terminal state the poll cannot leave, so it fails at once. REPLAY_STATE_RUNNING (state.rs 1625) is never assigned anywhere in the crate, so the only states a replay ever reaches without a successful cancel are STARTING and COMPLETED.

## Steps to Reproduce
1. events.create_event_bus(Name=B); events.create_archive(ArchiveName=A, EventSourceArn=<B's arn>); poll describe_archive until State='ENABLED'.
2. events.put_events(...) onto B with Time values inside a past window.
3. events.start_replay(ReplayName=R, EventSourceArn=<A's arn>, EventStartTime/EventEndTime bracketing those Times, Destination={'Arn': <B's arn>}) -> 200, State='STARTING'.
4. events.describe_replay(ReplayName=R) to monitor progress (as StartReplay's documentation directs).
5. events.cancel_replay(ReplayName=R) -> HTTP 400 IllegalStatusException 'Replay R is not in a valid state for this operation.'. Expected HTTP 200 with {ReplayArn: <R's arn>, State: 'CANCELLING'}.  (Cause 1; the refusal appears for any cancel issued more than ~400 ms after step 3, i.e. effectively always.)
6. events.describe_replay(ReplayName=R) -> State='COMPLETED'. Expected the state to settle at 'CANCELLED' with a ReplayEndTime, never 'COMPLETED'.  (Cause 2)
Re-replay events-replay-archived-events-start-monitor-cancel steps task#6 and task#7 to confirm both causes are closed.
