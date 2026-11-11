// SPDX-License-Identifier: CC0-1.0

//! Contributor Insights rule EVALUATION: the rule syntax a `RuleDefinition` carries, read
//! over the log events ingested into the log groups the rule names, and the report
//! `GetInsightRuleReport` answers from what that reading collected.
//!
//! "Rules evaluate log events in a CloudWatch Logs log group, enabling you to find
//! contributor data for the log events in that log group." (`PutInsightRule`) — so a rule is
//! not a document that sits in the store: an ENABLED rule is a standing evaluation of the log
//! events its log groups RECEIVE. Each event the rule's filters admit contributes one
//! OBSERVATION ([`ContributorObservation`]): the values its `Contribution.Keys` extract — "If
//! the rule contains more than one key, then each unique combination of values for the keys is
//! counted as a unique contributor." (`GetInsightRuleReportOutput$KeyLabels`) — and the amount
//! it contributes, which is one occurrence for a rule that aggregates on count and the value
//! its `Contribution.ValueOf` names for one that aggregates on sum.
//!
//! Evaluation happens at INGEST, where the event arrives ([`State::record_ingested_log_events`],
//! called by the CloudWatch Logs side of this emulator): that is what makes a rule created
//! after an event was ingested report nothing for it, and a DISABLED rule — one that is not
//! "evaluating log events" — collect nothing while it is disabled, without either question
//! being asked again at read time.
//!
//! Reporting is then pure ([`report`]): the observations inside the requested window are
//! bucketed into `Period`-wide datapoints anchored at multiples of the period, which is what
//! makes the series a report answers with strictly ascending in `Timestamp` with no instant
//! repeated, and grouped by contributor and aggregated by the statistic the request ranks them
//! by — their total over the window, or the largest value any one period of it holds for them. The series spans
//! every period of the window, so a period that holds no observation is reported with each
//! statistic asked for at zero rather than left out. A report reads; it never changes what a
//! later caller sees.

use std::collections::{BTreeMap, BTreeSet};

use ::serde_json::Value;

use super::{ComAmazonawsCloudwatchGraniteserviceversion20100801State as State, StoredInsightRule};
use crate::ops::types::{
    InsightRuleContributor, InsightRuleContributorDatapoint, InsightRuleMetricDatapoint,
};

/// The log formats a Contributor Insights rule can be written against: "LogFormat" in the
/// Contributor Insights rule syntax takes `JSON` or `CLF`.
pub const LOG_FORMAT_JSON: &str = "JSON";

/// The other one — a space-separated log record, whose fields are named by POSITION
/// ([`LogRuleDefinition::fields`]). See [`LOG_FORMAT_JSON`].
pub const LOG_FORMAT_CLF: &str = "CLF";

/// The two log formats a rule definition may name.
pub const LOG_FORMATS: [&str; 2] = [LOG_FORMAT_JSON, LOG_FORMAT_CLF];

/// What a rule aggregates contributor data on: the count of occurrences of each contributor.
/// This is the keyword the rule's DEFINITION carries; the statistic a report names that
/// aggregation by is [`aggregation_statistic`].
pub const AGGREGATE_ON_COUNT: &str = "Count";

/// The aggregation that sums the field a contribution's `ValueOf` names. See
/// [`AGGREGATE_ON_COUNT`].
pub const AGGREGATE_ON_SUM: &str = "Sum";

/// The two aggregations a rule definition may name.
pub const AGGREGATE_ON: [&str; 2] = [AGGREGATE_ON_COUNT, AGGREGATE_ON_SUM];

/// The statistic a report names `aggregate_on` by: "Specifies whether this rule aggregates
/// contributor data by COUNT or SUM" (`GetInsightRuleReportOutput$AggregationStatistic`).
///
/// A report names an aggregation by the per-datapoint STATISTIC that aggregation computes, not
/// by the keyword the definition spells it with: counting occurrences computes "the number of
/// occurrences that matched the rule during this data point"
/// (`InsightRuleMetricDatapoint$SampleCount`), and summing the field a contribution names
/// computes "the sum of the values from all contributors"
/// (`InsightRuleMetricDatapoint$Sum`). A rule whose definition names no aggregation counts
/// (the reading [`LogRuleDefinition::parse`] makes), so every rule has one of the two.
#[must_use]
pub fn aggregation_statistic(aggregate_on: &str) -> &'static str {
    if aggregate_on == AGGREGATE_ON_SUM {
        SUM_METRIC
    } else {
        SAMPLE_COUNT_METRIC
    }
}

/// The statistic a report ranks its contributors by when the request names none: the first of
/// the two the member allows ("Determines what statistic to use to rank the contributors.
/// Valid values are Sum and Maximum.", `GetInsightRuleReportInput$OrderBy`), which is the
/// contributor's whole aggregated value.
pub const ORDER_BY_SUM: &str = "Sum";

/// The other ranking: the largest value the contributor reached in any ONE period of the
/// window. See [`ORDER_BY_SUM`].
pub const ORDER_BY_MAXIMUM: &str = "Maximum";

/// The two rankings `OrderBy` may name.
pub const ORDER_BY: [&str; 2] = [ORDER_BY_SUM, ORDER_BY_MAXIMUM];

/// The statistics a report's `Metrics` may ask for, each a per-datapoint aggregate of the
/// observations in that datapoint's period, as `GetInsightRuleReport` documents them
/// (`smithy_query(query="doc",shape="GetInsightRuleReportInput",member="Metrics")`).
pub const INSIGHT_RULE_METRICS: [&str; 7] = [
    UNIQUE_CONTRIBUTORS_METRIC,
    MAX_CONTRIBUTOR_VALUE_METRIC,
    SAMPLE_COUNT_METRIC,
    SUM_METRIC,
    MINIMUM_METRIC,
    MAXIMUM_METRIC,
    AVERAGE_METRIC,
];

/// "the number of unique contributors for each data point". See [`INSIGHT_RULE_METRICS`].
const UNIQUE_CONTRIBUTORS_METRIC: &str = "UniqueContributors";

