// SPDX-License-Identifier: CC0-1.0

//! The `AWS/SQS` series a queue publishes about itself: the backlog gauges its depth is
//! sampled into, and the counters its data-plane calls are counted into.
//!
//! # What the series are
//!
//! A queue's backlog is not only three attributes `GetQueueAttributes` reports: Amazon SQS
//! publishes it as the `AWS/SQS` metrics [`APPROXIMATE_NUMBER_OF_MESSAGES_VISIBLE`],
//! [`APPROXIMATE_NUMBER_OF_MESSAGES_NOT_VISIBLE`] and
//! [`APPROXIMATE_NUMBER_OF_MESSAGES_DELAYED`], beside
//! [`APPROXIMATE_AGE_OF_OLDEST_MESSAGE`], each dimensioned by the queue it is about
//! ([`QUEUE_NAME_DIMENSION`]). Those series are what a queue-depth dashboard plots and what
//! every backlog alarm — including the ones a scaling policy on a queue is built from — is
//! evaluated against, so a queue whose depth is never published has no backlog to graph and
//! no breach to fire on.
//!
//! Beside them a queue publishes what its data plane DID: [`NUMBER_OF_MESSAGES_SENT`] and
//! [`SENT_MESSAGE_SIZE`] for the messages sent to it, [`NUMBER_OF_MESSAGES_RECEIVED`] and
//! [`NUMBER_OF_EMPTY_RECEIVES`] for what its consumers' polls got, and
//! [`NUMBER_OF_MESSAGES_DELETED`] for what they deleted. Those are counters of real calls:
//! each observation is one call's own contribution, so a `Sum` over a period is the traffic
//! that period carried, and a queue nothing called in a period has no datapoint in it. A
//! message that arrives in a queue because a redrive MOVED it there is not a message sent to
//! that queue, so nothing counts it as one; it shows up in that queue's depth, which is where
//! a dead-letter queue's growth is read.
//!
//! # What one sample says
//!
//! One sample of a queue is one reading of the SAME split
//! [`Queue::counts`](super::Queue::counts) reports to `GetQueueAttributes`: every message the
//! queue holds is counted in exactly ONE of the three series, by where it stands at the
//! instant it was sampled — delayed while its `DelaySeconds` has not elapsed, not visible
//! while it is in flight with a consumer, and visible otherwise. So a message sent with a
//! delay is counted in the delayed series and NOT in the visible one until that delay
//! expires, at which point the next sample counts it in the visible series instead; a reader
//! comparing the two series can tell a delayed backlog from a ready one, which is the whole
//! point of publishing them separately. The same reading carries the age of the oldest message
//! the queue could deliver, which a message still inside its delay is not part of.
//!
//! Each measurement of a reading is published as a gauge — one observation of its value, in
//! the unit that measurement is measured in — so an `Average` or a `Maximum` over a period is
//! what the queue stood at in it, and a period nothing sampled carries no datapoint rather
//! than a zero.
//!
//! # When a sample happens
//!
//! Once every [`DEPTH_PUBLISH_INTERVAL`] for as long as the queue exists, and once at the
//! moment it comes into being — so each period a reader can ask for carries exactly the one
//! reading taken in it, and the series is a sequence of readings of the queue rather than a log
//! of what happened to it. That is what makes an `Average`, a `Minimum` and a `Maximum` over a
//! period agree about the depth that period stood at, and it is why a split that changes
//! between two samples is reported by the next one rather than by an extra datapoint of its
//! own. A queue nobody has sent to reports zero in all three series, which is what tells an
//! idle queue from a queue that does not exist: nothing samples a queue this store does not
//! hold, so that one has no series at all.
//!
//! Two things drive those samples:
//!
//! * [`publish_queue_metrics`], called from the one seam every request of this service passes
//!   through ([`crate::service`]), so what a call did to a queue is published as part of
//!   serving it — the counters it contributed to, and the reading of its depth when one is
//!   due;
//! * [`drive_queue_metric_clock`], the one bounded in-process task per store, which samples the
//!   queues while the store holds one. A delay elapsing, a visibility timeout lapsing
//!   and a retention period running out are transitions the CLOCK makes due with nobody
//!   calling, and a client watching a queue's depth makes no further SQS call before it looks
//!   for them, so without the task those movements would never reach the series at all. It
//!   does not change WHEN a transition happens: it applies what the stored instants have
//!   already made due, exactly as the next call would have.
//!
//! Nothing here fails a request: the datapoints are published off the answer, so a CloudWatch
//! store this engine does not hold, or one at capacity, drops the sample and the request is
//! answered as it was.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch;

