// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-location-locationservice service.
//!
//! One lock holds every tracker resource plus its device position histories, so a
//! position written by `BatchUpdateDevicePosition` is readable by
//! `GetDevicePosition` on a later request and a deleted tracker takes its
//! histories with it.

use std::collections::{BTreeMap, BTreeSet};

/// The geometry checks a geofence write makes, kept beside this module rather than
/// in one operation's file because every geofence write shares them.
mod geofence_geometry;
pub use geofence_geometry::validate_geofence_geometry;

/// The resource a tag operation's `ResourceArn` names, and the tag map it reads and
/// writes on it, kept beside this module because the three tag operations and the
/// resources' own reads all reach the same per-resource tag map.
mod resource_tags;
pub use resource_tags::{
    merge_tags, not_an_arn_of_this_service, remove_tags, unresolvable_arn_not_found,
    validate_tag_keys, TagTarget, TaggedKind, UnresolvedArn, ANY_RESOURCE, ARN_VENDOR,
    MAX_TAGS_PER_RESOURCE, RESERVED_TAG_KEY_PREFIX, RESOURCE_ARN_MEMBER, TAGS_MEMBER,
};

/// The map styles a map resource can carry and the data provider each is sourced
/// from, kept beside this module because a map write checks the style and every map
/// read reports its provider.
mod map_styles;
pub use map_styles::{
    map_data_source, map_style, MapStyle, MAP_STYLES, MAP_STYLE_MEMBER, PROVIDER_ESRI,
    PROVIDER_GRAB, PROVIDER_HERE, PROVIDER_OPEN_DATA,
};

/// Epoch milliseconds. Amazon Location renders every timestamp in ISO 8601
/// `YYYY-MM-DDThh:mm:ss.sssZ` (see the `CreateTime`/`SampleTime` member docs in the
/// Smithy model), so millisecond resolution is the precision the wire carries.
pub type EpochMillis = i64;

/// The region used for generated ARNs when a request carries no SigV4 credential
/// scope to read one from (an unsigned, host-routed request).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `PositionFiltering` applied when `CreateTracker` leaves it unset. The model's
/// `CreateTrackerRequest$PositionFiltering` doc: "This field is optional. If not
/// specified, the default value is `TimeBased`."
pub const DEFAULT_POSITION_FILTERING: &str = "TimeBased";

/// `PricingPlan` every tracker read reports. The model's
/// `DescribeTrackerResponse$PricingPlan` doc: "Always returns `RequestBasedUsage`."
pub const ALWAYS_PRICING_PLAN: &str = "RequestBasedUsage";

/// Per-service state for com-amazonaws-location-locationservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLocationLocationserviceState {
    /// Tracker resources by `TrackerName`. A `BTreeMap` so list operations have a
    /// stable, total order to paginate over with a key-based cursor.
    pub trackers: BTreeMap<String, Tracker>,
    /// Route calculator resources by `CalculatorName`, ordered for the same reason
    /// as [`Self::trackers`].
    pub route_calculators: BTreeMap<String, RouteCalculator>,
    /// Geofence collection resources by `CollectionName`, ordered for the same
    /// reason as [`Self::trackers`].
    pub geofence_collections: BTreeMap<String, GeofenceCollection>,
    /// Map resources by `MapName`, ordered for the same reason as
    /// [`Self::trackers`].
    pub maps: BTreeMap<String, Map>,
}

/// One map resource: the members a map read reports for it, with the configuration
/// kept as the modelled `MapConfiguration` the map was created with so a read
/// reports the configuration the create stored.
#[derive(Debug, Clone)]
pub struct Map {
    pub name: String,
    pub arn: String,
    /// `DescribeMap`/`ListMaps` model `Description` as required, and the member is
    /// `len 0..1000`, so an unset description is the empty string.
    pub description: String,
    pub configuration: crate::ops::types::MapConfiguration,
    /// The data provider the configured style's tiles come from, resolved from the
    /// style catalog when the map was created.
    pub data_source: String,
    pub tags: BTreeMap<String, String>,
    pub create_time: EpochMillis,
    pub update_time: EpochMillis,
}

