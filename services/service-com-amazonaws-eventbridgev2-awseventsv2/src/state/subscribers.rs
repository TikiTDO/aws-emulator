// SPDX-License-Identifier: CC0-1.0

//! The subscriber registry: the slice of this service's state that
//! `CreateSubscriber` mints into, `DescribeSubscriber` projects, and `PutEvents`
//! reads to decide where an accepted event goes.
//!
//! A subscriber belongs to one event bus — "Creates a subscriber on an event
//! bus, which delivers matching events to the configured target"
//! (`CreateSubscriber`) — so every stored subscriber names that bus and lives in
//! the bus's region and account. Nothing a response carries is computed from the
//! request alone: every member is read back out of here.
//!
//! This module holds the subscriber and what it DECIDES about an event — which
//! events it is owed ([`Subscriber::delivers`], [`filters_match`]) and what the
//! target is handed ([`delivered_body`]). The call that reaches the target is
//! [`super::subscriber_delivery`].

use std::collections::BTreeMap;
use std::time::SystemTime;

use super::{base36, spread, ComAmazonawsEventbridgev2Awseventsv2State, StoredEvent, ARN_SERVICE};
use crate::ops::types::{
    BatchConfiguration, Filter, FilterConfiguration, InvokeConfiguration, LogConfiguration,
    OnFailureConfiguration, PointInTimeConfiguration, RetryPolicy, Transformer,
};

/// The resource-type segment of a `SubscriberArn`, from the modelled pattern
/// `^arn:aws…:events:…:…:subscriber\/<name>\/[a-z0-9]{25}$`.
pub const ARN_RESOURCE_TYPE_SUBSCRIBER: &str = "subscriber";

/// Width of the id segment of a `SubscriberArn`: the modelled pattern pins it at
/// exactly `[a-z0-9]{25}`.
pub const SUBSCRIBER_ID_LEN: usize = 25;

/// The most subscribers one emulator process holds.
///
/// This is the EMULATOR's own bound rather than a published AWS quota: without
/// it any caller could grow one process's heap without limit. A create past it
/// answers the `LimitExceededException` `CreateSubscriber` models — "The request
/// would exceed a service quota for the account".
pub const MAX_SUBSCRIBERS_PER_EMULATOR: usize = 1_000;

/// `SubscriberState::RUNNING`, an entry of
/// [`crate::ops::types::SUBSCRIBER_STATE_VALUES`].
pub const SUBSCRIBER_STATE_RUNNING: &str = "RUNNING";

/// `SubscriberState::STOPPED`, the other entry of
/// [`crate::ops::types::SUBSCRIBER_STATE_VALUES`].
pub const SUBSCRIBER_STATE_STOPPED: &str = "STOPPED";

/// `OrderingType::UNORDERED` — "delivers without an ordering guarantee"
/// (`OrderingType`), the mode a create that names none takes: FIFO delivery is
/// the mode a caller asks for by naming `Type` FIFO and an `EventGroupId` to
/// order within.
pub const ORDERING_TYPE_UNORDERED: &str = "UNORDERED";

/// `StartingPosition::LATEST` — delivery begins at the END of the bus's log as
/// it stands when the subscriber is created, so the subscriber is owed the
/// events published after it exists and none of the bus's backlog. It is the
/// position a create that names none takes.
pub const STARTING_POSITION_LATEST: &str = "LATEST";

/// `StartingPosition::POINT_IN_TIME` — delivery begins at the point of the
/// bus's retained log the subscriber's `PointInTimeConfiguration` names, so the
/// subscriber is owed the backlog from that point on.
pub const STARTING_POSITION_POINT_IN_TIME: &str = "POINT_IN_TIME";

/// `PointType::HORIZON` — the point is the horizon of the bus's retained log:
/// the oldest event the bus still holds. An entry of
/// [`crate::ops::types::POINT_TYPE_VALUES`].
pub const POINT_TYPE_HORIZON: &str = "HORIZON";

/// `PointType::TIMESTAMP` — the point is the instant `StartingPoint` names
/// ("Timestamp to start from. Required when PointType is TIMESTAMP"). An entry
/// of [`crate::ops::types::POINT_TYPE_VALUES`].
pub const POINT_TYPE_TIMESTAMP: &str = "TIMESTAMP";

/// `FilterScope::DATA` — the filter is matched against the event's own payload.
/// An entry of [`crate::ops::types::FILTER_SCOPE_VALUES`].
pub const FILTER_SCOPE_DATA: &str = "DATA";

