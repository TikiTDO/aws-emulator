// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as a group's capacity changes.
//!
//! # The rule this module implements
//!
//! Amazon EC2 Auto Scaling is an event SOURCE, not only a control plane a client polls:
//! every instance a group LAUNCHES, and every instance it SCALES IN, is published onto the
//! account's default event bus ([`DEFAULT_EVENT_BUS`]) under the `source` this service owns
//! ([`EVENT_SOURCE`]) and the `detail-type` of that lifecycle step
//! ([`INSTANCE_LAUNCH_SUCCESSFUL`], [`INSTANCE_TERMINATE_SUCCESSFUL`]), carrying the
//! scaling activity that moved it in the event's `detail` and naming the group and the
//! instance in the envelope's `resources`. A rule on that bus whose `EventPattern` matches
//! then delivers it to the rule's targets, so a client that reacts to a scale-out or a
//! scale-in through EventBridge observes it without calling this service again — and a
//! rule whose pattern does not match receives nothing, which is only distinguishable from
//! "nothing was ever published" because the publication happens.
//!
//! The model's own `AutoScalingNotificationTypes` catalog
//! (`autoscaling:EC2_INSTANCE_LAUNCH`, `autoscaling:EC2_INSTANCE_TERMINATE` and their
//! `_ERROR` counterparts, [`super::AUTO_SCALING_NOTIFICATION_TYPES`]) is this service's own
//! statement that an instance launch and an instance termination are the events it
//! announces; the bus carries them under the detail types above.
//!
//! # How a lifecycle step becomes an event
//!
//! Nothing here decides WHEN an instance moves: the stored due instant does
//! ([`super::LIFECYCLE_TRANSITION`], materialised by
//! [`super::AutoScalingState::materialize_due`]). The ONE place a launch or a termination
//! records its scaling activity also queues the event that reports it
//! ([`super::AutoScalingState::record_instance_lifecycle_change`]), due at the instant that
//! activity completes — so a step is queued exactly once however it was asked for (a
//! capacity change, an instance refresh's replacement, a forced delete of the group) and
//! none can be published twice. The queue is bounded ([`MAX_PENDING_EVENTS`], oldest
//! dropped first) because every step appends to it.
//!
//! A queued event is published with no lock held ([`publish_pending`]): EventBridge is
//! reached the way every emulated service reaches another, through the framework's
//! in-process callback. Publication is best effort — an emulator with no EventBridge
//! mounted has nowhere to publish — so a failure is logged and the operation's own answer
//! stands.
//!
//! The SAME drain publishes the Amazon SNS notifications the step owes, onto the topics the
//! group's own notification configurations selected for it when it was recorded
//! ([`super::sns_notifications`], [`PendingLifecycleEvent::notification_topics`]): one
//! lifecycle step is announced once, to the bus and to the configured topics together, so
//! the two reports of it cannot disagree and neither can be published twice.
//!
//! WHICH call publishes: none of them. A door that changes a group's capacity starts the
//! clock pump ([`drive_lifecycle_clock`]) and answers its caller; the pump waits for the
//! earliest instant the store owes something at, materialises what is then due and
//! publishes what that produced, until nothing is owed. That is what a client watching a
//! group through EventBridge needs: it makes no further Auto Scaling call, so an event
//! published only by the next reader would never be published at all. The pump changes WHEN
//! nothing — it materialises only what the stored due instant has already made due, exactly
//! as a reading door does.

use std::collections::VecDeque;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::{AutoScalingState, StoredGroup, ACTIVITY_SUCCESSFUL, GROUP_STATE_IN_SERVICE};

/// The `source` of every event Amazon EC2 Auto Scaling publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its ARNs carry.
pub const EVENT_SOURCE: &str = "aws.autoscaling";

/// The `detail-type` of the event published when a group has LAUNCHED an instance.
pub const INSTANCE_LAUNCH_SUCCESSFUL: &str = "EC2 Instance Launch Successful";