/// One tracker resource and everything stored under it.
#[derive(Debug, Clone)]
pub struct Tracker {
    pub name: String,
    pub arn: String,
    /// `DescribeTracker`/`ListTrackers` model `Description` as required, and the
    /// member is `len 0..1000`, so an unset description is the empty string.
    pub description: String,
    pub position_filtering: String,
    pub kms_key_id: Option<String>,
    pub event_bridge_enabled: bool,
    pub kms_key_enable_geospatial_queries: bool,
    pub tags: BTreeMap<String, String>,
    pub create_time: EpochMillis,
    pub update_time: EpochMillis,
    /// Geofence-collection ARNs associated through `AssociateTrackerConsumer`.
    pub consumer_arns: BTreeSet<String>,
    /// Position history per `DeviceId`, kept sorted ascending by `sample_time`, so
    /// the latest position is the last element.
    pub devices: BTreeMap<String, Vec<StoredPosition>>,
}

/// One route calculator resource.
#[derive(Debug, Clone)]
pub struct RouteCalculator {
    pub name: String,
    pub arn: String,
    /// `DescribeRouteCalculator` models `Description` as required, and the member
    /// is `len 0..1000`, so an unset description is the empty string.
    pub description: String,
    /// The data provider the calculator sources traffic and road network data from,
    /// as the create request spelled it.
    pub data_source: String,
    pub tags: BTreeMap<String, String>,
    pub create_time: EpochMillis,
    pub update_time: EpochMillis,
}

/// One geofence collection resource and everything stored under it.
#[derive(Debug, Clone)]
pub struct GeofenceCollection {
    pub name: String,
    pub arn: String,
    /// `DescribeGeofenceCollection` models `Description` as required, and the
    /// member is `len 0..1000`, so an unset description is the empty string.
    pub description: String,
    /// The KMS customer managed key the collection was created with, if the create
    /// request named one.
    pub kms_key_id: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub create_time: EpochMillis,
    pub update_time: EpochMillis,
    /// The geofences held by this collection, keyed by `GeofenceId` and ordered for
    /// the same reason as [`ComAmazonawsLocationLocationserviceState::trackers`].
    /// `DescribeGeofenceCollection` reports `GeofenceCount` as this map's size, so
    /// the count a read answers is however many geofences the collection holds.
    pub geofences: BTreeMap<String, Geofence>,
}

/// One geofence stored under a collection: the members `GetGeofence` reports for
/// it, with the boundary kept as the modelled `GeofenceGeometry` the geofence was
/// put with.
#[derive(Debug, Clone)]
pub struct Geofence {
    pub geofence_id: String,
    pub geometry: crate::ops::types::GeofenceGeometry,
    /// `GetGeofenceResponse$Status` — the geofence's evaluation status.
    pub status: String,
    pub create_time: EpochMillis,
    pub update_time: EpochMillis,
    /// `GeofenceProperties`: user-defined properties, `len 0..3`.
    pub properties: BTreeMap<String, String>,
}

/// One stored device position sample.
#[derive(Debug, Clone)]
pub struct StoredPosition {
    pub device_id: String,
    pub sample_time: EpochMillis,
    /// Stamped by the service when it accepted the update, never taken from the
    /// request.
    pub received_time: EpochMillis,
    /// WGS 84 `[longitude, latitude]` — the `Position` shape is `len 2..2`.
    pub position: [f64; 2],
    /// `PositionalAccuracy$Horizontal`, in metres.
    pub accuracy: Option<f64>,
    pub position_properties: BTreeMap<String, String>,
}

impl StoredPosition {
    /// The modelled `DevicePosition` this sample is reported as.
    ///
    /// Every position read of this service projects a stored sample the same way, so
    /// the projection lives on the sample rather than in one operation's file:
    /// `GetDevicePosition` reports the device's newest one and
    /// `GetDevicePositionHistory` reports a page of them, and both must agree member
    /// for member.
    pub fn to_device_position(&self) -> crate::ops::types::DevicePosition {
        crate::ops::types::DevicePosition {
            device_id: Some(self.device_id.clone()),
            sample_time: to_date_time(self.sample_time),
            received_time: to_date_time(self.received_time),
            position: self.position.to_vec(),
            accuracy: self
                .accuracy
                .map(|horizontal| crate::ops::types::PositionalAccuracy { horizontal }),
            position_properties: if self.position_properties.is_empty() {
                // No properties were stored with the sample, so the member is
                // absent rather than an empty map.
                None
            } else {
                Some(
                    self.position_properties
                        .iter()
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect(),
                )
            },
        }
    }
}

