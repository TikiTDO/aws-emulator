// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-costexplorer-awsinsightsindexservice service.
//!
//! The Cost Category definition registry lives here: it is the single source of truth
//! for `CreateCostCategoryDefinition`, `DescribeCostCategoryDefinition`,
//! `ListCostCategoryDefinitions`, `UpdateCostCategoryDefinition` and
//! `DeleteCostCategoryDefinition`, so none of those five operations derives identity or
//! content of its own.
//!
//! The cost anomaly MONITOR registry lives here for the same reason: `CreateAnomalyMonitor`
//! writes it and `GetAnomalyMonitors` reports it, and the anomaly subscription and tagging
//! operations resolve a monitor ARN against it, so the monitor's identity and content are
//! derived in exactly one place. The two registries are related by
//! [`delete_anomaly_monitor`](ComAmazonawsCostexplorerAwsinsightsindexserviceState::delete_anomaly_monitor),
//! which is what a monitor's delete does to the subscriptions that alerted on it.
//!
//! The cost anomaly SUBSCRIPTION registry is the third, and its record, admission rules
//! and threshold expansion live in [`anomaly_subscriptions`] — one module per resource, so
//! each file holds one registry's concern.
//!
//! The billing-date helpers live here rather than in one operation's file: all five
//! operations have to agree on what a `ZonedDateTime` means, and duplicating that across
//! five handlers is how they would drift apart. The pagination cursor codec and the page
//! walk every paginated read shares are in [`pagination`], for the same reason.

mod anomaly_subscriptions;
mod pagination;

pub use anomaly_subscriptions::{
    absolute_threshold_of, admitted_subscribers, threshold_expression_for, unknown_subscription,
    validate_anomaly_subscription, validate_monitor_arn_list, validate_subscribers,
    validate_threshold, AnomalySubscriptionRecord, MAX_ANOMALY_SUBSCRIPTIONS,
    MAX_THRESHOLD_EXPRESSION_DEPTH, SUBSCRIBER_STATUS_CONFIRMED, SUBSCRIPTION_ID_AS_RESOURCE,
    THRESHOLD_ABSOLUTE_DIMENSION, THRESHOLD_DIMENSIONS, THRESHOLD_MATCH_OPTION,
    THRESHOLD_VALUE_MAX,
};
pub use pagination::{
    decode_next_token, encode_next_token, page, requested_page_size, Page, UNPAGED,
};

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    AnomalyMonitor, CostCategory, CostCategoryProcessingStatus, CostCategoryReference,
    CostCategoryRule, CostCategorySplitChargeRule, Expression, ResourceTag,
    UnknownMonitorException,
};

/// How long a definition's Cost Explorer component reports `PROCESSING` before it
/// reports `APPLIED`.
///
/// Cost Explorer documents no duration for this, so the span is CHOSEN: it is long
/// enough that a caller issuing a Describe immediately after a Create still observes the
/// `PROCESSING` component the real service returns, and short enough that nothing waits
/// on it for long. It is never zero, and it is never slept on — the due instant is
/// stored and compared when a reader asks.
pub const PROCESSING_SPAN: Duration = Duration::from_millis(500);

/// The `CostCategoryStatusComponent` value Cost Explorer reports for its own component.
///
/// From the model's `CostCategoryStatusComponent` enum (see
/// `COST_CATEGORY_STATUS_COMPONENT_VALUES`), which declares exactly this one value.
pub const PROCESSING_COMPONENT: &str = "COST_EXPLORER";

// ---------------------------------------------------------------------------
// Wall-clock instants and `ZonedDateTime` members
// ---------------------------------------------------------------------------

/// An instant on the wall clock: the one value every "as of" comparison in this service
/// is made on.
///
/// Conversion to and from a `ZonedDateTime` member goes through
/// [`aws_smithy_types::date_time::DateTime`] — the same RFC-3339 implementation the rest
/// of the emulator uses — so an offset-bearing timestamp resolves to the instant it
/// names rather than to its local reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Timestamp {
    secs: i64,
    nanos: u32,
}