/// `FilterScope::METADATA` — the filter is matched against the metadata the
/// publisher sent with the event. An entry of
/// [`crate::ops::types::FILTER_SCOPE_VALUES`].
pub const FILTER_SCOPE_METADATA: &str = "METADATA";

/// `FilterScope::SYSTEM_METADATA` — the filter is matched against the event's
/// system metadata. An entry of
/// [`crate::ops::types::FILTER_SCOPE_VALUES`].
pub const FILTER_SCOPE_SYSTEM_METADATA: &str = "SYSTEM_METADATA";

/// `TransformerType::RAW` — the target is handed the event's payload alone. An
/// entry of [`crate::ops::types::TRANSFORMER_TYPE_VALUES`].
pub const TRANSFORMER_TYPE_RAW: &str = "RAW";

/// `TransformerType::WITH_METADATA` — the target is handed the event with its
/// metadata, which is also what a subscriber that configures no transformer
/// hands it. An entry of [`crate::ops::types::TRANSFORMER_TYPE_VALUES`].
pub const TRANSFORMER_TYPE_WITH_METADATA: &str = "WITH_METADATA";

/// The member of a delivered event that carries the service-assigned `EventId`
/// `PutEvents` reported for it.
pub const DELIVERED_EVENT_ID: &str = "EventId";

/// The member of a delivered event that carries the `SequenceNumber`
/// `PutEvents` reported for it.
pub const DELIVERED_SEQUENCE_NUMBER: &str = "SequenceNumber";

/// The member of a delivered event that carries the ARN of the bus it was
/// published to.
pub const DELIVERED_EVENT_BUS_ARN: &str = "EventBusArn";

/// The member of a delivered event that carries the event's payload — the
/// `DATA` a filter of that scope is matched against.
pub const DELIVERED_DATA: &str = "Data";

/// The member of a delivered event that carries the publisher's metadata — the
/// `METADATA` a filter of that scope is matched against.
pub const DELIVERED_METADATA: &str = "Metadata";

/// The member of a delivered event that carries the event's system metadata —
/// the `SYSTEM_METADATA` a filter of that scope is matched against.
pub const DELIVERED_SYSTEM_METADATA: &str = "SystemMetadata";

/// Customer-controlled run state of a stored subscriber, as the
/// `SubscriberState` enum words it on the wire.
///
/// It is the subscriber's OWN state rather than a provisioning state:
/// `StateReason` documents that "EventSources and subscribers are provisioned
/// synchronously and report failures directly on the request", so a created
/// subscriber is operational as soon as the call that minted it returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriberRunState {
    /// `RUNNING` — the subscriber delivers matching events to its target.
    Running,
    /// `STOPPED` — the caller has stopped delivery.
    Stopped,
}

impl SubscriberRunState {
    /// The `SubscriberState` member value this state serializes as. Each is an
    /// entry of [`crate::ops::types::SUBSCRIBER_STATE_VALUES`].
    pub fn wire_value(self) -> &'static str {
        match self {
            Self::Running => SUBSCRIBER_STATE_RUNNING,
            Self::Stopped => SUBSCRIBER_STATE_STOPPED,
        }
    }

    /// The state a request naming `value` asks for.
    ///
    /// `None` for a value `SubscriberState` does not declare, which the boundary
    /// validator refuses before a handler runs; a request naming no state at all
    /// takes [`Self::Running`], because a subscriber a caller created to receive
    /// events receives them without a second call.
    pub fn from_wire(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some(SUBSCRIBER_STATE_RUNNING) => Some(Self::Running),
            Some(SUBSCRIBER_STATE_STOPPED) => Some(Self::Stopped),
            Some(_) => None,
        }
    }
}