use super::{service_state, ComAmazonawsSqsAmazonsqsState, Queue, Visibility};

/// The namespace Amazon SQS publishes a queue's own metrics under.
pub const NAMESPACE: &str = "AWS/SQS";

/// The dimension that names the queue a series belongs to.
pub const QUEUE_NAME_DIMENSION: &str = "QueueName";

/// How many messages of the queue are available for retrieval — the series the queue
/// attribute `ApproximateNumberOfMessages` counts the same messages into.
pub const APPROXIMATE_NUMBER_OF_MESSAGES_VISIBLE: &str = "ApproximateNumberOfMessagesVisible";

/// How many are in flight: handed to a consumer and not yet deleted or returned.
pub const APPROXIMATE_NUMBER_OF_MESSAGES_NOT_VISIBLE: &str =
    "ApproximateNumberOfMessagesNotVisible";

/// How many are waiting for their delay to elapse before they can be retrieved at all.
pub const APPROXIMATE_NUMBER_OF_MESSAGES_DELAYED: &str = "ApproximateNumberOfMessagesDelayed";

/// How old the oldest message the queue could DELIVER is, in [`SECONDS_UNIT`]: the backlog's
/// age beside its size. A message still inside its `DelaySeconds` cannot be retrieved, so it
/// is not part of that backlog and the gauge reports zero for a queue holding only delayed
/// messages — see [`oldest_message_age`].
pub const APPROXIMATE_AGE_OF_OLDEST_MESSAGE: &str = "ApproximateAgeOfOldestMessage";

/// How many messages were added to the queue by sends.
pub const NUMBER_OF_MESSAGES_SENT: &str = "NumberOfMessagesSent";

/// How large each of those messages was, in [`BYTES_UNIT`].
pub const SENT_MESSAGE_SIZE: &str = "SentMessageSize";

/// How many messages the queue's consumers were handed.
pub const NUMBER_OF_MESSAGES_RECEIVED: &str = "NumberOfMessagesReceived";

/// How many `ReceiveMessage` calls were answered with no message.
pub const NUMBER_OF_EMPTY_RECEIVES: &str = "NumberOfEmptyReceives";

/// How many messages a consumer deleted from the queue.
pub const NUMBER_OF_MESSAGES_DELETED: &str = "NumberOfMessagesDeleted";

/// The unit a count of messages is published in: `Count`, the `StandardUnit` value for a
/// number of things.
pub const COUNT_UNIT: &str = "Count";

/// The unit a message SIZE is published in.
pub const BYTES_UNIT: &str = "Bytes";

/// The unit the age of the oldest message is published in.
pub const SECONDS_UNIT: &str = "Seconds";

/// The `StorageResolution` these datapoints are published at, in seconds: the regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// How often a queue's depth is sampled: once a minute, and exactly once.
///
/// One minute is the resolution the datapoints are stored at
/// ([`STANDARD_STORAGE_RESOLUTION_SECONDS`]) and the granularity the finest read of them can
/// ask for, so a queue that exists carries one reading in every period a reader can select —
/// including the zeroes of an idle one, which is what tells an idle queue from one that does
/// not exist — and no period ever holds two readings of the same queue to fold together.
pub const DEPTH_PUBLISH_INTERVAL: Duration = Duration::from_secs(60);

/// How often the clock looks at the store for a sample that has come due.
///
/// Not a publishing schedule — what is published is decided by [`sample_due`] alone — only how
/// soon after a minute opens the reading of it is taken.
const DEPTH_SAMPLE_TICK: Duration = Duration::from_secs(1);

/// How long a running clock's claim on the store stands before another call may start one.
///
/// The claim is a LEASE rather than a flag, renewed on every wake-up, so a clock whose task
/// ended without releasing it — one whose wake-up panicked, or that was cancelled with its
/// runtime — cannot stop the store from ever having a clock again: the lease lapses and the
/// next call starts a new one. Several ticks long, so a clock that is merely busy keeps it.
const DEPTH_CLOCK_LEASE: Duration = Duration::from_secs(5);

/// How many times one clock wakes up before it stands down.
///
/// A backstop, not a schedule: it bounds the work one request can start. Whatever the clock
/// has not reached when it is spent is sampled by the next clock — every request starts one
/// if none is running — or by the next request's own publish.
const MAX_DEPTH_CLOCK_WAKE_UPS: usize = 3_600;

