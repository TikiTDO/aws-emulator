// SPDX-License-Identifier: CC0-1.0

//! The replay a `StartReplay` creates, and the `ReplayState` machine every
//! replay operation reads and writes.

use super::*;

/// The `ReplayState` a started replay is answered in.
///
/// "Starts the specified replay" (`StartReplay`) is the beginning of a job that
/// runs on its own afterwards, so the create answers a non-terminal state and
/// `DescribeReplay` is what reports its progress ("You can use DescribeReplay to
/// determine the progress of a replay").
pub const REPLAY_STATE_STARTING: &str = "STARTING";

/// The `ReplayState` of a replay that is re-publishing its window.
pub const REPLAY_STATE_RUNNING: &str = "RUNNING";

/// The `ReplayState` of a replay that has finished re-publishing its window.
pub const REPLAY_STATE_COMPLETED: &str = "COMPLETED";

/// The `ReplayState` a `CancelReplay` puts a replay in: the cancellation is
/// accepted and under way, and the replay has not settled yet.
pub const REPLAY_STATE_CANCELLING: &str = "CANCELLING";

/// The `ReplayState` a cancelled replay settles in — terminal, and NOT
/// `COMPLETED`: a replay that was stopped did not finish its window.
pub const REPLAY_STATE_CANCELLED: &str = "CANCELLED";

/// The `ReplayState` of a replay that stopped on a failure.
pub const REPLAY_STATE_FAILED: &str = "FAILED";

/// Whether `state` is a `ReplayState` no further transition leaves.
///
/// `CancelReplay` "can be canceled only when the state is Running or Starting"
/// (`IllegalStatusException`), so this is also what tells a replay that can still
/// be cancelled from one that cannot, and a terminal state is the one that earns
/// the replay its `ReplayEndTime`.
pub fn is_terminal_replay_state(state: &str) -> bool {
    matches!(
        state,
        REPLAY_STATE_COMPLETED | REPLAY_STATE_CANCELLED | REPLAY_STATE_FAILED
    )
}

/// How long a replay stays in one non-terminal `ReplayState` before the clock
/// owes it the next one.
///
/// A replay is asynchronous in AWS — the call answers a non-terminal state and
/// the caller polls `DescribeReplay` ("You can use DescribeReplay to determine
/// the progress of a replay") — so a replay answered `STARTING` works its way to
/// `COMPLETED` through the states [`next_replay_state`] lays out rather than
/// settling instantly, and one answered `CANCELLING` reaches `CANCELLED` a span
/// after the cancel. AWS publishes no duration for any of them (every one depends
/// on the window replayed); this is the emulator's own span for a clock-owed
/// transition, and it is stored as a DUE INSTANT rather than waited for. It is
/// long enough that a client doing what `StartReplay` directs — start, poll the
/// progress, then stop the replay — still finds the replay in flight when its
/// `CancelReplay` arrives.
pub const REPLAY_TRANSITION: Duration = Duration::from_secs(3);

/// The `ReplayState` the clock moves a replay to after `state`, and `None` for a
/// state nothing follows.
///
/// This is the whole progression of a replay, in one place: a started replay is
/// `STARTING` while it is being set up, `RUNNING` while it re-publishes its
/// window, and `COMPLETED` once that window is drained; a cancelled one is
/// `CANCELLING` until it settles `CANCELLED`. Both non-terminal states a started
/// replay passes through are states "a replay can be canceled" in
/// ([`ReplayRecord::is_cancellable`]), so a replay is stoppable for as long as it
/// has not finished its window, and a terminal state is one the clock never
/// leaves.
pub fn next_replay_state(state: &str) -> Option<&'static str> {
    match state {
        REPLAY_STATE_STARTING => Some(REPLAY_STATE_RUNNING),
        REPLAY_STATE_RUNNING => Some(REPLAY_STATE_COMPLETED),
        REPLAY_STATE_CANCELLING => Some(REPLAY_STATE_CANCELLED),
        _ => None,
    }
}

