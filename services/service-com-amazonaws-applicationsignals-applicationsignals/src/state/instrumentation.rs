// SPDX-License-Identifier: CC0-1.0

//! State and wording for instrumentation configurations: their identity, storage,
//! status history and refusals.

use super::*;

/// The resource-type segment of an instrumentation configuration ARN. The modelled
/// `InstrumentationConfigurationArn` doc spells the whole format:
/// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
pub const INSTRUMENTATION_CONFIG_ARN_RESOURCE: &str = "instrumentationConfig";

/// The `InstrumentationType` whose configurations are temporary
/// (`smithy_query(shape="InstrumentationType")`: "Temporary instrumentation that expires
/// automatically (default 24 hours)").
pub const INSTRUMENTATION_TYPE_BREAKPOINT: &str = "BREAKPOINT";

/// How many digest bytes a `LocationHash` renders: the member is modelled `length 16..16`
/// and the ARN pattern spells that suffix `[0-9a-f]{16}`, so 16 hex characters \u2014 8 bytes.
pub(super) const LOCATION_HASH_BYTES: usize = 8;

/// How long a BREAKPOINT configuration lives when the request names no `ExpiresAt`.
/// `CreateInstrumentationConfiguration`'s doc: "For BREAKPOINT type configurations, they
/// expire after 24 hours unless a shorter expiration is provided", and the `ExpiresAt`
/// member's: "For BREAKPOINT: optional, defaults to 24 hours".
pub const BREAKPOINT_DEFAULT_EXPIRY: Duration = Duration::from_secs(24 * 60 * 60);

/// The shortest BREAKPOINT expiry a request may name. `ExpiresAt`'s doc: "must be between
/// 5 min and 24 hours".
pub const BREAKPOINT_MIN_EXPIRY: Duration = Duration::from_secs(5 * 60);

/// The longest BREAKPOINT expiry a request may name, from the same sentence.
pub const BREAKPOINT_MAX_EXPIRY: Duration = BREAKPOINT_DEFAULT_EXPIRY;

/// `CaptureLimitsConfig.MaxHits` when omitted. Model doc
/// (`smithy_query(query="shape",shape="CaptureLimitsConfig")`): "The maximum number of times
/// the instrumentation point can be hit before it is automatically disabled. Defaults to 100."
pub(super) const DEFAULT_MAX_HITS: i32 = 100;

/// `CaptureLimitsConfig.MaxStringLength` when omitted. Model doc: "The maximum length of
/// captured string values in characters. Strings longer than this are truncated. Defaults to
/// 128."
pub(super) const DEFAULT_MAX_STRING_LENGTH: i32 = 128;

/// `CaptureLimitsConfig.MaxCollectionWidth` when omitted. Model doc: "The maximum number of
/// items to capture from any collection to prevent large payloads. Defaults to 10."
pub(super) const DEFAULT_MAX_COLLECTION_WIDTH: i32 = 10;

/// `CaptureLimitsConfig.MaxCollectionDepth` when omitted. Model doc: "The maximum nesting
/// depth to traverse inside collections. Defaults to 3."
pub(super) const DEFAULT_MAX_COLLECTION_DEPTH: i32 = 3;

/// `CaptureLimitsConfig.MaxStackFrames` when omitted. Model doc: "The maximum number of stack
/// frames to capture in stack traces. Defaults to 2."
pub(super) const DEFAULT_MAX_STACK_FRAMES: i32 = 2;

/// `CaptureLimitsConfig.MaxStackTraceSize` when omitted. Model doc: "The maximum total size,
/// in bytes, of a captured stack trace. Defaults to 1000."
pub(super) const DEFAULT_MAX_STACK_TRACE_SIZE: i32 = 1000;

/// `CaptureLimitsConfig.MaxObjectDepth` when omitted. Model doc: "The maximum depth for nested
/// object traversal when capturing structured data. Defaults to 3."
pub(super) const DEFAULT_MAX_OBJECT_DEPTH: i32 = 3;

/// `CaptureLimitsConfig.MaxFieldsPerObject` when omitted. Model doc: "The maximum number of
/// fields to capture for any object. Defaults to 10."
pub(super) const DEFAULT_MAX_FIELDS_PER_OBJECT: i32 = 10;

/// `CodeCaptureConfiguration.CaptureReturn` when omitted. Model doc
/// (`smithy_query(query="shape",shape="CodeCaptureConfiguration")`): "Whether to capture the
/// return value. Defaults to false."
pub(super) const DEFAULT_CAPTURE_RETURN: bool = false;

/// `CodeCaptureConfiguration.CaptureStackTrace` when omitted. Model doc: "Whether to capture a
/// stack trace when the instrumentation point is hit. Defaults to true."
pub(super) const DEFAULT_CAPTURE_STACK_TRACE: bool = true;

/// The `SyncInterval` a list reports. `InstrumentationConfigurationsPage.SyncInterval`'s
/// doc: "The suggested number of seconds to wait before the next sync request. This is at
/// least 60 seconds to prevent excessive polling", which is also the member's modelled
/// `range min 60`. This emulator holds one process's state and has no load to spread, so it
/// suggests exactly that floor.
pub const SYNC_INTERVAL_SECONDS: i32 = 60;

/// The page `ListInstrumentationConfigurations` returns when the caller names no
/// `MaxResults`: the member's modelled `@default` of 50 ("The default is 50 and the maximum
/// is 100").
pub const DEFAULT_INSTRUMENTATION_CONFIGURATION_PAGE: i32 = 50;

/// The [`NEXT_TOKEN_TAG`] of `ListInstrumentationConfigurations`, which every token it
/// issues carries together with a fingerprint of the scope that issued it (see
/// [`instrumentation_token_tag`]).
pub const INSTRUMENTATION_CONFIGURATIONS_TOKEN_TAG: &str = "instrumentation-configurations/1/";

/// The most instrumentation configurations this service holds, after which a create is
/// refused with the modelled `ServiceQuotaExceededException`.
///
/// AWS publishes no figure for this quota, so the bound is this emulator's own; what is not
/// chosen is that there IS one. `CreateInstrumentationConfiguration` is the only writer, and
/// only an explicit delete or a BREAKPOINT expiry removes a record, so without a cap the
/// store's size would be a property of the caller rather than of the service \u2014 and the
/// operation models `ServiceQuotaExceededException` for precisely that refusal.
pub const MAX_INSTRUMENTATION_CONFIGURATIONS: usize = 1_000;

/// The page `GetInstrumentationConfigurationStatus` returns when the caller names no
/// `MaxResults`: the member's modelled `@default` of 60 ("The maximum number of status events
/// to return in one call. The default is 60").
pub const DEFAULT_STATUS_EVENT_PAGE: i32 = 60;

/// The window a status read covers when the request names neither `StartTime` nor `EndTime`.
/// `GetInstrumentationConfigurationStatus`'s doc: "If no status or time window is provided,
/// the operation defaults to ACTIVE events from the last hour", and the members' own: "When
/// both are omitted, the time range defaults to the last hour".
pub const STATUS_EVENT_DEFAULT_WINDOW: Duration = Duration::from_secs(60 * 60);

/// The `InstrumentationConfigurationStatus` a status read reports when the request names
/// none. `GetInstrumentationConfigurationStatus.Status`'s doc: "If omitted, only ACTIVE
/// status events are returned", and the response member's: "If not specified in the request,
/// this is ACTIVE" (`smithy_query(query="shape",shape="InstrumentationConfigurationStatus")`
/// -> `["READY", "ERROR", "ACTIVE", "DISABLED"]`).
pub const DEFAULT_STATUS_EVENT_STATUS: &str = "ACTIVE";

/// The WIRE token one `InstrumentationConfigurationStatus` is stated with in a response.
///
/// The modelled enum carries the value set `["READY", "ERROR", "ACTIVE", "DISABLED"]` and no
/// entry of it declares a `value` distinct from its name
/// (`smithy_query(query="shape",shape="InstrumentationConfigurationStatus")`), while this
/// service states a status on the wire CAPITALISED: the status whose modelled name is
/// `ACTIVE` is served as `Active`. The token is therefore the name with its first letter
/// upper-case and the rest lower-case — every value of the enum is one word, so that is the
/// whole rule — and a status is compared, stored and filtered by under its modelled name, so
/// only what a response STATES is rendered here.
pub fn status_wire_token(status: &str) -> String {
    let mut characters = status.chars();
    match characters.next() {
        None => String::new(),
        Some(first) => {
            first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
        }
    }
}

/// The `UnprocessedStatusEventFailureReason` a reported status event earns when it names an
/// identity this service holds no configuration for
/// (`smithy_query(query="shape",shape="UnprocessedStatusEventFailureReason")` ->
/// `["THROTTLED", "INTERNAL_ERROR", "VALIDATION_ERROR"]`): the report is about a
/// configuration that does not exist, which is a property of the request.
pub const STATUS_EVENT_FAILED_VALIDATION: &str = "VALIDATION_ERROR";