/// The `detail-type` of the event published when a group has TERMINATED one.
pub const INSTANCE_TERMINATE_SUCCESSFUL: &str = "EC2 Instance Terminate Successful";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many lifecycle events the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every
/// lifecycle step appends to it, so without a bound a client scaling without end could grow
/// it without end. The oldest event is dropped first, so the steps the live groups just
/// took are the ones that survive. It is far above what a realistic burst produces (one
/// capacity change contributes one event per instance it moves).
pub const MAX_PENDING_EVENTS: usize = 4_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a drain holding more events than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many events ONE drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one pass draining all of it
/// would fan out to a rule's targets that many times: this caps one pass, and
/// [`publish_pending`] repeats the pass until nothing is due, so nothing waits for a later
/// wake-up.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// The shortest wait one wake-up of the clock pump makes.
///
/// A due instant already in the past is retired on the NEXT wake-up, not on this one's
/// instant, so the pump always hands the runtime back and a wake-up that retires nothing
/// cannot turn the loop into a spin. It is far below [`super::LIFECYCLE_TRANSITION`], so it
/// never delays a publication noticeably.
const MIN_CLOCK_WAIT: Duration = Duration::from_millis(5);

/// The longest wait one wake-up makes before it looks at the store again.
///
/// Every instant a group's capacity change owes falls due within a few spans of the one
/// before it, so a wait is never near this value; it is a backstop, so a due instant far in
/// the future (a capacity change holding for a long cooldown) cannot park the pump
/// indefinitely.
const MAX_CLOCK_WAIT: Duration = Duration::from_secs(5);

/// How many wake-ups one pump makes before it stands down.
///
/// A backstop, not a budget: what the store owes is already finite (a group holds at most
/// [`super::MAX_GROUP_CAPACITY`] instances and the group map is bounded), and every wake-up
/// either retires a due instant or publishes what one produced. The cap guarantees
/// termination even if a later change left a due instant that materialising does not clear,
/// and the next write starts a fresh pump.
const MAX_CLOCK_WAKE_UPS: usize = 10_000;

/// Which lifecycle step a group took with one of its instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceLifecycle {
    /// The group launched the instance.
    Launched,
    /// The group terminated it.
    Terminated,
}

impl InstanceLifecycle {
    /// The `detail-type` the event reporting this step carries.
    #[must_use]
    pub fn detail_type(self) -> &'static str {
        match self {
            Self::Launched => INSTANCE_LAUNCH_SUCCESSFUL,
            Self::Terminated => INSTANCE_TERMINATE_SUCCESSFUL,
        }
    }

    /// The event type of this service's own notification catalog
    /// ([`super::AUTO_SCALING_NOTIFICATION_TYPES`]) that a group's notification
    /// configurations select this step by, and that the notification reporting it carries.
    #[must_use]
    pub fn notification_event(self) -> &'static str {
        match self {
            Self::Launched => super::sns_notifications::EC2_INSTANCE_LAUNCH,
            Self::Terminated => super::sns_notifications::EC2_INSTANCE_TERMINATE,
        }
    }

    /// The `Activity$Description` the scaling activity for this step carries, naming the
    /// instance it moved — the one wording for it in this crate, so the activity log and
    /// the event agree about what happened.
    #[must_use]
    pub fn description(self, instance_id: &str) -> String {
        match self {
            Self::Launched => format!("Launching a new EC2 instance: {instance_id}"),
            Self::Terminated => format!("Terminating EC2 instance: {instance_id}"),
        }
    }
}