/// "the value of the top contributor for each data point". See [`INSIGHT_RULE_METRICS`].
const MAX_CONTRIBUTOR_VALUE_METRIC: &str = "MaxContributorValue";

/// "the number of data points matched by the rule". See [`INSIGHT_RULE_METRICS`].
const SAMPLE_COUNT_METRIC: &str = "SampleCount";

/// "the sum of the values from all contributors during the time period represented by that
/// data point". See [`INSIGHT_RULE_METRICS`].
const SUM_METRIC: &str = "Sum";

/// "the minimum value from a single observation during the time period represented by that
/// data point". See [`INSIGHT_RULE_METRICS`].
const MINIMUM_METRIC: &str = "Minimum";

/// "the maximum value from a single observation during the time period represented by that
/// data point". See [`INSIGHT_RULE_METRICS`].
const MAXIMUM_METRIC: &str = "Maximum";

/// "the average value from all contributors during the time period represented by that data
/// point". See [`INSIGHT_RULE_METRICS`].
const AVERAGE_METRIC: &str = "Average";

/// How many contributors a report carries when the request asks for no number: "The maximum
/// number of contributors to include in the report. The range is 1 to 100. If you omit this,
/// the default of 10 is used." (`GetInsightRuleReportInput$MaxContributorCount`)
pub const DEFAULT_MAX_CONTRIBUTOR_COUNT: i32 = 10;

/// The smallest number of contributors a request may ask for. See
/// [`DEFAULT_MAX_CONTRIBUTOR_COUNT`].
pub const MIN_MAX_CONTRIBUTOR_COUNT: i32 = 1;

/// The largest number of contributors a request may ask for. See
/// [`DEFAULT_MAX_CONTRIBUTOR_COUNT`].
pub const MAX_MAX_CONTRIBUTOR_COUNT: i32 = 100;

/// How many observations ONE rule holds at a time.
///
/// EMULATOR CEILING, not a documented limit: the observations live in this process's memory,
/// so their number has to be finite. Reaching it drops the ones collected first, the direction
/// Contributor Insights data ages out in, so the recent window a report asks about stays exact.
pub const MAX_RULE_OBSERVATIONS: usize = 20_000;

/// How many observations the WHOLE store holds at a time, across every rule.
///
/// The second half of the same ceiling, and the one that makes it a bound: a caller may create
/// as many rules as `crate::state::MAX_INSIGHT_RULES` allows and point every one of them at the
/// same log group, so the per-rule ceiling alone would bound the retained footprint only by the
/// PRODUCT of the two. What one rule may keep is therefore narrowed to its share of this
/// ([`State::keep_contributor_observations`]).
pub const MAX_STORED_OBSERVATIONS: usize = 100_000;

/// How many rules ONE ingest evaluates its batch against.
///
/// EMULATOR CEILING on the first factor of an ingest's cost: the rules collecting from one log
/// group. A rule past it collects nothing from that batch.
pub const MAX_COLLECTING_RULES_PER_INGEST: usize = 100;

/// How many (rule, event) pairs ONE ingest evaluates, whatever the number of rules collecting
/// from the log group and the size of the batch.
///
/// EMULATOR CEILING on the PRODUCT of those two factors, which is what an ingest's work is
/// proportional to — each factor is bounded on its own (the rules a caller may create, the
/// 10,000 events one batch may carry), and a bound on each is not a bound on the product. It is
/// charged as the evaluation runs ([`evaluate_ingested_log_events`]), so a batch that would
/// exceed it stops rather than running to the end.
pub const MAX_EVALUATIONS_PER_INGEST: usize = 20_000;

/// One log event's contribution to a rule: which contributor it belongs to, when it was
/// observed, and how much it contributed.
#[derive(Debug, Clone, PartialEq)]
pub struct ContributorObservation {
    /// The instant the log event carried, in milliseconds since the epoch — the unit a log
    /// event's timestamp is in.
    pub timestamp_ms: i64,
    /// The values the rule's `Contribution.Keys` extracted, in the order the rule lists them:
    /// "each combination of values for the keys counts as a unique contributor"
    /// (`GetInsightRuleReportOutput$Contributors`).
    pub keys: Vec<String>,
    /// What the event contributed: one occurrence for a rule aggregating on count, and the
    /// value the contribution's `ValueOf` named for one aggregating on sum.
    pub value: f64,
}

/// The form of the log records a rule reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// `JSON`: each field is addressed by a JSON path (`$.ip`).
    Json,
    /// `CLF`: a space-separated record whose fields are addressed by position, directly
    /// (`$4`) or through the name the definition's `Fields` gives that position.
    Clf,
}

/// A Contributor Insights rule definition, read out of the JSON document a `RuleDefinition`
/// carries.
///
/// Only the members the evaluation needs are read; a definition carrying more than the rule
/// needs is still a rule (the same reading `PutInsightRule`'s syntax check makes).
#[derive(Debug, Clone)]
pub struct LogRuleDefinition {
    /// The log groups the rule collects from, by NAME — a group the definition named by ARN
    /// is held as the group that ARN addresses, so both forms select the same log group.
    pub log_group_names: Vec<String>,
    /// The form of the log records in those groups.
    pub log_format: LogFormat,
    /// For a `CLF` rule, the name each 1-based field POSITION is addressed by, as the
    /// definition's `Fields` member declares them.
    pub fields: BTreeMap<usize, String>,
    /// The fields a contributor is identified by, in the order the rule lists them.
    pub keys: Vec<String>,
    /// The field a summing rule sums, when the contribution names one.
    pub value_of: Option<String>,
    /// The filters an event must satisfy to be evaluated at all — all of them, so a rule with
    /// no filter admits every event of its log groups.
    pub filters: Vec<Filter>,
    /// What the rule aggregates contributor data on ([`AGGREGATE_ON`]).
    pub aggregate_on: String,
}

