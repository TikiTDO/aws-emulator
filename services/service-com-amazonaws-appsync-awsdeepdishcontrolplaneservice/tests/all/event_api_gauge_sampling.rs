// SPDX-License-Identifier: CC0-1.0

//! The two `AWS/AppSync` GAUGES an Event API's data plane reports under `[EventAPIId]`,
//! observed over time: `ActiveConnections` and `ActiveSubscriptions`.
//!
//! The rule these cases assert, one level above the recorded case (one connection, one
//! subscription, an acknowledged unsubscribe on a socket deliberately held open):
//!
//! > Those two series measure quantities that HAVE a value at every instant, so while an
//! > Event API holds a real-time connection they are OBSERVED on a cadence and not only
//! > when they change: every reading taken after a change reads what the plane holds THEN,
//! > so a subscription that ended reads zero subscriptions while the connection that held
//! > it is still open, and it keeps reading that for as long as the connection stays open.
//! > A plane nothing is connected to is observed not at all.
//!
//! What a reader sees is therefore a series that KEEPS reporting: the readings taken after
//! the change are what a dashboard or an alarm over any later period is computed from, and
//! a series that stopped at its last change would leave every such period without data and
//! the value before the change looking like the value the quantity still has.
//!
//! The rows span what the rule generalises over: how many subscriptions the connection
//! held, how many of them ended, how many connections the API holds, and a subscription
//! that never ended at all (the cadence reports the value the gauge has, whatever it is).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::{
    cloudwatch, connected, create_event_api, create_namespace, emulator, exchange, ClientSocket,
    EventApi,
};

/// The namespace every subscription of this suite is made on a channel of.
const NAMESPACE: &str = "default";

/// How long a case waits for the cadence to take further readings: generous, because the
/// cadence is the service's own and a loaded machine may lag it, and bounded so a case that
/// will never be answered fails instead of hanging.
const READING_DEADLINE: Duration = Duration::from_secs(90);

/// How often a case looks again while waiting for those readings.
const POLL_SPACING: Duration = Duration::from_millis(250);

/// How many further readings of a gauge a case waits for before grading them. Two, so what
/// is graded is a CADENCE and not one trailing datapoint.
const READINGS_AWAITED: f64 = 2.0;

/// The metric names the two gauges carry; the service's own copies are
/// `service::event_api::metrics::{ACTIVE_CONNECTIONS_METRIC, ACTIVE_SUBSCRIPTIONS_METRIC}`.
const ACTIVE_CONNECTIONS: &str = "ActiveConnections";
const ACTIVE_SUBSCRIPTIONS: &str = "ActiveSubscriptions";

/// One row of the table: a lifecycle on an API of its own, and what the two gauges must
/// read for as long as it stays that way.
struct Row {
    case: &'static str,
    /// How many connections the case opens on the API.
    connections: usize,
    /// How many subscriptions each of those connections makes.
    subscriptions_each: usize,
    /// How many subscriptions each connection then unsubscribes.
    unsubscribed_each: usize,
    /// What `ActiveSubscriptions` must read afterwards.
    expected_subscriptions: f64,
}

/// What one reading of a gauge series amounts to in the store: how many observations it
/// holds, and what they add up to.
///
/// The pair is what makes a CADENCE observable: the count says how many readings were
/// taken, and the sum over exactly those readings says what each of them read.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Observations {
    count: f64,
    sum: f64,
}