/// The [`NEXT_TOKEN_TAG`] of `GetInstrumentationConfigurationStatus`, which every token it
/// issues carries together with a fingerprint of the query that issued it (see
/// [`instrumentation_status_token_tag`]).
pub const INSTRUMENTATION_STATUS_EVENTS_TOKEN_TAG: &str = "instrumentation-status-events/1/";

/// The most status events this service keeps for one instrumentation configuration, past
/// which the OLDEST event it holds is dropped.
///
/// AWS publishes no retention for reported status events, so the bound is this emulator's
/// own; what is not chosen is that there IS one.
/// `ReportInstrumentationConfigurationStatus` is the only writer and the caller chooses how
/// often it writes ("Report ACTIVE periodically (for example, every minute) while
/// instrumentation is running"), so without a cap the store's size would be a property of
/// the caller rather than of the service. Dropping the oldest event keeps the recent history
/// a read asks for: the cap holds more than ten pages at the modelled `MaxResults` maximum of
/// 100, and far more than the hour a read covers by default at the cadence the operation
/// suggests.
pub const MAX_STATUS_EVENTS_PER_CONFIGURATION: usize = 1_000;

/// How long a scope whose set has EMPTIED keeps its last-change instant before the record
/// itself is dropped.
///
/// A list tells every caller to come back after [`SYNC_INTERVAL_SECONDS`], but that member
/// is a FLOOR ("at least 60 seconds"), not a cadence: a conforming caller may poll a good
/// deal later. The horizon is therefore a generous multiple of the advertised interval, so a
/// caller polling slower than the floor still learns that its configuration lapsed, while
/// the record does not hold quota indefinitely after it stopped being news. Once the horizon
/// passes, a later sync is answered with `Changed: false` over an empty set, which is what a
/// scope this service holds nothing for means. Keeping the record for ever instead would let
/// a caller grow the store with scope keys of its own choosing \u2014 the count is also capped
/// (see
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::instrumentation_entry_count`]),
/// and this horizon is what keeps a retained record from holding that quota for ever.
pub const EMPTY_SCOPE_RETENTION: Duration =
    Duration::from_secs(EMPTY_SCOPE_RETENTION_INTERVALS * SYNC_INTERVAL_SECONDS as u64);

/// How many advertised sync intervals [`EMPTY_SCOPE_RETENTION`] spans.
pub(super) const EMPTY_SCOPE_RETENTION_INTERVALS: u64 = 10;

/// The most REMOVALS of instrumentation configurations this store remembers, past which the
/// oldest it holds is forgotten.
///
/// A remembered removal is what lets a read report a configuration as deleted rather than as
/// one that never existed, so it outlives the record it is about and needs a bound of its
/// own. The bound is the one that already caps the live records
/// ([`MAX_INSTRUMENTATION_CONFIGURATIONS`]): the store remembers at most as many removals as
/// configurations it would hold, so the memory cannot cost more than the records it is about,
/// and the caller that fills it is the one that created and removed that many.
pub(super) const MAX_REMEMBERED_INSTRUMENTATION_REMOVALS: usize = MAX_INSTRUMENTATION_CONFIGURATIONS;

/// The set of instrumentation configurations one list call reports over: the service, the
/// environment it runs in and the instrumentation type, which is every selector the
/// instrumentation operations take beside the location.
///
/// `InstrumentationType` is part of the scope rather than a filter over one store because
/// the model says so (`ListInstrumentationConfigurations.InstrumentationType`: "Required to
/// determine which backing store to query").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InstrumentationScope {
    pub instrumentation_type: String,
    pub service: String,
    pub environment: String,
}

impl InstrumentationScope {
    /// The scope a request's `InstrumentationType`, `Service` and `Environment` name.
    pub fn new(instrumentation_type: &str, service: &str, environment: &str) -> Self {
        Self {
            instrumentation_type: instrumentation_type.to_string(),
            service: service.to_string(),
            environment: environment.to_string(),
        }
    }
}

/// One [`InstrumentationScope`]'s configurations, plus when the set last changed.
#[derive(Debug, Default)]
pub struct InstrumentationConfigurationSet {
    /// The scope's configurations, keyed by [`instrumentation_key`].
    pub configurations: BTreeMap<String, InstrumentationConfiguration>,
    /// The instant this set last gained, lost or expired a configuration, or `None` while
    /// it has never held one.
    ///
    /// This is what `ListInstrumentationConfigurations` answers `Changed` from: the member
    /// is documented as "whether there are configuration changes since the provided
    /// SyncedAt timestamp", so the service has to remember when the set last changed rather
    /// than deduce it from the records it still holds \u2014 a deletion leaves no record to read
    /// a timestamp off.
    pub last_changed_at: Option<DateTime>,
    /// The status events reported for each configuration of this scope, keyed by the same
    /// [`instrumentation_key`] `configurations` is keyed by — so a configuration and the
    /// history reported against it are one record's two halves, and the history goes when
    /// the configuration goes.
    pub status_history: BTreeMap<String, InstrumentationStatusHistory>,
}

impl InstrumentationConfigurationSet {
    /// Record that this set changed at `at`, keeping the LATEST change instant: a change is
    /// news to every caller that synced before it, and an older change must not hide a
    /// newer one.
    pub(super) fn record_change(&mut self, at: DateTime) {
        let keep = self
            .last_changed_at
            .is_none_or(|previous| instant_nanos(&previous) < instant_nanos(&at));
        if keep {
            self.last_changed_at = Some(at);
        }
    }
}

/// A status event's key within one configuration's history: the MINUTE it was reported for
/// ([`status_event_minute`]), then the status reported in that minute.
///
/// Time first, so the ordered map is a chronology a read walks backwards ("sorted with the
/// most recent first") and the oldest event is the one the cap drops. The status is part of
/// the key because one minute can carry one event per status, and because a repeated report
/// of the SAME status for the SAME minute is the same event: the operation is `@idempotent`,
/// so re-reporting one must not add a second event.
pub(super) type StatusEventKey = (i64, u32, String);

/// The status events reported against ONE instrumentation configuration.
///
/// Bounded at [`MAX_STATUS_EVENTS_PER_CONFIGURATION`] events by [`Self::record`], the only
/// writer, so a caller reporting for ever does not grow the store for ever.
#[derive(Debug, Default)]
pub struct InstrumentationStatusHistory {
    /// Each event's optional `ErrorCause`, keyed by [`StatusEventKey`].
    pub(super) events: BTreeMap<StatusEventKey, Option<String>>,
}

impl InstrumentationStatusHistory {
    /// Record that this configuration was in `status` at `at`, with the `error_cause` the
    /// report carried.
    ///
    /// The event is recorded for the MINUTE the report names ([`status_event_minute`]), so
    /// recording the same status for the same minute again REPLACES the event rather than
    /// adding one: that is what makes a repeated report of one event a no-op, and what makes
    /// two reports of one status inside a minute one event. Once the history is at its cap the
    /// oldest event it holds is dropped.
    pub(super) fn record(&mut self, status: &str, at: &DateTime, error_cause: Option<&str>) {
        self.events.insert(
            status_event_key(status, at),
            error_cause.map(ToString::to_string),
        );
        while self.events.len() > MAX_STATUS_EVENTS_PER_CONFIGURATION {
            self.events.pop_first();
        }
    }

    /// How many events this history holds, over every status.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether this history holds no event at all.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// One page of the events [`StatusEventQuery`] asks for, most recent first, together with
    /// the instant of the last event it reported when a further page remains (`None` once the
    /// page is the last one).
    ///
    /// An event is held — and so reported — at the minute it was recorded for, which is the
    /// instant the window selects it by: the bounds the request names are its own exact
    /// instants, so a window answers for the events whose recorded minute falls inside it.
    ///
    /// The window is inclusive at both ends: an event is reported for an instant, and an
    /// instant the caller names as a bound is part of the range it asks about. At most
    /// `page_size + 1` matching events are collected, and the walk itself is over a history
    /// bounded by [`MAX_STATUS_EVENTS_PER_CONFIGURATION`].
    pub(super) fn page(
        &self,
        query: &StatusEventQuery,
    ) -> (Vec<InstrumentationStatusEvent>, Option<StatusEventInstant>) {
        // A page of nothing could not advance its own cursor, so a caller following
        // `NextToken` would loop for ever. `MaxResults` is modelled `range 1..100`, so this
        // only ever clamps a bound no request can state.
        let page_size = query.page_size.max(1);
        let lower = status_event_instant(&query.start);
        let upper = status_event_instant(&query.end);
        let mut window: Vec<(&StatusEventKey, &Option<String>)> = self
            .events
            .iter()
            .rev()
            .filter(|(key, _)| {
                let at = (key.0, key.1);
                key.2 == query.status
                    && at >= lower
                    && at <= upper
                    && query.older_than.is_none_or(|bound| at < bound)
            })
            .take(page_size.saturating_add(1))
            .collect();

        let next = if window.len() > page_size {
            window.truncate(page_size);
            window.last().map(|(key, _)| (key.0, key.1))
        } else {
            None
        };

        let events = window
            .into_iter()
            .map(|(key, error_cause)| InstrumentationStatusEvent {
                time: DateTime::from_secs_and_nanos(key.0, key.1),
                error_cause: error_cause.clone(),
            })
            .collect();
        (events, next)
    }
}

/// What one status read asks of a configuration's history: the status whose chronology it
/// walks, the window it covers, where the previous page of the sequence stopped and how many
/// events a page holds.
#[derive(Debug, Clone)]
pub struct StatusEventQuery {
    pub status: String,
    pub start: DateTime,
    pub end: DateTime,
    pub older_than: Option<StatusEventInstant>,
    pub page_size: usize,
}

/// The [`StatusEventKey`] of `status` reported for `at`, which is the key of the MINUTE `at`
/// falls on ([`status_event_minute`]).
pub(super) fn status_event_key(status: &str, at: &DateTime) -> StatusEventKey {
    let (secs, subsec_nanos) = status_event_minute(at);
    (secs, subsec_nanos, status.to_string())
}

/// How many seconds make a minute, the granularity a reported status event is recorded at.
pub(super) const SECONDS_PER_MINUTE: i64 = 60;

/// The minute `at` belongs to: the whole minute NEAREST it, carrying no subsecond part.
///
/// `InstrumentationStatusEvent.Time` is "The time when the status was reported, rounded to the
/// nearest minute", so the instant an event is HELD at is a minute boundary rather than the
/// instant the report carried — which is also why two reports of one status inside a minute
/// are one event (see [`InstrumentationStatusHistory::record`]).
///
/// An instant exactly half a minute from both neighbouring boundaries is a tie, and goes to
/// the LATER one; the subsecond part can only move an instant further from the earlier
/// boundary, so it never decides which boundary is nearest except at that tie.
pub(super) fn status_event_minute(at: &DateTime) -> StatusEventInstant {
    let (secs, _) = status_event_instant(at);
    let shifted = secs.saturating_add(SECONDS_PER_MINUTE / 2);
    (
        shifted
            .div_euclid(SECONDS_PER_MINUTE)
            .saturating_mul(SECONDS_PER_MINUTE),
        0,
    )
}

/// One instant of a status chronology, to the precision a `Timestamp` carries: whole seconds
/// and the nanoseconds within them.
pub type StatusEventInstant = (i64, u32);

/// How many nanoseconds make a second, which is the exclusive bound on the subsecond part of
/// an instant: a value at or above it belongs to the next second and is no subsecond part at
/// all.
pub(super) const NANOS_PER_SECOND: u32 = 1_000_000_000;

/// The [`StatusEventInstant`] of `at`.
pub(super) fn status_event_instant(at: &DateTime) -> StatusEventInstant {
    (at.secs(), at.subsec_nanos())
}

/// The boundary a status-event `NextToken` carries: the WINDOW the page sequence is being
/// taken over, and the instant of the last event the previous page reported, which the next
/// call resolves as an exclusive bound on a chronology read backwards.
///
/// The window travels in the token because the default window is RELATIVE ("the last hour"):
/// recomputing it on the next call would move both its bounds, so a page sequence that named
/// no window would drift across its own pages. Carrying it keeps every page of one sequence
/// over one window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusEventCursor {
    pub start: StatusEventInstant,
    pub end: StatusEventInstant,
    pub older_than: StatusEventInstant,
}

impl StatusEventCursor {
    /// The cursor for the page after one taken over `[start, end]` whose last event was at
    /// `older_than`.
    pub fn new(start: &DateTime, end: &DateTime, older_than: StatusEventInstant) -> Self {
        Self {
            start: status_event_instant(start),
            end: status_event_instant(end),
            older_than,
        }
    }

    /// This cursor as the opaque payload of a `NextToken`.
    pub fn encode(&self) -> String {
        let parts = [self.start, self.end, self.older_than]
            .map(|(secs, nanos)| format!("{secs}.{nanos}"));
        parts.join("/")
    }

    /// The cursor a `NextToken` payload carries, or `None` when it is not one [`Self::encode`]
    /// produced.
    ///
    /// A `NextToken` is opaque but not authenticated, so every field it carries is a value a
    /// caller may have written: an instant is accepted only when its subsecond part is a real
    /// one (strictly under a second, [`NANOS_PER_SECOND`]), which is the range
    /// [`Self::window`]'s timestamps are defined over, and only when each of its two numbers is
    /// spelled the way [`Self::encode`] spells it ([`canonical_number`]). A payload carrying
    /// anything else is not a cursor this service issued, and is refused exactly like a token
    /// that is not hex.
    pub fn decode(payload: &str) -> Option<Self> {
        let mut parts = payload.split('/');
        let mut instant = || -> Option<StatusEventInstant> {
            let (secs, nanos) = parts.next()?.split_once('.')?;
            let secs = canonical_number::<i64>(secs)?;
            let nanos = canonical_number::<u32>(nanos).filter(|nanos| *nanos < NANOS_PER_SECOND)?;
            Some((secs, nanos))
        };
        let start = instant()?;
        let end = instant()?;
        let older_than = instant()?;
        Some(Self {
            start,
            end,
            older_than,
        })
    }

    /// The window this cursor's page sequence is taken over.
    pub fn window(&self) -> (DateTime, DateTime) {
        (
            DateTime::from_secs_and_nanos(self.start.0, self.start.1),
            DateTime::from_secs_and_nanos(self.end.0, self.end.1),
        )
    }

    /// Whether this cursor's window is the one `[start, end]` names.
    pub fn covers(&self, start: &DateTime, end: &DateTime) -> bool {
        self.start == status_event_instant(start) && self.end == status_event_instant(end)
    }
}

/// The `NextToken` tag one status read issues its page tokens under: the operation's
/// [`INSTRUMENTATION_STATUS_EVENTS_TOKEN_TAG`] plus a fingerprint of the configuration and
/// the status whose chronology the page was taken over.
///
/// A cursor is a boundary within ONE chronology, so a token issued for another configuration
/// or another status would silently skip or repeat events if it were honoured. Binding the
/// chronology's identity into the tag makes such a token fail to decode, and it is refused
/// like any other token this service did not issue. The window is not part of the identity:
/// it travels inside the cursor (see [`StatusEventCursor`]).
pub fn instrumentation_status_token_tag(
    scope: &InstrumentationScope,
    key: &str,
    status: &str,
) -> String {
    let fingerprint = stable_hash(&[
        Some(scope.instrumentation_type.as_str()),
        Some(scope.service.as_str()),
        Some(scope.environment.as_str()),
        Some(key),
        Some(status),
    ]);
    format!("{INSTRUMENTATION_STATUS_EVENTS_TOKEN_TAG}{fingerprint}/")
}

/// The member path a refusal of the status window's start names, spelled the way the boundary
/// validator spells a member path for this service — lower camel case, as the refusal of a
/// modelled constraint on a member of the same operation spells it.
pub(super) const START_TIME_MEMBER_PATH: &str = "startTime";

/// [`START_TIME_MEMBER_PATH`] for the member that closes the window.
pub(super) const END_TIME_MEMBER_PATH: &str = "endTime";

/// The refusal a request earns for naming ONE member of a pair whose documented rule is both
/// or neither: the clause names the member that is MISSING — the one the request must add —
/// and the sibling whose presence is what requires it.
///
/// Worded through the service's one constraint-violation builder, in the form an absent member
/// is stated in ([`absent_member_clause`]) plus the condition the model cannot express, so a
/// conditionally required member reads like an unconditionally required one.
pub(super) fn paired_member_refusal(absent_member_path: &str, present_member_path: &str) -> String {
    validation_envelope(&[constraint_clause(
        absent_member_path,
        &format!("Member must not be null when {present_member_path} is provided"),
    )])
}

/// The window a status read covers: the one the request names, or the documented default.
///
/// `StartTime` and `EndTime` "must both be provided together or both be omitted", and "when
/// both are omitted, the time range defaults to the last hour" — so a request naming exactly
/// one of them names no window, and neither does one whose window ends before it begins.
/// `Err` carries the refusal such a request earns.
///
/// A window the REQUEST names is the pair of instants it named, compared exactly as given. The
/// default window is this read's OWN, derived from its clock, so it is derived in the domain
/// the events it selects are held at: an event is held at the whole minute nearest the instant
/// its report named ([`status_event_minute`]), so the default window runs from the minute
/// nearest an hour ago to the minute nearest now. That is what makes "the last hour" cover
/// every event whose report named an instant inside it — the held minute is monotone in the
/// reported instant, so a report in the hour is held no later than the minute nearest now —
/// including a report of the minute this call falls in, whose minute can be the boundary
/// AHEAD of the call. Deriving the end as the bare instant instead would leave an event just
/// reported outside "the last hour" until the clock reached its minute.
///
/// Both refusals are worded through the service's one constraint-violation builder
/// ([`constraint_clause`], under [`validation_envelope`]), so a constraint the model cannot
/// express and is therefore enforced here names its member path and its requirement exactly
/// as a modelled constraint refused at the boundary does.
pub fn status_event_window(
    start: Option<&DateTime>,
    end: Option<&DateTime>,
    now: &DateTime,
) -> Result<(DateTime, DateTime), String> {
    // Compared and derived on the exact instants a `Timestamp` carries — seconds and the
    // nanoseconds within them, the same pair the history is keyed by — so one precision model
    // governs the window and the events it selects.
    match (start, end) {
        (None, None) => {
            let (end_secs, end_nanos) = status_event_minute(now);
            Ok((
                DateTime::from_secs_and_nanos(
                    end_secs.saturating_sub(STATUS_EVENT_DEFAULT_WINDOW.as_secs() as i64),
                    end_nanos,
                ),
                DateTime::from_secs_and_nanos(end_secs, end_nanos),
            ))
        }
        (Some(start), Some(end))
            if status_event_instant(end) >= status_event_instant(start) =>
        {
            Ok((*start, *end))
        }
        // A window runs forwards, so the member at fault is the one that closes it, and the
        // bound it violated is the member that opens it.
        (Some(_), Some(_)) => Err(validation_envelope(&[constraint_clause(
            END_TIME_MEMBER_PATH,
            &format!(
                "Member must have value greater than or equal to the value of \
                 {START_TIME_MEMBER_PATH}"
            ),
        )])),
        (Some(_), None) => Err(paired_member_refusal(
            END_TIME_MEMBER_PATH,
            START_TIME_MEMBER_PATH,
        )),
        (None, Some(_)) => Err(paired_member_refusal(
            START_TIME_MEMBER_PATH,
            END_TIME_MEMBER_PATH,
        )),
    }
}

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// How many entries the instrumentation store holds against its quota: one per stored
    /// configuration, and at least one for a retained scope whose set has emptied.
    ///
    /// A retained scope costs quota exactly like a configuration, so
    /// [`MAX_INSTRUMENTATION_CONFIGURATIONS`] bounds the number of SCOPE KEYS the store
    /// holds as well as the number of records inside them: a caller cannot grow the store
    /// past the cap by creating configurations in fresh scopes and letting them lapse. Both
    /// halves of a scope key are modelled `length 1..255`, so a bounded count of keys is a
    /// bounded size too.
    pub fn instrumentation_entry_count(&self) -> usize {
        self.instrumentation_configurations
            .values()
            .map(|set| set.configurations.len().max(1))
            .sum()
    }

    /// Whether another instrumentation configuration would put this service over its
    /// [`MAX_INSTRUMENTATION_CONFIGURATIONS`] quota.
    pub fn at_instrumentation_configuration_quota(&self) -> bool {
        self.instrumentation_entry_count() >= MAX_INSTRUMENTATION_CONFIGURATIONS
    }

    /// The configuration stored in `scope` under `key` ([`instrumentation_key`]), or `None`
    /// when the scope holds no configuration at that location.
    pub fn instrumentation_configuration(
        &self,
        scope: &InstrumentationScope,
        key: &str,
    ) -> Option<&InstrumentationConfiguration> {
        self.instrumentation_configurations
            .get(scope)
            .and_then(|set| set.configurations.get(key))
    }

    /// The configuration stored at the identity `service`, `environment` and `key`
    /// ([`instrumentation_key`]) name — whatever its instrumentation type — or `None` when
    /// no configuration is stored at that location.
    ///
    /// The instrumentation type decides WHICH BACKING STORE a read addresses, but it is not
    /// part of a configuration's identity: the create's uniqueness rule names four parts
    /// ("If a configuration already exists for the same service, environment, signal type,
    /// and location, this operation returns a conflict instead of overwriting it"), and
    /// those same four parts are what an `InstrumentationConfigurationArn` spells
    /// ([`instrumentation_configuration_arn`]). So this is the lookup a create conflicts on,
    /// and keeping it in step with the ARN is what makes one ARN name one record — and
    /// therefore makes the tag set stored under that ARN one record's.
    ///
    /// The scan is over the scopes of one service and environment, bounded like every other
    /// walk of this store by [`MAX_INSTRUMENTATION_CONFIGURATIONS`].
    pub fn instrumentation_configuration_at_location(
        &self,
        service: &str,
        environment: &str,
        key: &str,
    ) -> Option<&InstrumentationConfiguration> {
        self.instrumentation_configurations
            .iter()
            .filter(|(scope, _)| scope.service == service && scope.environment == environment)
            .find_map(|(_, set)| set.configurations.get(key))
    }

    /// Store `configuration` in `scope`, recording `at` as the instant the scope's set
    /// changed.
    pub fn put_instrumentation_configuration(
        &mut self,
        scope: InstrumentationScope,
        configuration: InstrumentationConfiguration,
        at: DateTime,
    ) {
        let key = instrumentation_key(&configuration.signal_type, &configuration.location_hash);
        let expires_at = configuration.expires_at;
        // What this writes is a NEW resource, so the location is no longer one the store
        // remembers removing a configuration from: a later read of it resolves this record,
        // and a read after THIS record is removed reports that removal, not the earlier one.
        self.forget_instrumentation_removal(&scope, &key);
        let set = self.instrumentation_configurations.entry(scope).or_default();
        set.configurations.insert(key, configuration);
        set.record_change(at);
        if let Some(expires_at) = expires_at {
            self.schedule_instrumentation_pass(expires_at);
        }
    }

    /// Whether the store remembers a configuration being removed from `scope` at `key`
    /// ([`instrumentation_key`]) — by a delete or by a lapse — and holds none there now.
    pub fn instrumentation_configuration_was_removed(
        &self,
        scope: &InstrumentationScope,
        key: &str,
    ) -> bool {
        self.removed_instrumentation_configurations
            .get(scope)
            .is_some_and(|removals| removals.contains_key(key))
    }

    /// Remember that the configuration stored in `scope` at `key` was removed at `at`.
    ///
    /// Called by BOTH removal paths — the explicit delete and the expiry pass — since each
    /// leaves the location one the store once held a configuration for.
    pub(super) fn remember_instrumentation_removal(
        &mut self,
        scope: &InstrumentationScope,
        key: &str,
        at: DateTime,
    ) {
        self.removed_instrumentation_configurations
            .entry(scope.clone())
            .or_default()
            .insert(key.to_string(), at);
        self.forget_oldest_instrumentation_removals();
    }

    /// Forget that `scope` ever lost a configuration at `key`, dropping the scope's entry
    /// once it remembers no removal at all.
    pub(super) fn forget_instrumentation_removal(&mut self, scope: &InstrumentationScope, key: &str) {
        let Some(removals) = self.removed_instrumentation_configurations.get_mut(scope) else {
            return;
        };
        removals.remove(key);
        if removals.is_empty() {
            self.removed_instrumentation_configurations.remove(scope);
        }
    }

    /// Forget the OLDEST removals until the store remembers no more than
    /// [`MAX_REMEMBERED_INSTRUMENTATION_REMOVALS`] of them.
    ///
    /// The oldest goes first because the most recent removals are the ones a client is still
    /// reading back after; the walk is over a map that is at most one entry past its own cap.
    pub(super) fn forget_oldest_instrumentation_removals(&mut self) {
        while self
            .removed_instrumentation_configurations
            .values()
            .map(BTreeMap::len)
            .sum::<usize>()
            > MAX_REMEMBERED_INSTRUMENTATION_REMOVALS
        {
            let oldest = self
                .removed_instrumentation_configurations
                .iter()
                .flat_map(|(scope, removals)| {
                    removals.iter().map(move |(key, at)| (scope, key, at))
                })
                .min_by_key(|(_, _, at)| instant_nanos(at))
                .map(|(scope, key, _)| (scope.clone(), key.clone()));
            let Some((scope, key)) = oldest else {
                return;
            };
            self.forget_instrumentation_removal(&scope, &key);
        }
    }

    /// Note that [`Self::expire_instrumentation_configurations`] has something to do at `due`,
    /// keeping the EARLIEST such instant: the pass walks the store only once the first thing
    /// it owes has fallen due.
    pub(super) fn schedule_instrumentation_pass(&mut self, due: DateTime) {
        let sooner = self
            .instrumentation_due_at
            .is_none_or(|known| instant_nanos(&due) < instant_nanos(&known));
        if sooner {
            self.instrumentation_due_at = Some(due);
        }
    }

    /// The next instant [`Self::expire_instrumentation_configurations`] has anything to do:
    /// the soonest `ExpiresAt` a stored configuration carries, or the soonest instant an
    /// emptied scope's record may be dropped, whichever comes first.
    pub(super) fn next_instrumentation_pass(&self) -> Option<DateTime> {
        self.instrumentation_configurations
            .values()
            .flat_map(|set| {
                let expiries = set
                    .configurations
                    .values()
                    .filter_map(|stored| stored.expires_at);
                let droppable = if set.configurations.is_empty() {
                    set.last_changed_at
                        .map(|changed| instant_plus(&changed, EMPTY_SCOPE_RETENTION))
                } else {
                    None
                };
                expiries.chain(droppable)
            })
            .min_by_key(instant_nanos)
    }

    /// Drop every configuration whose `ExpiresAt` has fallen due by `now`, record the instant
    /// each lapse became due as a change to its scope's set, and drop a scope whose set has
    /// been empty for longer than [`EMPTY_SCOPE_RETENTION`].
    ///
    /// This is the one write a reporting operation may make: a BREAKPOINT configuration
    /// "expires after 24 hours unless a shorter expiration is provided", so its removal was
    /// owed by the CLOCK at `ExpiresAt` whether or not anybody called. The change is dated
    /// at the expiry instant rather than at `now` for the same reason: that is when the set
    /// changed, so a caller that synced after it is not told the set changed again. Dropping
    /// an emptied scope once its retention horizon has passed is owed by the clock in the
    /// same way.
    ///
    /// The store is walked only when the clock owes something ([`Self::next_instrumentation_pass`]),
    /// so the common call pays no scan at all — and the walk itself is over a store bounded
    /// by [`MAX_INSTRUMENTATION_CONFIGURATIONS`] entries, scopes included, so its cost cannot
    /// grow with the scopes a caller mints.
    ///
    /// Each lapsed record is taken away by [`Self::remove_instrumentation_configuration`],
    /// the one place that knows what a configuration record owns: a lapse and a delete remove
    /// the same things, so anything a record gains later is torn down for both at once.
    pub fn expire_instrumentation_configurations(&mut self, now: &DateTime) {
        let threshold = instant_nanos(now);
        if self
            .instrumentation_due_at
            .is_none_or(|due| instant_nanos(&due) > threshold)
        {
            return;
        }

        let lapsed: Vec<(InstrumentationScope, String, DateTime)> = self
            .instrumentation_configurations
            .iter()
            .flat_map(|(scope, set)| {
                set.configurations.iter().filter_map(move |(key, stored)| {
                    let expires_at = stored.expires_at?;
                    (instant_nanos(&expires_at) <= threshold)
                        .then(|| (scope.clone(), key.clone(), expires_at))
                })
            })
            .collect();

        for (scope, key, expires_at) in lapsed {
            // A lapse takes the configuration away exactly as a delete does — the record,
            // its status history, its tags and the location remembered as one a
            // configuration was removed from all go together — so it IS that removal, dated
            // at the instant the lapse fell due rather than at the instant of a call. The
            // only thing the removal does beyond this is schedule the horizon of a scope it
            // empties, which the whole-store re-derivation below computes again anyway.
            self.remove_instrumentation_configuration(&scope, &key, expires_at);
        }

        // An emptied scope keeps its last-change instant only while a caller could still be
        // syncing from before that change; past the horizon the record has nothing left to
        // tell anyone, and keeping it would let a caller grow the store with scope keys it
        // chooses.
        self.instrumentation_configurations.retain(|_, set| {
            !set.configurations.is_empty()
                || set.last_changed_at.is_none_or(|changed| {
                    instant_nanos(&changed).saturating_add(duration_nanos(EMPTY_SCOPE_RETENTION))
                        > threshold
                })
        });

        self.instrumentation_due_at = self.next_instrumentation_pass();
    }

    /// Record that the configuration stored in `scope` under `key` ([`instrumentation_key`])
    /// was in `status` at `at`, with the `error_cause` the report carried, and answer whether
    /// the event was recorded.
    ///
    /// `false` — and nothing recorded — when the scope holds no configuration at that
    /// location: a status event is reported AGAINST a configuration, so there is nothing for
    /// one naming no configuration to be recorded against. The event is recorded for the
    /// MINUTE `at` falls on ([`status_event_minute`]), so recording the same status for that
    /// minute again replaces the event rather than adding one — repeating a report changes
    /// nothing further, and two reports of one status inside a minute are one event.
    pub fn record_instrumentation_status(
        &mut self,
        scope: &InstrumentationScope,
        key: &str,
        status: &str,
        at: &DateTime,
        error_cause: Option<&str>,
    ) -> bool {
        let Some(set) = self.instrumentation_configurations.get_mut(scope) else {
            return false;
        };
        if !set.configurations.contains_key(key) {
            return false;
        }
        set.status_history
            .entry(key.to_string())
            .or_default()
            .record(status, at, error_cause);
        true
    }

    /// The status history of the configuration stored in `scope` under `key`
    /// ([`instrumentation_key`]), or `None` when the scope holds no history for that
    /// location — which is the state of a configuration nothing has reported for.
    pub fn instrumentation_status_history(
        &self,
        scope: &InstrumentationScope,
        key: &str,
    ) -> Option<&InstrumentationStatusHistory> {
        self.instrumentation_configurations
            .get(scope)
            .and_then(|set| set.status_history.get(key))
    }

    /// One page of the events [`StatusEventQuery`] asks for, of the configuration stored in
    /// `scope` under `key`, together with the cursor for the page after it.
    ///
    /// A configuration nothing has reported for has an empty history, which is not an error:
    /// the resource exists and its status events are the empty list.
    pub fn page_instrumentation_status_events(
        &self,
        scope: &InstrumentationScope,
        key: &str,
        query: &StatusEventQuery,
    ) -> (Vec<InstrumentationStatusEvent>, Option<StatusEventCursor>) {
        let (events, next) = match self.instrumentation_status_history(scope, key) {
            Some(history) => history.page(query),
            None => (Vec::new(), None),
        };
        (
            events,
            next.map(|older_than| {
                StatusEventCursor::new(&query.start, &query.end, older_than)
            }),
        )
    }

    /// Whether `scope`'s set has changed since `since`.
    ///
    /// Strictly after: a caller that synced at the instant of the last change already saw
    /// it, which is what makes an unchanged-since poll answer `Changed: false`. A scope that
    /// has never held a configuration has nothing new either.
    pub fn instrumentation_changed_since(
        &self,
        scope: &InstrumentationScope,
        since: &DateTime,
    ) -> bool {
        self.instrumentation_configurations
            .get(scope)
            .and_then(|set| set.last_changed_at)
            .is_some_and(|changed| instant_nanos(&changed) > instant_nanos(since))
    }

    /// One page of `scope`'s configurations, in key order, starting strictly after `cursor`,
    /// together with the token for the page after it (`None` once the page is the last one).
    ///
    /// At most `page_size + 1` records are visited, so the response is bounded by the page
    /// rather than by the number of stored configurations. The cursor is a stored KEY rather
    /// than an offset, so a configuration created or expired between two pages cannot make
    /// the next page skip or repeat an entry — the rule every read of this service whose page
    /// is stored records takes from [`page_keyed`]. The key a boundary carries is the one the
    /// set holds the record under, which is that configuration's own
    /// [`instrumentation_key`].
    ///
    /// The token is opaque but not authenticated, so the key inside the tag is a value a
    /// caller may have written: it is honoured only when it is a boundary this paging can have
    /// minted over the set being read ([`issued_key`]), and `Err` otherwise carries this
    /// read's wording for a token it did not issue for that scope
    /// ([`INSTRUMENTATION_CURSOR_SCOPE`]).
    pub fn page_instrumentation_configurations(
        &self,
        scope: &InstrumentationScope,
        cursor: Option<&str>,
        page_size: usize,
    ) -> Result<
        (
            Vec<InstrumentationConfigurationWithoutServiceEnv>,
            Option<String>,
        ),
        String,
    > {
        let stored = self.instrumentation_configurations.get(scope);
        // The key of the LAST configuration this scope holds, and so the one position past
        // which this paging can have minted no boundary over the set being read. A scope
        // holding no configurations has minted none at all.
        let last_key = stored
            .and_then(|set| set.configurations.keys().next_back())
            .map(String::as_str);
        let boundary = match cursor {
            None => None,
            Some(cursor) => Some(
                issued_key(cursor, last_key, well_formed_instrumentation_key)
                    .ok_or_else(|| unissued_next_token_in(INSTRUMENTATION_CURSOR_SCOPE))?,
            ),
        };
        let Some(set) = stored else {
            // A scope holding no configurations reports none, and the only cursor that could
            // have reached this line over an empty set was refused just above.
            return Ok((Vec::new(), None));
        };
        let lower = match boundary {
            Some(key) => Bound::Excluded(key),
            None => Bound::Unbounded,
        };
        let (window, next) = page_keyed::<String, str, _>(&set.configurations, lower, page_size);
        let next_token = next.map(|key| encode_cursor(&instrumentation_token_tag(scope), key));
        Ok((
            window.into_iter().map(without_service_env).collect(),
            next_token,
        ))
    }
}

/// A configuration's key within its [`InstrumentationScope`]: the two parts of the identity
/// a create conflicts on ("If a configuration already exists for the same service,
/// environment, signal type, and location, this operation returns a conflict") that the
/// scope does not carry. The other two are the scope's service and environment; the scope's
/// instrumentation type is not one of them (see
/// [`ComAmazonawsApplicationsignalsApplicationsignalsState::instrumentation_configuration_at_location`]).
///
/// `/` separates the two parts unambiguously: `signal_type` is an enum value
/// (`DynamicInstrumentationSignalType`) and a location hash is 16 hex characters, so neither
/// can contain one.
pub fn instrumentation_key(signal_type: &str, location_hash: &str) -> String {
    format!("{signal_type}/{location_hash}")
}

/// Whether `key` is spelled the way [`instrumentation_key`] spells a key of one scope's
/// ordered set: a modelled `DynamicInstrumentationSignalType` value, the one separator, and a
/// `LocationHash` — the 16 lowercase hex characters [`stable_hash`] writes, which is the
/// length and alphabet the member's own model fixes (`length 16..16`, and the
/// `InstrumentationConfigurationArn` pattern's hash segment).
///
/// Anything else is a value no record of this store is keyed by, and so no boundary the paging
/// over such a set can have minted ([`issued_key`]).
pub(super) fn well_formed_instrumentation_key(key: &str) -> bool {
    let Some((signal_type, location_hash)) = key.split_once('/') else {
        return false;
    };
    crate::ops::types::DYNAMIC_INSTRUMENTATION_SIGNAL_TYPE_VALUES.contains(&signal_type)
        && location_hash.len() == LOCATION_HASH_BYTES * 2
        && location_hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

/// The ARN of the configuration at `location_hash` in `scope`, in the format the modelled
/// `InstrumentationConfigurationArn` doc states:
/// `arn:${Partition}:application-signals:${Region}:${Account}:instrumentationConfig/${service}/${environment}/${signalType}/${locationHash}`.
///
/// The four parts it spells are a configuration's whole identity \u2014 the instrumentation type
/// is not among them \u2014 which is the identity a create conflicts on
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::instrumentation_configuration_at_location`]),
/// so one ARN names at most one stored record.
pub fn instrumentation_configuration_arn(
    region: &str,
    scope: &InstrumentationScope,
    signal_type: &str,
    location_hash: &str,
) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!(
            "{INSTRUMENTATION_CONFIG_ARN_RESOURCE}/{}/{}/{signal_type}/{location_hash}",
            scope.service, scope.environment
        ),
    )
}