/// A stored subscriber. Every member of `CreateSubscriberResponse` and
/// `DescribeSubscriberResponse` is either a field here or computed from one.
#[derive(Debug, Clone)]
pub struct Subscriber {
    pub arn: String,
    pub name: String,
    /// The `EventBusArn` of the bus this subscriber is on.
    pub event_bus_arn: String,
    pub account_id: String,
    /// The region this subscriber lives in: the region of its bus, since a bus
    /// belongs to exactly one region and the subscriber is created on that bus.
    pub region: String,
    pub invoke_configuration: InvokeConfiguration,
    pub description: Option<String>,
    pub filter_configuration: Option<FilterConfiguration>,
    /// The `Type` — the delivery ordering mode — as reads report it.
    pub ordering_type: String,
    pub starting_position: String,
    pub point_in_time_configuration: Option<PointInTimeConfiguration>,
    pub batch_configuration: Option<BatchConfiguration>,
    pub transformer: Option<Transformer>,
    pub retry_policy: Option<RetryPolicy>,
    pub on_failure_configuration: Option<OnFailureConfiguration>,
    pub log_configuration: Option<LogConfiguration>,
    pub state: SubscriberRunState,
    /// Whether the bus owner has withdrawn this subscriber, which `Revoked`
    /// reports.
    pub revoked: bool,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// The `ClientToken` the create that minted this subscriber carried, if any.
    pub client_token: Option<String>,
    /// The position in its bus's log this subscriber delivers from next: the
    /// `SequenceNumber` of the earliest event it has still to be delivered.
    ///
    /// A subscriber CONSUMES its bus's log from the position its
    /// `StartingPosition` put it at ([`starting_sequence_number`]):
    /// `ResumePosition` documents the "position from which delivery resumes when
    /// a subscriber is unpaused", so a subscriber holds a position on the log
    /// rather than only seeing what is published while it watches. The position
    /// advances as the events it is owed are taken
    /// ([`ComAmazonawsEventbridgev2Awseventsv2State::take_due_delivery`]) and
    /// stands still while the subscriber is not delivering, which is what leaves
    /// a stopped subscriber its backlog.
    pub next_sequence_number: u64,
}

impl Subscriber {
    /// Whether this subscriber is owed the events its bus accepts right now.
    ///
    /// "Delivery requires State RUNNING on a subscriber that is not revoked" —
    /// `SubscriberState`'s own documentation — so both halves of that sentence
    /// are read here rather than at each delivery site.
    pub fn delivers(&self) -> bool {
        self.state == SubscriberRunState::Running && !self.revoked
    }

    /// The ARN of the resource this subscriber invokes, as its
    /// `InvokeConfiguration` names it.
    pub fn target_arn(&self) -> &str {
        &self.invoke_configuration.target_arn
    }

    /// The instant past which this subscriber is owed no event — "Timestamp to
    /// stop at. Optional." (`PointInTimeConfiguration$EndPoint`) — or `None`
    /// for one whose delivery is not bounded ahead.
    pub fn end_point(&self) -> Option<SystemTime> {
        self.point_in_time_configuration
            .as_ref()?
            .end_point
            .as_ref()
            .and_then(super::from_date_time)
    }
}

/// The position on `bus`'s log a subscriber created with this starting position
/// begins delivering from.
///
/// `LATEST` begins at the END of the log as it stands now, so the subscriber is
/// owed what is published after it exists. `POINT_IN_TIME` begins inside the
/// retained log, at the point its `PointInTimeConfiguration` names: the
/// `HORIZON` — the oldest event the bus still holds — or the first event
/// received at or after the `StartingPoint` a `TIMESTAMP` names. A point past
/// every stored event, and one carrying no instant to place, is the end of the
/// log, which owes the subscriber nothing published before it existed.
///
/// The position is by the log's own order (`SequenceNumber`), which is receipt
/// order, so a `TIMESTAMP` is placed by when the bus RECEIVED an event rather
/// than by the `Time` its publisher stated: a client-stated time can precede
/// another event's and would otherwise put the position behind events already
/// delivered.
pub fn starting_sequence_number(
    bus: &super::EventBus,
    starting_position: &str,
    point: Option<&PointInTimeConfiguration>,
) -> u64 {
    let tip = bus.next_sequence_number;
    if starting_position != STARTING_POSITION_POINT_IN_TIME {
        return tip;
    }
    let horizon = bus
        .events
        .first()
        .map_or(tip, |oldest| oldest.sequence_number);
    match point.map(|point| point.point_type.as_str()) {
        // A `POINT_IN_TIME` subscriber carrying no point at all is at the
        // horizon: the whole retained log is the backlog it asked for.
        None | Some(POINT_TYPE_HORIZON) => horizon,
        Some(POINT_TYPE_TIMESTAMP) => {
            let Some(from) = point
                .and_then(|point| point.starting_point.as_ref())
                .and_then(super::from_date_time)
            else {
                return tip;
            };
            bus.events
                .iter()
                .find(|stored| stored.received_at >= from)
                .map_or(tip, |first| first.sequence_number)
        }
        Some(_) => tip,
    }
}