/// Largest `Updates` batch `BatchUpdateDevicePosition` accepts, which is the API's
/// own maximum: the `DevicePositionUpdateList` shape is `len 1..10`. The handler
/// enforces it itself as well as relying on the request boundary, so the amount of
/// work one request can ask for is never chosen by the caller.
pub const MAX_BATCH_UPDATES: usize = 10;

/// How long a device's position history is kept, as a span of SAMPLE time.
///
/// Amazon Location Service retains device position history for 30 days
/// (<https://docs.aws.amazon.com/location/latest/developerguide/device-tracking.html>:
/// "Device position history is stored for 30 days"). The window is measured from
/// the newest stored sample rather than from the wall clock, so a client that
/// writes historical samples still reads back the history it just wrote; the
/// retained span is the documented one either way.
pub const POSITION_HISTORY_RETENTION_MILLIS: EpochMillis = 30 * 24 * 60 * 60 * 1_000;

/// Emulator memory bound, NOT an AWS quota: samples kept per device, oldest
/// evicted first. The real service bounds history with position filtering plus
/// retention; this cap exists so repeated writes cannot grow the in-memory store
/// without limit, and is chosen large enough that a client never meets it in a
/// normal session.
pub const MAX_POSITIONS_PER_DEVICE: usize = 200;

/// Emulator memory bound, NOT an AWS quota: devices tracked per tracker. When it
/// is reached, the device whose newest sample is oldest is evicted to make room,
/// so a caller minting unbounded device ids cannot grow the store without limit.
pub const MAX_DEVICES_PER_TRACKER: usize = 1_000;

/// Emulator memory bound, NOT an AWS quota: tracker resources held at once.
/// `CreateTracker` refuses past it with the modelled `ServiceQuotaExceededException`
/// rather than growing the store without limit.
pub const MAX_TRACKERS: usize = 1_000;

/// Emulator memory bound, NOT an AWS quota: route calculator resources held at
/// once. `CreateRouteCalculator` refuses past it with the modelled
/// `ServiceQuotaExceededException` rather than growing the store without limit.
pub const MAX_ROUTE_CALCULATORS: usize = 1_000;

/// Emulator memory bound, NOT an AWS quota: geofence collection resources held at
/// once. `CreateGeofenceCollection` refuses past it with the modelled
/// `ServiceQuotaExceededException` rather than growing the store without limit.
pub const MAX_GEOFENCE_COLLECTIONS: usize = 1_000;

/// Emulator memory bound, NOT an AWS quota: map resources held at once. `CreateMap`
/// refuses past it with the modelled `ServiceQuotaExceededException` rather than
/// growing the store without limit.
pub const MAX_MAPS: usize = 1_000;

/// Emulator memory bound, NOT an AWS quota: geofences held per collection, the
/// least recently updated evicted first. The real service bounds a collection with
/// its own quota; this cap exists so repeated writes cannot grow the in-memory store
/// without limit, and is chosen large enough that a client never meets it in a
/// normal session. [`GeofenceCollection::record_geofence`] is where it is applied,
/// and is the contract a geofence write uses: every write into this map goes through
/// that helper.
pub const MAX_GEOFENCES_PER_COLLECTION: usize = 1_000;

/// Largest `Entries` batch `BatchPutGeofence` accepts, which is the API's own
/// maximum: the `BatchPutGeofenceRequestEntryList` shape is `len 1..10`. The handler
/// enforces it itself as well as relying on the request boundary, so the amount of
/// work one request can ask for is never chosen by the caller.
pub const MAX_BATCH_PUT_GEOFENCE_ENTRIES: usize = 10;

/// The `Status` a stored geofence reports: the `GetGeofenceResponse$Status` member's
/// "`ACTIVE` — The geofence has been indexed by the system".
///
/// A geofence write is committed by the call that makes it — `BatchPutGeofence` and
/// `PutGeofence` model no status member at all, and the geofence is readable in full
/// the moment the write answers — so a stored geofence is an indexed one and the
/// clock owes it no later transition out of `PENDING`.
pub const GEOFENCE_STATUS_ACTIVE: &str = "ACTIVE";