/// One lifecycle step of one instance, for the activity it records and the event it
/// publishes.
#[derive(Debug, Clone)]
pub struct InstanceLifecycleChange<'a> {
    /// Which step the group took.
    pub kind: InstanceLifecycle,
    /// The group it took it with.
    pub group_name: &'a str,
    /// That group's ARN, which the event's envelope names it by.
    pub group_arn: &'a str,
    /// The instance that moved.
    pub instance_id: &'a str,
    /// `Activity$Cause`: what asked for the change.
    pub cause: &'a str,
    /// When the step started, which is the activity's `StartTime`.
    pub start_time: SystemTime,
    /// When it completes — the activity's `EndTime`, and the instant the event that
    /// reports it is published at.
    pub due: SystemTime,
}

/// One lifecycle event waiting to be published, rendered against the activity and the
/// placement the store held when the step was recorded.
#[derive(Debug, Clone)]
pub struct PendingLifecycleEvent {
    /// WHICH step it reports, which is what the `detail-type` it is published under and
    /// the notification event it is published as are both read from.
    pub kind: InstanceLifecycle,
    /// The Amazon SNS topics the group's notification configurations selected this step
    /// for when it was recorded, which is where its notifications go
    /// ([`super::sns_notifications`]). Empty for a group configured for no topic, or for
    /// none that asked about this step.
    pub notification_topics: Vec<String>,
    /// The group the step belongs to.
    pub group_name: String,
    /// That group's ARN, carried in the envelope's `resources`.
    pub group_arn: String,
    /// The Region the group lives in, which is the Region the event is published in.
    pub region: String,
    /// The instance that moved, named in `detail.EC2InstanceId` and in `resources`.
    pub instance_id: String,
    /// The Availability Zone the group placed it in, when the record holds one.
    pub availability_zone: Option<String>,
    /// The subnet the account's Amazon EC2 store placed it in, when the group placed it in
    /// one.
    pub subnet_id: Option<String>,
    /// The scaling activity that moved it.
    pub activity_id: String,
    /// That activity's `Description`.
    pub description: String,
    /// That activity's `Cause`.
    pub cause: String,
    /// That activity's `StartTime`.
    pub start_time: SystemTime,
    /// WHEN the step completed: the event's `time`, its `detail.EndTime`, and the instant
    /// from which it may be published.
    pub due: SystemTime,
}