impl Timestamp {
    /// The current instant.
    pub fn now() -> Self {
        let since_epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            secs: i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
            nanos: since_epoch.subsec_nanos(),
        }
    }

    /// Parse a `ZonedDateTime` member into the instant it names, honouring its offset.
    ///
    /// `None` when the text is not an RFC-3339 timestamp. The modelled pattern is
    /// enforced before a handler runs, so a handler only reaches `None` for a value that
    /// pattern admits but no calendar does.
    pub fn parse(value: &str) -> Option<Self> {
        // The underlying RFC-3339 reader accepts the `Z` form, so a value carrying a
        // numeric offset is resolved by reading its civil part as UTC and shifting by the
        // offset it declares: the calendar conversion (month lengths, leap years) still
        // comes from the library, and only the constant offset is applied here.
        let (civil, offset_secs) = split_utc_offset(value)?;
        let parsed = DateTime::from_str(&format!("{civil}Z"), Format::DateTime).ok()?;
        Some(Self {
            secs: parsed.secs().checked_sub(offset_secs)?,
            nanos: parsed.subsec_nanos(),
        })
    }

    /// Render as the `ZonedDateTime` form the model's pattern accepts:
    /// `^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(([+-]\d\d:\d\d)|Z)$`.
    ///
    /// Seconds precision and a `Z` suffix: the sub-second part is dropped because a
    /// fractional-second RFC-3339 rendering would not match that pattern. `None` only for
    /// an instant outside the calendar the formatter can render.
    pub fn to_zoned_date_time(self) -> Option<String> {
        DateTime::from_secs(self.secs).fmt(Format::DateTime).ok()
    }
}

/// The current instant as a `ZonedDateTime`.
///
/// Infallible: `now` is always renderable, so [`Timestamp::to_zoned_date_time`]'s `None`
/// arm is unreachable here. It is written as a fallback rather than an `expect` because a
/// handler must never panic.
pub fn now_zoned_date_time() -> String {
    Timestamp::now().to_zoned_date_time().unwrap_or_default()
}

/// The current instant as a `YearMonthDay` member — the shape the anomaly monitor dates
/// target.
///
/// `YearMonthDay`'s pattern, `^(\d{4}-\d{2}-\d{2})(T\d{2}:\d{2}:\d{2}Z)?$`, admits the
/// same seconds-precision RFC-3339 reading with a `Z` suffix that `ZonedDateTime`'s does,
/// so ONE renderer serves both shapes rather than two that could drift.
pub fn now_year_month_day() -> String {
    now_zoned_date_time()
}

/// Split a `ZonedDateTime` into its civil part and the UTC offset it declares, in
/// seconds.
///
/// The rule is RFC 3339's, not this service's, so it is read through the ONE reader every
/// service with a string-typed timestamp shares
/// (`service_common::rfc3339::split_utc_offset`): the model's pattern admits two of the
/// forms that reader knows — `Z` or `±hh:mm` — and a value reaching a handler has already
/// been checked against that pattern, so the wider set it accepts is unreachable here.
fn split_utc_offset(value: &str) -> Option<(&str, i64)> {
    ::service_common::rfc3339::split_utc_offset(value)
}

/// The calendar fields a `ZonedDateTime` member carries, READ IN THE OFFSET THE CALLER
/// WROTE THEM IN — which is the month, day and time of day the caller named.
#[derive(Debug, Clone, Copy)]
struct ZonedDateTimeFields {
    year: i64,
    month: i64,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
}

/// Read the calendar fields out of a `ZonedDateTime` member.
///
/// The layout is fixed by the model's own pattern,
/// `^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(([+-]\d\d:\d\d)|Z)$`, so the field positions are
/// exact; the separators are re-checked here rather than assumed, and `None` means the
/// text does not have that layout. This reads fields, it does not do calendar
/// arithmetic — the instant a timestamp names always comes from [`Timestamp::parse`].
fn zoned_date_time_fields(value: &str) -> Option<ZonedDateTimeFields> {
    // 0123456789012345678
    // YYYY-MM-DDThh:mm:ss
    if value.len() < 19 || !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    Some(ZonedDateTimeFields {
        year: value.get(0..4)?.parse().ok()?,
        month: value.get(5..7)?.parse().ok()?,
        day: value.get(8..10)?.parse().ok()?,
        hour: value.get(11..13)?.parse().ok()?,
        minute: value.get(14..16)?.parse().ok()?,
        second: value.get(17..19)?.parse().ok()?,
    })
}

/// A month's position on the calendar, so two months can be compared and shifted without
/// reasoning about month lengths or leap years.
fn month_index(year: i64, month: i64) -> i64 {
    year * 12 + month - 1
}

/// The first day of the month `shift` months from the month `at` names, as a
/// `ZonedDateTime` at midnight UTC — a billing start date by construction.
///
/// `None` when the shifted month falls outside the four-digit years a `ZonedDateTime` can
/// express. Every caller here shifts by 0 or by [`EFFECTIVE_START_MONTHS_BACK`] from the
/// current month, so that cannot happen; the fallible signature is kept so a wrong window
/// can never be returned silently in its place.
pub fn month_start_shifted(at: Timestamp, shift: i64) -> Option<String> {
    let fields = zoned_date_time_fields(&at.to_zoned_date_time()?)?;
    let shifted = month_index(fields.year, fields.month) + shift;
    let year = shifted.div_euclid(12);
    let month = shifted.rem_euclid(12) + 1;
    if !(0..=9999).contains(&year) {
        return None;
    }
    Some(format!("{year:04}-{month:02}-01T00:00:00Z"))
}