/// The `NextToken` tag `scope` issues its page tokens under: the operation's
/// [`INSTRUMENTATION_CONFIGURATIONS_TOKEN_TAG`] plus a fingerprint of the scope.
///
/// A cursor is a key within ONE scope's ordered set, so a token issued for another scope
/// would silently skip entries if it were honoured. Binding the scope into the tag makes
/// such a token fail to decode, and it is refused like any other token this service did not
/// issue.
pub fn instrumentation_token_tag(scope: &InstrumentationScope) -> String {
    let fingerprint = stable_hash(&[
        Some(scope.instrumentation_type.as_str()),
        Some(scope.service.as_str()),
        Some(scope.environment.as_str()),
    ]);
    format!("{INSTRUMENTATION_CONFIGURATIONS_TOKEN_TAG}{fingerprint}/")
}

/// The member path a refusal of the location a create names states, in the lowerCamel casing
/// this service's `ValidationException` envelope states a member with — the same casing the
/// framework-level refusal of the same member's modelled bounds uses.
pub const LOCATION_MEMBER_PATH: &str = "location";

/// [`LOCATION_MEMBER_PATH`], for the `LocationIdentifier` the reads and the delete take the
/// instrumentation point under.
pub const LOCATION_IDENTIFIER_MEMBER_PATH: &str = "locationIdentifier";