impl AutoScalingState {
    /// Record the scaling activity ONE lifecycle step of a group produces and queue the
    /// event that reports it.
    ///
    /// THE ONE place a launch or a termination of a group's instance is written down, so
    /// the activity a client reads and the event a bus carries are one fact: the activity
    /// is appended through [`Self::record_activity`] (completing at `change.due`) and the
    /// event is queued for that same instant. Answers the activity's id, which is what a
    /// caller that reports the step further needs.
    ///
    /// `engine` is the shared engine the caller holds: the subnet the event names is the
    /// one the account's own Amazon EC2 store placed the instance in, read from there
    /// rather than guessed from the group's `VPCZoneIdentifier`.
    pub(crate) fn record_instance_lifecycle_change<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        change: &InstanceLifecycleChange<'_>,
    ) -> String {
        let description = change.kind.description(change.instance_id);
        let activity_id = self.record_activity(
            change.group_name,
            change.group_arn,
            description.clone(),
            change.cause.to_string(),
            GROUP_STATE_IN_SERVICE,
            change.start_time,
            Some(change.due),
        );
        let region = self
            .groups
            .get(change.group_name)
            .map(|group| group.region.clone())
            .unwrap_or_else(|| super::DEFAULT_REGION.to_string());
        let availability_zone = self.instance_zone(change.group_name, change.instance_id);
        let subnet_id = super::ec2_instances::subnet_of(engine, change.instance_id);
        // The topics the group is configured to be told about THIS step, read while the
        // group's own record is in hand: a configuration removed later did select the step
        // that happened under it, and a group that is deleted still owes the terminations
        // its delete performed.
        let notification_topics = self
            .groups
            .get(change.group_name)
            .map(|group| group.notification_topics(change.kind.notification_event()))
            .unwrap_or_default();
        self.queue_lifecycle_event(PendingLifecycleEvent {
            kind: change.kind,
            notification_topics,
            group_name: change.group_name.to_string(),
            group_arn: change.group_arn.to_string(),
            region,
            instance_id: change.instance_id.to_string(),
            availability_zone,
            subnet_id,
            activity_id: activity_id.clone(),
            description,
            cause: change.cause.to_string(),
            start_time: change.start_time,
            due: change.due,
        });
        activity_id
    }

    /// The Availability Zone the group's own record holds for `instance_id`.
    fn instance_zone(&self, group_name: &str, instance_id: &str) -> Option<String> {
        self.groups
            .get(group_name)?
            .instances
            .iter()
            .find(|instance| instance.instance_id == instance_id)
            .map(|instance| instance.availability_zone.clone())
    }

    /// Queue one lifecycle event for publication, keeping the queue bounded.
    ///
    /// Public to the crate because a door that terminates a group's instances WITHOUT
    /// recording an activity per instance — a forced delete, whose one activity reports the
    /// whole group — still owes the events those terminations produce.
    pub(crate) fn queue_lifecycle_event(&mut self, event: PendingLifecycleEvent) {
        while self.pending_lifecycle_events.len() >= MAX_PENDING_EVENTS {
            self.pending_lifecycle_events.pop_front();
            ::tracing::warn!(
                "the queue of Auto Scaling instance lifecycle events waiting to be \
                 published to EventBridge is full at {MAX_PENDING_EVENTS}; the oldest event \
                 was dropped"
            );
        }
        self.pending_lifecycle_events.push_back(event);
    }

    /// Take at most `limit` of the queued events the clock has made due at `now`, oldest
    /// first.
    ///
    /// Taking them out under the lock is what stops two concurrent drains from publishing
    /// one event twice. An event whose step has not completed yet stays queued: the step is
    /// not a fact until its own instant.
    fn take_due_lifecycle_events(
        &mut self,
        now: SystemTime,
        limit: usize,
    ) -> Vec<PendingLifecycleEvent> {
        let mut taken = Vec::new();
        let mut kept = VecDeque::with_capacity(self.pending_lifecycle_events.len());
        while let Some(event) = self.pending_lifecycle_events.pop_front() {
            if taken.len() < limit && event.due <= now {
                taken.push(event);
            } else {
                kept.push_back(event);
            }
        }
        self.pending_lifecycle_events = kept;
        taken
    }

    /// How many events are queued for publication — reporting only, for a reader that has
    /// to tell "nothing moved" from "nothing was published".
    #[must_use]
    pub fn pending_lifecycle_event_count(&self) -> usize {
        self.pending_lifecycle_events.len()
    }

    /// The earliest instant at which this store owes something, if any — the instant the
    /// clock pump waits for.
    ///
    /// Every instant the clock already holds: a queued event's own completion, a capacity
    /// change held for a cooldown, the transient lifecycle state of an instance, and the
    /// next step of a running instance refresh. They are exactly what
    /// [`Self::materialize_due`] retires.
    #[must_use]
    pub fn next_clock_due_at(&self) -> Option<SystemTime> {
        self.pending_lifecycle_events
            .iter()
            .map(|event| event.due)
            .chain(self.groups.values().flat_map(group_due_instants))
            .chain(
                self.instance_refreshes
                    .iter()
                    .filter_map(|refresh| refresh.next_step_due),
            )
            .min()
    }

    /// What the clock pump waits for next, or `None` — having RELEASED its claim — when
    /// this store owes nothing more.
    ///
    /// Observing "nothing is owed" and standing down happen in ONE critical section, which
    /// is what makes the hand-over race-free: a write stores its change (and so its due
    /// instant) under this same lock BEFORE it tries to claim the pump, so either this call
    /// sees that instant and the running pump keeps it, or the write's claim finds the flag
    /// already cleared and starts a pump of its own.
    ///
    /// An event the clock has ALREADY made due is owed at `now` rather than at some later
    /// instant, so a queued publication keeps the pump awake on its next wake-up: one pass
    /// publishes at most [`MAX_PUBLISHED_PER_DRAIN`] of them, and the rest are owed another
    /// wake-up rather than the next call.
    pub fn next_lifecycle_due_or_release_pump(
        &mut self,
        now: SystemTime,
        generation: u64,
    ) -> Option<SystemTime> {
        if self
            .pending_lifecycle_events
            .iter()
            .any(|event| event.due <= now)
        {
            return Some(now);
        }
        match self.next_clock_due_at() {
            Some(due) => Some(due),
            None => {
                self.release_lifecycle_clock_pump(generation);
                None
            }
        }
    }

    /// Claim the single clock pump for this store: the claim's GENERATION when the caller
    /// may start one, `None` when one is already running.
    ///
    /// The generation is what makes a release safe: a pump releases its own claim and never
    /// a later pump's (see [`Self::release_lifecycle_clock_pump`]).
    pub fn start_lifecycle_clock_pump(&mut self) -> Option<u64> {
        if self.lifecycle_clock_pump_running {
            return None;
        }
        self.lifecycle_clock_pump_running = true;
        self.lifecycle_clock_pump_generation = self.lifecycle_clock_pump_generation.wrapping_add(1);
        Some(self.lifecycle_clock_pump_generation)
    }

    /// Release the claim `generation` took — and ONLY that one.
    ///
    /// A pump that ended on the wake-up cap must not clear a claim a pump started since is
    /// holding, or two pumps would run against one store.
    pub fn release_lifecycle_clock_pump(&mut self, generation: u64) {
        if self.lifecycle_clock_pump_generation == generation {
            self.lifecycle_clock_pump_running = false;
        }
    }

    /// Keep the running pump's task handle, aborting the one it replaces — but only while
    /// `generation` is still the LIVE claim.
    ///
    /// The handle is stored after the task is spawned, and a pump that ran out of work
    /// releases its claim before this runs, so a start call may have claimed and stored its
    /// own pump in between: storing this handle then would abort that LIVE pump and leave
    /// the claim held by nobody. A handle whose claim has moved on belongs to a task that
    /// has already ended, so it is simply dropped. While the claim IS this pump's, the
    /// replaced handle is aborted rather than merely forgotten, which is what guarantees no
    /// earlier pump task runs beside the one holding the claim.
    pub fn set_lifecycle_clock_pump(
        &mut self,
        generation: u64,
        handle: ::tokio::task::JoinHandle<()>,
    ) {
        if self.lifecycle_clock_pump_generation != generation {
            return;
        }
        if let Some(previous) = self.lifecycle_clock_pump.replace(handle) {
            previous.abort();
        }
    }
}