/// Emulator memory bound, NOT an AWS quota: consumer associations held per tracker.
/// [`Tracker::associate_consumer`] reports no room past it rather than growing the
/// store without limit, so a caller minting unbounded collection ARNs is refused
/// instead of accumulating them. It is the contract an association write uses:
/// `AssociateTrackerConsumer` is a scaffolded 501 today, so the only way into this
/// set goes through that helper.
pub const MAX_CONSUMERS_PER_TRACKER: usize = 100;

impl Tracker {
    /// Record `sample` in its device's history, keeping the history sorted ascending
    /// by `sample_time`, then apply the retention window and the memory bounds.
    ///
    /// A sample is keyed by `(DeviceId, SampleTime)`: an update for an instant the
    /// device already holds REPLACES it rather than adding a second sample for the
    /// same instant.
    ///
    /// Every other branch here shrinks or keeps the history, so the state this
    /// tracker holds is bounded by
    /// `MAX_DEVICES_PER_TRACKER * MAX_POSITIONS_PER_DEVICE` however many updates
    /// arrive.
    pub fn record_position(&mut self, sample: StoredPosition) {
        if self.devices.len() >= MAX_DEVICES_PER_TRACKER
            && !self.devices.contains_key(&sample.device_id)
        {
            self.evict_least_recently_sampled_device();
        }

        let history = self.devices.entry(sample.device_id.clone()).or_default();
        // `partition_point` with `<` lands on the first sample at this instant, so
        // an existing one is overwritten and a new instant is inserted in order.
        let at = history.partition_point(|p| p.sample_time < sample.sample_time);
        match history.get_mut(at) {
            Some(existing) if existing.sample_time == sample.sample_time => *existing = sample,
            _ => history.insert(at, sample),
        }

        // Retention: drop everything older than the window, measured back from
        // the newest sample now held.
        if let Some(newest) = history.last().map(|p| p.sample_time) {
            let cutoff = newest.saturating_sub(POSITION_HISTORY_RETENTION_MILLIS);
            let expired = history.partition_point(|p| p.sample_time < cutoff);
            if expired > 0 {
                history.drain(..expired);
            }
        }

        // Memory bound: keep the newest `MAX_POSITIONS_PER_DEVICE` samples.
        if history.len() > MAX_POSITIONS_PER_DEVICE {
            let excess = history.len() - MAX_POSITIONS_PER_DEVICE;
            history.drain(..excess);
        }
    }

    /// Drop the device whose newest sample is the oldest, to make room under
    /// [`MAX_DEVICES_PER_TRACKER`].
    fn evict_least_recently_sampled_device(&mut self) {
        let stale = self
            .devices
            .iter()
            .min_by_key(|(_, history)| {
                history.last().map_or(EpochMillis::MIN, |p| p.sample_time)
            })
            .map(|(device_id, _)| device_id.clone());
        if let Some(device_id) = stale {
            // Logged because this discards stored positions on an otherwise
            // successful write: someone debugging a device that stopped answering
            // GetDevicePosition needs to see that the bound, not the caller,
            // dropped it. The ids are rendered with Debug, so a value carrying
            // control characters cannot forge a line of the emulator's log.
            ::tracing::warn!(
                tracker = ?self.name,
                device_id = ?device_id,
                max_devices_per_tracker = MAX_DEVICES_PER_TRACKER,
                "evicting the least recently sampled device to stay within the emulator's per-tracker device bound"
            );
            self.devices.remove(&device_id);
        }
    }

    /// The most recently sampled position for `device_id`, or `None` when the
    /// device has no stored position under this tracker.
    pub fn latest_position(&self, device_id: &str) -> Option<&StoredPosition> {
        self.devices.get(device_id)?.last()
    }

