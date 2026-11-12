// SPDX-License-Identifier: CC0-1.0

//! The `AWS/AppSync` metrics an Event API's DATA plane publishes about its own work.
//!
//! A connection, a subscription and a published event leave no response a later caller can
//! read: nothing in the control plane reports that a subscriber is attached or that an
//! event was filtered out by a channel namespace's handler. The only way a client learns
//! any of it — a CloudWatch alarm, a dashboard, an on-call rule — is the series AppSync
//! publishes about itself, so this module publishes one datapoint per thing that happened,
//! as it happens.
//!
//! WHICH dimension set each metric lands under is part of the contract and is the reason
//! this module exists rather than one `publish` call per site:
//!
//! * what happens on a CHANNEL NAMESPACE — a subscribe, an unsubscribe, a publish and the
//!   events it carried — is reported under [`EVENT_API_ID_DIMENSION`] AND
//!   [`CHANNEL_NAMESPACE_NAME_DIMENSION`], because the quantity belongs to that namespace
//!   of that API;
//! * what happens on a CONNECTION — a connect, a disconnect, and the connection and
//!   subscription counts that follow — is reported under [`EVENT_API_ID_DIMENSION`] alone,
//!   because a connection is made to the API and belongs to no namespace: it exists before
//!   any channel is named and outlives every subscription on it.
//!
//! The datapoints are appended to the CloudWatch store directly rather than sent through
//! `PutMetricData`: CloudWatch reserves the `AWS/` namespaces for the services that own
//! them ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`), so a service's own metric never travels over that API. It
//! goes into the very store `ListMetrics`, `GetMetricStatistics` and `GetMetricData` read,
//! so one metric and its reader cannot disagree.
//!
//! The counts are never kept here. Each observation is a datapoint of what ONE call did,
//! so the total a client reads is the `Sum` CloudWatch computes over a period, and the two
//! GAUGES ([`ACTIVE_CONNECTIONS_METRIC`], [`ACTIVE_SUBSCRIPTIONS_METRIC`]) are published
//! with the count the store holds AT that moment, every time it changes AND on the cadence
//! the plane observes them at while a connection is open ([`super::gauges`]) — which is
//! what lets a reader see subscriptions fall back to zero, and STAY there, while a
//! connection stays open.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The CloudWatch store AppSync's own metrics live in: the CloudWatch service's OWN state
/// struct, reached through the SAME shared engine this service's store lives in.
pub type CloudWatchState =
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::ComAmazonawsCloudwatchGraniteserviceversion20100801State;

/// The CloudWatch store in the SAME shared engine, or `None` when the engine carries no
/// CloudWatch — a deployment with nothing to publish to.
pub fn cloudwatch_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<CloudWatchState>> {
    ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(engine)
}

/// The namespace AppSync publishes its own metrics to.
pub const METRIC_NAMESPACE: &str = "AWS/AppSync";

/// The dimension that names the Event API every one of these series is of.
pub const EVENT_API_ID_DIMENSION: &str = "EventAPIId";

/// The dimension that names the channel namespace a namespace-scoped series is of.
pub const CHANNEL_NAMESPACE_NAME_DIMENSION: &str = "ChannelNamespaceName";

/// One successful subscribe on a channel of the namespace.
pub const SUBSCRIBE_SUCCESS_METRIC: &str = "SubscribeSuccess";

/// One successful unsubscribe from a channel of the namespace.
pub const UNSUBSCRIBE_SUCCESS_METRIC: &str = "UnsubscribeSuccess";

/// One invocation of the namespace's `onPublish` handler.
pub const PUBLISH_HANDLER_INVOCATIONS_METRIC: &str = "PublishHandlerInvocations";

/// The events a publish carried that the namespace accepted — every published event,
/// including one its handler then filtered out: the publish of it succeeded.
pub const SUCCESSFUL_EVENTS_METRIC: &str = "SuccessfulEvents";

/// The events the namespace's handler did not return, i.e. the ones it filtered out, which
/// are therefore delivered to no subscriber.
pub const DROPPED_EVENTS_METRIC: &str = "DroppedEvents";

/// The events the namespace could not accept — one that is not an event document at all,
/// or every event of a publish whose handler failed.
pub const FAILED_EVENTS_METRIC: &str = "FailedEvents";

/// One realtime connection established on the API.
pub const CONNECT_SUCCESS_METRIC: &str = "ConnectSuccess";

/// One realtime connection closed.
pub const DISCONNECT_SUCCESS_METRIC: &str = "DisconnectSuccess";