/// The depth of one queue at one instant: how many of its messages stood in each of the three
/// states the series count. Every message the queue holds is in exactly one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QueueDepth {
    /// Messages available for retrieval.
    pub visible: usize,
    /// Messages in flight with a consumer.
    pub not_visible: usize,
    /// Messages whose delay has not elapsed.
    pub delayed: usize,
}

impl QueueDepth {
    /// The depth `queue` stands at `now`.
    ///
    /// The SAME reading `GetQueueAttributes` reports ([`Queue::counts`](super::Queue::counts)),
    /// so the attribute and the metric cannot disagree about what a queue holds.
    pub fn of(queue: &Queue, now: SystemTime) -> Self {
        let (visible, not_visible, delayed) = queue.counts(now);
        Self {
            visible,
            not_visible,
            delayed,
        }
    }
}

/// One READING of a queue: the split its messages stood in, and how long the oldest of them
/// had been there.
#[derive(Debug, Clone, Copy, Default)]
pub struct QueueReading {
    /// What the three depth gauges report.
    pub depth: QueueDepth,
    /// What [`APPROXIMATE_AGE_OF_OLDEST_MESSAGE`] reports.
    pub oldest_message_age: Duration,
}

impl QueueReading {
    /// The reading `queue` gives at `now`.
    pub fn of(queue: &Queue, now: SystemTime) -> Self {
        Self {
            depth: QueueDepth::of(queue, now),
            oldest_message_age: oldest_message_age(queue, now),
        }
    }

    /// What this reading reports in each series: the metric, the value and the unit it is
    /// measured in.
    fn observations(self) -> [(&'static str, f64, &'static str); 4] {
        [
            (
                APPROXIMATE_NUMBER_OF_MESSAGES_VISIBLE,
                count_as_f64(self.depth.visible),
                COUNT_UNIT,
            ),
            (
                APPROXIMATE_NUMBER_OF_MESSAGES_NOT_VISIBLE,
                count_as_f64(self.depth.not_visible),
                COUNT_UNIT,
            ),
            (
                APPROXIMATE_NUMBER_OF_MESSAGES_DELAYED,
                count_as_f64(self.depth.delayed),
                COUNT_UNIT,
            ),
            (
                APPROXIMATE_AGE_OF_OLDEST_MESSAGE,
                self.oldest_message_age.as_secs_f64().round(),
                SECONDS_UNIT,
            ),
        ]
    }
}

/// How long the oldest message `queue` could deliver at `now` has been in it, and no time at
/// all when it has none to deliver.
///
/// The age is of the oldest message the queue could HAND OVER: a message still inside its
/// `DelaySeconds` cannot be retrieved at all, so it is not part of the backlog whose age this
/// gauge reports and contributes nothing to it — the delayed series is where it is counted
/// while the delay is in force. One in flight with a consumer is a message the queue is
/// waiting to hear about, so it does count: it is still an undeleted message of the queue.
///
/// The span is counted from the instant the queue HOLDING the message enqueued it
/// ([`StoredMessage::retention_from`](super::StoredMessage::retention_from)) rather than from
/// the send that first created it, so the age a dead-letter queue reports is how long the
/// message has been in that queue — which is what the gauge is read for.
fn oldest_message_age(queue: &Queue, now: SystemTime) -> Duration {
    queue
        .messages
        .iter()
        .filter(|message| message.visibility(now) != Visibility::Delayed)
        .filter_map(|message| now.duration_since(message.retention_from).ok())
        .max()
        .unwrap_or_default()
}

/// Which counter one data-plane call is counted into.
///
/// Each is a count of what really happened on the queue: the messages a send added, how large
/// each of them was, the messages a consumer was handed, the polls that got none, and the
/// messages a consumer deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueTraffic {
    /// [`NUMBER_OF_MESSAGES_SENT`].
    MessagesSent,
    /// [`SENT_MESSAGE_SIZE`].
    SentMessageSize,
    /// [`NUMBER_OF_MESSAGES_RECEIVED`].
    MessagesReceived,
    /// [`NUMBER_OF_EMPTY_RECEIVES`].
    EmptyReceives,
    /// [`NUMBER_OF_MESSAGES_DELETED`].
    MessagesDeleted,
}