    /// The samples `device_id` holds whose `SampleTime` lies in the HALF-OPEN window
    /// `[start, end)` — the start instant included, the end instant excluded — oldest
    /// first.
    ///
    /// Half-open is what the model's member names state
    /// (`StartTimeInclusive`/`EndTimeExclusive`), so a sample stamped exactly at the
    /// end instant is outside the window while one stamped exactly at the start is
    /// inside it. A device with no stored samples has an empty history, not a missing
    /// one: a history read over a window is a query, and a window holding nothing is
    /// an empty answer.
    ///
    /// The returned slice is a window of the device's history, which
    /// [`Self::record_position`] keeps sorted ascending by sample time, so the order
    /// is sample-time ascending and the two `partition_point` probes are logarithmic
    /// in the history's length rather than linear.
    pub fn position_history(
        &self,
        device_id: &str,
        start: EpochMillis,
        end: EpochMillis,
    ) -> &[StoredPosition] {
        let Some(history) = self.devices.get(device_id) else {
            return &[];
        };
        let from = history.partition_point(|p| p.sample_time < start);
        let to = history.partition_point(|p| p.sample_time < end);
        // Both probes land at or before the history's end and `to` is at or after
        // `from` for every window that runs forwards, so the range is in bounds;
        // asking for it keeps the helper total (an empty window, never a panic) for
        // one that does not.
        history.get(from..to.max(from)).unwrap_or_default()
    }

    /// Associate `consumer_arn` with this tracker, reporting whether the tracker
    /// holds it afterwards.
    ///
    /// An ARN the tracker already holds stays held and is never refused (the set
    /// holds each one once). `false` is "no room": the tracker already holds
    /// [`MAX_CONSUMERS_PER_TRACKER`] other consumers, so the association is not made
    /// and the caller refuses it rather than growing the store without limit.
    #[must_use]
    pub fn associate_consumer(&mut self, consumer_arn: String) -> bool {
        if self.consumer_arns.contains(&consumer_arn) {
            return true;
        }
        if self.consumer_arns.len() >= MAX_CONSUMERS_PER_TRACKER {
            return false;
        }
        self.consumer_arns.insert(consumer_arn)
    }
}

impl GeofenceCollection {
    /// Store `geofence` under this collection, keyed by its `GeofenceId`, and apply
    /// the memory bound.
    ///
    /// A put for an id the collection already holds REPLACES it, so re-putting a
    /// geofence never grows the collection; a NEW id past
    /// [`MAX_GEOFENCES_PER_COLLECTION`] evicts the least recently updated geofence
    /// first, so what one collection holds is bounded however many puts arrive.
    pub fn record_geofence(&mut self, geofence: Geofence) {
        if self.geofences.len() >= MAX_GEOFENCES_PER_COLLECTION
            && !self.geofences.contains_key(&geofence.geofence_id)
        {
            self.evict_least_recently_updated_geofence();
        }
        self.geofences
            .insert(geofence.geofence_id.clone(), geofence);
    }

    /// Drop the geofence whose `UpdateTime` is the oldest, to make room under
    /// [`MAX_GEOFENCES_PER_COLLECTION`].
    fn evict_least_recently_updated_geofence(&mut self) {
        let stale = self
            .geofences
            .iter()
            .min_by_key(|(_, geofence)| geofence.update_time)
            .map(|(geofence_id, _)| geofence_id.clone());
        if let Some(geofence_id) = stale {
            // Logged for the reason the device eviction above is: this discards a
            // stored geofence on an otherwise successful write, and the id is
            // rendered with Debug so a value carrying control characters cannot
            // forge a line of the emulator's log.
            ::tracing::warn!(
                collection = ?self.name,
                geofence_id = ?geofence_id,
                max_geofences_per_collection = MAX_GEOFENCES_PER_COLLECTION,
                "evicting the least recently updated geofence to stay within the emulator's per-collection geofence bound"
            );
            self.geofences.remove(&geofence_id);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLocationLocationserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLocationLocationserviceState>)
}

/// Now, in epoch milliseconds. The one clock every handler in this crate reads.
pub fn now_millis() -> EpochMillis {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // `as i64` cannot overflow for any clock this emulator runs under: the value
    // is milliseconds since 1970 and `i64` covers ~292 million years.
    since_epoch.as_millis() as i64
}

/// The wire form of a stored timestamp.
pub fn to_date_time(millis: EpochMillis) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// The stored form of a wire timestamp, truncated to the millisecond precision the
/// service's ISO 8601 rendering carries.
pub fn from_date_time(value: &::aws_smithy_types::DateTime) -> EpochMillis {
    value
        .to_millis()
        .unwrap_or_else(|_| value.secs().saturating_mul(1_000))
}

/// The region a request is addressed to: the third field of its SigV4 credential
/// scope, falling back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:geo:<region>:<account>:tracker/<name>` — the format
/// `CreateTrackerResponse$TrackerArn` documents.
pub fn tracker_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("geo", region, &format!("tracker/{name}"))
}

