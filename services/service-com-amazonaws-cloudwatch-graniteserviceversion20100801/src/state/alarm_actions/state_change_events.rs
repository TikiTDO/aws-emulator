// SPDX-License-Identifier: CC0-1.0

//! The EVENT an alarm state transition publishes, and where it is published.
//!
//! # The rule this module implements
//!
//! CloudWatch is an event SOURCE as well as a control plane a client polls: every change of an
//! alarm's `StateValue` — whichever call or clock materialised it, a metric alarm's evaluation,
//! a composite alarm's rule, or a state a caller forced with `SetAlarmState` — is published
//! onto the account's default event bus ([`DEFAULT_EVENT_BUS`]) under the `source` this service
//! owns ([`EVENT_SOURCE`]) and the `detail-type` of an alarm state change
//! ([`STATE_CHANGE_DETAIL_TYPE`]), with the alarm's own ARN in the event's `resources` and a
//! `detail` carrying the alarm's name, the state it moved TO, the state it moved FROM and the
//! configuration that was evaluated. A rule on that bus whose `EventPattern` matches then
//! delivers it to that rule's targets, which is how a transition starts a function, a queue or
//! a workflow without the client calling anything else.
//!
//! The event is NOT one of the alarm's configured actions, and nothing about the alarm's
//! actions governs it: an alarm carrying no action list at all, one whose `ActionsEnabled` is
//! false, a composite alarm whose actions are suppressed and a transition an alarm mute rule
//! mutes each publish the same event — "`ActionsEnabled`: indicates whether actions should be
//! executed during any changes to the alarm state" is about the ARNs the alarm names, and a
//! consumer of the bus is not one of them. What publishes nothing is a transition that did not
//! happen: an evaluation that computes the state the alarm already reports moves it nowhere,
//! and so has no state change to publish.
//!
//! # The payload a LAMBDA alarm action is handed
//!
//! The same facts are what a Lambda function configured as an alarm action receives
//! ([`AlarmStateChange::lambda_payload`]), which is why one record carries both: the function is
//! handed the transition under `alarmData`, beside the alarm's ARN, the account and Region it
//! belongs to and the instant it moved — so the two renderings of one transition cannot drift
//! apart.
//!
//! # How a transition becomes an event
//!
//! Nothing here decides WHAT happened: the evaluation or the write that moved the alarm does,
//! and the record it queued is what the event reports ([`super::AlarmNotification`]), so an
//! event reports the alarm a read of the same instant reports and never a second, invented copy
//! of it.
//!
//! Publication is made with NO store lock held: EventBridge is reached the way every emulated
//! service reaches another, through the framework's in-process callback
//! ([`::service_common::eventbridge_client`]), so this crate names no other service's crate. It
//! is best effort, as the delivery of an alarm action is — an emulator with no EventBridge
//! mounted has nowhere to publish — so a failure is logged and the alarm's own state stands.

use ::serde_json::{Map, Value};

use crate::ops::types::{CompositeAlarm, Dimension, MetricAlarm, MetricDataQuery};

/// The `source` of every event CloudWatch publishes: EventBridge names the publishing service
/// as `aws.<service>`, and this service's namespace is the one its ARNs carry.
pub const EVENT_SOURCE: &str = "aws.cloudwatch";

/// The `detail-type` of the event an alarm's state change publishes.
pub const STATE_CHANGE_DETAIL_TYPE: &str = "CloudWatch Alarm State Change";

/// The event bus this service publishes onto: the events an AWS service generates go to the
/// account's default event bus, which is the bus a rule carrying no `EventBusName` is created
/// on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many entries one `PutEvents` request carries: the bound the service this publishes to
/// declares (`PutEventsRequestEntryList` is `@length(min: 1, max: 10)`), so a settle that owes
/// more than that publishes them in several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// One alarm state transition, as the event it publishes and the payload a Lambda alarm action
/// is handed report it.
///
/// Built once, where the transition was materialised, and carried on the queued record of that
/// transition: both renderings read the same three documents, so the event a bus consumer
/// matches on and the payload a function is invoked with cannot disagree about one transition.
#[derive(Debug, Clone)]
pub struct AlarmStateChange {
    /// The alarm that moved, by name.
    pub alarm_name: String,
    /// That alarm's own ARN: the resource the event names in its `resources`, and the Region
    /// and account every other member of the payload is read off.
    pub alarm_arn: Option<String>,
    /// WHEN it moved — the instant the transition was materialised at, which is the event's own
    /// `time` and not the instant it happened to be published.
    pub at_secs: f64,
    /// `detail.state`: the state the alarm moved to, its explanation and the instant it moved.
    state: Value,
    /// `detail.previousState`: the state it moved out of, as the alarm reported it until then.
    previous_state: Value,
    /// `detail.configuration`: WHAT was evaluated — a metric alarm's metrics, a composite
    /// alarm's rule.
    configuration: Value,
}