impl QueueTraffic {
    /// The metric this traffic is counted into.
    pub fn metric_name(self) -> &'static str {
        match self {
            Self::MessagesSent => NUMBER_OF_MESSAGES_SENT,
            Self::SentMessageSize => SENT_MESSAGE_SIZE,
            Self::MessagesReceived => NUMBER_OF_MESSAGES_RECEIVED,
            Self::EmptyReceives => NUMBER_OF_EMPTY_RECEIVES,
            Self::MessagesDeleted => NUMBER_OF_MESSAGES_DELETED,
        }
    }

    /// The unit that metric is measured in: a message size is bytes, everything else is a
    /// number of things.
    pub fn unit(self) -> &'static str {
        match self {
            Self::SentMessageSize => BYTES_UNIT,
            _ => COUNT_UNIT,
        }
    }
}

/// One observation of one counter that this service has not published yet: what happened, how
/// much of it, and when.
///
/// The counting happens where the call acts on the queue — under the store's lock — and the
/// publish happens with that lock released, in the CloudWatch store, so the observations wait
/// here in between.
#[derive(Debug, Clone, Copy)]
pub struct CountedTraffic {
    /// Which counter it belongs to.
    pub traffic: QueueTraffic,
    /// Its contribution to that counter.
    pub value: f64,
    /// The instant it happened at, which is the instant its datapoint carries.
    pub at: SystemTime,
}

/// How many counter observations one queue holds before the oldest are dropped.
///
/// The EMULATOR's own bound, so a store nobody publishes from cannot grow without end: the
/// observations are drained by the seam every request of this service passes through
/// ([`crate::service`]), so a queue only ever holds what the calls since the last request left.
const MAX_COUNTED_TRAFFIC_PER_QUEUE: usize = 4_096;

impl Queue {
    /// Count what one data-plane call did to this queue into the counter that counts it.
    ///
    /// A contribution of nothing is no observation, and neither is a negative or unmeasurable
    /// one: a counter counts what happened.
    pub fn count_traffic(&mut self, traffic: QueueTraffic, value: f64, at: SystemTime) {
        if !value.is_finite() || value <= 0.0 {
            return;
        }
        self.counted_traffic.push(CountedTraffic {
            traffic,
            value,
            at,
        });
        let overflow = self
            .counted_traffic
            .len()
            .saturating_sub(MAX_COUNTED_TRAFFIC_PER_QUEUE);
        if overflow > 0 {
            self.counted_traffic.drain(..overflow);
        }
    }

    /// Count one message of `size` bytes sent to this queue: one message added, of that size.
    ///
    /// `size` is the message's own size as Amazon SQS measures it against
    /// `MaximumMessageSize` — the body plus its message attributes — which is what
    /// [`SENT_MESSAGE_SIZE`] reports.
    pub fn count_sent(&mut self, size: usize, at: SystemTime) {
        self.count_traffic(QueueTraffic::MessagesSent, 1.0, at);
        self.count_traffic(QueueTraffic::SentMessageSize, count_as_f64(size), at);
    }

    /// Take the counter observations recorded so far, leaving none behind: publishing is
    /// at-most-once.
    fn take_counted_traffic(&mut self) -> Vec<CountedTraffic> {
        ::std::mem::take(&mut self.counted_traffic)
    }
}

/// Publish what every queue of this store owes — the reading of its depth when one is due, and
/// every counter observation its calls have left — and answer whether this store holds a queue
/// at all.
///
/// Call it with NO store lock held: it takes this service's lock to read the queues and
/// CloudWatch's to append the datapoints, one after the other. The answer is what tells the
/// clock whether there is anything left to sample — a store holding no queue publishes nothing
/// until a request creates one.
pub fn publish_queue_metrics<Eng: Send + Sync + 'static>(engine: &Eng) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let now = SystemTime::now();
    let (readings, counted, holds_queues) = {
        let mut store = state.lock();
        let (readings, counted) = due_observations(&mut store, now);
        (readings, counted, !store.queues.is_empty())
    };
    if !readings.is_empty() || !counted.is_empty() {
        append(engine, &readings, &counted, now);
    }
    holds_queues
}

/// Start the one in-process task that samples this store's queues while it holds one, and
/// stands down once it holds none or its wake-ups are spent.
///
/// At most one clock runs per store ([`DEPTH_CLOCK_LEASE`]), and with no async runtime to
/// spawn onto there is no clock — the series are then published by the next request alone, as
/// [`publish_queue_metrics`] always does.
pub fn drive_queue_metric_clock<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    if !claim_depth_clock(&mut state.lock(), SystemTime::now()) {
        return;
    }
    let engine = Arc::clone(engine);
    let handle = runtime.spawn(async move {
        for _ in 0..MAX_DEPTH_CLOCK_WAKE_UPS {
            // Every wake-up waits, so a store with nothing to sample costs a yield rather
            // than a spin, and the runtime is handed back in between.
            ::tokio::time::sleep(DEPTH_SAMPLE_TICK).await;
            let holds_queues = publish_queue_metrics(&*engine);
            if !renew_depth_clock(&*engine, holds_queues) {
                return;
            }
        }
        release_depth_clock(&*engine);
    });
    // The handle is kept in the store, so this task can be stopped: the next claim aborts a
    // clock that outlived its lease ([`claim_depth_clock`]).
    state.lock().depth_clock = Some(handle);
}