/// `arn:aws:geo:<region>:<account>:route-calculator/<name>` — the format
/// `CreateRouteCalculatorResponse$CalculatorArn` documents.
pub fn route_calculator_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("geo", region, &format!("route-calculator/{name}"))
}

/// `arn:aws:geo:<region>:<account>:geofence-collection/<name>` — the format
/// `CreateGeofenceCollectionResponse$CollectionArn` documents.
pub fn geofence_collection_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("geo", region, &format!("geofence-collection/{name}"))
}

/// `arn:aws:geo:<region>:<account>:map/<name>` — the format
/// `CreateMapResponse$MapArn` documents ("Format example:
/// arn:aws:geo:region:account-id:map/ExampleMap").
pub fn map_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("geo", region, &format!("map/{name}"))
}

/// The data providers a route calculator can source traffic and road network data
/// from, spelled exactly as a request must spell them.
///
/// Source: the `CreateRouteCalculatorRequest$DataSource` member doc, which lists
/// `Esri`, `Grab` and `Here` as the valid values and notes that the field is
/// case-sensitive.
pub const ROUTE_CALCULATOR_DATA_SOURCES: [&str; 3] = ["Esri", "Grab", "Here"];

/// `Ok` when `data_source` names one of [`ROUTE_CALCULATOR_DATA_SOURCES`], matched
/// case-sensitively as the member documents, otherwise the sentence the refusal
/// states.
///
/// The sentence follows the form this service states a member requirement in —
/// the member name as the model spells it, quoted, then what the member has to
/// satisfy — which is also the form its `fieldList` entry repeats.
///
/// The member is a plain `String` in the model rather than an enum, so the request
/// boundary accepts any text and the provider set is the service's own check.
pub fn validate_route_calculator_data_source(data_source: &str) -> Result<(), String> {
    if ROUTE_CALCULATOR_DATA_SOURCES.contains(&data_source) {
        return Ok(());
    }
    Err(format!(
        "'DataSource' must be one of [{}]",
        ROUTE_CALCULATOR_DATA_SOURCES.join(", ")
    ))
}

/// `Ok` when a request's deprecated `PricingPlan` carries the one value it still
/// admits, otherwise the sentence the refusal states.
///
/// The member doc on every operation that still accepts it: "No longer used. If
/// included, the only allowed value is `RequestBasedUsage`." The member IS a
/// modelled enum, so the request boundary already refuses a value outside the
/// enum; what it cannot refuse is one of the enum's two retired values, which is
/// this check.
pub fn validate_deprecated_pricing_plan(pricing_plan: Option<&str>) -> Result<(), String> {
    match pricing_plan {
        None => Ok(()),
        Some(plan) if plan == ALWAYS_PRICING_PLAN => Ok(()),
        Some(_) => Err(format!("'PricingPlan' must be {ALWAYS_PRICING_PLAN}")),
    }
}

/// An opaque pagination cursor: the INCLUSIVE start key of the next page (the key
/// of its first item), tagged with the collection it was minted for.
///
/// Tagging is what makes a token checkable: a cursor from another list operation,
/// or a client-invented string, fails [`decode_next_token`] and is refused with
/// `ValidationException` rather than silently treated as a resource name.
pub fn encode_next_token(kind: &str, key: &str) -> String {
    format!("{kind}:{key}")
}

/// The inclusive start key a cursor carries, or `Err` with the reason it is
/// malformed. `kind` is the collection the caller is paging. A key whose item has
/// since been deleted still names the right page: the next item in key order
/// follows it.
pub fn decode_next_token(kind: &str, token: &str) -> Result<String, String> {
    let key = token
        .strip_prefix(kind)
        .and_then(|rest| rest.strip_prefix(':'))
        .ok_or_else(|| format!("NextToken is not a valid pagination token for {kind}"))?;
    if key.is_empty() {
        return Err("NextToken is not a valid pagination token: it names no page".to_string());
    }
    Ok(key.to_string())
}

