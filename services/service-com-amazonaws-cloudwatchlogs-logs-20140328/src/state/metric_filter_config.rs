// SPDX-License-Identifier: CC0-1.0

//! A METRIC FILTER's configuration as `PutMetricFilter` stores it: the filter record, the
//! metric transformations it emits metric data through, and the rules a transformation is
//! held to before it is stored ([`check_metric_transformation`]).
//!
//! The running of a stored filter over an ingested log event lives in
//! [`super::metric_filters`]; this is the configuration-time half.

use super::*;

/// One metric filter — how CloudWatch Logs "would extract metric observations from
/// ingested log events and transform them into metric data in a CloudWatch metric"
/// (`MetricFilter`) — as it is stored. It lives on the log group `PutMetricFilter`
/// "associates" it with, so deleting that group removes its metric filters with it.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MetricFilterRecord {
    pub filter_name: String,
    pub filter_pattern: String,
    /// The transformations this filter emits metric data through. The modelled
    /// `MetricTransformations` list is `length 1..1`, so this holds exactly one today;
    /// it is a list because the member is one, and a reader hands back what was put.
    pub transformations: Vec<MetricTransformationRecord>,
    pub apply_on_transformed_logs: bool,
    pub field_selection_criteria: Option<String>,
    /// The system fields the filter emits as additional dimensions, when the put named
    /// any; `None` otherwise, which is what `MetricFilter$emitSystemFieldDimensions`
    /// reports by omitting the member ("Returns the emitSystemFieldDimensions value if it
    /// was specified when the metric filter was created").
    pub emit_system_field_dimensions: Option<Vec<String>>,
    /// `MetricFilter$creationTime`. A put over a filter the group already holds UPDATES
    /// it, so this keeps the instant the first put created it.
    pub creation_time_ms: i64,
}

impl MetricFilterRecord {
    /// This filter as `DescribeMetricFilters` reports it, on the log group it is stored
    /// on. Every member comes from what was put, so a reader hands back the filter the
    /// group is actually configured with.
    pub fn to_shape(&self, log_group_name: &str) -> crate::ops::types::MetricFilter {
        crate::ops::types::MetricFilter {
            filter_name: Some(self.filter_name.clone()),
            filter_pattern: Some(self.filter_pattern.clone()),
            metric_transformations: Some(
                self.transformations
                    .iter()
                    .map(MetricTransformationRecord::to_shape)
                    .collect(),
            ),
            creation_time: Some(self.creation_time_ms),
            log_group_name: Some(log_group_name.to_string()),
            apply_on_transformed_logs: self.apply_on_transformed_logs,
            field_selection_criteria: self.field_selection_criteria.clone(),
            emit_system_field_dimensions: self.emit_system_field_dimensions.clone(),
        }
    }
}

/// One metric transformation — "how to transform ingested log events to metric data in a
/// CloudWatch metric" — as it is stored.
///
/// The modelled shape carries its `dimensions` in a `HashMap`, whose iteration order is
/// not stable; a stored transformation keeps them in a `BTreeMap` so everything derived
/// from them (a refusal, a reader's output) is the same on every run.
#[derive(Debug, Clone)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct MetricTransformationRecord {
    pub metric_name: String,
    pub metric_namespace: String,
    pub metric_value: String,
    /// "(Optional) The value to emit when a filter pattern does not match a log event."
    /// Never set together with `dimensions` — see [`METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS`].
    pub default_value: Option<f64>,
    /// "The fields to use as dimensions for the metric", keyed by dimension name. Empty
    /// for a transformation that declares none, which a reader reports by omitting the
    /// member.
    pub dimensions: BTreeMap<String, String>,
    /// "The unit to assign to the metric. If you omit this, the unit is set as `None`."
    /// `None` here is the ABSENT member: the model's `StandardUnit` enum spells the unit
    /// "None" itself, so a transformation put without a unit reports no unit rather than
    /// the string.
    pub unit: Option<String>,
}

impl MetricTransformationRecord {
    /// This transformation as a reader reports it.
    pub fn to_shape(&self) -> crate::ops::types::MetricTransformation {
        crate::ops::types::MetricTransformation {
            metric_name: self.metric_name.clone(),
            metric_namespace: self.metric_namespace.clone(),
            metric_value: self.metric_value.clone(),
            default_value: self.default_value,
            dimensions: (!self.dimensions.is_empty()).then(|| {
                self.dimensions
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect()
            }),
            unit: self.unit.clone(),
        }
    }
}

/// The refusal for a metric transformation that declares both `dimensions` and a
/// `defaultValue`.
///
/// The two are mutually exclusive by what each one MEANS: a dimensioned metric is one
/// metric per value found for the dimension fields, so there is no dimension value to
/// publish a "value to emit when a filter pattern does not match a log event" under — AWS
/// states the rule as "you can't specify a default value for the metric" when publishing
/// dimensions with metrics from log events.
///
/// This is the whole message the service answers for the error class: fixed text that
/// interpolates nothing from the request or from state — no filter name, no log group, no
/// metric name, no dimension name and no default value — so every transformation that
/// trips the rule is refused with exactly this sentence.
pub const METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS: &str =
    "Invalid metric transformation: dimensions and default value are mutually exclusive \
     properties";