/// The `codeLocation` step a refusal names between the location member and one of
/// `CodeLocation`'s own members: the union member both `Location` and `LocationIdentifier`
/// carry a full code location under.
pub(super) const CODE_LOCATION_PATH_STEP: &str = "codeLocation";

/// The `ProgrammingLanguage` a code location must name a `ClassName` for: `CodeLocation`'s
/// `ClassName` doc states "This is required for Java and optional for Python module-level
/// functions", so the requirement holds for exactly this one value of the sibling `Language`
/// (`smithy_query(query="doc",shape="CodeLocation",member="ClassName")`).
pub(super) const CLASS_NAME_REQUIRED_FOR_LANGUAGE: &str = "Java";

/// The refusal a `CodeLocation` carried under `member_path` earns, or `None` when it meets
/// every requirement its members document.
///
/// `ClassName` is modelled OPTIONAL — the model has no way to say "required when a sibling
/// carries one particular value" — so the boundary validator admits a location of the
/// language that requires it with the member absent, and the rule is enforced here and
/// refused with 400 rather than stored as an instrumentation point that names no class to
/// find the method in.
///
/// The path is a parameter because a refusal states the member the REQUEST carries, which is
/// `Location` for a create and `LocationIdentifier` for the reads and the delete; the
/// requirement is worded through the same clause builder as every other constraint this
/// service states.
pub fn code_location_refusal(member_path: &str, code: &CodeLocation) -> Option<String> {
    let class_name_missing = code.language == CLASS_NAME_REQUIRED_FOR_LANGUAGE
        && code.class_name.is_none();
    class_name_missing.then(|| {
        validation_envelope(&[constraint_clause(
            &format!("{member_path}.{CODE_LOCATION_PATH_STEP}.className"),
            // The language is taken from the REQUEST rather than from the constant, so the
            // clause states the value the rule fired on whichever language it fires for.
            &format!("Member must not be null for a {} code location", code.language),
        )])
    })
}