/// The instants one group's own record is waiting on.
fn group_due_instants(group: &StoredGroup) -> Vec<SystemTime> {
    group
        .pending_capacity
        .as_ref()
        .map(|pending| pending.due)
        .into_iter()
        .chain(
            group
                .instances
                .iter()
                .filter_map(|instance| instance.transition_due),
        )
        .collect()
}

/// The `detail` of an instance lifecycle event: the scaling activity that moved the
/// instance, as the store holds it.
///
/// Every member is read off the queued step, which was itself read off the activity this
/// service recorded, so the event reports the activity a `DescribeScalingActivities` of the
/// same instant reports and never a second, invented copy of it. The activity has completed
/// by the instant the event is published — that completion is what the event reports — so
/// its status is [`ACTIVITY_SUCCESSFUL`] and it carries no status message.
fn lifecycle_event_detail(event: &PendingLifecycleEvent) -> ::serde_json::Value {
    let mut placement = ::serde_json::Map::new();
    if let Some(zone) = event.availability_zone.as_ref() {
        placement.insert(
            "Availability Zone".to_string(),
            ::serde_json::Value::String(zone.clone()),
        );
    }
    if let Some(subnet_id) = event.subnet_id.as_ref() {
        placement.insert(
            "Subnet ID".to_string(),
            ::serde_json::Value::String(subnet_id.clone()),
        );
    }
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "StatusCode".to_string(),
        ::serde_json::Value::String(ACTIVITY_SUCCESSFUL.to_string()),
    );
    detail.insert(
        "Description".to_string(),
        ::serde_json::Value::String(event.description.clone()),
    );
    detail.insert(
        "AutoScalingGroupName".to_string(),
        ::serde_json::Value::String(event.group_name.clone()),
    );
    detail.insert(
        "ActivityId".to_string(),
        ::serde_json::Value::String(event.activity_id.clone()),
    );
    detail.insert(
        "Details".to_string(),
        ::serde_json::Value::Object(placement),
    );
    // The request that carried the activity out is the activity itself, which is how the
    // envelope reports it: one id for the activity and for the request behind it.
    detail.insert(
        "RequestId".to_string(),
        ::serde_json::Value::String(event.activity_id.clone()),
    );
    detail.insert(
        "StatusMessage".to_string(),
        ::serde_json::Value::String(String::new()),
    );
    detail.insert(
        "EC2InstanceId".to_string(),
        ::serde_json::Value::String(event.instance_id.clone()),
    );
    detail.insert(
        "StartTime".to_string(),
        ::serde_json::Value::String(date_time_text(event.start_time)),
    );
    detail.insert(
        "EndTime".to_string(),
        ::serde_json::Value::String(date_time_text(event.due)),
    );
    detail.insert(
        "Cause".to_string(),
        ::serde_json::Value::String(event.cause.clone()),
    );
    ::serde_json::Value::Object(detail)
}