/// Mint the ARN of a subscriber:
/// `arn:aws:events:<region>:<account>:subscriber/<name>/<id>`, the form the
/// `SubscriberArn` pattern pins.
pub fn subscriber_arn(region: &str, name: &str, subscriber_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{ARN_RESOURCE_TYPE_SUBSCRIBER}/{name}/{subscriber_id}"),
    )
}

/// The subscriber id for `seed`: [`SUBSCRIBER_ID_LEN`] lowercase base-36 digits,
/// as the `SubscriberArn` pattern's `[a-z0-9]{25}` segment requires.
///
/// `seed` comes from
/// [`ComAmazonawsEventbridgev2Awseventsv2State::next_id_seed`], which hands out
/// each value once, and the mapping from seed to id is injective, so no two
/// subscribers of one store can carry the same id.
pub fn new_subscriber_id(seed: u64, now: SystemTime) -> String {
    base36(spread(seed, now), SUBSCRIBER_ID_LEN)
}

/// One accepted event as its bus's subscribers see it: the three parts a filter
/// is scoped to, beside the identity `PutEvents` reported for it.
///
/// The parts are the event's own, as this service models an event:
/// `PutRawEvents` publishes `Data`, `Metadata` and `SystemMetadata`, and
/// `FilterScope` scopes a filter to exactly one of those three. A structured
/// `PutEvents` entry carries the same three parts — its `Detail` is the data,
/// the members the publisher sets beside it are the metadata, and its
/// `SystemMetadata` is the system metadata — so one rendering serves both
/// filtering and delivery.
#[derive(Debug, Clone)]
pub struct DeliveredEvent {
    /// The `EventId` `PutEvents` reported for this event.
    pub event_id: String,
    /// The `SequenceNumber` `PutEvents` reported for this event.
    pub sequence_number: u64,
    /// The bus this event was published to.
    pub event_bus_arn: String,
    /// The event's payload: its `Detail` as a document.
    pub data: ::serde_json::Value,
    /// The metadata the publisher sent with the event.
    pub metadata: ::serde_json::Value,
    /// The event's system metadata.
    pub system_metadata: ::serde_json::Value,
}

/// Render a stored event as its bus's subscribers see it.
///
/// `Detail` is parsed as a document, because a `DATA`-scoped filter reaches INTO
/// it and a target is handed it as a document rather than as a quoted string. An
/// entry that carried no `Detail`, or one whose text is not a document, has an
/// empty object for its data — the same value a `DATA` clause finds nothing in.
pub fn delivered_event(event_bus_arn: &str, stored: &StoredEvent) -> DeliveredEvent {
    let mut metadata = ::serde_json::Map::new();
    metadata.insert(
        SOURCE_METADATA.to_string(),
        ::serde_json::Value::String(stored.source.clone()),
    );
    metadata.insert(
        DETAIL_TYPE_METADATA.to_string(),
        ::serde_json::Value::String(stored.detail_type.clone()),
    );
    metadata.insert(
        RESOURCES_METADATA.to_string(),
        ::serde_json::Value::Array(
            stored
                .resources
                .iter()
                .map(|resource| ::serde_json::Value::String(resource.clone()))
                .collect(),
        ),
    );
    metadata.insert(
        TIME_METADATA.to_string(),
        ::serde_json::Value::String(to_event_time(stored.time)),
    );

    let mut system_metadata = ::serde_json::Map::new();
    if let Some(group) = &stored.event_group_id {
        system_metadata.insert(
            EVENT_GROUP_ID_METADATA.to_string(),
            ::serde_json::Value::String(group.clone()),
        );
    }
    if let Some(deduplication_id) = &stored.deduplication_id {
        system_metadata.insert(
            DEDUPLICATION_ID_METADATA.to_string(),
            ::serde_json::Value::String(deduplication_id.clone()),
        );
    }

    DeliveredEvent {
        event_id: stored.event_id.clone(),
        sequence_number: stored.sequence_number,
        event_bus_arn: event_bus_arn.to_string(),
        data: stored
            .detail
            .as_deref()
            .and_then(|detail| ::serde_json::from_str(detail).ok())
            .unwrap_or_else(|| ::serde_json::Value::Object(::serde_json::Map::new())),
        metadata: ::serde_json::Value::Object(metadata),
        system_metadata: ::serde_json::Value::Object(system_metadata),
    }
}