/// How many realtime connections are open on the API at the moment of the observation.
pub const ACTIVE_CONNECTIONS_METRIC: &str = "ActiveConnections";

/// How many subscriptions those connections hold at the moment of the observation.
pub const ACTIVE_SUBSCRIPTIONS_METRIC: &str = "ActiveSubscriptions";

/// The `StandardUnit` a bare count of things carries: `Count`.
const COUNT_UNIT: &str = "Count";

/// The `StorageResolution` these datapoints are published at, in seconds: regular
/// resolution, "which CloudWatch stores at 1-minute resolution"
/// (`MetricDatum$StorageResolution`).
const STANDARD_STORAGE_RESOLUTION_SECONDS: i32 = 60;

/// What ONE publish did with the events it carried — each a tally of that one call's work,
/// never a running total kept here: the totals a client reads are the `Sum` CloudWatch
/// computes over the datapoints.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PublishCounts {
    /// How many times the namespace's `onPublish` handler ran for this publish.
    pub handler_invocations: u32,
    /// How many of the events the namespace accepted.
    pub successful: u32,
    /// How many of them its handler filtered out.
    pub dropped: u32,
    /// How many of them it could not accept.
    pub failed: u32,
}

/// The publisher of the metrics one Event API's data plane produces: the store the
/// datapoints go to, and the API they are all of.
///
/// It holds no counts of its own — every observation is a datapoint in the CloudWatch
/// store, never a second copy kept here.
#[derive(Clone)]
pub struct EventApiMetrics {
    /// The store the datapoints are appended to, or `None` when the engine carries no
    /// CloudWatch.
    store: Option<::engine::ServiceState<CloudWatchState>>,
    /// The `apiId` every series this publisher writes is dimensioned on.
    api_id: String,
}

impl ::std::fmt::Debug for EventApiMetrics {
    /// Which API this publisher is of and whether it has a store to append to, which is
    /// all of it that is not that store (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("EventApiMetrics")
            .field("api_id", &self.api_id)
            .field("published", &self.store.is_some())
            .finish()
    }
}

impl EventApiMetrics {
    /// The publisher appending `api_id`'s series to `store`.
    pub fn new(store: Option<::engine::ServiceState<CloudWatchState>>, api_id: &str) -> Self {
        Self {
            store,
            api_id: api_id.to_string(),
        }
    }

    /// Publish that one realtime connection was established, and the counts that leaves.
    pub fn connect_success(&self, counts: ConnectionCounts) {
        self.publish_api_series(CONNECT_SUCCESS_METRIC, 1.0);
        self.publish_connection_counts(counts);
    }

    /// Publish that one realtime connection closed, and the counts that leaves.
    pub fn disconnect_success(&self, counts: ConnectionCounts) {
        self.publish_api_series(DISCONNECT_SUCCESS_METRIC, 1.0);
        self.publish_connection_counts(counts);
    }

    /// Publish that one subscribe on a channel of `namespace` succeeded, and the counts
    /// that leaves.
    pub fn subscribe_success(&self, namespace: &str, counts: ConnectionCounts) {
        self.publish_namespace_series(SUBSCRIBE_SUCCESS_METRIC, namespace, 1.0);
        self.publish_connection_counts(counts);
    }

    /// Publish that one unsubscribe from a channel of `namespace` succeeded, and the
    /// counts that leaves.
    pub fn unsubscribe_success(&self, namespace: &str, counts: ConnectionCounts) {
        self.publish_namespace_series(UNSUBSCRIBE_SUCCESS_METRIC, namespace, 1.0);
        self.publish_connection_counts(counts);
    }

    /// Publish what one publish to a channel of `namespace` did with its events.
    ///
    /// Every count is published, a zero included: "how many events failed" is as much a
    /// measurement when it is none as when it is many, and a series that skipped its zeros
    /// would leave a client unable to tell a namespace that dropped nothing from one that
    /// is not publishing at all.
    pub fn published(&self, namespace: &str, counts: PublishCounts) {
        for (metric, value) in [
            (
                PUBLISH_HANDLER_INVOCATIONS_METRIC,
                counts.handler_invocations,
            ),
            (SUCCESSFUL_EVENTS_METRIC, counts.successful),
            (DROPPED_EVENTS_METRIC, counts.dropped),
            (FAILED_EVENTS_METRIC, counts.failed),
        ] {
            self.publish_namespace_series(metric, namespace, f64::from(value));
        }
    }

    /// Publish ONE SAMPLE of the two gauges, taken while the API holds a connection.
    ///
    /// What [`super::gauges`] calls on its cadence: the same two series the lifecycle
    /// publishes, with the counts the store holds at the moment of the sample, so every
    /// period of a held-open connection carries a reading of what the plane holds then
    /// rather than only the reading taken at the last change.
    pub fn sampled(&self, counts: ConnectionCounts) {
        self.publish_connection_counts(counts);
    }