/// The date-time text a timestamp is reported as inside an event's `detail`: RFC 3339 in
/// UTC, which is the form every AWS event body carries an instant in.
fn date_time_text(at: SystemTime) -> String {
    super::to_date_time(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// `at` as the epoch milliseconds the event's `time` carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}

/// The ARN of the Amazon EC2 instance an event is about, which the envelope's `resources`
/// names beside the group's: the event names every resource it is about, which is what a
/// rule filtering on `resources` reads.
fn instance_arn(region: &str, instance_id: &str) -> String {
    ::engine::emulator_arn("ec2", region, &format!("instance/{instance_id}"))
}

/// Publish every lifecycle event this store has made due, and leave none due behind.
///
/// Called with NO lock held: each pass takes its events out in one critical section, so
/// nothing is published twice, and the in-process `PutEvents` and `Publish` calls run
/// outside it. The passes repeat until nothing is due — one pass is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]) so no single call is unbounded, and the loop is what
/// guarantees the tail of a burst goes out with it. Nothing about the answer an operation
/// gives changes either way.
async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    engine: &Eng,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    loop {
        let due = state
            .lock()
            .take_due_lifecycle_events(SystemTime::now(), MAX_PUBLISHED_PER_DRAIN);
        if due.is_empty() {
            return;
        }
        publish_batch(callback, &due).await;
        // The same steps, to the topics the group's own notification configurations asked
        // to be told about them: one step is announced once, to the bus and to those topics
        // together.
        let notifications: Vec<super::sns_notifications::PendingNotification> = due
            .iter()
            .flat_map(super::sns_notifications::owed_for_lifecycle_event)
            .collect();
        super::sns_notifications::publish(callback, &notifications).await;
    }
}

/// Publish one pass, in `PutEvents` requests of at most [`MAX_PUT_EVENTS_ENTRIES`] entries,
/// each addressed to the Region the group it is about lives in.
async fn publish_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    events: &[PendingLifecycleEvent],
) {
    let mut regions: Vec<&str> = events.iter().map(|event| event.region.as_str()).collect();
    regions.sort_unstable();
    regions.dedup();
    for region in regions {
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
        let of_region: Vec<&PendingLifecycleEvent> = events
            .iter()
            .filter(|event| event.region == region)
            .collect();
        for window in of_region.chunks(MAX_PUT_EVENTS_ENTRIES) {
            let mut request = client.put_events();
            for event in window {
                let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                    .event_bus_name(DEFAULT_EVENT_BUS)
                    .source(EVENT_SOURCE)
                    .detail_type(event.kind.detail_type())
                    // The event's `time` is WHEN the instance moved, not when the queue was
                    // drained: `PutEventsRequestEntry$Time` is "the time stamp of the
                    // event, per RFC3339. If no time stamp is provided, the time stamp of
                    // the PutEvents call is used."
                    .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                        epoch_millis(event.due),
                    ))
                    .resources(&event.group_arn)
                    .resources(instance_arn(&event.region, &event.instance_id))
                    .detail(lifecycle_event_detail(event).to_string())
                    .build();
                request = request.entries(entry);
            }
            if let Err(error) = request.send().await {
                // Named rather than only counted: an event that was not delivered is
                // otherwise invisible, so the group and the instances of the batch that
                // did not reach the bus are what a developer needs to see.
                let dropped: Vec<&str> = window
                    .iter()
                    .map(|event| event.instance_id.as_str())
                    .collect();
                ::tracing::warn!(
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    region,
                    instances = ?dropped,
                    "Auto Scaling instance lifecycle events could not be published to \
                     EventBridge"
                );
            }
        }
    }
}