/// The cursor tag for `ListTrackers`.
pub const TRACKERS_CURSOR: &str = "trackers";

/// The cursor tag for `ListGeofenceCollections`.
pub const GEOFENCE_COLLECTIONS_CURSOR: &str = "geofence-collections";

/// The cursor tag for `GetDevicePositionHistory`, whose page key is the INCLUSIVE
/// sample time (epoch milliseconds) of the next page's first sample rather than a
/// resource name: a device's history is keyed by `SampleTime`, one sample per
/// instant.
pub const DEVICE_POSITIONS_CURSOR: &str = "device-positions";

/// A position-history cursor for the page starting at `sample_time`.
pub fn encode_device_positions_token(sample_time: EpochMillis) -> String {
    encode_next_token(DEVICE_POSITIONS_CURSOR, &sample_time.to_string())
}

/// The inclusive sample time a position-history cursor names, or the reason it is
/// malformed.
///
/// A token this service did not mint — one from another list operation, or a
/// client-invented string — is refused here rather than read as a page key, so a
/// client is never silently handed the wrong page.
pub fn decode_device_positions_token(token: &str) -> Result<EpochMillis, String> {
    let key = decode_next_token(DEVICE_POSITIONS_CURSOR, token)?;
    key.parse::<EpochMillis>().map_err(|_| {
        format!("NextToken is not a valid pagination token for {DEVICE_POSITIONS_CURSOR}")
    })
}

/// How far back a position-history read looks when the request names no
/// `StartTimeInclusive`.
///
/// The model's `GetDevicePositionHistoryRequest$StartTimeInclusive` doc: "By default,
/// the value will be 24 hours prior to the time that the request is made."
pub const DEFAULT_HISTORY_WINDOW_MILLIS: EpochMillis = 24 * 60 * 60 * 1_000;

/// The `ValidationException` sentence a position-history read whose window does not
/// run forwards is refused with.
///
/// The model states the requirement on both members —
/// `GetDevicePositionHistoryRequest$EndTimeExclusive`: "The time specified for
/// `EndTimeExclusive` must be after the time for `StartTimeInclusive`" — so the
/// sentence names the two members and nothing else. It carries no request value:
/// both instants are `@sensitive`.
pub const INVERTED_HISTORY_WINDOW: &str = "'EndTimeExclusive' must be after 'StartTimeInclusive'.";

/// Largest page any list operation in this service returns, and the page size used
/// when the caller sends no `MaxResults`: the model bounds `MaxResults` at
/// `range 1..100`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The page size to use for a request's `MaxResults`.
///
/// The model already bounds `MaxResults` at `range 1..100` and the request
/// boundary refuses a violation before any handler runs, so the clamp here is
/// defence in depth: it keeps this helper total for every `i32` a future caller
/// could hand it, including a member some other list operation models without a
/// range.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

/// Longitude bound of the WGS 84 coordinate domain, in degrees.
pub const MAX_LONGITUDE: f64 = 180.0;

/// Latitude bound of the WGS 84 coordinate domain, in degrees.
pub const MAX_LATITUDE: f64 = 90.0;

/// The `[longitude, latitude]` pair a `Position` carries, or the reason it is not a
/// WGS 84 coordinate.
///
/// The `Position` shape documents "WGS 84 format: `[X or longitude, Y or latitude]`"
/// and is `len 2..2` (refused at the request boundary), but puts no `range` on its
/// members, so the coordinate domain is the service's own check and belongs here.
pub fn wgs84_position(position: &[f64]) -> Result<[f64; 2], String> {
    let [longitude, latitude] = match position {
        [longitude, latitude] => [*longitude, *latitude],
        other => {
            return Err(format!(
                "Position must hold exactly 2 values, [longitude, latitude]; got {}",
                other.len()
            ))
        }
    };
    if !(-MAX_LONGITUDE..=MAX_LONGITUDE).contains(&longitude) {
        return Err(outside_wgs84_domain(LONGITUDE_AXIS, MAX_LONGITUDE));
    }
    if !(-MAX_LATITUDE..=MAX_LATITUDE).contains(&latitude) {
        return Err(outside_wgs84_domain(LATITUDE_AXIS, MAX_LATITUDE));
    }
    Ok([longitude, latitude])
}