/// One entry of a contribution's `Filters`: the field it tests and the test it applies.
#[derive(Debug, Clone)]
pub struct Filter {
    /// The field the test reads, addressed the way the rule's log format addresses fields.
    pub match_field: String,
    /// The test the field's value has to pass.
    pub test: FilterTest,
}

/// The tests a filter entry of the Contributor Insights rule syntax may carry.
#[derive(Debug, Clone)]
pub enum FilterTest {
    /// `In`: the field's value is one of these.
    In(Vec<String>),
    /// `NotIn`: the field carries a value, and it is none of these.
    NotIn(Vec<String>),
    /// `StartsWith`: the field's value begins with one of these.
    StartsWith(Vec<String>),
    /// `GreaterThan`: the field's value is a number above this one.
    GreaterThan(f64),
    /// `LessThan`: the field's value is a number below this one.
    LessThan(f64),
    /// `EqualTo`: the field's value is this number.
    EqualTo(f64),
    /// `NotEqualTo`: the field carries a number, and it is not this one.
    NotEqualTo(f64),
    /// `IsPresent`: whether the field is carried at all.
    IsPresent(bool),
}

/// One log record, in the form its rule's log format reads.
#[derive(Debug, Clone)]
pub enum LogRecord {
    /// The parsed document of a `JSON` rule's log event.
    Json(Value),
    /// The whitespace-separated fields of a `CLF` rule's log event.
    Clf(Vec<String>),
}

/// One ingested batch, read ONCE in each log format the rules collecting from its log group
/// use.
///
/// Reading is what an ingest's cost is made of — a `JSON` rule's record is a parsed document —
/// so the batch is read per FORMAT (there are two) rather than per rule: the work is then the
/// batch's size plus the evaluation of each (rule, event) pair, instead of the batch's size
/// multiplied by the number of rules. A message that is not a document is held as the `None`
/// it read as, so it is not read again either.
#[derive(Debug, Default)]
pub struct ParsedBatch {
    /// Each event read as a JSON document, for the `JSON` rules; empty when no rule reads that
    /// format.
    json: Vec<Option<LogRecord>>,
    /// Each event read as a space-separated record, for the `CLF` rules; empty when no rule
    /// reads that format.
    clf: Vec<Option<LogRecord>>,
}

impl ParsedBatch {
    /// Read `messages` in each format `definitions` use.
    ///
    /// Call it with NO store guard held: this is the reading, and it needs nothing from the
    /// store but the definitions it was given.
    #[must_use]
    pub fn of(messages: &[(i64, String)], definitions: &[LogRuleDefinition]) -> Self {
        let uses = |format: LogFormat| {
            definitions
                .iter()
                .any(|definition| definition.log_format == format)
        };
        Self {
            json: if uses(LogFormat::Json) {
                messages
                    .iter()
                    .map(|(_, message)| {
                        ::serde_json::from_str(message).ok().map(LogRecord::Json)
                    })
                    .collect()
            } else {
                Vec::new()
            },
            clf: if uses(LogFormat::Clf) {
                messages
                    .iter()
                    .map(|(_, message)| {
                        Some(LogRecord::Clf(
                            message.split_whitespace().map(ToString::to_string).collect(),
                        ))
                    })
                    .collect()
            } else {
                Vec::new()
            },
        }
    }

    /// The event at `index` as a record of `format`, or `None` when the message is not one.
    #[must_use]
    pub fn record(&self, format: LogFormat, index: usize) -> Option<&LogRecord> {
        match format {
            LogFormat::Json => self.json.get(index)?.as_ref(),
            LogFormat::Clf => self.clf.get(index)?.as_ref(),
        }
    }
}

impl LogRuleDefinition {
    /// The rule `definition` describes, or `None` for a document that is not a log rule this
    /// evaluation can read — a managed rule's definition, which names no log group and no
    /// contribution, among them.
    #[must_use]
    pub fn parse(definition: &str) -> Option<Self> {
        let document: Value = ::serde_json::from_str(definition).ok()?;
        let rule = document.as_object()?;

        let log_format = match rule.get("LogFormat").and_then(Value::as_str)? {
            LOG_FORMAT_JSON => LogFormat::Json,
            LOG_FORMAT_CLF => LogFormat::Clf,
            _ => return None,
        };

        let mut log_group_names: Vec<String> = rule
            .get("LogGroupNames")
            .and_then(Value::as_array)
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(ToString::to_string))
                    .collect()
            })
            .unwrap_or_default();
        log_group_names.extend(
            rule.get("LogGroupARNArray")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|arn| arn.as_str().and_then(log_group_name_of_arn)),
        );
        if log_group_names.is_empty() {
            return None;
        }

        let contribution = rule.get("Contribution").and_then(Value::as_object)?;
        let keys: Vec<String> = contribution
            .get("Keys")
            .and_then(Value::as_array)?
            .iter()
            .filter_map(|key| key.as_str().map(ToString::to_string))
            .collect();
        if keys.is_empty() {
            return None;
        }

        Some(Self {
            log_group_names,
            log_format,
            fields: clf_fields(rule.get("Fields")),
            keys,
            value_of: contribution
                .get("ValueOf")
                .and_then(Value::as_str)
                .filter(|value_of| !value_of.is_empty())
                .map(ToString::to_string),
            filters: contribution
                .get("Filters")
                .and_then(Value::as_array)
                .map(|filters| filters.iter().filter_map(Filter::parse).collect())
                .unwrap_or_default(),
            aggregate_on: rule
                .get("AggregateOn")
                .and_then(Value::as_str)
                .filter(|aggregate_on| AGGREGATE_ON.contains(aggregate_on))
                .unwrap_or(AGGREGATE_ON_COUNT)
                .to_string(),
        })
    }

    /// Whether this rule collects from the log group named `log_group_name`.
    #[must_use]
    pub fn selects_log_group(&self, log_group_name: &str) -> bool {
        self.log_group_names
            .iter()
            .any(|selected| selected == log_group_name)
    }

    /// The observation the log event `record` came from contributes to this rule, or `None`
    /// when the event contributes nothing: a filter rejects it, a contribution key is a field
    /// it does not carry, or a summing rule finds no number in the field it sums.
    ///
    /// `record` is the event read ONCE for the whole ingest ([`ParsedBatch`]), so a batch is
    /// not re-read for each rule collecting from the log group it arrived in.
    #[must_use]
    pub fn observation(
        &self,
        record: &LogRecord,
        timestamp_ms: i64,
    ) -> Option<ContributorObservation> {
        if !self.filters.iter().all(|filter| filter.admits(self, record)) {
            return None;
        }
        let mut keys = Vec::with_capacity(self.keys.len());
        for key in &self.keys {
            keys.push(self.field(record, key)?);
        }
        let value = if self.aggregate_on == AGGREGATE_ON_SUM {
            let summed = self.field(record, self.value_of.as_deref()?)?;
            summed.parse::<f64>().ok().filter(|value| value.is_finite())?
        } else {
            // "If this rule aggregates by COUNT, the top contributor for each data point is
            // the contributor with the most occurrences in that period."
            // (`GetInsightRuleReportInput$Metrics`) — one matched event is one occurrence.
            1.0
        };
        Some(ContributorObservation {
            timestamp_ms,
            keys,
            value,
        })
    }

    /// The value `record` carries for the field `selector` addresses, as the text a
    /// contributor key is made of, or `None` when the record does not carry it.
    fn field(&self, record: &LogRecord, selector: &str) -> Option<String> {
        match record {
            LogRecord::Json(document) => json_field(document, selector).and_then(scalar_text),
            LogRecord::Clf(fields) => {
                let position = self.clf_position(selector)?;
                fields.get(position.checked_sub(1)?).cloned()
            }
        }
    }

    /// The 1-based field position `selector` addresses in a `CLF` record: a position written
    /// directly (`$4`, `4`), or the name the definition's `Fields` gives one.
    fn clf_position(&self, selector: &str) -> Option<usize> {
        let direct = selector.strip_prefix('$').unwrap_or(selector);
        if let Ok(position) = direct.parse::<usize>() {
            return Some(position);
        }
        self.fields
            .iter()
            .find(|(_, name)| name.as_str() == selector)
            .map(|(position, _)| *position)
    }
}