/// [`code_location_refusal`] for the `Location` union a create names the instrumentation
/// point with.
pub fn location_refusal(location: &Location) -> Option<String> {
    match location {
        Location::CodeLocation(code) => code_location_refusal(LOCATION_MEMBER_PATH, code),
    }
}

/// [`code_location_refusal`] for the `LocationIdentifier` union the reads and the delete name
/// the instrumentation point with.
///
/// A pre-computed hash carries no code location, so there is no conditional requirement to
/// read against it: it is 16 hex characters the model bounds on its own.
pub fn identified_location_refusal(identifier: &LocationIdentifier) -> Option<String> {
    match identifier {
        LocationIdentifier::CodeLocation(code) => {
            code_location_refusal(LOCATION_IDENTIFIER_MEMBER_PATH, code)
        }
        LocationIdentifier::LocationHash(_) => None,
    }
}

/// The `LocationHash` of a location: "A stable hash computed from the location that uniquely
/// identifies this instrumentation point within the service, environment, and signal type."
///
/// Stable (the same location always hashes the same, in this process and the next, because
/// nothing but the location's own members feeds it) and 16 lowercase hex characters, as the
/// member's `length 16..16` and the ARN pattern's `[0-9a-f]{16}` suffix require.
pub fn location_hash(location: &Location) -> String {
    match location {
        Location::CodeLocation(code) => {
            let line_number = code.line_number.map(|line| line.to_string());
            stable_hash(&[
                // The union member name, so two variants could never collide on one
                // rendering of their contents.
                Some("CodeLocation"),
                Some(code.language.as_str()),
                code.code_unit.as_deref(),
                code.class_name.as_deref(),
                code.method_name.as_deref(),
                Some(code.file_path.as_str()),
                line_number.as_deref(),
            ])
        }
    }
}