    /// Publish the two gauges: the connections open on the API and the subscriptions they
    /// hold, as the store holds them at this moment.
    ///
    /// Both counts are bounded by the data plane's own caps
    /// (`crate::state::event_data_plane`'s connections per API and subscriptions per
    /// connection), so neither loses anything as a metric value.
    fn publish_connection_counts(&self, counts: ConnectionCounts) {
        for (metric, value) in [
            (ACTIVE_CONNECTIONS_METRIC, counts.connections),
            (ACTIVE_SUBSCRIPTIONS_METRIC, counts.subscriptions),
        ] {
            self.publish_api_series(metric, f64::from(u32::try_from(value).unwrap_or(u32::MAX)));
        }
    }

    /// Append one observation of a series dimensioned on the API alone.
    fn publish_api_series(&self, metric: &str, value: f64) {
        self.append(metric, api_dimensions(&self.api_id), value);
    }

    /// Append one observation of a series dimensioned on the API and one of its channel
    /// namespaces.
    fn publish_namespace_series(&self, metric: &str, namespace: &str, value: f64) {
        self.append(metric, namespace_dimensions(&self.api_id, namespace), value);
    }

    /// Append one observation, and make every alarm watching that series due for
    /// evaluation — exactly as an ingested `PutMetricData` datapoint does, so an alarm
    /// cannot tell a service's own metric from a client's.
    fn append(&self, metric: &str, dimensions: cloudwatch::DimensionSet, value: f64) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if !append_observation(store, metric, dimensions, value) {
            ::tracing::warn!(
                api_id = %self.api_id,
                %metric,
                "the CloudWatch store is at capacity; an AppSync metric could not be published"
            );
        }
    }
}

/// Append ONE observation of an `AWS/AppSync` series to `store`, and make every alarm
/// watching that series due for evaluation — `false` when the store is at capacity and the
/// datapoint could not be stored.
///
/// The ONE way a datapoint of this service's own metrics reaches the store, shared by every
/// plane that publishes one (a GraphQL API's real-time plane publishes its own series
/// through it, [`super::super::graphql_api::metrics`]), so the namespace a series lands in,
/// the unit and resolution it is stored at and the alarm wake-up it owes cannot differ
/// between them. An alarm is evaluated FROM these datapoints, so storing a series it
/// watches is what makes its evaluation due.
pub fn append_observation(
    store: &::engine::ServiceState<CloudWatchState>,
    metric: &str,
    dimensions: cloudwatch::DimensionSet,
    value: f64,
) -> bool {
    let key = cloudwatch::MetricKey {
        namespace: METRIC_NAMESPACE.to_string(),
        metric_name: metric.to_string(),
        dimensions,
    };
    let datum = cloudwatch::StoredDatum {
        timestamp_secs: cloudwatch::now_secs(),
        sample_count: 1.0,
        sum: value,
        minimum: value,
        maximum: value,
        unit: Some(COUNT_UNIT.to_string()),
        storage_resolution: STANDARD_STORAGE_RESOLUTION_SECONDS,
    };
    let watched = cloudwatch::WrittenSeries {
        key: key.clone(),
        units: vec![datum.unit().to_string()],
    };
    let at_secs = datum.timestamp_secs;
    let mut store = store.lock();
    if store.append_datapoints(key, vec![datum]).is_err() {
        return false;
    }
    store.make_alarms_due(&[watched], at_secs);
    true
}

/// What the two gauges read after one change: the connections open on the API and the
/// subscriptions they hold.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionCounts {
    pub connections: usize,
    pub subscriptions: usize,
}

/// The dimension set of a series about the API itself: the API, and nothing else — a
/// connection belongs to no channel namespace.
pub fn api_dimensions(api_id: &str) -> cloudwatch::DimensionSet {
    [(EVENT_API_ID_DIMENSION.to_string(), api_id.to_string())]
        .into_iter()
        .collect()
}

/// The dimension set of a series about one channel namespace of the API: the API and that
/// namespace, which together identify it.
pub fn namespace_dimensions(api_id: &str, namespace: &str) -> cloudwatch::DimensionSet {
    [
        (EVENT_API_ID_DIMENSION.to_string(), api_id.to_string()),
        (
            CHANNEL_NAMESPACE_NAME_DIMENSION.to_string(),
            namespace.to_string(),
        ),
    ]
    .into_iter()
    .collect()
}