impl Filter {
    /// The filter `entry` states, or `None` when the entry states no test — a `Match` with no
    /// operation beside it constrains nothing, so it admits every event rather than none.
    fn parse(entry: &Value) -> Option<Self> {
        let entry = entry.as_object()?;
        let match_field = entry.get("Match").and_then(Value::as_str)?.to_string();
        let texts = |member: &str| {
            entry
                .get(member)
                .and_then(Value::as_array)
                .map(|values| values.iter().filter_map(scalar_text).collect::<Vec<_>>())
        };
        let number = |member: &str| entry.get(member).and_then(Value::as_f64);
        let test = if let Some(values) = texts("In") {
            FilterTest::In(values)
        } else if let Some(values) = texts("NotIn") {
            FilterTest::NotIn(values)
        } else if let Some(values) = texts("StartsWith") {
            FilterTest::StartsWith(values)
        } else if let Some(bound) = number("GreaterThan") {
            FilterTest::GreaterThan(bound)
        } else if let Some(bound) = number("LessThan") {
            FilterTest::LessThan(bound)
        } else if let Some(bound) = number("EqualTo") {
            FilterTest::EqualTo(bound)
        } else if let Some(bound) = number("NotEqualTo") {
            FilterTest::NotEqualTo(bound)
        } else {
            // The last test the syntax defines: an entry carrying none of them states no
            // test at all, which is the `None` this reading answers with.
            FilterTest::IsPresent(entry.get("IsPresent").and_then(Value::as_bool)?)
        };
        Some(Self { match_field, test })
    }

    /// Whether this filter admits the event `record` came from.
    ///
    /// A test other than `IsPresent(false)` needs a value to read, so an event that does not
    /// carry the field is not admitted by it: the filter selects events whose field has the
    /// property it names, and an absent field has no property.
    fn admits(&self, definition: &LogRuleDefinition, record: &LogRecord) -> bool {
        let value = definition.field(record, &self.match_field);
        match (&self.test, value) {
            (FilterTest::IsPresent(present), value) => *present == value.is_some(),
            (_, None) => false,
            (FilterTest::In(values), Some(value)) => values.contains(&value),
            (FilterTest::NotIn(values), Some(value)) => !values.contains(&value),
            (FilterTest::StartsWith(prefixes), Some(value)) => prefixes
                .iter()
                .any(|prefix| value.starts_with(prefix.as_str())),
            (FilterTest::GreaterThan(bound), Some(value)) => {
                value.parse::<f64>().is_ok_and(|value| value > *bound)
            }
            (FilterTest::LessThan(bound), Some(value)) => {
                value.parse::<f64>().is_ok_and(|value| value < *bound)
            }
            (FilterTest::EqualTo(bound), Some(value)) => {
                // Exact equality of the two numbers: the filter names a value a log field is
                // expected to CARRY, not a neighbourhood of it.
                value.parse::<f64>().is_ok_and(|value| value == *bound)
            }
            (FilterTest::NotEqualTo(bound), Some(value)) => {
                value.parse::<f64>().is_ok_and(|value| value != *bound)
            }
        }
    }
}