/// The axis name the coordinate-domain refusal states for a `Position`'s first
/// value: "WGS 84 format: `[X or longitude, Y or latitude]`".
const LONGITUDE_AXIS: &str = "longitude";

/// The axis name the same refusal states for a `Position`'s second value.
const LATITUDE_AXIS: &str = "latitude";

/// The sentence a coordinate outside its WGS 84 bound is refused with.
///
/// It names the axis and the two bounds of that axis's domain, and nothing else:
/// no model member name, and never the submitted coordinate — `Position` is
/// `@sensitive`, so a value read from the request may not appear in an error
/// string. The only variable parts are the bounds, which are the domain constants
/// (`±180` for longitude, `±90` for latitude), so the text a caller matches on is
/// the same for every rejected coordinate of an axis.
fn outside_wgs84_domain(axis: &str, bound: f64) -> String {
    format!("{axis} must be between {} and {bound} degrees", -bound)
}

/// The `BatchItemError$Code` a per-entry input failure carries: the
/// `BatchItemErrorCode` enum's "Input fails to satisfy the constraints specified by
/// the service" member.
pub const BATCH_VALIDATION_ERROR: &str = "ValidationError";

/// The `ResourceNotFoundException` sentence a position read states when it resolved
/// its tracker but the device has no stored position under it.
///
/// A device is not a named resource of this API — it exists only as the key of the
/// samples reported for it — so this is NOT the [`resource_not_found`] sentence a
/// tracker, a geofence collection, a geofence or a route calculator the service
/// cannot resolve by name is refused with. It is fixed text: the device id it
/// concerns is already the request's own `DeviceId`.
pub const NO_POSITION_RECORDS: &str = "records not found for given deviceId";

/// How this service names a tracker resource in a refusal about it.
pub const TRACKER_RESOURCE: &str = "Tracker";

/// How this service names a geofence collection resource in a refusal about it.
pub const GEOFENCE_COLLECTION_RESOURCE: &str = "Geofence collection";

/// How this service names a geofence resource in a refusal about it.
pub const GEOFENCE_RESOURCE: &str = "Geofence";

/// How this service names a route calculator resource in a refusal about it.
pub const ROUTE_CALCULATOR_RESOURCE: &str = "Route calculator";

/// How this service names a map resource in a refusal about it.
pub const MAP_RESOURCE: &str = "Map";

/// The `ResourceNotFoundException` sentence stated for a name no resource of that
/// kind is stored under: the resource's own name as this service writes it, then the
/// name the request named it by, then a period.
///
/// `resource` is one of the `*_RESOURCE` constants above; `name` is taken from the
/// request (a URI label or a body member), so the only variable part of the sentence
/// is the name the caller asked about.
pub fn resource_not_found(resource: &str, name: &str) -> String {
    format!("{resource} not found: {name}.")
}

/// The `ConflictException` sentence stated for a create whose name a resource of
/// that kind already holds, built from the same two parts as
/// [`resource_not_found`].
pub fn resource_already_exists(resource: &str, name: &str) -> String {
    format!("{resource} already exists: {name}.")
}

/// The `Tags` member a resource read projects for a resource holding `tags`.
///
/// Always `Some`: a read projects the resource's tag map unconditionally, so an
/// untagged resource reports an EMPTY map rather than omitting the member, and a
/// client never has to tell "no tags" from "not returned".
pub fn projected_tags(
    tags: &BTreeMap<String, String>,
) -> Option<::std::collections::HashMap<String, String>> {
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The `ValidationException` this service states for a member the request got wrong:
/// the sentence as `message`, the reason the model's enum has for a bad field value,
/// and a `fieldList` entry naming the member and repeating the sentence.
///
/// Every operation lifts this into its OWN error enum's `ValidationException` variant,
/// so the SHAPE of such a refusal is written once here rather than per operation.
pub fn field_validation_exception(
    member: &str,
    why: String,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: why.clone(),
        reason: FIELD_VALIDATION_FAILED.to_string(),
        field_list: vec![crate::ops::types::ValidationExceptionField {
            name: member.to_string(),
            message: why,
        }],
    }
}

/// The `ValidationExceptionReason` a bad member value carries: the model's enum member
/// "FieldValidationFailed".
pub const FIELD_VALIDATION_FAILED: &str = "FieldValidationFailed";