/// The refusal for a metric transformation that declares MORE dimensions than one may
/// carry, counting the system fields the same put emits as dimensions
/// ([`MAX_METRIC_FILTER_DIMENSIONS`]).
///
/// This is the whole message the service answers for the error class: fixed text that
/// interpolates nothing — not the maximum, not how many were declared, and nothing of the
/// request — so every put past the limit is refused with exactly this sentence.
pub const METRIC_FILTER_TOO_MANY_DIMENSIONS: &str = "Number of dimensions exceeds the \
     maximum number of dimensions supported per metric filter";

/// The refusal for a dimension value that is not a field of the log event — one that is not
/// a valid SELECTOR. See [`is_log_event_field_reference`].
///
/// Fixed text for the error class too, beside
/// [`METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS`]: it names neither the dimension nor the
/// value the request stated.
pub const METRIC_FILTER_DIMENSION_VALUE_NOT_A_FIELD: &str =
    "Invalid metric transformation: dimension value must be valid selector";

/// Whether a metric filter's dimension value names a FIELD of the log event, which is the
/// only thing a dimension value may be: dimensions are published "with metrics from values
/// in JSON or space-delimited log events", so the value is a selector the filter pattern's
/// syntax resolves against each event — `$.some.path` in a JSON log event, `$field` or `$1`
/// in a space-delimited one — and never a literal, which would make the dimension a
/// constant instead of a field.
pub fn is_log_event_field_reference(value: &str) -> bool {
    let Some(selector) = value.strip_prefix(FIELD_REFERENCE_PREFIX) else {
        return false;
    };
    match selector.strip_prefix('.') {
        // A JSON selector: `$.` followed by at least one path segment, each of them named
        // — `$.a.`, `$.a..b` and `$.` select nothing.
        Some(path) => !path.is_empty() && path.split('.').all(|segment| !segment.is_empty()),
        // A space-delimited field: `$` followed by the name the pattern declared, or the
        // 1-based position of the field.
        None => !selector.is_empty(),
    }
}

/// The regular expression patterns a `filterPattern` carries: the `%…%` literals of the
/// filter-pattern syntax, which is the quantity `PutMetricFilter`'s two quotas
/// ([`MAX_REGEX_PATTERNS_PER_FILTER_PATTERN`], [`MAX_REGEX_PATTERNS_PER_LOG_GROUP`]) bound.
///
/// A `%` inside a quoted string is data rather than a delimiter, and a backslash escapes
/// the character after it — the same reading [`filter_pattern_structure`] gives a pattern.
/// An unterminated `%` opens no pattern, so it counts for nothing.
pub fn filter_pattern_regex_count(pattern: &str) -> usize {
    let mut count = 0usize;
    let mut quoted = false;
    let mut open = false;
    let mut chars = pattern.chars();
    while let Some(character) = chars.next() {
        match character {
            '\\' => {
                chars.next();
            }
            '"' if !open => quoted = !quoted,
            '%' if !quoted => {
                if open {
                    count = count.saturating_add(1);
                }
                open = !open;
            }
            _ => {}
        }
    }
    count
}

/// Hold one `metricTransformations` entry to the rules `PutMetricFilter` documents for it,
/// answering the transformation to store. The `Err` carries the message to refuse with, as
/// the service states the rule.
///
/// `emitted_system_fields` is how many system fields the same put asks to emit as
/// dimensions: they "count toward the total dimension limit for metric filters", so the
/// declared dimensions and the emitted system fields are counted together against
/// [`MAX_METRIC_FILTER_DIMENSIONS`].
///
/// Everything the model already constrains — the metric name's and namespace's pattern and
/// length, the `unit` enum, the one-entry list — is refused by the boundary walk before a
/// handler runs, so this holds only the rules that span members.
pub fn check_metric_transformation(
    transformation: &crate::ops::types::MetricTransformation,
    emitted_system_fields: usize,
) -> Result<MetricTransformationRecord, String> {
    let dimensions: BTreeMap<String, String> = transformation
        .dimensions
        .iter()
        .flatten()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();

    if dimensions.len().saturating_add(emitted_system_fields) > MAX_METRIC_FILTER_DIMENSIONS {
        return Err(METRIC_FILTER_TOO_MANY_DIMENSIONS.to_string());
    }
    if !dimensions.is_empty() && transformation.default_value.is_some() {
        return Err(METRIC_FILTER_DEFAULT_VALUE_WITH_DIMENSIONS.to_string());
    }
    if dimensions
        .values()
        .any(|value| !is_log_event_field_reference(value))
    {
        return Err(METRIC_FILTER_DIMENSION_VALUE_NOT_A_FIELD.to_string());
    }

    Ok(MetricTransformationRecord {
        metric_name: transformation.metric_name.clone(),
        metric_namespace: transformation.metric_namespace.clone(),
        metric_value: transformation.metric_value.clone(),
        default_value: transformation.default_value,
        dimensions,
        unit: transformation.unit.clone(),
    })
}