/// The `Fields` a `CLF` definition declares: the name each 1-based field position is addressed
/// by, as the rule syntax writes them (`"Fields": {"4": "IpAddress"}`).
fn clf_fields(fields: Option<&Value>) -> BTreeMap<usize, String> {
    fields
        .and_then(Value::as_object)
        .map(|fields| {
            fields
                .iter()
                .filter_map(|(position, name)| {
                    Some((position.parse::<usize>().ok()?, name.as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The log group a `LogGroupARNArray` entry addresses: the resource an ARN of the form
/// `arn:<partition>:logs:<region>:<account>:log-group:<name>` names, without the `:*` suffix
/// such an ARN may carry.
fn log_group_name_of_arn(arn: &str) -> Option<String> {
    // Read through this service's ONE ARN parse, so a log group named by ARN is split exactly
    // the way every other ARN this service reads is.
    let resource = super::parse_resource_arn(arn)?.resource;
    let name = resource.strip_prefix("log-group:")?;
    Some(name.strip_suffix(":*").unwrap_or(name).to_string())
}

/// One JSON document member, addressed the way a `JSON` rule's key addresses it: `$.ip`,
/// `$.request.path` for a nested member, and `$.hops[0]` for one inside an array.
///
/// The path is read and evaluated by the ONE bounded reference-path evaluation every service
/// whose API takes such a path shares (`service_common::json_reference_path`), so a key that
/// asks for more than one place — a wildcard, a descendant, a filter — names nothing here
/// rather than walking the whole document.
fn json_field<'document>(document: &'document Value, selector: &str) -> Option<&'document Value> {
    ::service_common::json_reference_path::select(selector, document)
}

/// One JSON value as the text a contributor key or a filter comparison is made of.
///
/// Only a scalar is one: a key whose value is an object or an array identifies no
/// contributor, so the event contributes nothing rather than a contributor keyed by a
/// rendered structure.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// What a report is asked for: the window, the period its datapoints are aggregated over, how
/// many contributors it carries and how they are ranked, and which per-datapoint statistics it
/// reports.
#[derive(Debug, Clone)]
pub struct ReportRequest {
    /// The start of the window, in epoch seconds, inclusive.
    pub start_secs: f64,
    /// The end of the window, in epoch seconds, exclusive.
    pub end_secs: f64,
    /// "The period, in seconds, to use for the statistics in the
    /// InsightRuleMetricDatapoint results." (`GetInsightRuleReportInput$Period`)
    pub period_secs: i64,
    /// How many contributors the report carries at most.
    pub max_contributors: usize,
    /// Aggregate and rank each contributor by the largest value any one period of the window
    /// holds for it, rather than by its total over the whole window ([`ORDER_BY`],
    /// [`contributor_aggregate`]).
    pub order_by_maximum: bool,
    /// The statistics each datapoint reports ([`INSIGHT_RULE_METRICS`]); a report asked for
    /// none carries no series, since there would be nothing in it to read.
    pub metrics: Vec<String>,
}

/// The report one rule answers a [`ReportRequest`] with — the output members
/// `GetInsightRuleReport` reports, computed from the observations the rule collected.
#[derive(Debug, Clone, Default)]
pub struct InsightReport {
    /// The attributes of the rule the report carries a series for
    /// (`GetInsightRuleReportOutput$RuleAttributes`).
    ///
    /// A rule attribute is a property of the RULE, not a statistic of a datapoint: the
    /// statistics are what `Metrics` asks for ([`INSIGHT_RULE_METRICS`]) and they are reported
    /// per datapoint. A rule the emulator evaluates is a log rule
    /// ([`LogRuleDefinition`]), whose properties are the log groups it reads, the format it
    /// reads them in, the fields it classifies contributors by — reported as
    /// [`InsightReport::key_labels`] — and the contribution it aggregates; none of them is an
    /// attribute a report carries its own series for, so a report of one comes to NO rule
    /// attribute.
    ///
    /// That is an EMPTY series, which is not the same thing as a missing one: a report answers
    /// every series it reports, so a caller can read, length and iterate this one and learn
    /// that the report has no attribute, rather than finding the member absent and being
    /// unable to tell "none" from "not answered".
    pub rule_attributes: Vec<String>,
    /// The strings used as the keys for the rule, which are the fields it classifies
    /// contributors by.
    pub key_labels: Vec<String>,
    /// The statistic the rule's aggregation computes ([`aggregation_statistic`]): every rule
    /// has one, since a rule whose definition names no aggregation counts.
    pub aggregation_statistic: Option<String>,
    /// "The sum of the values from all individual contributors that match the rule."
    /// (`GetInsightRuleReportOutput$AggregateValue`)
    pub aggregate_value: f64,
    /// "An approximate count of the unique contributors found by this rule in this time
    /// period." (`GetInsightRuleReportOutput$ApproximateUniqueCount`) — every contributor the
    /// window holds, not only the ones the report carries.
    pub approximate_unique_count: i64,
    /// The top contributors, each carrying the aggregate the request's ranking statistic comes
    /// to for it, ranked by it and cut to the number the request asked for.
    pub contributors: Vec<InsightRuleContributor>,
    /// The time series of per-period aggregates, ascending by timestamp.
    pub metric_datapoints: Vec<InsightRuleMetricDatapoint>,
}

/// What one period of the window holds: the observations that fell in it, folded.
#[derive(Debug, Clone, Default)]
struct PeriodAggregate {
    /// How many observations the period holds.
    sample_count: f64,
    /// Their total.
    sum: f64,
    /// The smallest single observation.
    minimum: f64,
    /// The largest single observation.
    maximum: f64,
    /// The total each contributor reached in this period, which is what the top contributor's
    /// value for it is the largest of.
    per_contributor: BTreeMap<Vec<String>, f64>,
}

/// The report `rule` answers `request` with.
///
/// PURE: it reads the rule's observations and writes nothing, so two reports of the same window
/// are the same report and neither changes what a later caller sees. Its cost is the rule's
/// observations plus the periods the window spans, and both are bounded
/// ([`MAX_RULE_OBSERVATIONS`], [`MAX_REPORTED_PERIODS`]).
#[must_use]
pub fn report(rule: &StoredInsightRule, request: &ReportRequest) -> InsightReport {
    // The definition as the write that stored the rule read it, so the definition TEXT has one
    // reading and a report does not re-read it. A rule whose definition is not a log rule this
    // evaluation reads classifies no contributor and has collected nothing, so it reports no
    // key and no contributor — but still the series its window spans.
    let definition = rule.collection.as_ref();
    // The other kind of collection a rule can hold: one a service minted for a RESOURCE
    // (`super::resource_insight_rules`), which classifies its contributors by the key labels it
    // was minted with and aggregates the measure it collects. The fold below is the same either
    // way; only where the rule's keys and its aggregation come from differs.
    let resource = rule.resource.as_ref();
    let period = request.period_secs.max(1);

    let mut periods: BTreeMap<i64, PeriodAggregate> = BTreeMap::new();
    let mut contributors: BTreeMap<Vec<String>, BTreeMap<i64, f64>> = BTreeMap::new();
    let mut aggregate_value = 0.0;
    for observation in &rule.observations {
        let at_secs = observation.timestamp_ms as f64 / 1_000.0;
        if at_secs < request.start_secs || at_secs >= request.end_secs {
            continue;
        }
        let bucket = period_start(at_secs, period);
        let aggregate = periods.entry(bucket).or_default();
        if aggregate.sample_count == 0.0 {
            aggregate.minimum = observation.value;
            aggregate.maximum = observation.value;
        } else {
            aggregate.minimum = aggregate.minimum.min(observation.value);
            aggregate.maximum = aggregate.maximum.max(observation.value);
        }
        aggregate.sample_count += 1.0;
        aggregate.sum += observation.value;
        *aggregate
            .per_contributor
            .entry(observation.keys.clone())
            .or_default() += observation.value;
        *contributors
            .entry(observation.keys.clone())
            .or_default()
            .entry(bucket)
            .or_default() += observation.value;
        aggregate_value += observation.value;
    }

    let aggregate_on = match resource {
        // A rule minted for a resource aggregates the measure each access contributed, so what
        // a datapoint of it reports is the sum of those values.
        Some(_) => AGGREGATE_ON_SUM,
        None => definition.map_or(AGGREGATE_ON_COUNT, |definition| {
            definition.aggregate_on.as_str()
        }),
    };
    InsightReport {
        // No rule attribute, since a log rule declares none
        // ([`InsightReport::rule_attributes`]) — an empty series, which the report still
        // answers.
        rule_attributes: Vec::new(),
        key_labels: resource
            .map(|collection| collection.key_labels.clone())
            .or_else(|| definition.map(|definition| definition.keys.clone()))
            .unwrap_or_default(),
        aggregation_statistic: Some(aggregation_statistic(aggregate_on).to_string()),
        aggregate_value,
        approximate_unique_count: i64::try_from(contributors.len()).unwrap_or(i64::MAX),
        contributors: ranked_contributors(&contributors, request),
        metric_datapoints: metric_datapoints(&periods, request),
    }
}

/// The contributors the report carries: each with the aggregate the request's ranking statistic
/// comes to for it ([`contributor_aggregate`]), ranked by that aggregate and cut to the number
/// the request asked for.
///
/// Contributors that rank equally are ordered by their keys, so two reports of the same window
/// list them in the same order rather than in whichever order the walk happened to take.
fn ranked_contributors(
    contributors: &BTreeMap<Vec<String>, BTreeMap<i64, f64>>,
    request: &ReportRequest,
) -> Vec<InsightRuleContributor> {
    let mut ranked: Vec<(f64, &Vec<String>, &BTreeMap<i64, f64>)> = contributors
        .iter()
        .map(|(keys, datapoints)| (contributor_aggregate(datapoints, request), keys, datapoints))
        .collect();
    // Descending by rank — the top contributor first — and by keys within one rank, which the
    // source map already holds them in.
    ranked.sort_by(|left, right| right.0.total_cmp(&left.0).then_with(|| left.1.cmp(right.1)));
    ranked.truncate(request.max_contributors);
    ranked
        .into_iter()
        .map(|(aggregate, keys, datapoints)| InsightRuleContributor {
            keys: keys.clone(),
            approximate_aggregate_value: aggregate,
            datapoints: datapoints
                .iter()
                .map(|(bucket, value)| InsightRuleContributorDatapoint {
                    #[allow(clippy::cast_precision_loss)]
                    timestamp: super::to_timestamp(*bucket as f64),
                    approximate_value: *value,
                })
                .collect(),
        })
        .collect()
}

/// What the request's ranking statistic aggregates one contributor's per-period values to: the
/// total over the whole window under `Sum`, and the largest value the contributor reached in
/// any ONE period under `Maximum` ([`ORDER_BY`]).
///
/// This is both the value the contributor is RANKED by and the one the report states for it —
/// "an approximation of the aggregate value that comes from this contributor"
/// (`InsightRuleContributor$ApproximateAggregateValue`): the aggregate a report reports is the
/// one the statistic it was asked to rank by computes, which is what lets the two rankings
/// disagree — a contributor with the larger window total may hold the smaller single-period
/// peak, so it ranks below a spikier one under `Maximum` and above it under `Sum`.
fn contributor_aggregate(datapoints: &BTreeMap<i64, f64>, request: &ReportRequest) -> f64 {
    if request.order_by_maximum {
        // A contributor's value may be negative, so this is the largest of the periods it
        // appears in rather than the largest of those and zero; a contributor the report
        // carries appears in at least one.
        datapoints.values().copied().reduce(f64::max).unwrap_or(0.0)
    } else {
        datapoints.values().sum()
    }
}

/// The series the report carries: one datapoint per `Period` of the window, ascending by
/// timestamp, each carrying exactly the statistics the request asked for.
///
/// "A time series of metric data points that matches the time period in the rule request."
/// (`GetInsightRuleReportOutput$MetricDatapoints`) — so the series is the WINDOW's periods and
/// not only the ones that hold an observation: a period that holds none is still a period of
/// the window, reported with each statistic asked for at zero, which is what gives a client
/// charting the report an x-axis over the whole window.
///
/// A request that asked for no statistic gets no series: "You can also optionally return one
/// or more statistics about each data point in the time series." (`GetInsightRuleReport`), so a
/// datapoint with no statistic on it would report nothing.
fn metric_datapoints(
    periods: &BTreeMap<i64, PeriodAggregate>,
    request: &ReportRequest,
) -> Vec<InsightRuleMetricDatapoint> {
    let asked: BTreeSet<&str> = request.metrics.iter().map(String::as_str).collect();
    if asked.is_empty() {
        return Vec::new();
    }
    // The period a datapoint of the window holds nothing in: every statistic of it is the
    // aggregate of no observation.
    let nothing_observed = PeriodAggregate::default();
    window_periods(request)
        .map(|bucket| {
            let aggregate = periods.get(&bucket).unwrap_or(&nothing_observed);
            let statistic = |name: &str, value: f64| asked.contains(name).then_some(value);
            #[allow(clippy::cast_precision_loss)]
            let unique_contributors = aggregate.per_contributor.len() as f64;
            // The largest total any ONE contributor reached in the period, and zero for a
            // period no contributor appeared in — a contributor's value may be negative, so
            // this is the largest of what is there rather than the largest of that and zero.
            let max_contributor_value = aggregate
                .per_contributor
                .values()
                .copied()
                .reduce(f64::max)
                .unwrap_or(0.0);
            // The mean of no observation is zero rather than the undefined value a division
            // by the empty count would be.
            let average = if aggregate.sample_count == 0.0 {
                0.0
            } else {
                aggregate.sum / aggregate.sample_count
            };
            InsightRuleMetricDatapoint {
                #[allow(clippy::cast_precision_loss)]
                timestamp: super::to_timestamp(bucket as f64),
                unique_contributors: statistic(UNIQUE_CONTRIBUTORS_METRIC, unique_contributors),
                max_contributor_value: statistic(
                    MAX_CONTRIBUTOR_VALUE_METRIC,
                    max_contributor_value,
                ),
                sample_count: statistic(SAMPLE_COUNT_METRIC, aggregate.sample_count),
                average: statistic(AVERAGE_METRIC, average),
                sum: statistic(SUM_METRIC, aggregate.sum),
                minimum: statistic(MINIMUM_METRIC, aggregate.minimum),
                maximum: statistic(MAXIMUM_METRIC, aggregate.maximum),
            }
        })
        .collect()
}

/// Every period of the window the report was asked over, by its start in epoch seconds,
/// ascending.
///
/// The periods are ANCHORED at multiples of the period, the same anchoring an observation is
/// bucketed by ([`period_start`]), so the series holds exactly the buckets an observation of
/// the window can fall in — which is what makes it strictly ascending with no instant repeated,
/// and the instant a datapoint is reported at decided by the clock rather than by when the
/// report was asked for. As many as [`reported_period_count`] counts, which is the number
/// `GetInsightRuleReport` holds against [`MAX_REPORTED_PERIODS`] before it answers at all, so
/// the `take` below only ever bounds a report nothing asked for.
fn window_periods(request: &ReportRequest) -> impl Iterator<Item = i64> + '_ {
    let period = request.period_secs.max(1);
    let first = period_start(request.start_secs, period);
    let end_secs = request.end_secs;
    (0..)
        .map(move |index: i64| first.saturating_add(index.saturating_mul(period)))
        .take_while(move |bucket| {
            #[allow(clippy::cast_precision_loss)]
            let at = *bucket as f64;
            at < end_secs
        })
        .take(MAX_REPORTED_PERIODS)
}

/// How many datapoints a report over `[start_secs, end_secs)` carries at a `period_secs`-wide
/// period: the periods [`window_periods`] enumerates, counted.
///
/// Counted from the ANCHORED start, the same instant the series opens at, so what
/// `GetInsightRuleReport` holds against [`MAX_REPORTED_PERIODS`] is exactly the series it would
/// answer with — a window that is not period-aligned opens one period before its own start,
/// and counting it from the un-anchored start would let a request one datapoint over the cap be
/// served with its last datapoint cut off.
#[must_use]
pub fn reported_period_count(start_secs: f64, end_secs: f64, period_secs: i64) -> f64 {
    let period = period_secs.max(1);
    #[allow(clippy::cast_precision_loss)]
    let anchored_start = period_start(start_secs, period) as f64;
    #[allow(clippy::cast_precision_loss)]
    let width = period as f64;
    ((end_secs - anchored_start) / width).ceil()
}

/// How many `Period`-wide datapoints ONE report carries, and so how many periods a report may
/// be asked over.
///
/// EMULATOR CEILING on the RATIO the caller chooses: the series is one datapoint per period of
/// the window, so the window divided by the `Period` is what the answer's size is proportional
/// to, and bounding each of them on its own (the model bounds `Period` only as `range 1..`)
/// does not bound that ratio. The number is the one CloudWatch publishes for the same kind of
/// read — "A GetMetricData operation that does not include a query can retrieve as many as 500
/// different metrics in a single request, with a total of as many as 100,800 data points."
/// (`GetMetricData`) — so a report a client could really ask for is still served. A request
/// asking for more is refused by `GetInsightRuleReport` rather than answered with a cut series.
pub const MAX_REPORTED_PERIODS: usize = 100_800;

/// The start of the `period`-second window `at_secs` falls in, in epoch seconds.
///
/// The windows are ANCHORED at multiples of the period, the same anchoring this service
/// aggregates metric datapoints into periods with, so the instant a datapoint is reported at is
/// decided by the observation's own time and not by when the report was asked for.
///
/// TOTAL over every instant a request can name: the cast saturates, so an instant outside the
/// epoch seconds an `i64` holds lands on the bound rather than wrapping, and the anchoring is
/// taken off saturatingly — the window the smallest representable instant falls in opens at that
/// bound, since no window opens further back than it. A caller's `StartTime` reaches here
/// unchanged ([`reported_period_count`]), so an arithmetic that was not total would be an
/// overflow a single request could ask for.
#[must_use]
pub fn period_start(at_secs: f64, period: i64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let secs = at_secs.floor() as i64;
    secs.saturating_sub(secs.rem_euclid(period.max(1)))
}

impl State {
    /// The rules that are COLLECTING from `log_group_name`, each with the definition it
    /// collects under: an ENABLED rule — "The state of the rule. Valid values are ENABLED and
    /// DISABLED." (`PutInsightRuleInput$RuleState`), and a rule that is not evaluating log
    /// events collects nothing from them — whose definition selects that log group.
    ///
    /// Cheap and read-only, so the reading of the batch and its evaluation can happen with no
    /// guard on this store held. Nothing is READ here: a rule's definition was read into the
    /// form this evaluation needs by the write that stored it
    /// ([`StoredInsightRule::collection`]), so the walk is a state check and a name comparison
    /// per rule. At most [`MAX_COLLECTING_RULES_PER_INGEST`] rules answer one ingest, which is
    /// the first factor of its cost.
    #[must_use]
    pub fn rules_collecting_from(&self, log_group_name: &str) -> Vec<(String, LogRuleDefinition)> {
        let mut collecting: Vec<(String, LogRuleDefinition)> = Vec::new();
        for (name, rule) in &self.insight_rules {
            if collecting.len() >= MAX_COLLECTING_RULES_PER_INGEST {
                break;
            }
            if rule.rule.state != super::INSIGHT_RULE_ENABLED {
                continue;
            }
            let Some(definition) = &rule.collection else {
                continue;
            };
            if definition.selects_log_group(log_group_name) {
                collecting.push((name.clone(), definition.clone()));
            }
        }
        collecting
    }

    /// Keep what one ingest's evaluation attributed to each rule, inside this store's ceilings.
    ///
    /// `collected` is `(rule name, the observations that rule's definition produced)`, computed
    /// OUTSIDE this store's guard ([`evaluate_ingested_log_events`]), so the critical section is
    /// the append and the eviction below and nothing else.
    pub fn keep_contributor_observations(
        &mut self,
        collected: &[(String, Vec<ContributorObservation>)],
    ) {
        for (name, observations) in collected {
            if observations.is_empty() {
                continue;
            }
            if let Some(rule) = self.insight_rules.get_mut(name) {
                rule.observations.extend(observations.iter().cloned());
            }
        }
        self.evict_contributor_observations();
    }

    /// Hold the observations this store retains inside BOTH ceilings: what one rule may keep
    /// ([`MAX_RULE_OBSERVATIONS`]), narrowed to its share of what the whole store may keep
    /// ([`MAX_STORED_OBSERVATIONS`]) when several rules are collecting.
    ///
    /// Without the second ceiling the retained footprint would be the PRODUCT of the per-rule
    /// ceiling and the number of rules a caller may create, which is not a bound. What goes is
    /// each rule's OLDEST observations, the direction Contributor Insights data ages out in, so
    /// the recent window a report asks about stays exact.
    fn evict_contributor_observations(&mut self) {
        let holding = self
            .insight_rules
            .values()
            .filter(|rule| !rule.observations.is_empty())
            .count()
            .max(1);
        let per_rule = (MAX_STORED_OBSERVATIONS / holding).clamp(1, MAX_RULE_OBSERVATIONS);
        for rule in self.insight_rules.values_mut() {
            if rule.observations.len() > per_rule {
                let excess = rule.observations.len() - per_rule;
                rule.observations.drain(..excess);
            }
        }
    }
}

/// What the rules collecting from one log group attribute to themselves out of one ingested
/// batch: `(rule name, its observations)`.
///
/// PURE, and computed with no store guard held. The cost of one ingest is bounded HERE rather
/// than by each factor's own ceiling: the batch is read once per log FORMAT ([`ParsedBatch`]),
/// and at most [`MAX_EVALUATIONS_PER_INGEST`] (rule, event) pairs are evaluated, so neither the
/// number of rules a caller created nor the size of the batch it sent multiplies into unbounded
/// work. A pair past the budget is not evaluated, which costs those events their contribution
/// and nothing else.
#[must_use]
pub fn evaluate_ingested_log_events(
    collecting: &[(String, LogRuleDefinition)],
    events: &[(i64, String)],
) -> Vec<(String, Vec<ContributorObservation>)> {
    let definitions: Vec<LogRuleDefinition> = collecting
        .iter()
        .map(|(_, definition)| definition.clone())
        .collect();
    let batch = ParsedBatch::of(events, &definitions);
    let mut budget = MAX_EVALUATIONS_PER_INGEST;
    let mut collected: Vec<(String, Vec<ContributorObservation>)> =
        Vec::with_capacity(collecting.len());
    for (name, definition) in collecting {
        let mut observations: Vec<ContributorObservation> = Vec::new();
        for (index, (timestamp_ms, _)) in events.iter().enumerate() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            if let Some(record) = batch.record(definition.log_format, index) {
                if let Some(observation) = definition.observation(record, *timestamp_ms) {
                    observations.push(observation);
                }
            }
        }
        collected.push((name.clone(), observations));
        if budget == 0 {
            break;
        }
    }
    collected
}

/// Evaluate the log events just ingested into `log_group_name` against the Contributor Insights
/// rules of the CloudWatch store the shared engine holds.
///
/// The seam the CloudWatch LOGS side of this emulator calls on ingest: a Contributor Insights
/// rule is CloudWatch's resource, so the evaluation and everything it collects live here, in
/// the one store `PutInsightRule` writes and `GetInsightRuleReport` reads. Nothing is collected
/// when that store is not mounted. Call it with no log-store guard held, so the two services'
/// stores are never locked at once.
///
/// The CloudWatch store's own guard is taken TWICE and briefly — once to read which rules
/// collect from the group, once to keep what they attributed — with the reading of the batch and
/// the whole evaluation in between, under no guard at all: a batch's evaluation holds nothing
/// up, and is bounded by [`MAX_EVALUATIONS_PER_INGEST`] besides.
pub fn record_log_events<Eng: Send + Sync + 'static>(
    engine: &Eng,
    log_group_name: &str,
    events: &[(i64, String)],
) {
    if events.is_empty() {
        return;
    }
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let collecting = state.lock().rules_collecting_from(log_group_name);
    if collecting.is_empty() {
        return;
    }
    let collected = evaluate_ingested_log_events(&collecting, events);
    state.lock().keep_contributor_observations(&collected);
}