/// Largest number of replays this store holds.
///
/// This emulator's OWN bound, like [`MAX_ARCHIVES`]: a `StartReplay` past it
/// answers the modelled `LimitExceededException` instead of growing memory
/// without limit. It is far above any realistic fixture's needs.
pub const MAX_REPLAYS: usize = 100;

/// A `ReplayState` the clock already owes a replay, and the instant it falls due.
///
/// A replay's progress is driven by the clock alone once a call has started or
/// cancelled it, so the next state is STORED as a due instant rather than waited
/// for: whichever operation next touches the store materialises it
/// ([`ComAmazonawsEventbridgeAwseventsState::settle_replays`]).
#[derive(Debug, Clone)]
pub struct PendingReplayState {
    pub due: SystemTime,
    /// One of `ReplayState`'s values: what the replay becomes at `due`.
    pub state: String,
}

/// The state the clock owes a replay that has just entered `state` at
/// `entered`, and nothing for a state no transition follows.
///
/// Written once, so the state a start seeds, the state a cancel seeds and the
/// state a settle seeds next are all one derivation: a replay cannot be left
/// owing a transition its own progression does not have.
fn pending_after(state: &str, entered: SystemTime) -> Option<PendingReplayState> {
    next_replay_state(state).map(|next| PendingReplayState {
        due: entered + REPLAY_TRANSITION,
        state: next.to_string(),
    })
}

/// A replay as this emulator persists it.
///
/// "A replay processes events to replay based on the time in the event"
/// (`StartReplay`): the record holds the archive the events come from, the
/// window of event time that selects them, the destination they are re-published
/// onto, and the progress `DescribeReplay` reports — the state, when the replay
/// started and ended, and the event time of the last event it replayed.
#[derive(Debug, Clone)]
pub struct ReplayRecord {
    pub name: String,
    /// Minted ONCE, when the replay is started, and never recomputed.
    pub arn: String,
    /// The `ArchiveArn` the request named, reported back as given.
    pub event_source_arn: String,
    pub description: Option<String>,
    /// The `ReplayDestination` the request named: the bus the events are
    /// re-published onto and, when it carries them, the rules they are confined
    /// to.
    pub destination: ReplayDestination,
    /// The window of EVENT time the replay selects from the archive.
    pub event_start_time: SystemTime,
    pub event_end_time: SystemTime,
    /// One of `ReplayState`'s values.
    pub state: String,
    pub state_reason: Option<String>,
    pub replay_start_time: SystemTime,
    /// The state the clock already owes this replay, or `None` when it is in the
    /// state it will stay in until a caller changes it.
    pub pending: Option<PendingReplayState>,
    pub replay_end_time: Option<SystemTime>,
    /// "The value returned for EventLastReplayedTime indicates the time within
    /// the specified time range associated with the last event replayed"
    /// (`StartReplay`), so it is the EVENT time of that event and not a clock
    /// reading. `None` for a replay whose window selected no event.
    pub event_last_replayed_time: Option<SystemTime>,
}