impl AlarmStateChange {
    /// The transition of a METRIC alarm to the state it now reports.
    #[must_use]
    pub fn of_metric_transition(
        alarm: &MetricAlarm,
        previous: &crate::state::AlarmStateSnapshot,
        at_secs: f64,
    ) -> Option<Self> {
        Some(Self {
            alarm_name: alarm.alarm_name.clone()?,
            alarm_arn: alarm.alarm_arn.clone(),
            at_secs,
            state: state_document(
                alarm.state_value.as_deref()?,
                alarm.state_reason.as_deref(),
                alarm.state_reason_data.as_deref(),
                Some(at_secs),
            ),
            previous_state: previous_document(previous),
            configuration: metric_alarm_configuration(alarm),
        })
    }

    /// The transition of a COMPOSITE alarm. Same three documents; what differs is the
    /// configuration, which is the rule the alarm evaluated rather than a metric.
    #[must_use]
    pub fn of_composite_transition(
        alarm: &CompositeAlarm,
        previous: &crate::state::AlarmStateSnapshot,
        at_secs: f64,
    ) -> Option<Self> {
        Some(Self {
            alarm_name: alarm.alarm_name.clone()?,
            alarm_arn: alarm.alarm_arn.clone(),
            at_secs,
            state: state_document(
                alarm.state_value.as_deref()?,
                alarm.state_reason.as_deref(),
                alarm.state_reason_data.as_deref(),
                Some(at_secs),
            ),
            previous_state: previous_document(previous),
            configuration: composite_alarm_configuration(alarm),
        })
    }

    /// The event's `detail`: the alarm's name and the three documents the transition is made
    /// of.
    ///
    /// Public because it IS the contract — these are the members a rule's `EventPattern`
    /// filters a state change on.
    #[must_use]
    pub fn detail(&self) -> Value {
        let mut detail = Map::new();
        detail.insert(
            "alarmName".to_string(),
            Value::from(self.alarm_name.as_str()),
        );
        detail.insert("state".to_string(), self.state.clone());
        detail.insert("previousState".to_string(), self.previous_state.clone());
        detail.insert("configuration".to_string(), self.configuration.clone());
        Value::Object(detail)
    }

    /// The document a LAMBDA alarm action's function is invoked with: the transition under
    /// `alarmData`, beside the identity of the alarm that owes it.
    ///
    /// The Region and the account are the alarm's OWN, read off its ARN rather than guessed:
    /// the alarm was created in one Region and the invocation reports that one.
    #[must_use]
    pub fn lambda_payload(&self) -> Value {
        let parsed = self.alarm_arn.as_deref().and_then(::service_common::arn::parse);
        let mut payload = Map::new();
        payload.insert("source".to_string(), Value::from(EVENT_SOURCE));
        if let Some(arn) = self.alarm_arn.as_deref() {
            payload.insert("alarmArn".to_string(), Value::from(arn));
        }
        if let Some(account) = parsed.map(|arn| arn.account).filter(|it| !it.is_empty()) {
            payload.insert("accountId".to_string(), Value::from(account));
        }
        if let Some(time) = crate::state::document_instant(self.at_secs) {
            payload.insert("time".to_string(), Value::from(time));
        }
        if let Some(region) = parsed.map(|arn| arn.region).filter(|it| !it.is_empty()) {
            payload.insert("region".to_string(), Value::from(region));
        }
        payload.insert("alarmData".to_string(), self.detail());
        Value::Object(payload)
    }

    /// The Region the event is published in: the alarm's own, and `None` for an alarm whose ARN
    /// names none — there is no Region to address the publication in for such an alarm.
    #[must_use]
    pub fn region(&self) -> Option<&str> {
        self.alarm_arn
            .as_deref()
            .and_then(::service_common::arn::parse)
            .map(|arn| arn.region)
            .filter(|region| !region.is_empty())
    }
}