/// The `LocationHash` a `GetInstrumentationConfiguration` request's `LocationIdentifier`
/// denotes, for either form the model admits: "either full code location or a pre-computed
/// hash".
///
/// A full location is hashed by [`location_hash`] — the same function the create path keyed
/// the record by, so a request naming the location and one naming the hash resolve to one
/// record — and a hash the caller already holds IS the value, taken verbatim.
pub fn identified_location_hash(identifier: &LocationIdentifier) -> String {
    match identifier {
        LocationIdentifier::CodeLocation(code) => {
            location_hash(&Location::CodeLocation(code.clone()))
        }
        LocationIdentifier::LocationHash(hash) => hash.clone(),
    }
}

/// 16 lowercase hex characters of SHA-256 over `fields`.
///
/// The digest is the maintained `sha2` crate's, and the rendering is INJECTIVE: each field
/// contributes a present/absent marker, its byte length and its bytes, so no two different
/// field lists produce the same input \u2014 which is what makes two locations that differ only
/// in where a value sits (an absent `ClassName` against an empty one, say) hash differently.
pub(super) fn stable_hash(fields: &[Option<&str>]) -> String {
    use ::sha2::Digest;

    let mut digest = ::sha2::Sha256::new();
    for field in fields {
        match field {
            Some(value) => {
                digest.update([1u8]);
                digest.update((value.len() as u64).to_le_bytes());
                digest.update(value.as_bytes());
            }
            None => digest.update([0u8]),
        }
    }
    ::hex::encode(&digest.finalize()[..LOCATION_HASH_BYTES])
}