/// The billing start date for a definition written now: the first day of the current
/// month.
///
/// Infallible: the current month is always expressible, so [`month_start_shifted`]'s
/// `None` arm is unreachable here, and a handler must never panic.
pub fn current_month_start() -> String {
    month_start_shifted(Timestamp::now(), 0).unwrap_or_else(now_zoned_date_time)
}

/// How many months back from the current one an `EffectiveStart` may reach.
///
/// From `CreateCostCategoryDefinition$EffectiveStart` / `UpdateCostCategoryDefinition$EffectiveStart`:
/// "Dates can't be before the previous twelve months, or in the future."
pub const EFFECTIVE_START_MONTHS_BACK: i64 = 12;

/// Check a caller-supplied `EffectiveStart` against the rules the model documents for it,
/// returning the refusal text when it breaks one.
///
/// The rules, quoted from `CreateCostCategoryDefinition$EffectiveStart`: "It can only be
/// a billing start date (first day of the month). ... Dates can't be before the previous
/// twelve months, or in the future." The day-of-month and time-of-day are read in the
/// offset the caller wrote them in — that is the month the caller named — while the
/// window is compared on the calendar, so an offset cannot push a month in or out of it.
pub fn validate_effective_start(value: &str) -> Result<(), String> {
    // Both readings have to succeed: the calendar fields as the caller wrote them, and
    // the instant the value names. One message covers both, because a caller fixes either
    // the same way.
    let fields = match (zoned_date_time_fields(value), Timestamp::parse(value)) {
        (Some(fields), Some(_)) => fields,
        _ => {
            return Err(format!(
                "EffectiveStart '{value}' is not a valid timestamp; expected a ZonedDateTime \
                 such as 2024-01-01T00:00:00Z"
            ))
        }
    };
    if fields.day != 1 || fields.hour != 0 || fields.minute != 0 || fields.second != 0 {
        return Err(format!(
            "EffectiveStart '{value}' is not a billing start date; it must be the first \
             day of a month at 00:00:00"
        ));
    }
    let Some(today) = zoned_date_time_fields(&now_zoned_date_time()) else {
        return Err("the current billing month could not be resolved".to_string());
    };
    let requested = month_index(fields.year, fields.month);
    let current = month_index(today.year, today.month);
    if requested > current {
        return Err(format!(
            "EffectiveStart '{value}' is in the future; it must be the first day of the \
             current month or of one of the previous {EFFECTIVE_START_MONTHS_BACK} months"
        ));
    }
    if current - requested > EFFECTIVE_START_MONTHS_BACK {
        return Err(format!(
            "EffectiveStart '{value}' is before the previous {EFFECTIVE_START_MONTHS_BACK} \
             months; it must be the first day of the current month or of one of the \
             previous {EFFECTIVE_START_MONTHS_BACK} months"
        ));
    }
    Ok(())
}

/// The earliest `EffectiveStart` the window above admits, as a `ZonedDateTime`.
///
/// Exposed so a caller can name the published lower bound without re-deriving the month
/// arithmetic.
pub fn earliest_effective_start() -> Option<String> {
    month_start_shifted(Timestamp::now(), -EFFECTIVE_START_MONTHS_BACK)
}

