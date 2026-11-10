// SPDX-License-Identifier: CC0-1.0

//! The `AWS/Cognito` CloudWatch series a user pool's own traffic publishes.
//!
//! A user pool is not only a store of users: every call that TERMINATES an
//! authentication or a registration attempt against one of its app clients is also an
//! observation, published as metric data in the [`NAMESPACE`] namespace. That series is
//! how a client watches authentication health — a dashboard over sign-in volume, an alarm
//! on a drop in [`SIGN_IN_SUCCESSES`] — so a pool that publishes nothing leaves every such
//! alarm without data, and makes "no traffic" indistinguishable from "all traffic
//! refused".
//!
//! # What one call contributes
//!
//! The counters are OUTCOME counters, not success-only counters: one terminated attempt
//! contributes exactly one datapoint, valued 1 where the attempt completed and 0 where it
//! was refused, so `Sum` counts the completions, `SampleCount` counts the attempts and
//! `Average` is the completion rate. An attempt the call did not terminate — one answered
//! with a CHALLENGE, which the caller answers next — contributes nothing yet: the call that
//! finishes it publishes its outcome ([`Attempt`]).
//!
//! Which series a call contributes to follows what it attempted, not which operation
//! served it ([`Counter`]): a sign-in lands in [`SIGN_IN_SUCCESSES`], a refresh-token
//! exchange in [`TOKEN_REFRESH_SUCCESSES`] and a self-service registration in
//! [`SIGN_UP_SUCCESSES`]. Each is keyed on the PAIR of dimensions
//! [`USER_POOL_DIMENSION`] + [`USER_POOL_CLIENT_DIMENSION`] — the pool and the app client
//! the attempt ran against. A dimension set is part of a metric's identity, so an app
//! client nothing was attempted against has no datapoints at all rather than a share of
//! the pool's.
//!
//! # The risk outcome
//!
//! A pool whose threat protection is switched on ([`super::UserPool::add_ons_enabled`])
//! also EVALUATES the events it serves, and the outcome of that evaluation is published as
//! its own series keyed on [`OPERATION_DIMENSION`] + [`USER_POOL_ID_DIMENSION`] — a
//! different dimension name for the pool than the counters use, and dimension matching is
//! exact, so a read keyed on the counters' name finds none of these. This emulator makes no
//! outbound call and holds no device or address reputation, so there is no risk signal for
//! an evaluation to find: a completed attempt is evaluated as [`NO_RISK`], under the
//! [`Counter::risk_operation`] the attempt belongs to. A pool with threat protection off
//! evaluates nothing and publishes nothing here.
//!
//! # Where the datapoints live
//!
//! In the CloudWatch emulator's OWN metric store — the same store `PutMetricData` writes
//! and `GetMetricStatistics` / `GetMetricData` / `ListMetrics` read — reached through the
//! shared engine, so one metric has one home and the publishers cannot drift. They are
//! appended to that store directly rather than sent through `PutMetricData`, which reserves
//! the `AWS/` namespaces for the services that own them ("you should not specify a
//! namespace that begins with `AWS/`", `PutMetricDataInput$Namespace`).

use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;

/// The namespace Amazon Cognito publishes a user pool's own metrics into.
pub const NAMESPACE: &str = "AWS/Cognito";

/// The dimension naming the user pool an authentication counter belongs to, by pool id.
pub const USER_POOL_DIMENSION: &str = "UserPool";

/// The dimension naming the app client the attempt ran against, by client id.
pub const USER_POOL_CLIENT_DIMENSION: &str = "UserPoolClient";

/// The dimension naming the operation a risk evaluation was made for.
pub const OPERATION_DIMENSION: &str = "Operation";

/// The dimension naming the user pool a RISK outcome belongs to, by pool id — a different
/// dimension name from [`USER_POOL_DIMENSION`], which the counters carry.
pub const USER_POOL_ID_DIMENSION: &str = "UserPoolId";

/// The counter every terminated sign-in attempt is observed in.
pub const SIGN_IN_SUCCESSES: &str = "SignInSuccesses";

/// The counter every terminated refresh-token exchange is observed in.
pub const TOKEN_REFRESH_SUCCESSES: &str = "TokenRefreshSuccesses";