/// One alarm state as the transition documents report it: the value, its explanation and the
/// instant the alarm moved there. A member the alarm does not carry is left out rather than
/// reported as null.
fn state_document(
    value: &str,
    reason: Option<&str>,
    reason_data: Option<&str>,
    at_secs: Option<f64>,
) -> Value {
    let mut state = Map::new();
    state.insert("value".to_string(), Value::from(value));
    if let Some(reason) = reason {
        state.insert("reason".to_string(), Value::from(reason));
    }
    // `StateReasonData` is itself a JSON document the alarm carries as text ("the reason that
    // this alarm is set to this specific state, in JSON format"), and it travels as that text:
    // a consumer reads the member and parses it, as it does for the alarm itself.
    if let Some(reason_data) = reason_data {
        state.insert("reasonData".to_string(), Value::from(reason_data));
    }
    if let Some(timestamp) = at_secs.and_then(crate::state::document_instant) {
        state.insert("timestamp".to_string(), Value::from(timestamp));
    }
    Value::Object(state)
}

/// The state the alarm moved OUT OF, as it reported it until this transition — including the
/// instant it had moved there, which the snapshot carries.
///
/// An alarm with no state at all before the transition — one whose very first evaluation this
/// is — reports a previous state with no value, since there is no state it moved out of.
fn previous_document(previous: &crate::state::AlarmStateSnapshot) -> Value {
    let Some(value) = previous.state_value.as_deref() else {
        return Value::Object(Map::new());
    };
    state_document(
        value,
        previous.state_reason.as_deref(),
        previous.state_reason_data.as_deref(),
        previous.state_updated_secs,
    )
}

/// A METRIC alarm's `configuration`: its description and the metrics it evaluates.
fn metric_alarm_configuration(alarm: &MetricAlarm) -> Value {
    let mut configuration = Map::new();
    if let Some(description) = alarm.alarm_description.as_deref() {
        configuration.insert("description".to_string(), Value::from(description));
    }
    // "One item in the Metrics array is the expression that the alarm watches"
    // (`PutMetricAlarmInput$Metrics`): a query-array alarm reports its queries, and an alarm
    // configured with a single metric reports the one query that metric IS.
    let metrics = match alarm.metrics.as_deref() {
        Some(queries) => queries.iter().map(query_document).collect(),
        None => implicit_query_document(alarm).into_iter().collect(),
    };
    configuration.insert("metrics".to_string(), Value::Array(metrics));
    Value::Object(configuration)
}

/// A COMPOSITE alarm's `configuration`: the rule it evaluates, its description, and the
/// suppressor alarm its configuration names.
fn composite_alarm_configuration(alarm: &CompositeAlarm) -> Value {
    let mut configuration = Map::new();
    if let Some(description) = alarm.alarm_description.as_deref() {
        configuration.insert("description".to_string(), Value::from(description));
    }
    if let Some(rule) = alarm.alarm_rule.as_deref() {
        configuration.insert("alarmRule".to_string(), Value::from(rule));
    }
    if let Some(suppressor) = alarm.actions_suppressor.as_deref() {
        configuration.insert("actionsSuppressor".to_string(), Value::from(suppressor));
    }
    if let Some(period) = alarm.actions_suppressor_wait_period {
        configuration.insert(
            "actionsSuppressorWaitPeriod".to_string(),
            Value::from(period),
        );
    }
    if let Some(period) = alarm.actions_suppressor_extension_period {
        configuration.insert(
            "actionsSuppressorExtensionPeriod".to_string(),
            Value::from(period),
        );
    }
    Value::Object(configuration)
}