/// The metadata member carrying a structured entry's `Source`.
const SOURCE_METADATA: &str = "Source";

/// The metadata member carrying a structured entry's `DetailType`.
const DETAIL_TYPE_METADATA: &str = "DetailType";

/// The metadata member carrying a structured entry's `Resources`.
const RESOURCES_METADATA: &str = "Resources";

/// The metadata member carrying a structured entry's `Time`.
const TIME_METADATA: &str = "Time";

/// The system-metadata member carrying an entry's `EventGroupId`, as
/// `PutEventsSystemMetadata` names it.
const EVENT_GROUP_ID_METADATA: &str = "EventGroupId";

/// The system-metadata member carrying an entry's `DeduplicationId`, as
/// `PutEventsSystemMetadata` names it.
const DEDUPLICATION_ID_METADATA: &str = "DeduplicationId";

impl DeliveredEvent {
    /// The part of this event a filter of `scope` is matched against, or `None`
    /// for a scope this slice does not place — a value `FilterScope` does not
    /// declare, which the boundary validator refuses before a handler stores it.
    pub fn scoped(&self, scope: &str) -> Option<&::serde_json::Value> {
        match scope {
            FILTER_SCOPE_DATA => Some(&self.data),
            FILTER_SCOPE_METADATA => Some(&self.metadata),
            FILTER_SCOPE_SYSTEM_METADATA => Some(&self.system_metadata),
            _ => None,
        }
    }

    /// This event as the whole document a target is handed: its identity, the
    /// publisher's metadata, its system metadata and its payload.
    pub fn document(&self) -> ::serde_json::Value {
        let mut document = ::serde_json::Map::new();
        document.insert(
            DELIVERED_EVENT_ID.to_string(),
            ::serde_json::Value::String(self.event_id.clone()),
        );
        // `SequenceNumber` is a string on the wire
        // (`PutEventsResultEntry$SequenceNumber`), so it travels as the same
        // text the publisher was told.
        document.insert(
            DELIVERED_SEQUENCE_NUMBER.to_string(),
            ::serde_json::Value::String(self.sequence_number.to_string()),
        );
        document.insert(
            DELIVERED_EVENT_BUS_ARN.to_string(),
            ::serde_json::Value::String(self.event_bus_arn.clone()),
        );
        document.insert(DELIVERED_METADATA.to_string(), self.metadata.clone());
        document.insert(
            DELIVERED_SYSTEM_METADATA.to_string(),
            self.system_metadata.clone(),
        );
        document.insert(DELIVERED_DATA.to_string(), self.data.clone());
        ::serde_json::Value::Object(document)
    }
}

/// Whether every filter of `configuration` matches `event`.
///
/// "All Filters are implicitly ANDed — an event must match every Filter to be
/// delivered" (`FilterConfiguration`), and each Filter names the `Scope` of the
/// event it reads: `DATA` the payload, `METADATA` the publisher's metadata,
/// `SYSTEM_METADATA` the system metadata ([`DeliveredEvent::scoped`]). A
/// subscriber that configures no filters at all constrains nothing, so every
/// event its bus accepts is one it is owed.
///
/// The pattern is read by the ONE shared EventBridge pattern matcher
/// (`service_common::event_pattern`), so a pattern that selects an event here
/// selects it wherever else this emulator evaluates one, and the work it costs
/// is charged to `budget`.
pub fn filters_match(
    configuration: Option<&FilterConfiguration>,
    event: &DeliveredEvent,
    budget: &mut ::service_common::event_pattern::MatchBudget,
) -> bool {
    let Some(filters) = configuration.and_then(|configuration| configuration.filters.as_deref())
    else {
        return true;
    };
    filters
        .iter()
        .all(|filter| filter_matches(filter, event, budget))
}

/// Whether one filter matches `event`.
///
/// A pattern this matcher cannot read selects nothing: a stored pattern was held
/// to [`parse_filter_pattern`] when it was written, so reaching here with an
/// unreadable one would mean the event is not the subscriber's.
fn filter_matches(
    filter: &Filter,
    event: &DeliveredEvent,
    budget: &mut ::service_common::event_pattern::MatchBudget,
) -> bool {
    let Some(scoped) = event.scoped(&filter.scope) else {
        return false;
    };
    let Ok(pattern) = parse_filter_pattern(&filter.pattern) else {
        return false;
    };
    ::service_common::event_pattern::matches_within(&pattern, scoped, budget)
}