/// A stored configuration as a list reports it: every member of the record except the
/// service and environment, "because they are provided in the request" and the page states
/// them once.
pub fn without_service_env(
    stored: &InstrumentationConfiguration,
) -> InstrumentationConfigurationWithoutServiceEnv {
    InstrumentationConfigurationWithoutServiceEnv {
        instrumentation_type: stored.instrumentation_type.clone(),
        signal_type: stored.signal_type.clone(),
        location: stored.location.clone(),
        location_hash: stored.location_hash.clone(),
        description: stored.description.clone(),
        expires_at: stored.expires_at,
        attribute_filters: stored.attribute_filters.clone(),
        capture_configuration: stored.capture_configuration.clone(),
        created_at: stored.created_at,
        arn: stored.arn.clone(),
    }
}

/// The member path a refusal of the `ExpiresAt` rule names, spelled the way the boundary
/// validator spells a member path for this service ([`START_TIME_MEMBER_PATH`]).
pub(super) const EXPIRES_AT_MEMBER_PATH: &str = "expiresAt";

/// The `ExpiresAt` a create stores for an `InstrumentationType`, or `Err` with the refusal
/// the request earns.
///
/// The rule is the operation's own doc: "For BREAKPOINT type configurations, they expire
/// after 24 hours unless a shorter expiration is provided. For PROBE type configurations,
/// they persist until explicitly deleted; an expiration cannot be set for PROBE
/// configurations", and the member's: "For BREAKPOINT: optional, defaults to 24 hours, must
/// be between 5 min and 24 hours. For PROBE: not supported."
///
/// The window is measured from the instant the create is served, which is what makes the
/// stored expiry a lifetime rather than a wall-clock instant the caller may pick freely.
///
/// Every refusal is worded through the service's one constraint-violation builder
/// ([`constraint_clause`], under [`validation_envelope`]), like the conditional requirement on
/// the location this same create enforces beside it: a constraint the model cannot express
/// names its member path and its requirement exactly as a modelled one refused at the
/// boundary does.
pub fn requested_expiry(
    instrumentation_type: &str,
    requested: Option<&DateTime>,
    now: &DateTime,
) -> Result<Option<DateTime>, String> {
    if instrumentation_type != INSTRUMENTATION_TYPE_BREAKPOINT {
        return match requested {
            // The type is taken from the REQUEST rather than from the constant, so the clause
            // states the type the rule fired on whichever type it fires for.
            Some(_) => Err(validation_envelope(&[constraint_clause(
                EXPIRES_AT_MEMBER_PATH,
                &format!(
                    "Member must be null for a {instrumentation_type} instrumentation \
                     configuration"
                ),
            )])),
            None => Ok(None),
        };
    }

    let Some(requested) = requested else {
        return Ok(Some(instant_plus(now, BREAKPOINT_DEFAULT_EXPIRY)));
    };

    let lifetime = instant_nanos(requested).saturating_sub(instant_nanos(now));
    // The two ends of the window are stated separately, each as the bound it violated, the
    // way the boundary validator states a minimum and a maximum.
    if lifetime < duration_nanos(BREAKPOINT_MIN_EXPIRY) {
        return Err(expiry_window_refusal(
            "greater than or equal to",
            BREAKPOINT_MIN_EXPIRY.as_secs(),
        ));
    }
    if lifetime > duration_nanos(BREAKPOINT_MAX_EXPIRY) {
        return Err(expiry_window_refusal(
            "less than or equal to",
            BREAKPOINT_MAX_EXPIRY.as_secs(),
        ));
    }
    Ok(Some(*requested))
}

/// The `CaptureConfiguration` stored for a request, with each member the model documents a
/// default for filled in when the request omitted it — the same rule
/// [`goal_with_defaults`] settles an SLO's `Goal` by, and [`requested_expiry`] a BREAKPOINT's
/// `ExpiresAt`.
///
/// A create is the only writer of this structure and configurations are immutable after it,
/// so settling the defaults once, here, is what makes every later read report the setting the
/// service APPLIED rather than the absence the request sent.
///
/// The two LISTS are left as the request sent them, because neither doc states a value the
/// service writes back for an omission: `CaptureArguments` names none at all ("Omit to
/// capture defaults, use an empty list to capture none, use `["*"]` to capture all
/// arguments"), and `CaptureLocals` states only that omitting it captures what an empty list
/// captures ("Omit or pass an empty list to capture none") — an equivalence of effect, not a
/// stored default like the "Defaults to …" sentence each of its siblings carries.
pub fn stored_capture_configuration(requested: &CaptureConfiguration) -> CaptureConfiguration {
    match requested {
        CaptureConfiguration::CodeCapture(code) => {
            CaptureConfiguration::CodeCapture(CodeCaptureConfiguration {
                capture_arguments: code.capture_arguments.clone(),
                capture_return: Some(code.capture_return.unwrap_or(DEFAULT_CAPTURE_RETURN)),
                capture_stack_trace: Some(
                    code.capture_stack_trace.unwrap_or(DEFAULT_CAPTURE_STACK_TRACE),
                ),
                capture_locals: code.capture_locals.clone(),
                capture_limits: stored_capture_limits(&code.capture_limits),
            })
        }
    }
}

/// The `CaptureLimitsConfig` stored for a request: every limit the model documents a default
/// for, settled to that default when the request named none.
pub(super) fn stored_capture_limits(requested: &CaptureLimitsConfig) -> CaptureLimitsConfig {
    CaptureLimitsConfig {
        max_hits: Some(requested.max_hits.unwrap_or(DEFAULT_MAX_HITS)),
        max_string_length: Some(
            requested
                .max_string_length
                .unwrap_or(DEFAULT_MAX_STRING_LENGTH),
        ),
        max_collection_width: Some(
            requested
                .max_collection_width
                .unwrap_or(DEFAULT_MAX_COLLECTION_WIDTH),
        ),
        max_collection_depth: Some(
            requested
                .max_collection_depth
                .unwrap_or(DEFAULT_MAX_COLLECTION_DEPTH),
        ),
        max_stack_frames: Some(requested.max_stack_frames.unwrap_or(DEFAULT_MAX_STACK_FRAMES)),
        max_stack_trace_size: Some(
            requested
                .max_stack_trace_size
                .unwrap_or(DEFAULT_MAX_STACK_TRACE_SIZE),
        ),
        max_object_depth: Some(requested.max_object_depth.unwrap_or(DEFAULT_MAX_OBJECT_DEPTH)),
        max_fields_per_object: Some(
            requested
                .max_fields_per_object
                .unwrap_or(DEFAULT_MAX_FIELDS_PER_OBJECT),
        ),
    }
}

/// The refusal an `ExpiresAt` outside the documented lifetime window earns: the bound it
/// violated, in seconds from the time of the request, worded through the service's one
/// constraint-violation builder.
pub(super) fn expiry_window_refusal(comparison: &str, bound_secs: u64) -> String {
    validation_envelope(&[constraint_clause(
        EXPIRES_AT_MEMBER_PATH,
        &format!(
            "Member must have value {comparison} {bound_secs} seconds after the time of the \
             request"
        ),
    )])
}

/// The modelled `ConflictException` for a create that names a configuration already stored.
///
/// The sentence states the identity DIMENSIONS the uniqueness rule is on and no VALUE of
/// any of them. The create's own doc is what those dimensions are — "if a configuration
/// already exists for the same service, environment, signal type, and location, this
/// operation returns a conflict instead of overwriting it" — and they are what the caller
/// has to change to get a create through; the ARN of the stored record is not, and
/// `ConflictException` carries only `Message`, so putting it there would disclose an
/// identifier the caller never named in the one field it reads.
pub fn instrumentation_configuration_already_exists() -> ConflictException {
    ConflictException {
        message: "An instrumentation configuration already exists for this service, \
                  environment, signalType and location."
            .to_string(),
    }
}

/// The only `DynamicInstrumentationDeletionStatus` the model declares
/// (`smithy_query(query="shape",shape="DynamicInstrumentationDeletionStatus")` ->
/// `["DELETED"]`), which `DeleteInstrumentationConfiguration` answers once the configuration
/// it named is no longer stored. Its member doc: "The value is DELETED when the
/// configuration has been removed."
pub const DELETION_STATUS_DELETED: &str = "DELETED";