/// One `MetricDataQuery` of a query-array alarm, as the configuration document reports it:
/// every member the query carries, under the model's member name in lower camel case.
fn query_document(query: &MetricDataQuery) -> Value {
    let mut document = Map::new();
    document.insert("id".to_string(), Value::from(query.id.as_str()));
    if let Some(stat) = query.metric_stat.as_ref() {
        document.insert(
            "metricStat".to_string(),
            metric_stat_document(
                stat.metric.namespace.as_deref(),
                stat.metric.metric_name.as_deref(),
                stat.metric.dimensions.as_deref(),
                Some(stat.period),
                Some(stat.stat.as_str()),
                stat.unit.as_deref(),
            ),
        );
    }
    if let Some(expression) = query.expression.as_deref() {
        document.insert("expression".to_string(), Value::from(expression));
    }
    if let Some(label) = query.label.as_deref() {
        document.insert("label".to_string(), Value::from(label));
    }
    if let Some(period) = query.period {
        document.insert("period".to_string(), Value::from(period));
    }
    if let Some(account) = query.account_id.as_deref() {
        document.insert("accountId".to_string(), Value::from(account));
    }
    // "When used in GetMetricData, this option indicates whether to return the timestamps and
    // raw data values of this metric": a query the alarm watches returns data, so the member is
    // reported as the alarm carries it and defaulted to the documented default when it carries
    // none.
    document.insert(
        "returnData".to_string(),
        Value::from(query.return_data.unwrap_or(true)),
    );
    Value::Object(document)
}

/// The ONE query an alarm configured with a single metric evaluates, or `None` for an alarm
/// that names no metric at all (one configured with a query language instead).
fn implicit_query_document(alarm: &MetricAlarm) -> Option<Value> {
    let metric_name = alarm.metric_name.as_deref()?;
    let mut document = Map::new();
    document.insert("id".to_string(), Value::from(implicit_query_id(alarm)));
    document.insert(
        "metricStat".to_string(),
        metric_stat_document(
            alarm.namespace.as_deref(),
            Some(metric_name),
            alarm.dimensions.as_deref(),
            alarm.period,
            alarm
                .statistic
                .as_deref()
                .or(alarm.extended_statistic.as_deref()),
            alarm.unit.as_deref(),
        ),
    );
    document.insert("returnData".to_string(), Value::from(true));
    Some(Value::Object(document))
}

/// The id of the query a single-metric alarm's own metric IS.
///
/// Such an alarm names no query of its own — the ids in a configuration document are the
/// caller's only for an alarm configured with a `Metrics` array — so the one CloudWatch reports
/// for it is a generated identifier. It is DERIVED from the alarm's identity, through the same
/// digest this crate mints its page tokens with, so one alarm reports one id for as long as it
/// exists and two alarms report two: an id drawn fresh per transition would make a consumer
/// correlating the transitions of one alarm see a different query each time.
fn implicit_query_id(alarm: &MetricAlarm) -> String {
    use ::sha2::Digest as _;

    let mut digest = ::sha2::Sha256::new();
    for part in [
        alarm.alarm_arn.as_deref(),
        alarm.alarm_name.as_deref(),
        alarm.namespace.as_deref(),
        alarm.metric_name.as_deref(),
    ] {
        digest.update(part.unwrap_or_default().as_bytes());
        digest.update([0u8]);
    }
    // The first 16 bytes of the digest, rendered by the tree's ONE renderer of an identifier in
    // that form ([`::service_common::uuid_from_random_bytes`]), which owns both the version and
    // variant bits and the group boundaries.
    let mut bytes = [0u8; 16];
    for (byte, digested) in bytes.iter_mut().zip(digest.finalize()) {
        *byte = digested;
    }
    ::service_common::uuid_from_random_bytes(bytes)
}

/// One `MetricStat` as the configuration document reports it: the metric it retrieves, the
/// period it is aggregated over and the statistic it is aggregated with.
fn metric_stat_document(
    namespace: Option<&str>,
    metric_name: Option<&str>,
    dimensions: Option<&[Dimension]>,
    period: Option<i32>,
    stat: Option<&str>,
    unit: Option<&str>,
) -> Value {
    let mut metric = Map::new();
    if let Some(namespace) = namespace {
        metric.insert("namespace".to_string(), Value::from(namespace));
    }
    if let Some(metric_name) = metric_name {
        metric.insert("name".to_string(), Value::from(metric_name));
    }
    // The dimensions are an OBJECT here rather than the modelled list of name/value pairs:
    // that is how a state-change event states the metric's dimensions, one member per
    // dimension name.
    if let Some(dimensions) = dimensions {
        metric.insert(
            "dimensions".to_string(),
            Value::Object(
                dimensions
                    .iter()
                    .map(|dimension| {
                        (
                            dimension.name.clone(),
                            Value::from(dimension.value.as_str()),
                        )
                    })
                    .collect(),
            ),
        );
    }
    let mut document = Map::new();
    document.insert("metric".to_string(), Value::Object(metric));
    if let Some(period) = period {
        document.insert("period".to_string(), Value::from(period));
    }
    if let Some(stat) = stat {
        document.insert("stat".to_string(), Value::from(stat));
    }
    if let Some(unit) = unit {
        document.insert("unit".to_string(), Value::from(unit));
    }
    Value::Object(document)
}