#[tokio::test]
async fn the_two_gauges_keep_reading_what_the_plane_holds_while_a_connection_is_open() {
    let rows: &[Row] = &[
        Row {
            case: "one subscription, unsubscribed",
            connections: 1,
            subscriptions_each: 1,
            unsubscribed_each: 1,
            expected_subscriptions: 0.0,
        },
        Row {
            case: "two subscriptions, both unsubscribed",
            connections: 1,
            subscriptions_each: 2,
            unsubscribed_each: 2,
            expected_subscriptions: 0.0,
        },
        Row {
            case: "two subscriptions, one unsubscribed",
            connections: 1,
            subscriptions_each: 2,
            unsubscribed_each: 1,
            expected_subscriptions: 1.0,
        },
        Row {
            case: "two connections, one subscription each, both unsubscribed",
            connections: 2,
            subscriptions_each: 1,
            unsubscribed_each: 1,
            expected_subscriptions: 0.0,
        },
        Row {
            case: "one subscription, never unsubscribed",
            connections: 1,
            subscriptions_each: 1,
            unsubscribed_each: 0,
            expected_subscriptions: 1.0,
        },
    ];

    let router = emulator();
    // Every row's lifecycle is driven first and its sockets are held open, so the readings
    // the cadence then takes are graded for every row over one and the same wait.
    let mut staged: Vec<(&Row, EventApi, Vec<ClientSocket>, Observations, Observations)> =
        Vec::new();
    for row in rows {
        let api = create_event_api(&router, &format!("gauges-{}", staged.len())).await;
        create_namespace(&router, &api.api_id, NAMESPACE, None).await;
        let sockets = lifecycle(row, &api).await;
        let subscriptions = observations(&router, ACTIVE_SUBSCRIPTIONS, &api.api_id).await;
        let connections = observations(&router, ACTIVE_CONNECTIONS, &api.api_id).await;
        staged.push((row, api, sockets, subscriptions, connections));
    }

    for (row, api, sockets, subscriptions_before, connections_before) in &mut staged {
        let case = row.case;
        // The subscription gauge: further readings are taken, and each of them reads the
        // subscriptions the plane holds now — zero where the subscriptions ended, on a
        // connection that is still open.
        let subscriptions = awaited_readings(
            &router,
            case,
            ACTIVE_SUBSCRIPTIONS,
            &api.api_id,
            *subscriptions_before,
        )
        .await;
        assert_each_reading(
            case,
            ACTIVE_SUBSCRIPTIONS,
            *subscriptions_before,
            subscriptions,
            row.expected_subscriptions,
        );

        // The connection gauge, over the same readings: the connections are deliberately
        // still open, so every reading reads them.
        let expected_connections = row.connections as f64;
        let connections = awaited_readings(
            &router,
            case,
            ACTIVE_CONNECTIONS,
            &api.api_id,
            *connections_before,
        )
        .await;
        assert_each_reading(
            case,
            ACTIVE_CONNECTIONS,
            *connections_before,
            connections,
            expected_connections,
        );

        // Every connection the row opened is still open, which is what the readings above
        // were taken while: each still answers over its own socket.
        for (connection, socket) in sockets.iter_mut().enumerate() {
            let answered = exchange(case, socket, &json!({"type": "connection_init"})).await;
            assert_eq!(
                answered.as_ref().map(|frame| frame["type"].clone()),
                Some(json!("connection_ack")),
                "case {case:?}: connection {connection} must still be held open and answer \
                 over its socket, got {answered:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_api_nothing_is_connected_to_is_observed_not_at_all() {
    // The other half of the rule: the cadence reports a quantity of the plane, so an API
    // that holds no connection has nothing to report. The rows span how an API comes to
    // hold none: one that was never connected to, and one whose connection closed.
    let router = emulator();
    let never = create_event_api(&router, "gauges-never-connected").await;
    create_namespace(&router, &never.api_id, NAMESPACE, None).await;

    let closed = create_event_api(&router, "gauges-connection-closed").await;
    create_namespace(&router, &closed.api_id, NAMESPACE, None).await;
    let case = "a connection that closed";
    let mut socket = connected(case, &closed).await;
    let subscribed = exchange(
        case,
        &mut socket,
        &json!({
            "type": "subscribe",
            "id": "sub-1",
            "channel": format!("/{NAMESPACE}/room1"),
        }),
    )
    .await;
    assert_eq!(
        subscribed.as_ref().map(|frame| frame["type"].clone()),
        Some(json!("subscribe_success")),
        "case {case:?}: subscribe must be answered subscribe_success, got {subscribed:?}"
    );
    let _ = socket.close(None).await;
    drop(socket);
    // The close is what publishes the plane's final reading, so wait for the series to
    // settle on it before grading what follows it.
    let closed_reading = awaited_settled_close(&router, case, &closed.api_id).await;

    let case = "an API that was never connected to";
    let never_observed = observations(&router, ACTIVE_SUBSCRIPTIONS, &never.api_id).await;
    assert_eq!(
        never_observed.count, 0.0,
        "case {case:?}: expected no {ACTIVE_SUBSCRIPTIONS} reading at all, got \
         {never_observed:?}"
    );

    // And the series of the API whose connection closed does not go on being written: the
    // readings it holds a moment later are the same readings.
    let case = "a connection that closed";
    let still = settled_readings(&router, ACTIVE_CONNECTIONS, &closed.api_id, closed_reading).await;
    assert_eq!(
        still, closed_reading,
        "case {case:?}: expected no further {ACTIVE_CONNECTIONS} reading once the API holds \
         no connection, got {still:?} after {closed_reading:?}"
    );
}

/// Drive one row's lifecycle on `api`: open its connections, subscribe on each, then
/// unsubscribe as many as the row ends, answering the sockets, which the caller keeps open.
async fn lifecycle(row: &Row, api: &EventApi) -> Vec<ClientSocket> {
    let case = row.case;
    let mut sockets = Vec::new();
    for connection in 0..row.connections {
        let mut socket = connected(case, api).await;
        for subscription in 0..row.subscriptions_each {
            let id = format!("sub-{connection}-{subscription}");
            let answered = exchange(
                case,
                &mut socket,
                &json!({
                    "type": "subscribe",
                    "id": id,
                    "channel": format!("/{NAMESPACE}/room{subscription}"),
                }),
            )
            .await;
            assert_eq!(
                answered.as_ref().map(|frame| frame["type"].clone()),
                Some(json!("subscribe_success")),
                "case {case:?}: subscribe {id} must be answered subscribe_success, got \
                 {answered:?}"
            );
        }
        for subscription in 0..row.unsubscribed_each {
            let id = format!("sub-{connection}-{subscription}");
            let answered = exchange(
                case,
                &mut socket,
                &json!({"type": "unsubscribe", "id": id}),
            )
            .await;
            assert_eq!(
                answered.as_ref().map(|frame| frame["type"].clone()),
                Some(json!("unsubscribe_success")),
                "case {case:?}: unsubscribe {id} must be answered unsubscribe_success, got \
                 {answered:?}"
            );
        }
        sockets.push(socket);
    }
    sockets
}

/// Wait until the series `metric` of `api_id` holds at least [`READINGS_AWAITED`] more
/// observations than `before`, and answer what it holds then.
///
/// A poll loop on the CONDITION, with a deadline generous enough that a machine slower than
/// the cadence still reaches it.
async fn awaited_readings(
    router: &MultiServiceRouter,
    case: &str,
    metric: &str,
    api_id: &str,
    before: Observations,
) -> Observations {
    let deadline = tokio::time::Instant::now() + READING_DEADLINE;
    loop {
        let now = observations(router, metric, api_id).await;
        if now.count - before.count >= READINGS_AWAITED {
            return now;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected at least {READINGS_AWAITED} further {metric} readings \
             while the connection is open, got {now:?} after {before:?}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Wait until the connection gauge of `api_id` has SETTLED on the final reading a closing
/// connection publishes: the close is accounted for (a `DisconnectSuccess` is reported) and
/// two looks running find the same readings, so the reading the close itself publishes is
/// already in them.
async fn awaited_settled_close(
    router: &MultiServiceRouter,
    case: &str,
    api_id: &str,
) -> Observations {
    let deadline = tokio::time::Instant::now() + READING_DEADLINE;
    let mut previous = observations(router, ACTIVE_CONNECTIONS, api_id).await;
    loop {
        tokio::time::sleep(POLL_SPACING).await;
        let now = observations(router, ACTIVE_CONNECTIONS, api_id).await;
        let disconnected = disconnects(router, api_id).await;
        if disconnected >= 1.0 && now == previous {
            return now;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected the closed connection to be reported and the gauge to \
             settle, got {disconnected} disconnects and {now:?} after {previous:?}"
        );
        previous = now;
    }
}

/// What the series `metric` of `api_id` holds once it has been given the room a further
/// reading would have been taken in: the readings it still holds.
///
/// The wait is a poll loop like every other here — it looks again until the room has
/// passed — so what it reports is what the series holds after it, not after a fixed sleep.
async fn settled_readings(
    router: &MultiServiceRouter,
    metric: &str,
    api_id: &str,
    before: Observations,
) -> Observations {
    let through = tokio::time::Instant::now() + ROOM_FOR_A_FURTHER_READING;
    let mut latest = before;
    while tokio::time::Instant::now() < through {
        latest = observations(router, metric, api_id).await;
        tokio::time::sleep(POLL_SPACING).await;
    }
    latest
}

/// How much room a case gives a further reading before concluding none was taken: several
/// times over the service's own cadence
/// (`service::event_api::gauges::SAMPLE_INTERVAL`), so a machine lagging it still would
/// have taken one.
const ROOM_FOR_A_FURTHER_READING: Duration = Duration::from_secs(20);

/// Assert that every reading taken since `before` read `expected`.
///
/// The arithmetic is what makes it exact: the readings taken since `before` are
/// `after.count - before.count`, and they add up to `after.sum - before.sum`, so every one
/// of them read `expected` exactly when that sum is the count times `expected`.
fn assert_each_reading(
    case: &str,
    metric: &str,
    before: Observations,
    after: Observations,
    expected: f64,
) {
    let readings = after.count - before.count;
    let read = after.sum - before.sum;
    assert!(
        readings >= READINGS_AWAITED,
        "case {case:?}: expected at least {READINGS_AWAITED} further {metric} readings, got \
         {readings} ({before:?} then {after:?})"
    );
    assert_eq!(
        read,
        readings * expected,
        "case {case:?}: expected every one of the {readings} further {metric} readings to \
         read {expected}, got {read} over them ({before:?} then {after:?})"
    );
}

/// What the series `metric` of `api_id` holds, as `GetMetricStatistics` reports it: the
/// observations of every period it covers, folded into one count and one sum.
///
/// Folded across the periods, so a reading that lands in the next minute while a case is
/// polling is counted exactly once and in the same arithmetic.
async fn observations(router: &MultiServiceRouter, metric: &str, api_id: &str) -> Observations {
    let datapoints = statistics(router, metric, api_id).await;
    let fold = |member: &str| {
        datapoints
            .iter()
            .filter_map(|point| point[member].as_f64())
            .sum::<f64>()
    };
    Observations {
        count: fold("SampleCount"),
        sum: fold("Sum"),
    }
}

/// How many `DisconnectSuccess` observations `api_id` holds — what a closed connection
/// publishes, which is how a case knows the close has been accounted for.
async fn disconnects(router: &MultiServiceRouter, api_id: &str) -> f64 {
    statistics(router, "DisconnectSuccess", api_id)
        .await
        .iter()
        .filter_map(|point| point["Sum"].as_f64())
        .sum()
}

/// Every datapoint of one `[EventAPIId]` series, as `GetMetricStatistics` reports them.
async fn statistics(router: &MultiServiceRouter, metric: &str, api_id: &str) -> Vec<Value> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let (status, body) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": "AWS/AppSync",
            "MetricName": metric,
            "Dimensions": [{"Name": "EventAPIId", "Value": api_id}],
            "StartTime": now - 1_800.0,
            "EndTime": now + 60.0,
            "Period": 60,
            "Statistics": ["SampleCount", "Sum", "Minimum", "Maximum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {metric} must answer 200, got {body}"
    );
    body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