/// Read a filter's `Pattern` as an event pattern, or report why it is not one.
///
/// `FilterConfiguration$Language` declares one language,
/// `EVENT_BRIDGE_PATTERN`, so a filter's pattern is an EventBridge event
/// pattern and is read by the one shared reader of those. The `Err` is the
/// reason, for a handler to word into its own modelled refusal.
pub fn parse_filter_pattern(pattern: &str) -> Result<::serde_json::Value, String> {
    ::service_common::event_pattern::parse(pattern)
}

/// What this subscriber's target is handed for `event`, or `None` when the
/// subscriber's `Transformer` names a transformation this slice does not perform
/// — in which case nothing is delivered rather than a document the transformer
/// did not produce.
///
/// `Transformer$Type` names the shape of that text: `RAW` hands the target the
/// event's payload alone, `WITH_METADATA` hands it the event with its metadata
/// ([`DeliveredEvent::document`]), which is also what a subscriber that
/// configures no transformer hands it. `JSONATA` names a transformation
/// computed by an expression; this slice performs none, so a subscriber
/// configuring it delivers nothing here.
pub fn delivered_body(subscriber: &Subscriber, event: &DeliveredEvent) -> Option<String> {
    let transformer_type = subscriber
        .transformer
        .as_ref()
        .and_then(|transformer| transformer.r#type.as_deref());
    match transformer_type {
        None | Some(TRANSFORMER_TYPE_WITH_METADATA) => Some(event.document().to_string()),
        Some(TRANSFORMER_TYPE_RAW) => Some(event.data.to_string()),
        Some(_) => None,
    }
}

/// `at` as the instant a delivered event's metadata carries.
///
/// An event's instant is RFC 3339 at whole-second precision closed by `Z`
/// (`2026-01-01T00:00:00Z`), the form an event carries it in, so the sub-second
/// remainder of a server clock reading is dropped before the instant is
/// formatted rather than written into a field no delivered event carries it in.
fn to_event_time(at: SystemTime) -> String {
    ::aws_smithy_types::DateTime::from_secs(super::to_date_time(at).secs())
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// One subscriber's due deliveries: the subscriber, and the events of its bus's
/// log it is owed and has not been delivered yet.
#[derive(Debug, Clone)]
pub struct DueDelivery {
    pub subscriber: Subscriber,
    pub events: Vec<StoredEvent>,
}

impl ComAmazonawsEventbridgev2Awseventsv2State {
    /// The subscriber with this ARN, or `None` when no such subscriber exists.
    pub fn subscriber(&self, arn: &str) -> Option<&Subscriber> {
        self.subscribers.get(arn)
    }

    /// The subscriber carrying this name in this region.
    ///
    /// A `SubscriberArn` names a subscriber by its name and a minted id alone —
    /// the bus is not part of it — so a subscriber name is unique per account
    /// and region, the way a bus name is. A scan is enough at an emulator's
    /// scale and cannot drift out of step with [`Self::subscribers`] the way a
    /// second index could.
    pub fn subscriber_by_name(&self, region: &str, name: &str) -> Option<&Subscriber> {
        self.subscribers
            .values()
            .find(|subscriber| subscriber.region == region && subscriber.name == name)
    }

    /// Take what the subscriber `arn` is owed right now, advancing its position
    /// past the events handed back.
    ///
    /// MUTATING, and called only where the spec says this call makes deliveries
    /// due — a publish onto the bus, and the create that puts a subscriber at a
    /// position with a backlog behind it. The position advances HERE rather than
    /// at the target, so one event is handed to one subscriber once however many
    /// times either is read afterwards.
    ///
    /// Nothing is owed to a subscriber that is not delivering — "Delivery
    /// requires State RUNNING on a subscriber that is not revoked" — and such a
    /// subscriber's position stands still, which is what leaves it the backlog
    /// `ResumePosition` resumes from. The events past the subscriber's
    /// `EndPoint` are not owed either, and the position moves past them: that
    /// bound is an instant, so an event received after it is never delivered.
    pub fn take_due_delivery(&mut self, arn: &str) -> Option<DueDelivery> {
        let subscriber = self.subscribers.get(arn)?;
        if !subscriber.delivers() {
            return None;
        }
        let bus = self.event_buses.get(&subscriber.event_bus_arn)?;
        let end_point = subscriber.end_point();
        let from = subscriber.next_sequence_number;
        let events: Vec<StoredEvent> = bus
            .events
            .iter()
            .filter(|stored| stored.sequence_number >= from)
            .filter(|stored| end_point.is_none_or(|end| stored.received_at <= end))
            .cloned()
            .collect();
        let tip = bus.next_sequence_number;
        let subscriber = self.subscribers.get_mut(arn)?;
        subscriber.next_sequence_number = tip;
        Some(DueDelivery {
            subscriber: subscriber.clone(),
            events,
        })
    }

    /// Take what every subscriber of this bus is owed, as
    /// [`Self::take_due_delivery`] does for one.
    pub fn take_due_deliveries(&mut self, event_bus_arn: &str) -> Vec<DueDelivery> {
        let on_this_bus: Vec<String> = self
            .subscribers
            .values()
            .filter(|subscriber| subscriber.event_bus_arn == event_bus_arn)
            .map(|subscriber| subscriber.arn.clone())
            .collect();
        on_this_bus
            .iter()
            .filter_map(|arn| self.take_due_delivery(arn))
            .filter(|due| !due.events.is_empty())
            .collect()
    }

    /// Forget the subscribers of a bus that has finished disappearing, and the
    /// client tokens that replayed them.
    ///
    /// A subscriber is created ON one bus, so it cannot outlive that bus: one
    /// left behind would name a bus no read can resolve.
    pub fn forget_subscribers_of_bus(&mut self, event_bus_arn: &str) {
        let orphaned: Vec<String> = self
            .subscribers
            .values()
            .filter(|subscriber| subscriber.event_bus_arn == event_bus_arn)
            .map(|subscriber| subscriber.arn.clone())
            .collect();
        for arn in orphaned {
            self.subscribers.remove(&arn);
            self.subscriber_idempotency.retain(|_, minted| *minted != arn);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        FILTER_SCOPE_DATA, FILTER_SCOPE_METADATA, FILTER_SCOPE_SYSTEM_METADATA,
        ORDERING_TYPE_UNORDERED, STARTING_POSITION_LATEST, SUBSCRIBER_STATE_RUNNING,
        SUBSCRIBER_STATE_STOPPED, TRANSFORMER_TYPE_RAW, TRANSFORMER_TYPE_WITH_METADATA,
    };
    use crate::ops::types::{
        FILTER_SCOPE_VALUES, ORDERING_TYPE_VALUES, STARTING_POSITION_VALUES,
        SUBSCRIBER_STATE_VALUES, TRANSFORMER_TYPE_VALUES,
    };

    /// Every value this slice puts on the wire, or keys a decision on, is a
    /// value the model declares, so these constants cannot drift from the
    /// generated enums.
    #[test]
    fn the_subscriber_vocabulary_is_the_modelled_one() {
        for (case, value, declared) in [
            (
                "SubscriberState::RUNNING",
                SUBSCRIBER_STATE_RUNNING,
                SUBSCRIBER_STATE_VALUES,
            ),
            (
                "SubscriberState::STOPPED",
                SUBSCRIBER_STATE_STOPPED,
                SUBSCRIBER_STATE_VALUES,
            ),
            (
                "OrderingType::UNORDERED",
                ORDERING_TYPE_UNORDERED,
                ORDERING_TYPE_VALUES,
            ),
            (
                "StartingPosition::LATEST",
                STARTING_POSITION_LATEST,
                STARTING_POSITION_VALUES,
            ),
            ("FilterScope::DATA", FILTER_SCOPE_DATA, FILTER_SCOPE_VALUES),
            (
                "FilterScope::METADATA",
                FILTER_SCOPE_METADATA,
                FILTER_SCOPE_VALUES,
            ),
            (
                "FilterScope::SYSTEM_METADATA",
                FILTER_SCOPE_SYSTEM_METADATA,
                FILTER_SCOPE_VALUES,
            ),
            (
                "TransformerType::RAW",
                TRANSFORMER_TYPE_RAW,
                TRANSFORMER_TYPE_VALUES,
            ),
            (
                "TransformerType::WITH_METADATA",
                TRANSFORMER_TYPE_WITH_METADATA,
                TRANSFORMER_TYPE_VALUES,
            ),
        ] {
            assert!(
                declared.contains(&value),
                "case {case:?}: {value:?} is not one of the modelled values {declared:?}"
            );
        }
    }
}