/// `secs` as the instant an event's `time` carries, and `None` for one the documents of the
/// same transition cannot report either ([`crate::state::document_instant`]) — a non-finite or
/// out-of-range instant, which no clock of this store mints.
fn representable_instant(secs: f64) -> Option<::aws_sdk_eventbridge::primitives::DateTime> {
    crate::state::document_instant(secs)
        .map(|_| ::aws_sdk_eventbridge::primitives::DateTime::from_secs_f64(secs))
}

/// Publish the state changes `changes` reports onto the account's default EventBridge bus.
///
/// Called with NO store lock held, after the transitions have been materialised and queued: a
/// publication reaches EventBridge's own handler, which matches the event against the account's
/// rules and delivers it to their targets — and a target may call back into this service, so
/// holding the alarm store's lock across it would deadlock the very call the event set off.
///
/// One client per Region, because every bus of one Region is reached through the same endpoint
/// and a client built for another Region would address the wrong one. A change whose alarm ARN
/// names no Region is published nowhere and says so: there is no bus to address.
pub async fn publish(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    changes: &[AlarmStateChange],
) {
    let mut by_region: Vec<(&str, Vec<&AlarmStateChange>)> = Vec::new();
    for change in changes {
        let Some(region) = change.region() else {
            ::tracing::warn!(
                alarm = %change.alarm_name,
                "this alarm's ARN names no Region, so its state change is published onto no \
                 event bus"
            );
            continue;
        };
        match by_region.iter_mut().find(|(held, _)| *held == region) {
            Some((_, grouped)) => grouped.push(change),
            None => by_region.push((region, vec![change])),
        }
    }
    for (region, grouped) in by_region {
        // The Region ends up in the endpoint URL this hop is addressed on, so it is read under
        // the Region grammar every model states for the field rather than as arbitrary text.
        let Some(region) = ::service_common::arn::region_label(region) else {
            ::tracing::warn!(
                region = %region,
                "this is no Region an event bus can be addressed in, so the alarm state changes \
                 of its alarms are published onto none"
            );
            continue;
        };
        let client = ::service_common::eventbridge_client::eventbridge_client(callback, region);
        for window in grouped.chunks(MAX_PUT_EVENTS_ENTRIES) {
            let mut request = client.put_events();
            for change in window {
                let mut entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                    .event_bus_name(DEFAULT_EVENT_BUS)
                    .source(EVENT_SOURCE)
                    .detail_type(STATE_CHANGE_DETAIL_TYPE)
                    .detail(change.detail().to_string());
                // "The time stamp of the event, per RFC3339. If no time stamp is provided, the
                // time stamp of the PutEvents call is used" — the event's own instant is when
                // the alarm moved. It is held to the instant the documents of this same
                // transition report, so the two readings of one transition agree; an instant
                // those documents cannot represent is left to the documented default rather
                // than converted into a different one.
                if let Some(instant) = representable_instant(change.at_secs) {
                    entry = entry.time(instant);
                }
                // "Resources: AWS resources, identified by Amazon Resource Name (ARN), which
                // the event primarily concerns" — the alarm that moved.
                if let Some(arn) = change.alarm_arn.as_deref() {
                    entry = entry.resources(arn);
                }
                request = request.entries(entry.build());
            }
            match request.send().await {
                // A publication the bus did not take every entry of is accounted for the same
                // way as one it answered with a fault: the delivery is best effort, and what a
                // client reads is the alarm's own state either way — but a rejected entry is
                // why a rule saw nothing, so it is said.
                Ok(answer) if answer.failed_entry_count() > 0 => {
                    ::tracing::warn!(
                        region = %region,
                        rejected = answer.failed_entry_count(),
                        "the default event bus did not accept every alarm state change of this \
                         publication"
                    );
                }
                Ok(_) => {}
                Err(error) => {
                    ::tracing::warn!(
                        region = %region,
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                        "an alarm state change could not be published to EventBridge"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alarm every row below is built from: one metric, one dimension.
    fn alarm() -> MetricAlarm {
        MetricAlarm {
            alarm_name: Some("cpu-too-high".to_string()),
            alarm_arn: Some(
                "arn:aws:cloudwatch:us-west-2:000000000000:alarm:cpu-too-high".to_string(),
            ),
            alarm_description: Some("the CPU is too high".to_string()),
            namespace: Some("AWS/EC2".to_string()),
            metric_name: Some("CPUUtilization".to_string()),
            period: Some(300),
            statistic: Some("Average".to_string()),
            dimensions: Some(vec![Dimension {
                name: "InstanceId".to_string(),
                value: "i-0123456789abcdef0".to_string(),
            }]),
            state_value: Some(crate::state::ALARM_STATE_ALARM.to_string()),
            state_reason: Some("Threshold Crossed".to_string()),
            ..MetricAlarm::default()
        }
    }

    /// The detail a metric alarm's transition reports: the alarm's name, both states and the
    /// metric that was evaluated, with the dimensions as an object.
    #[test]
    fn a_metric_transition_reports_both_states_and_the_metric() {
        let previous = crate::state::AlarmStateSnapshot {
            state_value: Some(crate::state::ALARM_STATE_OK.to_string()),
            state_reason: Some("within the threshold".to_string()),
            state_reason_data: None,
            state_updated_secs: Some(1_800_000_000.0),
        };
        let change = AlarmStateChange::of_metric_transition(&alarm(), &previous, 1_800_000_060.0)
            .expect("an alarm carrying a name and a state owes a state change");
        let detail = change.detail();
        for (case, pointer, expected) in [
            ("the alarm's name", "/alarmName", Value::from("cpu-too-high")),
            ("the new state", "/state/value", Value::from("ALARM")),
            ("the previous state", "/previousState/value", Value::from("OK")),
            (
                "the metric's namespace",
                "/configuration/metrics/0/metricStat/metric/namespace",
                Value::from("AWS/EC2"),
            ),
            (
                "the metric's dimension",
                "/configuration/metrics/0/metricStat/metric/dimensions/InstanceId",
                Value::from("i-0123456789abcdef0"),
            ),
            (
                "the statistic",
                "/configuration/metrics/0/metricStat/stat",
                Value::from("Average"),
            ),
        ] {
            let found = detail.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected:?} at {pointer}, got {found:?} in {detail}"
            );
        }
    }

    /// The id a single-metric alarm's implicit query carries is the SAME on every transition of
    /// that alarm, and a different alarm's is a different one.
    #[test]
    fn the_implicit_query_id_is_stable_per_alarm() {
        let first = implicit_query_id(&alarm());
        let again = implicit_query_id(&alarm());
        assert_eq!(
            first, again,
            "case the same alarm twice: expected one id, got {first:?} and {again:?}"
        );
        let mut other = alarm();
        other.alarm_name = Some("cpu-too-low".to_string());
        other.alarm_arn =
            Some("arn:aws:cloudwatch:us-west-2:000000000000:alarm:cpu-too-low".to_string());
        let different = implicit_query_id(&other);
        assert_ne!(
            first, different,
            "case another alarm: expected a different id, got {different:?} for both"
        );
    }

    /// The payload a Lambda alarm action is handed carries the transition under `alarmData`,
    /// beside the alarm's ARN and the account and Region that ARN names.
    #[test]
    fn the_lambda_payload_names_the_alarm_and_its_account() {
        let change = AlarmStateChange::of_metric_transition(
            &alarm(),
            &crate::state::AlarmStateSnapshot::default(),
            1_800_000_060.0,
        )
        .expect("an alarm carrying a name and a state owes a state change");
        let payload = change.lambda_payload();
        for (case, pointer, expected) in [
            ("the source", "/source", Value::from(EVENT_SOURCE)),
            (
                "the alarm's ARN",
                "/alarmArn",
                Value::from("arn:aws:cloudwatch:us-west-2:000000000000:alarm:cpu-too-high"),
            ),
            ("the account", "/accountId", Value::from("000000000000")),
            ("the Region", "/region", Value::from("us-west-2")),
            (
                "the alarm's name",
                "/alarmData/alarmName",
                Value::from("cpu-too-high"),
            ),
            ("the new state", "/alarmData/state/value", Value::from("ALARM")),
        ] {
            let found = payload.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected:?} at {pointer}, got {found:?} in {payload}"
            );
        }
    }
}