/// The reading one queue owes, named by the queue it is of.
type DueReading = (String, QueueReading);

/// One counter observation of one queue, named by the queue it is of.
type DueCount = (String, CountedTraffic);

/// What the queues of `store` owe at `now`: the reading of each queue that is due one, and
/// every counter observation its calls have recorded since the last publish, each as
/// `(QueueName, …)`.
///
/// Applying what the clock owes on a queue comes first ([`Queue::materialise_due`]): a message
/// whose retention has run out is not part of any backlog, and one whose visibility timeout
/// has lapsed is visible again — both were owed at this instant whether or not anybody called,
/// which is why the sampling pass may materialise them. The bookkeeping of queues that no
/// longer exist goes with them, so a deleted queue leaves nothing behind and one created again
/// under its name starts afresh.
fn due_observations(
    store: &mut ComAmazonawsSqsAmazonsqsState,
    now: SystemTime,
) -> (Vec<DueReading>, Vec<DueCount>) {
    let ComAmazonawsSqsAmazonsqsState {
        queues,
        last_sampled_at,
        ..
    } = store;
    last_sampled_at.retain(|key, _| queues.contains_key(key));
    let mut readings: Vec<DueReading> = Vec::new();
    let mut counted: Vec<DueCount> = Vec::new();
    for (key, queue) in queues.iter_mut() {
        queue.materialise_due(now);
        // The counters are observations of calls that have already happened, so they are
        // published as soon as they are seen rather than on the sampling cadence.
        counted.extend(
            queue
                .take_counted_traffic()
                .into_iter()
                .map(|observation| (queue.name.clone(), observation)),
        );
        if !sample_due(last_sampled_at.get(key).copied(), now) {
            continue;
        }
        last_sampled_at.insert(key.clone(), now);
        readings.push((queue.name.clone(), QueueReading::of(queue, now)));
    }
    (readings, counted)
}

/// Whether a queue last read at `sampled` owes another reading at `now`.
///
/// ONE reading per queue per minute, which is the cadence these gauges are published at
/// ([`DEPTH_PUBLISH_INTERVAL`]): a queue that exists owes its first one at once — the series of
/// a queue nobody has sent to report zero, which is what tells an idle queue from a queue that
/// does not exist, since nothing samples one this store does not hold — and one more every
/// minute it still exists, so each period a reader can ask for carries exactly the one reading
/// taken in it. A depth that changes between two readings is reported by the next one: the
/// series is a sequence of readings of the queue, not a log of what happened to it, which is
/// what makes a reader's `Average`, `Minimum` and `Maximum` over a period agree about the depth
/// that period stood at.
fn sample_due(sampled: Option<SystemTime>, now: SystemTime) -> bool {
    sampled.is_none_or(|last| minute_of(last) != minute_of(now))
}

/// Which minute `at` falls in, counted from the epoch: the slot one sample is taken per.
fn minute_of(at: SystemTime) -> u64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        / DEPTH_PUBLISH_INTERVAL.as_secs()
}