impl ReplayRecord {
    /// A replay just started from `event_source_arn`: in [`REPLAY_STATE_STARTING`]
    /// and having replayed nothing yet, with the state that FOLLOWS starting due
    /// [`REPLAY_TRANSITION`] later ([`next_replay_state`]) — never a terminal
    /// state straight away, so the replay is still cancellable while a client
    /// monitors it. `Description` is written onto the record by its creator.
    pub fn new(
        name: &str,
        arn: String,
        event_source_arn: String,
        destination: ReplayDestination,
        event_start_time: SystemTime,
        event_end_time: SystemTime,
        started: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            arn,
            event_source_arn,
            description: None,
            destination,
            event_start_time,
            event_end_time,
            state: REPLAY_STATE_STARTING.to_string(),
            state_reason: None,
            replay_start_time: started,
            pending: pending_after(REPLAY_STATE_STARTING, started),
            replay_end_time: None,
            event_last_replayed_time: None,
        }
    }

    /// Whether a `CancelReplay` may still stop this replay: "a replay can be
    /// canceled only when the state is Running or Starting"
    /// (`IllegalStatusException`), so a replay that has settled — or is already
    /// cancelling — is not one this call can change.
    pub fn is_cancellable(&self) -> bool {
        matches!(
            self.state.as_str(),
            REPLAY_STATE_STARTING | REPLAY_STATE_RUNNING
        )
    }

    /// Accept a cancellation at `now`: the replay moves to
    /// [`REPLAY_STATE_CANCELLING`] and the clock owes it
    /// [`REPLAY_STATE_CANCELLED`] [`REPLAY_TRANSITION`] later, REPLACING the
    /// progress it was owed — a replay that was stopped settles cancelled, not
    /// completed, because it did not finish its window.
    pub fn cancel(&mut self, now: SystemTime) {
        self.state = REPLAY_STATE_CANCELLING.to_string();
        self.pending = pending_after(REPLAY_STATE_CANCELLING, now);
    }

    /// Materialise every state this replay was ALREADY owed at `now`, and answer
    /// whether any was.
    ///
    /// The whole progression is driven by the clock once a call has started or
    /// cancelled the replay, so a replay nothing read for longer than a span
    /// advances through every state it passed in the meantime — each from the
    /// instant the one before it fell DUE rather than from the instant this call
    /// noticed, so two reads of one replay cannot disagree on where it is or on
    /// when it ended.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        let mut settled = false;
        while let Some(owed) = self.pending.clone().filter(|owed| owed.due <= now) {
            self.state = owed.state;
            self.pending = pending_after(&self.state, owed.due);
            // A replay that has reached a state nothing leaves ended when that
            // state fell due; one still on its way reports no end yet.
            if is_terminal_replay_state(&self.state) {
                self.replay_end_time = Some(owed.due);
            }
            settled = true;
        }
        settled
    }

    /// Whether `time` — one archived event's own event time — lies in the window
    /// this replay selects.
    ///
    /// "Only events that occurred between the EventStartTime and EventEndTime are
    /// replayed" (`StartReplay$EventEndTime`), so the window is closed at both
    /// ends: an event at either bound occurred between them.
    pub fn replays(&self, time: SystemTime) -> bool {
        time >= self.event_start_time && time <= self.event_end_time
    }

    /// Every member `DescribeReplay` reports, read back from the stored record,
    /// so a read and what the replay did cannot disagree.
    pub fn to_describe_output(&self) -> DescribeReplayOutput {
        DescribeReplayOutput {
            replay_name: Some(self.name.clone()),
            replay_arn: Some(self.arn.clone()),
            description: self.description.clone(),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            event_source_arn: Some(self.event_source_arn.clone()),
            destination: Some(self.destination.clone()),
            event_start_time: Some(to_wire_timestamp(self.event_start_time)),
            event_end_time: Some(to_wire_timestamp(self.event_end_time)),
            event_last_replayed_time: self.event_last_replayed_time.map(to_wire_timestamp),
            replay_start_time: Some(to_wire_timestamp(self.replay_start_time)),
            replay_end_time: self.replay_end_time.map(to_wire_timestamp),
        }
    }

    /// The `Replay` projection `ListReplays` returns, read off the SAME stored
    /// record [`Self::to_describe_output`] reports — so a listing and a read of
    /// one replay cannot disagree. The summary carries every member of the
    /// `Replay` shape except the destination, which the shape does not have.
    pub fn to_summary(&self) -> ReplaySummary {
        ReplaySummary {
            replay_name: Some(self.name.clone()),
            event_source_arn: Some(self.event_source_arn.clone()),
            state: Some(self.state.clone()),
            state_reason: self.state_reason.clone(),
            event_start_time: Some(to_wire_timestamp(self.event_start_time)),
            event_end_time: Some(to_wire_timestamp(self.event_end_time)),
            event_last_replayed_time: self.event_last_replayed_time.map(to_wire_timestamp),
            replay_start_time: Some(to_wire_timestamp(self.replay_start_time)),
            replay_end_time: self.replay_end_time.map(to_wire_timestamp),
        }
    }
}