/// The counter every terminated self-service registration is observed in.
pub const SIGN_UP_SUCCESSES: &str = "SignUpSuccesses";

/// The risk outcome of an event threat protection evaluated and found nothing against.
pub const NO_RISK: &str = "NoRisk";

/// The [`OPERATION_DIMENSION`] value a sign-in evaluation carries.
pub const SIGN_IN_OPERATION: &str = "SignIn";

/// The [`OPERATION_DIMENSION`] value a registration evaluation carries.
pub const SIGN_UP_OPERATION: &str = "SignUp";

/// The `StandardUnit` these observations carry: each is a number of attempts.
pub const COUNT_UNIT: &str = "Count";

/// The resolution these series are stored at, in seconds — the regular resolution a metric
/// has unless its publisher asks for the high-resolution one ("This field is optional, if
/// you do not specify it the default of 60 is used", `MetricDatum$StorageResolution`).
pub const STORAGE_RESOLUTION_SECS: i32 = 60;

/// What a completed attempt observes: the counters count outcomes, so a completion is the
/// one sample that raises `Sum`.
const COMPLETED_VALUE: f64 = 1.0;

/// What a refused attempt observes: a zero-valued sample rather than no sample, which is what
/// keeps `SampleCount` the number of attempts.
const REFUSED_VALUE: f64 = 0.0;

/// `AuthFlowType`'s refresh-token flow and the alias that enum carries for the same flow —
/// the flows that exchange a refresh token instead of signing a user in.
pub const REFRESH_TOKEN_AUTH_FLOWS: [&str; 2] = ["REFRESH_TOKEN_AUTH", "REFRESH_TOKEN"];

/// Which [`NAMESPACE`] counter an attempt is observed in — what the call attempted, not
/// which operation served it, so the self-service and the administrative surface of one
/// attempt contribute to the same series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counter {
    /// Signing a user in: the credential-checking flows of `InitiateAuth` and
    /// `AdminInitiateAuth`, and the challenge responses that complete one.
    SignIn,
    /// Exchanging a refresh token for a fresh access and id token.
    TokenRefresh,
    /// Registering a user through an app client.
    Registration,
}

impl Counter {
    /// Which counter an `AuthFlow` attempts: a refresh-token flow exchanges a token the pool
    /// issued, and every other flow — whatever credential or challenge it starts from —
    /// signs a user in.
    #[must_use]
    pub fn attempted(auth_flow: &str) -> Self {
        if REFRESH_TOKEN_AUTH_FLOWS.contains(&auth_flow) {
            Self::TokenRefresh
        } else {
            Self::SignIn
        }
    }

    /// The metric name this counter's datapoints are stored under.
    #[must_use]
    pub fn metric_name(self) -> &'static str {
        match self {
            Self::SignIn => SIGN_IN_SUCCESSES,
            Self::TokenRefresh => TOKEN_REFRESH_SUCCESSES,
            Self::Registration => SIGN_UP_SUCCESSES,
        }
    }

    /// The [`OPERATION_DIMENSION`] value threat protection evaluates this attempt under, or
    /// `None` for an attempt it evaluates no risk for.
    ///
    /// Threat protection evaluates the events that authenticate or register a user; a
    /// refresh-token exchange presents a credential this pool itself issued to a session it
    /// already evaluated, so it is not an event of its own.
    #[must_use]
    pub fn risk_operation(self) -> Option<&'static str> {
        match self {
            Self::SignIn => Some(SIGN_IN_OPERATION),
            Self::Registration => Some(SIGN_UP_OPERATION),
            Self::TokenRefresh => None,
        }
    }
}

/// What a call did to the attempt it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attempt {
    /// The attempt is over and it succeeded: this call issued what it asked for.
    Completed,
    /// The attempt is over and it was refused — including a refusal raised by request
    /// validation, before any credential was checked.
    Refused,
    /// The attempt is NOT over: this call answered with a challenge, and the call that
    /// answers that challenge publishes the outcome.
    Outstanding,
}

/// What `answered` did to the attempt: `completed` decides whether an accepted answer
/// FINISHED it (an answer that carries the issued tokens) or only carried it on.
pub fn attempt_of<Out, Err>(
    answered: &Result<Out, Err>,
    completed: impl FnOnce(&Out) -> bool,
) -> Attempt {
    match answered {
        Ok(output) if completed(output) => Attempt::Completed,
        Ok(_) => Attempt::Outstanding,
        Err(_) => Attempt::Refused,
    }
}