/// Resolve an `EffectiveOn`-style member to the instant a read is answered as of: the
/// value the caller named, or now when they named none.
pub fn effective_on_or_now(value: Option<&str>) -> Result<Timestamp, String> {
    match value {
        None => Ok(Timestamp::now()),
        Some(raw) => {
            Timestamp::parse(raw).ok_or_else(|| format!("'{raw}' is not a valid ZonedDateTime"))
        }
    }
}

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Mint the resource id a Cost Explorer ARN carries — a cost category's or a cost anomaly
/// monitor's.
///
/// Shaped like the UUID those ARNs carry (8-4-4-4-12 hex), and built as an RFC 9562
/// VERSION 8 (custom) UUID: the 16 bytes are a SHA-256 of this service's own monotonic
/// sequence number and the creation instant in nanoseconds, so every resource this process
/// creates gets a distinct id and no caller input reaches it. Version 8 is what RFC 9562
/// reserves for a vendor-defined construction, so the value does not claim to be random
/// (version 4) or name-based (version 5).
pub fn mint_resource_id(sequence: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // Version nibble 8, then the IETF variant bits, as RFC 9562 lays them out.
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// A stored Cost Category definition: everything the five lifecycle operations report.
///
/// ONE VERSION PER DEFINITION. The real service keeps a version per `EffectiveStart`, so
/// a read with an `EffectiveOn` inside an earlier version's period sees that version's
/// rules; here an update overwrites the stored content, so `EffectiveOn` resolves against
/// the LATEST version only — it decides whether the definition was in effect at that
/// instant, not which revision of its rules applied. Keeping per-`EffectiveStart`
/// versions is its own piece of work; this limitation is stated here rather than left to
/// be discovered.
#[derive(Debug, Clone)]
pub struct CostCategoryDefinition {
    /// The generated ARN this definition is registered under.
    pub arn: String,
    /// The caller-chosen unique name.
    pub name: String,
    /// `CostCategoryRuleVersion` as the caller sent it.
    pub rule_version: String,
    /// The categorisation rules, in the order they are processed.
    pub rules: Vec<CostCategoryRule>,
    /// The split charge rules, when the caller supplied any.
    pub split_charge_rules: Option<Vec<CostCategorySplitChargeRule>>,
    /// The value charges fall into when no rule matches.
    pub default_value: Option<String>,
    /// Resource tags supplied at create time (reported by the tagging operations, which
    /// are still scaffolded, and kept here so they survive for them).
    pub resource_tags: Vec<ResourceTag>,
    /// The billing start date: a `ZonedDateTime`, always the first day of some month.
    pub effective_start: String,
    /// The billing end date, set when the definition is deleted.
    pub effective_end: Option<String>,
    /// When this definition's Cost Explorer component finishes processing the most
    /// recent write. Compared against the clock by a reader; never slept on.
    pub applied_at: Instant,
}

impl CostCategoryDefinition {
    /// Whether this definition was in effect at `at`: on or after its start, and before
    /// its end when it has one — a HALF-OPEN window, so an `EffectiveOn` exactly equal to
    /// a reported `EffectiveEnd` is outside it (the end is the instant the definition
    /// stopped being in effect).
    ///
    /// This is the one rule `EffectiveOn` (Describe, List) and the as-of-now default both
    /// go through, so a deleted definition disappears from "now" while remaining
    /// describable at a date inside its lifetime.
    pub fn effective_on(&self, at: Timestamp) -> bool {
        let Some(start) = Timestamp::parse(&self.effective_start) else {
            // Unreachable: every write stores a value this parses. Logged rather than
            // ignored so a future regression shows up as a warning instead of as a
            // resource that silently disappeared.
            tracing::warn!(
                arn = %self.arn,
                effective_start = %self.effective_start,
                "stored EffectiveStart is not a ZonedDateTime; treating the definition as \
                 not in effect"
            );
            return false;
        };
        if at < start {
            return false;
        }
        match self.effective_end.as_deref().and_then(Timestamp::parse) {
            Some(end) => at < end,
            None => true,
        }
    }

    /// The processing status list: the Cost Explorer component, `PROCESSING` until
    /// [`PROCESSING_SPAN`] after the write that last touched this definition and
    /// `APPLIED` from then on.
    pub fn processing_status(&self) -> Vec<CostCategoryProcessingStatus> {
        let status = if Instant::now() >= self.applied_at {
            "APPLIED"
        } else {
            "PROCESSING"
        };
        vec![CostCategoryProcessingStatus {
            component: Some(PROCESSING_COMPONENT.to_string()),
            status: Some(status.to_string()),
        }]
    }

    /// The distinct cost category values this definition can assign, in rule order.
    ///
    /// `CostCategoryReference$Values` is documented as "a list of unique cost category
    /// values in a specific cost category", so it is DERIVED from the stored rules
    /// rather than stored twice.
    pub fn values(&self) -> Vec<String> {
        let mut values: Vec<String> = Vec::new();
        for rule in &self.rules {
            if let Some(value) = rule.value.as_deref().filter(|v| !v.is_empty()) {
                if !values.iter().any(|seen| seen == value) {
                    values.push(value.to_string());
                }
            }
        }
        values
    }

    /// The full `CostCategory` structure Describe answers with.
    pub fn to_cost_category(&self) -> CostCategory {
        CostCategory {
            cost_category_arn: self.arn.clone(),
            effective_start: self.effective_start.clone(),
            effective_end: self.effective_end.clone(),
            name: self.name.clone(),
            rule_version: self.rule_version.clone(),
            rules: self.rules.clone(),
            split_charge_rules: self.split_charge_rules.clone(),
            processing_status: Some(self.processing_status()),
            default_value: self.default_value.clone(),
        }
    }

    /// The `CostCategoryReference` List answers with.
    pub fn to_reference(&self) -> CostCategoryReference {
        CostCategoryReference {
            cost_category_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            effective_start: Some(self.effective_start.clone()),
            effective_end: self.effective_end.clone(),
            number_of_rules: i32::try_from(self.rules.len()).unwrap_or(i32::MAX),
            processing_status: Some(self.processing_status()),
            values: Some(self.values()),
            default_value: self.default_value.clone(),
            // OMITTED, not reported as an empty list: what resource types a cost category
            // supports comes from the cost category RESOURCE ASSOCIATION surface, which is
            // not implemented, so the service has no answer here. An empty array would
            // assert "this cost category supports no resource types", which it does not
            // know.
            supported_resource_types: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Cost anomaly monitors
// ---------------------------------------------------------------------------

/// The `MonitorType` value of an Amazon Web Services managed monitor (model enum
/// `MonitorType`).
pub const MONITOR_TYPE_DIMENSIONAL: &str = "DIMENSIONAL";

/// The `MonitorType` value of a customer managed monitor (model enum `MonitorType`).
pub const MONITOR_TYPE_CUSTOM: &str = "CUSTOM";

/// The `MonitorDimension` values a managed monitor analyses on its own, with no
/// `MonitorSpecification` to narrow them.
///
/// From `AnomalyMonitor$MonitorSpecification`: "If MonitorDimension is SERVICE or
/// LINKED_ACCOUNT, do not specify this field".
pub const SELF_SUFFICIENT_MONITOR_DIMENSIONS: &[&str] = &["SERVICE", "LINKED_ACCOUNT"];

/// The `MonitorDimension` value whose managed monitor is configured by a tag key.
///
/// From `AnomalyMonitor$MonitorSpecification`: "If MonitorDimension is TAG, set this field
/// to `{ "Tags": { "Key": "your tag key" } }`".
pub const MONITOR_DIMENSION_TAG: &str = "TAG";

/// The `MonitorDimension` value whose managed monitor is configured by a cost category
/// key.
///
/// From `AnomalyMonitor$MonitorSpecification`: "If MonitorDimension is COST_CATEGORY, set
/// this field to `{ "CostCategories": { "Key": "your cost category key" } }`".
pub const MONITOR_DIMENSION_COST_CATEGORY: &str = "COST_CATEGORY";

/// The `Dimensions$Key` a customer managed monitor's specification may select on.
///
/// From `AnomalyMonitor$MonitorSpecification`, which gives exactly one `Dimensions` form
/// for a customer managed monitor: "To track linked accounts, set this field to `{
/// "Dimensions": { "Key": "LINKED_ACCOUNT", "Values": [ "your list of up to 10 account
/// IDs" ] } }`".
pub const CUSTOM_MONITOR_DIMENSION_KEY: &str = "LINKED_ACCOUNT";

/// Whether an `Expression` selects any costs at all: a specification that names none of
/// the selectors the shape carries configures nothing.
fn expression_selects_anything(expression: &Expression) -> bool {
    expression.dimensions.is_some()
        || expression.tags.is_some()
        || expression.cost_categories.is_some()
        || expression.product_attributes.is_some()
        || expression.and.is_some()
        || expression.or.is_some()
        || expression.not.is_some()
}

/// Check an `AnomalyMonitor` against the combination rules its own members document,
/// returning the refusal text when it breaks one.
///
/// The rules come from `AnomalyMonitor$MonitorType`, `$MonitorDimension` and
/// `$MonitorSpecification`: a managed (`DIMENSIONAL`) monitor names the dimension it
/// analyses, and carries a specification only for the two dimensions that need a key to be
/// configured, while a customer managed (`CUSTOM`) monitor names no dimension and is
/// configured entirely by its specification. The model expresses none of this, so a
/// combination its schema admits but its documentation forbids is refused here.
///
/// Shared by every operation that accepts a whole `AnomalyMonitor`, so create and update
/// cannot drift apart on it.
pub fn validate_anomaly_monitor(monitor: &AnomalyMonitor) -> Result<(), String> {
    let specification = monitor.monitor_specification.as_ref();
    match monitor.monitor_type.as_str() {
        MONITOR_TYPE_DIMENSIONAL => {
            let Some(dimension) = monitor.monitor_dimension.as_deref() else {
                return Err(format!(
                    "MonitorDimension is required for a {MONITOR_TYPE_DIMENSIONAL} monitor"
                ));
            };
            if SELF_SUFFICIENT_MONITOR_DIMENSIONS.contains(&dimension) {
                if specification.is_some() {
                    return Err(format!(
                        "MonitorSpecification must not be specified when MonitorDimension is \
                         {dimension}"
                    ));
                }
                return Ok(());
            }
            // The remaining two dimensions are each configured by one key, so the
            // specification must carry that key and nothing else can stand in for it.
            let configured_key = match dimension {
                MONITOR_DIMENSION_TAG => specification
                    .and_then(|expression| expression.tags.as_ref())
                    .and_then(|tags| tags.key.as_deref()),
                MONITOR_DIMENSION_COST_CATEGORY => specification
                    .and_then(|expression| expression.cost_categories.as_ref())
                    .and_then(|categories| categories.key.as_deref()),
                // Unreachable: `MonitorDimension` is a modelled enum, so the boundary
                // validator has already refused anything outside it. Refused rather than
                // admitted so a dimension added to the model later cannot be stored with
                // no rule applied to it.
                other => {
                    return Err(format!(
                        "MonitorDimension {other} is not a dimension this service can monitor"
                    ))
                }
            };
            let selector = if dimension == MONITOR_DIMENSION_TAG {
                "Tags"
            } else {
                "CostCategories"
            };
            match configured_key {
                Some(key) if !key.is_empty() => Ok(()),
                _ => Err(format!(
                    "MonitorSpecification must specify {selector} with a Key when \
                     MonitorDimension is {dimension}"
                )),
            }
        }
        MONITOR_TYPE_CUSTOM => {
            if let Some(dimension) = monitor.monitor_dimension.as_deref() {
                return Err(format!(
                    "MonitorDimension must not be specified for a {MONITOR_TYPE_CUSTOM} monitor, \
                     got {dimension}"
                ));
            }
            let Some(specification) = specification else {
                return Err(format!(
                    "MonitorSpecification is required for a {MONITOR_TYPE_CUSTOM} monitor"
                ));
            };
            if !expression_selects_anything(specification) {
                return Err(format!(
                    "MonitorSpecification must select the costs a {MONITOR_TYPE_CUSTOM} monitor \
                     analyzes"
                ));
            }
            match specification
                .dimensions
                .as_ref()
                .and_then(|dimensions| dimensions.key.as_deref())
            {
                Some(key) if key != CUSTOM_MONITOR_DIMENSION_KEY => Err(format!(
                    "MonitorSpecification Dimensions Key must be {CUSTOM_MONITOR_DIMENSION_KEY} \
                     for a {MONITOR_TYPE_CUSTOM} monitor, got {key}"
                )),
                _ => Ok(()),
            }
        }
        // Unreachable for the same reason as the dimension arm above: `MonitorType` is a
        // modelled enum.
        other => Err(format!(
            "MonitorType {other} is not a monitor type this service can create"
        )),
    }
}

// ---------------------------------------------------------------------------
// Unknown-resource refusals
// ---------------------------------------------------------------------------

/// The fixed half of the sentence Cost Explorer refuses an unresolvable monitor reference
/// with. The only variable part is the monitor id, which [`unknown_monitor`] appends.
const NO_MONITOR_PRESENT: &str = "No monitor present with";

/// The member word an operation whose own SUBJECT is the monitor names the id under: the
/// monitor is the resource the call was made against (`DeleteAnomalyMonitor`,
/// `UpdateAnomalyMonitor`).
pub const MONITOR_ID_AS_RESOURCE: &str = "resourceId";

/// The member word an operation that REFERENCES a monitor from another resource names the
/// id under: the monitor is something the request points at rather than the resource the
/// call is about (the anomaly SUBSCRIPTION operations, whose `MonitorArnList` names the
/// monitors a subscription alerts on).
pub const MONITOR_ID_AS_MONITOR: &str = "monitorId";

/// The bare resource id an ARN carries: the last segment of its resource portion, which is
/// what an unresolvable-reference refusal names rather than the whole ARN.
///
/// A value carrying no `/` at all is its own id — the ARN members are plain strings
/// (`GenericString`, pattern `^[\S\s]*$`), so a caller can send one, and it is then the
/// only id the request holds. Shared by every refusal of this kind, so the monitor and the
/// subscription surfaces cannot drift apart on what an id is.
pub fn resource_id(arn: &str) -> &str {
    match arn.rsplit_once('/') {
        Some((_, id)) => id,
        None => arn,
    }
}

/// The refusal every operation that resolves a monitor ARN against
/// [`anomaly_monitors`](ComAmazonawsCostexplorerAwsinsightsindexserviceState::anomaly_monitors)
/// answers with when the account holds no monitor under it.
///
/// One helper, so no two operations can drift apart on the wording: the modelled
/// `UnknownMonitorException` carrying Cost Explorer's own fixed sentence over the monitor
/// ID the request named, under the `member` word the refusing operation uses — one of
/// [`MONITOR_ID_AS_RESOURCE`] or [`MONITOR_ID_AS_MONITOR`]. The whole ARN is NOT echoed:
/// the service names the id component alone.
///
/// The status the refusal is served at is the service's, not the shape's modelled
/// default; `crate::scaffolded` holds that one rule for every operation at once.
pub fn unknown_monitor(member: &str, monitor_arn: &str) -> UnknownMonitorException {
    UnknownMonitorException {
        message: Some(format!(
            "{NO_MONITOR_PRESENT} {member}: {}",
            resource_id(monitor_arn)
        )),
    }
}

/// A stored cost anomaly detection monitor: everything `GetAnomalyMonitors` reports, and
/// what the subscription and tagging surfaces resolve a monitor ARN against.
#[derive(Debug, Clone)]
pub struct AnomalyMonitorRecord {
    /// The generated ARN this monitor is registered under.
    pub arn: String,
    /// The caller-chosen monitor name.
    pub monitor_name: String,
    /// `MonitorType` as the caller sent it: `DIMENSIONAL` or `CUSTOM`.
    pub monitor_type: String,
    /// The dimension a managed monitor analyses; absent on a customer managed one.
    pub monitor_dimension: Option<String>,
    /// The expression that controls which costs the monitor analyses, when it has one.
    pub monitor_specification: Option<Expression>,
    /// When the monitor was created, as a `YearMonthDay`.
    pub creation_date: String,
    /// When the monitor was last written, as a `YearMonthDay`.
    pub last_updated_date: String,
    /// When the monitor last evaluated for anomalies, as a `YearMonthDay`.
    ///
    /// `None` until an evaluation has happened. Nothing in this emulator evaluates cost
    /// data, so it stays `None` and the member is OMITTED rather than reported as the
    /// creation date, which would claim an evaluation that never ran.
    pub last_evaluated_date: Option<String>,
    /// Resource tags supplied at create time, kept here so the tagging operations report
    /// the tags the monitor was created with.
    pub resource_tags: Vec<ResourceTag>,
}

impl AnomalyMonitorRecord {
    /// The `AnomalyMonitor` structure the read operations answer with.
    pub fn to_anomaly_monitor(&self) -> AnomalyMonitor {
        AnomalyMonitor {
            monitor_arn: Some(self.arn.clone()),
            monitor_name: self.monitor_name.clone(),
            creation_date: Some(self.creation_date.clone()),
            last_updated_date: Some(self.last_updated_date.clone()),
            last_evaluated_date: self.last_evaluated_date.clone(),
            monitor_type: self.monitor_type.clone(),
            monitor_dimension: self.monitor_dimension.clone(),
            monitor_specification: self.monitor_specification.clone(),
            // "The value for evaluated dimensions": how many dimension values the monitor
            // has evaluated. Nothing here evaluates cost data, so no dimension value has
            // been evaluated and the count is the member's own default of zero rather than
            // a number derived from the request.
            dimensional_value_count: 0,
        }
    }
}

/// Per-service state for com-amazonaws-costexplorer-awsinsightsindexservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCostexplorerAwsinsightsindexserviceState {
    /// Every Cost Category definition the registry still holds, keyed by its ARN. A
    /// deleted definition is TOMBSTONED (its `effective_end` is set) rather than dropped,
    /// so a Describe at a date inside its lifetime still answers — but only for as long
    /// as [`release_ended_definitions`](ComAmazonawsCostexplorerAwsinsightsindexserviceState::release_ended_definitions)
    /// retains it.
    ///
    /// A `BTreeMap` so List has one stable, total order to paginate over.
    ///
    /// BOUNDED on both halves: at most [`MAX_LIVE_COST_CATEGORIES`] definitions are in
    /// effect at once (Create refuses past that) and at most
    /// [`MAX_ENDED_COST_CATEGORIES`] ended ones are retained, so the store cannot grow
    /// without limit however many create/delete cycles a caller runs.
    pub cost_categories: BTreeMap<String, CostCategoryDefinition>,
    /// Every cost anomaly detection monitor this account holds, keyed by its ARN.
    ///
    /// A `BTreeMap` so the read side has one stable, total order to paginate over, exactly
    /// as the cost category registry above does.
    ///
    /// BOUNDED at [`MAX_ANOMALY_MONITORS`] entries, refused by Create past that, so the
    /// store cannot grow without limit however many monitors a caller creates.
    pub anomaly_monitors: BTreeMap<String, AnomalyMonitorRecord>,
    /// Every cost anomaly subscription this account holds, keyed by its ARN.
    ///
    /// A `BTreeMap` for the same reason as the two registries above: one stable, total
    /// order for the read side to paginate over.
    ///
    /// BOUNDED at [`MAX_ANOMALY_SUBSCRIPTIONS`] entries, refused by Create past that, so
    /// the store cannot grow without limit however many subscriptions a caller creates.
    pub anomaly_subscriptions: BTreeMap<String, AnomalySubscriptionRecord>,
    /// How many resource ids this service has minted — cost category definitions, anomaly
    /// monitors and anomaly subscriptions share the counter, so every id it hands out is
    /// distinct from every previous one whatever kind of resource asked for it.
    pub resources_minted: u64,
}

/// The number of cost category definitions that may be in effect at once.
///
/// Cost Explorer publishes a quota of 50 cost categories per account, and
/// `ServiceQuotaExceededException` (HTTP 402, "You've reached the limit on the number of
/// resources you can create") is the error Create and Update model for reaching it.
pub const MAX_LIVE_COST_CATEGORIES: usize = 50;

/// How many cost anomaly detection monitors an account may hold.
///
/// This service's model documents no number for it, so the bound is CHOSEN: it is what
/// keeps the registry from growing without limit however many monitors a caller creates,
/// and it is high enough that no ordinary use reaches it. `LimitExceededException` (HTTP
/// 400) is the only failure `CreateAnomalyMonitor` models, so that is what a create past
/// the bound is refused with.
pub const MAX_ANOMALY_MONITORS: usize = 100;

/// How many ENDED (tombstoned) definitions the registry retains.
///
/// A tombstone is not a resource the API counts — it is kept only so a Describe or List
/// naming an `EffectiveOn` inside a deleted definition's lifetime can still be answered.
/// The bound is therefore a CHOSEN retention limit on the emulator's own store rather
/// than an AWS quota: past it the longest-ended tombstones are released first.
pub const MAX_ENDED_COST_CATEGORIES: usize = 200;

impl ComAmazonawsCostexplorerAwsinsightsindexserviceState {
    /// Whether some definition that is in effect at `at` already carries `name`.
    ///
    /// `CostCategoryName` is documented as "the unique name of the cost category", so
    /// Create refuses a name a live definition already holds.
    pub fn name_taken(&self, name: &str, at: Timestamp) -> bool {
        self.cost_categories
            .values()
            .any(|def| def.name == name && def.effective_on(at))
    }

    /// How many definitions are in effect at `at` — the count
    /// [`MAX_LIVE_COST_CATEGORIES`] bounds. Tombstones do not count against it, so a
    /// delete frees a slot exactly as it does on the real service.
    pub fn live_count(&self, at: Timestamp) -> usize {
        self.cost_categories
            .values()
            .filter(|def| def.effective_on(at))
            .count()
    }

    /// Delete the monitor `monitor_arn` names, and dissociate it from every subscription
    /// that alerted on it.
    ///
    /// `false` when the account holds no monitor under that ARN — nothing is written, so
    /// the caller refuses the request against a registry exactly as it was.
    ///
    /// A subscription alerts on the monitors its `MonitorArnList` names, so deleting a
    /// monitor drops that ARN from every such list. The subscriptions themselves SURVIVE:
    /// one whose only monitor is deleted is left alerting on no monitor at all, and only
    /// `DeleteAnomalySubscription` removes it.
    ///
    /// Lives here rather than in the delete handler because it spans two registries, and
    /// the rule that relates them belongs in one place.
    pub fn delete_anomaly_monitor(&mut self, monitor_arn: &str) -> bool {
        if self.anomaly_monitors.remove(monitor_arn).is_none() {
            return false;
        }
        for subscription in self.anomaly_subscriptions.values_mut() {
            subscription
                .monitor_arn_list
                .retain(|arn| arn != monitor_arn);
        }
        true
    }

    /// Delete the subscription `subscription_arn` names.
    ///
    /// `false` when the account holds no subscription under that ARN.
    pub fn delete_anomaly_subscription(&mut self, subscription_arn: &str) -> bool {
        self.anomaly_subscriptions.remove(subscription_arn).is_some()
    }

    /// The id for the next resource this service mints, advancing the shared sequence.
    pub fn next_resource_id(&mut self) -> String {
        self.resources_minted = self.resources_minted.wrapping_add(1);
        mint_resource_id(self.resources_minted)
    }

    /// Release ended definitions the retention policy no longer keeps: first those whose
    /// end date has fallen out of the twelve-month window an `EffectiveStart` can name
    /// (the oldest lifetime a definition can have), then, if more than
    /// [`MAX_ENDED_COST_CATEGORIES`] remain, the longest-ended ones.
    ///
    /// Called ONLY from the write paths — nothing else adds entries — so no read changes
    /// what a later caller sees.
    pub fn release_ended_definitions(&mut self, at: Timestamp) {
        let horizon = month_start_shifted(at, -EFFECTIVE_START_MONTHS_BACK)
            .as_deref()
            .and_then(Timestamp::parse);
        if let Some(horizon) = horizon {
            self.cost_categories
                .retain(|_, def| match def.effective_end.as_deref() {
                    None => true,
                    // RETAINED when the end date cannot be parsed: only a value no handler
                    // writes gets here, and keeping a record the policy cannot date is
                    // safer than dropping one a reader may still be entitled to.
                    Some(end) => Timestamp::parse(end).is_none_or(|ended| ended >= horizon),
                });
        }

        let mut ended: Vec<(Timestamp, String)> = self
            .cost_categories
            .iter()
            .filter_map(|(arn, def)| {
                let ended = Timestamp::parse(def.effective_end.as_deref()?)?;
                Some((ended, arn.clone()))
            })
            .collect();
        if ended.len() <= MAX_ENDED_COST_CATEGORIES {
            return;
        }
        ended.sort();
        let excess = ended.len() - MAX_ENDED_COST_CATEGORIES;
        for (_, arn) in ended.into_iter().take(excess) {
            self.cost_categories.remove(&arn);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCostexplorerAwsinsightsindexserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCostexplorerAwsinsightsindexserviceState>)
}
