// SPDX-License-Identifier: CC0-1.0

//! The `AWS/AppSync` metrics a GraphQL API's REAL-TIME plane publishes about its own work:
//! the connects it admitted, the ones it refused, and the disconnects that followed.
//!
//! A real-time connect leaves no response a later caller can read — nothing in the control
//! plane reports that a subscriber attached, or that a handshake was turned away. The only
//! way a client learns any of it is the series AppSync publishes about itself, so this
//! module publishes one datapoint per thing that happened, as it happens.
//!
//! WHICH dimension set they land under is part of the contract: a connect is made to the
//! API, so every series here is reported under [`GRAPHQL_API_ID_DIMENSION`] alone — the
//! dimension that names a GraphQL API, as opposed to the `EventAPIId` an Event API's own
//! series carry ([`super::super::event_api::metrics`]).
//!
//! The datapoints go into the CloudWatch store directly rather than through
//! `PutMetricData`, which reserves the `AWS/` namespaces for the services that own them
//! ("you should not specify a namespace that begins with `AWS/`",
//! `PutMetricData$Namespace`); the append and the alarm wake-up it owes are the Event API
//! plane's own ([`super::super::event_api::metrics::EventApiMetrics`] documents both), so
//! one service's metrics have ONE way into the store they are read from.

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

use super::super::event_api::metrics::{self as appsync_metrics, CloudWatchState};

/// The dimension that names the GraphQL API every one of these series is of.
pub const GRAPHQL_API_ID_DIMENSION: &str = "GraphQLAPIId";

/// One attempt to establish a real-time connection on the API: every opening handshake
/// this endpoint judged, whether it completed it or refused it.
///
/// So the attempts a client made are reported beside what became of them
/// ([`CONNECT_SUCCESS_METRIC`], [`CONNECT_CLIENT_ERROR_METRIC`]): a connect that was turned
/// away is counted here as much as one that was established, which is what lets a reader
/// tell a client that is being refused from one that is not connecting at all.
pub const CONNECT_REQUESTS_METRIC: &str = "ConnectRequests";

/// One real-time connection established on the API: a handshake this endpoint completed
/// whose `connection_init` it then acknowledged.
pub const CONNECT_SUCCESS_METRIC: &str = "ConnectSuccess";

/// One connect this endpoint refused because of the CLIENT: material that authorizes no
/// connection to the API, or a handshake addressed to something this endpoint does not
/// serve.
pub const CONNECT_CLIENT_ERROR_METRIC: &str = "ConnectClientError";

/// One real-time connection closed.
pub const DISCONNECT_SUCCESS_METRIC: &str = "DisconnectSuccess";

/// The publisher of the metrics one GraphQL API's real-time plane produces: the store the
/// datapoints go to, and the API they are all of.
///
/// It holds no counts of its own — every observation is a datapoint in the CloudWatch
/// store, never a second copy kept here, so the total a client reads is the `Sum`
/// CloudWatch computes over a period.
#[derive(Clone)]
pub struct GraphqlApiMetrics {
    /// The store the datapoints are appended to, or `None` when the engine carries no
    /// CloudWatch.
    store: Option<::engine::ServiceState<CloudWatchState>>,
    /// The `apiId` every series this publisher writes is dimensioned on.
    api_id: String,
}

impl ::std::fmt::Debug for GraphqlApiMetrics {
    /// Which API this publisher is of and whether it has a store to append to, which is
    /// all of it that is not that store (a store handle has no rendering of its own).
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("GraphqlApiMetrics")
            .field("api_id", &self.api_id)
            .field("published", &self.store.is_some())
            .finish()
    }
}

impl GraphqlApiMetrics {
    /// The publisher appending `api_id`'s series to `store`.
    pub fn new(store: Option<::engine::ServiceState<CloudWatchState>>, api_id: &str) -> Self {
        Self {
            store,
            api_id: api_id.to_string(),
        }
    }

    /// Publish that one attempt to establish a real-time connection was made.
    pub fn connect_requested(&self) {
        self.publish(CONNECT_REQUESTS_METRIC);
    }

    /// Publish that one real-time connection was established on the API.
    pub fn connect_success(&self) {
        self.publish(CONNECT_SUCCESS_METRIC);
    }

    /// Publish that one connect was refused on the client's own account.
    pub fn connect_client_error(&self) {
        self.publish(CONNECT_CLIENT_ERROR_METRIC);
    }

    /// Publish that one real-time connection closed.
    pub fn disconnect_success(&self) {
        self.publish(DISCONNECT_SUCCESS_METRIC);
    }

    /// Append ONE observation of `metric`, dimensioned on the API alone.
    fn publish(&self, metric: &str) {
        let Some(store) = self.store.as_ref() else {
            return;
        };
        appsync_metrics::append_observation(store, metric, api_dimensions(&self.api_id), 1.0);
    }
}

/// The dimension set of a series about one GraphQL API: the API, and nothing else.
pub fn api_dimensions(api_id: &str) -> cloudwatch::DimensionSet {
    [(
        GRAPHQL_API_ID_DIMENSION.to_string(),
        api_id.to_string(),
    )]
    .into_iter()
    .collect()
}