/// Append every queue's observations to the CloudWatch metric store, and make the alarms
/// watching those series due for evaluation.
///
/// The datapoints go into the CloudWatch emulator's OWN store — the same one `ListMetrics`,
/// `GetMetricStatistics` and the alarm evaluation read — so one metric has one home and the
/// publisher and its readers cannot drift. They never travel over `PutMetricData`, which
/// refuses a publish into a namespace the services own.
fn append<Eng: Send + Sync + 'static>(
    engine: &Eng,
    readings: &[DueReading],
    counted: &[DueCount],
    at: SystemTime,
) {
    let Some(state) = cloudwatch::state::service_state(engine) else {
        return;
    };
    let sampled_secs = epoch_secs(at);
    let mut store = state.lock();
    let mut written: Vec<cloudwatch::state::WrittenSeries> = Vec::new();
    // A reading of the queue: one observation of each gauge, all at the instant the reading was
    // taken.
    let gauges = readings.iter().flat_map(|(queue_name, reading)| {
        reading
            .observations()
            .into_iter()
            .map(move |(metric_name, value, unit)| {
                (queue_name, metric_name, value, unit, sampled_secs)
            })
    });
    // A counter observation: what one call contributed, at the instant that call happened.
    let counters = counted.iter().map(|(queue_name, observation)| {
        (
            queue_name,
            observation.traffic.metric_name(),
            observation.value,
            observation.traffic.unit(),
            epoch_secs(observation.at),
        )
    });
    for (queue_name, metric_name, value, unit, timestamp_secs) in gauges.chain(counters) {
        // The series identity is the namespace, the metric and the queue's NAME: those are the
        // dimensions a reader selects a queue's series by. A queue belongs to one region, and
        // the metric store these datapoints go to holds one set of series for the whole
        // emulator rather than one per region, so two same-named queues in two regions report
        // into the one series until that store itself is keyed by region.
        let key = cloudwatch::state::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: metric_name.to_string(),
            dimensions: [(QUEUE_NAME_DIMENSION.to_string(), queue_name.clone())]
                .into_iter()
                .collect(),
        };
        let datum = cloudwatch::state::StoredDatum {
            timestamp_secs,
            // One observation: a gauge's datapoint IS the reading taken of the queue, and a
            // counter's is the one call's own contribution.
            sample_count: 1.0,
            sum: value,
            minimum: value,
            maximum: value,
            unit: Some(unit.to_string()),
            storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
        };
        if store.append_datapoints(key.clone(), vec![datum]).is_err() {
            // The metric store holds no more series. Whatever request this observation came
            // off was already answered, so nothing is failed for it; the dropped observation
            // is reported where a developer can see it.
            ::tracing::warn!(
                queue = %queue_name,
                metric = %metric_name,
                "the CloudWatch metric store is at capacity, so this queue's measurement was \
                 not published"
            );
            continue;
        }
        written.push(cloudwatch::state::WrittenSeries {
            key,
            units: vec![unit.to_string()],
        });
    }
    // A backlog alarm is an alarm on one of these series, so the arriving datapoints are what
    // makes its next evaluation due — exactly as a published datapoint does.
    store.make_alarms_due(&written, sampled_secs);
}

/// A message count, or a size in bytes, as the value its datapoint carries. Both are bounded
/// by this store's own per-queue caps, so the conversion is exact.
pub fn count_as_f64(count: usize) -> f64 {
    f64::from(u32::try_from(count).unwrap_or(u32::MAX))
}

/// `at` as the epoch seconds a stored datapoint carries, which is the unit CloudWatch's own
/// readers window by.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// Take the clock claim for this store when no clock holds a live lease, reporting whether the
/// caller may start one.
///
/// The task of a clock whose lease has lapsed without being released is ABORTED as the claim is
/// taken: such a clock has either ended already (where the abort does nothing) or is stuck, and
/// leaving a stuck one running beside the new one would be two clocks over one store. It is the
/// handle this store holds that makes that possible, which is why the clock is kept there rather
/// than dropped on the floor.
fn claim_depth_clock(store: &mut ComAmazonawsSqsAmazonsqsState, now: SystemTime) -> bool {
    if store.depth_clock_until.is_some_and(|until| now < until) {
        return false;
    }
    if let Some(previous) = store.depth_clock.take() {
        previous.abort();
    }
    store.depth_clock_until = now.checked_add(DEPTH_CLOCK_LEASE);
    true
}

/// Renew the running clock's lease, reporting whether it is to keep sampling.
///
/// A store holding no queue has nothing to sample, so the clock releases its claim and stands
/// down; the next request starts one again.
fn renew_depth_clock<Eng: Send + Sync + 'static>(engine: &Eng, holds_queues: bool) -> bool {
    let Some(state) = service_state(engine) else {
        return false;
    };
    let mut store = state.lock();
    if !holds_queues {
        release(&mut store);
        return false;
    }
    store.depth_clock_until = SystemTime::now().checked_add(DEPTH_CLOCK_LEASE);
    true
}

/// Release the claim of a clock that has spent its wake-ups.
fn release_depth_clock<Eng: Send + Sync + 'static>(engine: &Eng) {
    if let Some(state) = service_state(engine) {
        release(&mut state.lock());
    }
}

/// Give up the clock claim: no clock holds the lease, and none holds a task either.
fn release(store: &mut ComAmazonawsSqsAmazonsqsState) {
    store.depth_clock_until = None;
    store.depth_clock = None;
}