/// Publish what one terminated attempt contributes to the [`NAMESPACE`] series of the pool
/// and app client it ran against.
///
/// `user_pool_id` is the pool the REQUEST named, for an operation that models one, and
/// `None` where the app client alone names the pool; either way the dimensions come from the
/// pool this service STORES that app client in, so an attempt against an app client no pool
/// holds — or one the named pool does not hold — publishes nothing: there is no series for
/// it to belong to.
///
/// Call it with no state guard of this service held, so the two services' stores are never
/// locked at once. Nothing is published on an engine that holds no CloudWatch store to
/// reach: a datapoint has one home and this path never makes a second one. A store at its
/// series capacity takes no new series; the attempt it came from is not failed for that,
/// because the call itself was already answered.
pub fn record<Eng: Send + Sync + 'static>(
    engine: &Eng,
    counter: Counter,
    user_pool_id: Option<&str>,
    client_id: &str,
    attempt: Attempt,
    at: SystemTime,
) {
    let value = match attempt {
        Attempt::Completed => COMPLETED_VALUE,
        Attempt::Refused => REFUSED_VALUE,
        Attempt::Outstanding => return,
    };
    // Which pool the app client lives in, and whether that pool evaluates the events it
    // serves, read under this service's own lock — which this statement releases before the
    // metric store is reached.
    let Some(state) = super::service_state(engine) else {
        return;
    };
    let addressing = {
        let guard = state.lock();
        let pool = match user_pool_id {
            Some(user_pool_id) => guard
                .pool(user_pool_id)
                .filter(|pool| pool.clients.contains_key(client_id)),
            None => guard.pool_of_client(client_id),
        };
        pool.map(|pool| (pool.id.clone(), pool.add_ons_enabled()))
    };
    let Some((user_pool_id, evaluates_risk)) = addressing else {
        return;
    };

    let mut published = vec![(
        MetricIdentity {
            metric_name: counter.metric_name(),
            dimensions: [
                (USER_POOL_DIMENSION, user_pool_id.clone()),
                (USER_POOL_CLIENT_DIMENSION, client_id.to_string()),
            ],
        },
        value,
    )];
    // The pool evaluated the event it just served, and this emulator has no risk signal for
    // that evaluation to find, so the outcome is that it found none.
    if evaluates_risk && attempt == Attempt::Completed {
        if let Some(operation) = counter.risk_operation() {
            published.push((
                MetricIdentity {
                    metric_name: NO_RISK,
                    dimensions: [
                        (OPERATION_DIMENSION, operation.to_string()),
                        (USER_POOL_ID_DIMENSION, user_pool_id),
                    ],
                },
                COMPLETED_VALUE,
            ));
        }
    }
    append(engine, &published, at);
}