/// The `BatchDeleteErrorCode` a batch delete reports against an ARN that names no stored
/// configuration (`smithy_query(query="shape",shape="BatchDeleteErrorCode")` ->
/// `["ResourceNotFoundException", "AccessDeniedException", "InternalServiceException"]`), the
/// per-item counterpart of the `ResourceNotFoundException` a single-resource delete answers.
pub const BATCH_DELETE_ERROR_RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The `ResourceType` an instrumentation configuration failure reports: the resource's TYPE
/// NAME (the modelled `InstrumentationConfiguration` shape), not the camel-case segment an
/// ARN spells ([`INSTRUMENTATION_CONFIG_ARN_RESOURCE`]) — the same distinction
/// [`SLO_RESOURCE_TYPE`] draws for an SLO, and what a client comparing `ResourceType`
/// compares against.
pub const INSTRUMENTATION_CONFIG_RESOURCE_TYPE: &str = "InstrumentationConfiguration";

/// The sentence a read of a location this store once held a configuration for, and holds
/// none at now, is refused with: the configuration the request named has been taken away.
///
/// Entirely fixed text. The refusal states the FACT about the configuration the request
/// identified, so it names no identifier of its own: the request already carries the service,
/// the environment, the signal type and the location it asked about, and the body a client
/// gets back adds nothing it did not send.
pub const INSTRUMENTATION_CONFIGURATION_DELETED: &str =
    "The requested instrumentation configuration has been deleted.";

/// The sentence a refusal about a location this store holds no configuration at and
/// remembers removing none from states, and the sentence EVERY delete of an identity it
/// cannot resolve states. Fixed text, for the same reason as
/// [`INSTRUMENTATION_CONFIGURATION_DELETED`].
pub const INSTRUMENTATION_CONFIGURATION_ABSENT: &str =
    "The requested instrumentation configuration does not exist.";

/// The modelled `ResourceNotFoundException` an instrumentation-configuration operation
/// refuses an identity it cannot resolve with: `Message` alone.
///
/// The shape this builds is the one with `ResourceType` and `ResourceId` declared unsupported
/// (`crate::ops::types::instrumentation_not_found_types`), so the body carries the sentence
/// and nothing else — unlike [`slo_not_found`], whose refusal is about a resource the request
/// named by an id it may not hold the full form of, and which therefore reports that id and
/// its type.
pub(super) fn instrumentation_configuration_not_found(message: &str) -> InstrumentationNotFound {
    InstrumentationNotFound {
        message: message.to_string(),
    }
}

/// What a READ of an instrumentation configuration answers for an identity that resolves no
/// stored record: the configuration has been deleted when the store remembers one being
/// removed from that location
/// ([`ComAmazonawsApplicationsignalsApplicationsignalsState::instrumentation_configuration_was_removed`]),
/// and no such configuration exists when it remembers none.
///
/// The distinction is the one a client reading back after a delete asks about: whether what
/// it created is gone, or whether it is asking about a location nothing was ever stored at —
/// two different answers, which one sentence for both would collapse.
pub fn read_instrumentation_configuration_not_found(removed: bool) -> InstrumentationNotFound {
    instrumentation_configuration_not_found(if removed {
        INSTRUMENTATION_CONFIGURATION_DELETED
    } else {
        INSTRUMENTATION_CONFIGURATION_ABSENT
    })
}

/// What a DELETE answers for an identity that resolves no stored configuration, whether or
/// not the store remembers removing one from that location.
///
/// The delete path states what it could not do — there is no configuration there to remove —
/// rather than what became of an earlier one, so a repeat of a successful delete and a delete
/// of a location nothing was ever stored at are refused with ONE sentence. The read path's
/// sentence is the one that distinguishes those two ([`read_instrumentation_configuration_not_found`]).
pub fn delete_instrumentation_configuration_not_found() -> InstrumentationNotFound {
    instrumentation_configuration_not_found(INSTRUMENTATION_CONFIGURATION_ABSENT)
}

/// What the per-item error of a batch delete states about an ARN that names no stored
/// configuration: the configuration is no longer there, by an earlier delete or by the lapse
/// of its lifetime — the two ways a location comes to hold none, which this path does not
/// distinguish.
///
/// Entirely fixed text, naming no identifier. The entry carries the ARN it is about in its
/// own `ResourceArn` member (`smithy_query(query="shape",shape="BatchDeleteError")`), which
/// is where a client reads WHICH item of the batch failed; repeating it in the prose would
/// state an identifier the entry already reports in the member a client parses.
pub const INSTRUMENTATION_CONFIGURATION_GONE: &str =
    "The instrumentation config is already deleted or expired.";

impl ComAmazonawsApplicationsignalsApplicationsignalsState {
    /// Remove the configuration stored in `scope` under `key` ([`instrumentation_key`]),
    /// along with its tags, and hand back the removed record. `None` — and nothing removed —
    /// when the scope holds no configuration at that location.
    ///
    /// This is the ONLY place a configuration record is taken away: the explicit delete and
    /// the clock-owed lapse ([`Self::expire_instrumentation_configurations`]) both come
    /// through here, so a record that grows a fourth thing keyed off its key or its ARN is
    /// torn down on both paths without either having to learn about it.
    ///
    /// `at` is recorded as the instant the scope's set changed, the mirror of
    /// [`Self::put_instrumentation_configuration`]: a deletion is news to every caller that
    /// synced before it, and it leaves no record for a list to read a timestamp off. A scope
    /// whose set this empties keeps that instant for [`EMPTY_SCOPE_RETENTION`] so a slower
    /// caller still learns of the change, and the expiry pass is scheduled for the end of
    /// that horizon so the emptied record is dropped once it has nothing left to tell.
    ///
    /// The location is also remembered as one a configuration was removed from, which is what
    /// a later READ of it reports ([`read_instrumentation_configuration_not_found`]).
    pub fn remove_instrumentation_configuration(
        &mut self,
        scope: &InstrumentationScope,
        key: &str,
        at: DateTime,
    ) -> Option<InstrumentationConfiguration> {
        let (removed, emptied) = {
            let set = self.instrumentation_configurations.get_mut(scope)?;
            let removed = set.configurations.remove(key)?;
            // The status history belongs to the configuration, so it goes with it: a later
            // configuration created at the same location is a NEW resource and has reported
            // nothing yet.
            set.status_history.remove(key);
            set.record_change(at);
            (removed, set.configurations.is_empty())
        };
        self.remember_instrumentation_removal(scope, key, at);
        // Tags are addressed by resource ARN, so a record that is gone takes its tags with
        // it rather than leaving them for the next resource minted at that ARN.
        self.tags.remove(&removed.arn);
        if emptied {
            self.schedule_instrumentation_pass(instant_plus(&at, EMPTY_SCOPE_RETENTION));
        }
        Some(removed)
    }

    /// The [`instrumentation_key`] of every configuration stored in `scope`, in the order
    /// the ordered store holds them — the same order a list reports.
    ///
    /// Handed back as owned keys so a caller may remove each one in turn while the borrow of
    /// the store is released, which is what a delete over a whole scope does.
    pub fn instrumentation_configuration_keys(&self, scope: &InstrumentationScope) -> Vec<String> {
        self.instrumentation_configurations
            .get(scope)
            .map(|set| set.configurations.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The scope and [`instrumentation_key`] of the configuration stored under `arn` within
    /// `instrumentation_type`, or `None` when no configuration of that type carries that ARN.
    ///
    /// An `InstrumentationConfigurationArn` spells the service, the environment, the signal
    /// type and the location hash but NOT the instrumentation type, so the type comes from
    /// the request — the same selector every instrumentation operation takes, and the one
    /// that "determines which backing store to query": an ARN naming a configuration of the
    /// OTHER type names nothing in the store the request addressed. The ARN is matched
    /// against the one each stored record was minted with, rather than by taking the identity
    /// apart again, because a service or environment name may itself contain `/` or `:` — the
    /// comparison [`names_stored_resource`] makes, so this delete resolves an ARN exactly as
    /// [`Self::tagged_resource_arn`] does. The scan is bounded by
    /// [`MAX_INSTRUMENTATION_CONFIGURATIONS`].
    pub fn locate_instrumentation_configuration(
        &self,
        instrumentation_type: &str,
        arn: &str,
    ) -> Option<(InstrumentationScope, String)> {
        self.instrumentation_configurations
            .iter()
            .filter(|(scope, _)| scope.instrumentation_type == instrumentation_type)
            .flat_map(|(scope, set)| {
                set.configurations
                    .iter()
                    .map(move |(key, configuration)| (scope, key, configuration))
            })
            .find(|(_, _, configuration)| names_stored_resource(arn, &configuration.arn))
            .map(|(scope, key, _)| (scope.clone(), key.clone()))
    }
}