/// Publish the lifecycle events the CLOCK owes, as they fall due, with no further call from
/// anybody.
///
/// Amazon EC2 Auto Scaling announces a scale-out and a scale-in as they happen, which is
/// the whole point of the event path: a client that reacts to a group through EventBridge
/// makes no further Auto Scaling call, so an event published only by the next reader would
/// never be published at all. This starts ONE in-process task that waits for the earliest
/// due instant the store holds, materialises what is then due
/// ([`AutoScalingState::materialize_due`], the same call every operation makes) and
/// publishes what that produced, until nothing is owed.
///
/// It does not change WHEN anything happens: `materialize_due` applies only what a stored
/// due instant has already made due, so a caller reading before that instant still sees the
/// transient state, and two reads at the same instant still answer identically. The task
/// waits for the clock so that no request ever has to.
///
/// At most one pump runs per store (the store carries the claim), it is bounded by
/// [`MAX_CLOCK_WAKE_UPS`] wake-ups, and it exits as soon as the store owes nothing. With no
/// async runtime to spawn onto there is no pump, and what the clock owes is materialised by
/// the next operation instead.
pub fn drive_lifecycle_clock<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().start_lifecycle_clock_pump() else {
        return;
    };
    let callback = ctx.callback().clone();
    let engine = ::std::sync::Arc::clone(engine);
    let pumped = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap below is what ended the loop; a pump that ran out
        // of work has already released its claim inside the one critical section that
        // observed it (`next_lifecycle_due_or_release_pump`).
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = state
                .lock()
                .next_lifecycle_due_or_release_pump(now, generation)
            else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps the
            // backstop above from ever costing a CPU-bound spin, and it hands the runtime
            // back between wake-ups.
            let wait = due
                .duration_since(now)
                .unwrap_or_default()
                .clamp(MIN_CLOCK_WAIT, MAX_CLOCK_WAIT);
            ::tokio::time::sleep(wait).await;
            state.lock().materialize_due(&*engine, SystemTime::now());
            publish_pending(&callback, &*engine).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            state.lock().release_lifecycle_clock_pump(generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely forgotten:
    // that is what guarantees no earlier pump task runs beside the one holding the claim.
    // It is stored under THIS pump's generation, so a handle whose claim has moved on
    // (a pump that already stood down and a later start that claimed it) is dropped
    // instead of aborting the live one.
    pumped
        .lock()
        .set_lifecycle_clock_pump(generation, handle);
}