/// One series this publisher writes: the metric name, and the two dimensions that name what
/// the observation is about.
struct MetricIdentity {
    /// The metric name the datapoint is stored under.
    metric_name: &'static str,
    /// The dimension set, as (name, value) pairs.
    dimensions: [(&'static str, String); 2],
}

/// Append one datapoint per entry of `published` to the CloudWatch metric store, and make
/// every alarm over the series they belong to due for evaluation.
///
/// An alarm is evaluated FROM these datapoints, so storing a series it watches is what makes
/// its evaluation due — an alarm cannot tell a service's own metric from one a client
/// published.
fn append<Eng: Send + Sync + 'static>(
    engine: &Eng,
    published: &[(MetricIdentity, f64)],
    at: SystemTime,
) {
    let Some(state) = cloudwatch::service_state(engine) else {
        return;
    };
    let timestamp_secs = epoch_secs(at);
    let mut store = state.lock();
    let mut written: Vec<cloudwatch::WrittenSeries> = Vec::with_capacity(published.len());
    for (identity, value) in published {
        let key = cloudwatch::MetricKey {
            namespace: NAMESPACE.to_string(),
            metric_name: identity.metric_name.to_string(),
            dimensions: identity
                .dimensions
                .iter()
                .map(|(name, dimension_value)| ((*name).to_string(), dimension_value.clone()))
                .collect(),
        };
        let datum = cloudwatch::StoredDatum {
            timestamp_secs,
            // One observation: this attempt's own contribution to the series.
            sample_count: 1.0,
            sum: *value,
            minimum: *value,
            maximum: *value,
            unit: Some(COUNT_UNIT.to_string()),
            storage_resolution: STORAGE_RESOLUTION_SECS,
        };
        let watched = key.clone();
        if store.append_datapoints(key, vec![datum]).is_err() {
            // The metric store holds no more series. The attempt itself was already
            // answered, so nothing is failed for it; the dropped observation is reported
            // where a developer can see it.
            ::tracing::warn!(
                metric = identity.metric_name,
                "the CloudWatch metric store is at capacity, so this observation was dropped"
            );
            continue;
        }
        written.push(cloudwatch::WrittenSeries {
            key: watched,
            units: vec![COUNT_UNIT.to_string()],
        });
    }
    store.make_alarms_due(&written, timestamp_secs);
}

/// `at` as the epoch seconds a stored datapoint carries, which is the unit the metric
/// store's own readers window by. An instant before the epoch (which no clock reports) reads
/// as the epoch itself rather than failing the publish.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: what a call did to the attempt decides its sample — a completed attempt
    /// observes 1, a refused one 0, and an outstanding challenge observes nothing yet.
    #[test]
    fn an_attempt_is_classified_by_what_the_call_answered() {
        for (case, answered, expected) in [
            ("tokens issued", Ok(true), Attempt::Completed),
            ("a challenge to answer", Ok(false), Attempt::Outstanding),
            ("a refusal", Err(()), Attempt::Refused),
        ] {
            let found = attempt_of(&answered, |issued| *issued);
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected:?}, got {found:?}"
            );
        }
    }

    /// RULE: a refresh-token flow is a token exchange and every other `AuthFlowType` member
    /// is a sign-in, whatever credential it starts from.
    ///
    /// The rows span the `AuthFlowType` members
    /// (`smithy_query(query="shape",shape="AuthFlowType")`), including a value outside the
    /// enumeration.
    #[test]
    fn a_flow_is_a_refresh_only_where_it_exchanges_a_refresh_token() {
        for (case, auth_flow, expected) in [
            ("the refresh flow", "REFRESH_TOKEN_AUTH", Counter::TokenRefresh),
            ("its alias", "REFRESH_TOKEN", Counter::TokenRefresh),
            ("a password sign-in", "USER_PASSWORD_AUTH", Counter::SignIn),
            ("an SRP sign-in", "USER_SRP_AUTH", Counter::SignIn),
            ("choice-based sign-in", "USER_AUTH", Counter::SignIn),
            ("a custom sign-in", "CUSTOM_AUTH", Counter::SignIn),
            (
                "an administrative password sign-in",
                "ADMIN_USER_PASSWORD_AUTH",
                Counter::SignIn,
            ),
            ("the legacy admin flow", "ADMIN_NO_SRP_AUTH", Counter::SignIn),
            ("a value the enumeration does not carry", "", Counter::SignIn),
        ] {
            let found = Counter::attempted(auth_flow);
            assert_eq!(
                found, expected,
                "case {case:?}: expected {expected:?}, got {found:?}"
            );
        }
    }

    /// RULE: a counter names its own series, and only the attempts that authenticate or
    /// register a user are events threat protection evaluates.
    #[test]
    fn each_counter_names_its_series_and_its_risk_operation() {
        for (case, counter, metric_name, risk_operation) in [
            (
                "a sign-in",
                Counter::SignIn,
                SIGN_IN_SUCCESSES,
                Some(SIGN_IN_OPERATION),
            ),
            (
                "a refresh",
                Counter::TokenRefresh,
                TOKEN_REFRESH_SUCCESSES,
                None,
            ),
            (
                "a registration",
                Counter::Registration,
                SIGN_UP_SUCCESSES,
                Some(SIGN_UP_OPERATION),
            ),
        ] {
            let found = counter.metric_name();
            assert_eq!(
                found, metric_name,
                "case {case:?}: expected {metric_name}, got {found}"
            );
            let found = counter.risk_operation();
            assert_eq!(
                found, risk_operation,
                "case {case:?}: expected {risk_operation:?}, got {found:?}"
            );
        }
    }
}
