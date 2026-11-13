// SPDX-License-Identifier: CC0-1.0

//! What an available cluster PUBLISHES about its own health, read where a client
//! reads it: Amazon CloudWatch `GetMetricStatistics` over the `AWS/Redshift`
//! `HealthStatus` metric.
//!
//! The rule under test: **a provisioned cluster that has reached `available`
//! publishes the `AWS/Redshift` `HealthStatus` metric once a minute — a 0/1 flag a
//! cluster that is available for queries holds at 1 — at two dimension sets at
//! once: cluster-wide (`ClusterIdentifier`) and once per node of the cluster's own
//! topology (`ClusterIdentifier` + `NodeID`, the node's role); a dimension set no
//! node of the cluster has is a series nobody published, which a read answers with
//! an empty `Datapoints` list rather than an error.**
//!
//! The rows differ along what the rule generalises over: the cluster's SHAPE, which
//! is what decides its node set — a `single-node` cluster (one shared node), and
//! `multi-node` clusters of two and of four compute nodes (a leader node and one
//! series per compute node). Each row asserts the populated series, the series that
//! must stay empty for the same cluster, and that the node series carry exactly the
//! nodes the control plane reports for that cluster in `Cluster$ClusterNodes` — so
//! the metric and `DescribeClusters` cannot disagree about one cluster's topology.
//!
//! Source for the `NodeID` spelling: the Amazon Redshift metric reference, whose
//! `NodeID` dimension "is either `Leader`, `Shared`, or `Compute-N` where N is 0,
//! 1, … for the number of nodes in the cluster"; `Shared` means the cluster has
//! only one node, with the leader and compute roles combined. The rows name those
//! values literally and the suite checks them against the mapping the service
//! publishes for a reported `ClusterNode$NodeRole`
//! ([`state::node_id`]), so neither side can drift alone.
//!
//! Every read goes over the wire of the service that owns it — Redshift's awsQuery
//! form wire, CloudWatch's awsJson one — through the same `MultiServiceRouter` the
//! production router builds, with both services mounted on ONE engine exactly as
//! the assembled emulator mounts them.

use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator};

/// How long any poll loop below is given before the suite calls the series
/// unobservable — generous next to the spans it waits for (a cluster's
/// provisioning span, then the accelerated publication of a series' run-up), so a
/// loaded runner cannot fail the suite.
const POLL_DEADLINE: Duration = Duration::from_secs(30);

/// The gap between two polls of one loop.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The period every read below asks for: one minute, the cadence the health check
/// runs at and the finest period a regular-resolution metric can be read at.
const PERIOD_SECONDS: i32 = 60;

/// How far back every read below starts: an hour, which holds far more than the
/// datapoints a freshly created cluster has published and stays well inside the
/// 1,440 datapoints one `GetMetricStatistics` call may return.
const READ_WINDOW_SECONDS: f64 = 60.0 * 60.0;

/// How close two instants or values have to be to count as the same number: the
/// epoch seconds and the 0/1 flag both travel as JSON numbers, so they are compared
/// with a tolerance rather than for bit equality.
const TOLERANCE: f64 = 1e-6;

/// One row: the cluster the case creates, the `NodeID` values its nodes publish
/// under, and `NodeID` values no node of it has.
struct Row {
    case: &'static str,
    identifier: &'static str,
    /// The members beyond the three required ones.
    params: Vec<(&'static str, &'static str)>,
    /// The `NodeID` of every node this cluster publishes a series for.
    node_ids: Vec<&'static str>,
    /// `NodeID` values that are well formed but name no node of THIS cluster, so
    /// its series under them must be empty.
    absent_node_ids: Vec<&'static str>,
}

/// Wall-clock now as epoch seconds, the clock a datapoint's time stamp is in.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// One `GetMetricStatistics` request over the health series of `cluster`, at the
/// dimension set `node_id` picks: cluster-wide for `None`, the node's own series
/// for `Some`.
fn health_request(cluster: &str, node_id: Option<&str>) -> Value {
    let mut dimensions = vec![json!({
        "Name": state::CLUSTER_IDENTIFIER_DIMENSION,
        "Value": cluster,
    })];
    if let Some(node_id) = node_id {
        dimensions.push(json!({
            "Name": state::NODE_ID_DIMENSION,
            "Value": node_id,
        }));
    }
    let end = now_secs();
    json!({
        "Namespace": state::METRIC_NAMESPACE,
        "MetricName": state::HEALTH_STATUS_METRIC,
        "Dimensions": dimensions,
        "StartTime": end - READ_WINDOW_SECONDS,
        "EndTime": end,
        "Period": PERIOD_SECONDS,
        "Statistics": ["Average", "Maximum", "Minimum"],
    })
}

/// One datapoint, reduced to what the assertions read.
#[derive(Debug, Clone, Copy)]
struct Point {
    timestamp: f64,
    average: f64,
    maximum: f64,
    minimum: f64,
}

/// The datapoints the health series of `cluster` at this dimension set holds,
/// oldest first.
async fn health_points(
    emulator: &Emulator,
    case: &str,
    cluster: &str,
    node_id: Option<&str>,
) -> Vec<Point> {
    let (status, body) = emulator
        .call_cloudwatch("GetMetricStatistics", &health_request(cluster, node_id))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetMetricStatistics for {node_id:?} to answer 200, \
         got {status} with body {body}"
    );
    let mut points: Vec<Point> = body["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| Point {
            timestamp: point["Timestamp"].as_f64().unwrap_or_default(),
            average: point["Average"].as_f64().unwrap_or_default(),
            maximum: point["Maximum"].as_f64().unwrap_or_default(),
            minimum: point["Minimum"].as_f64().unwrap_or_default(),
        })
        .collect();
    points.sort_by(|left, right| left.timestamp.total_cmp(&right.timestamp));
    points
}

/// Poll that series until it holds at least `wanted` datapoints, answering them;
/// panics quoting what it last saw when the deadline passes first.
///
/// The loop makes no mutating call: only the publication the clock owes can end it,
/// which is the property under test.
async fn poll_for_points(
    emulator: &Emulator,
    case: &str,
    cluster: &str,
    node_id: Option<&str>,
    wanted: usize,
) -> Vec<Point> {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last: Vec<Point> = Vec::new();
    while Instant::now() < deadline {
        last = health_points(emulator, case, cluster, node_id).await;
        if last.len() >= wanted {
            return last;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the {node_id:?} health series of {cluster:?} never reached {wanted} \
         datapoints within {POLL_DEADLINE:?}; last answer: {last:?}"
    );
}

/// Poll `DescribeClusters` until the cluster is available, answering the
/// `ClusterNode$NodeRole` list it reports then.
async fn poll_until_available(emulator: &Emulator, case: &str, cluster: &str) -> Vec<String> {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let read = emulator
            .call("DescribeClusters", &[("ClusterIdentifier", cluster)])
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected DescribeClusters to answer 200, got {} with body {}",
            read.status, read.body
        );
        if read.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE) {
            return all_text(&read.body, "NodeRole");
        }
        last = read.body;
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: {cluster:?} never became available within {POLL_DEADLINE:?}; \
         last answer: {last}"
    );
}

#[tokio::test]
async fn an_available_cluster_publishes_its_health_cluster_wide_and_per_node() {
    let emulator = Emulator::with_cloudwatch();

    let rows: Vec<Row> = vec![
        Row {
            case: "a single-node cluster publishes one shared-node series and no leader series",
            identifier: "health-single",
            params: vec![
                ("ClusterType", "single-node"),
                ("Encrypted", "true"),
                ("PubliclyAccessible", "false"),
                ("AutomatedSnapshotRetentionPeriod", "1"),
            ],
            node_ids: vec!["Shared"],
            absent_node_ids: vec!["Leader", "Compute-0"],
        },
        Row {
            case: "a two-node cluster publishes a leader series and one series per compute node",
            identifier: "health-multi-two",
            params: vec![
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "2"),
                ("Encrypted", "true"),
                ("PubliclyAccessible", "false"),
            ],
            node_ids: vec!["Leader", "Compute-0", "Compute-1"],
            absent_node_ids: vec!["Shared", "Compute-2"],
        },
        Row {
            case: "a four-node cluster publishes a series per node of its larger topology",
            identifier: "health-multi-four",
            params: vec![
                ("ClusterType", "multi-node"),
                ("NumberOfNodes", "4"),
                ("Port", "5455"),
            ],
            node_ids: vec![
                "Leader",
                "Compute-0",
                "Compute-1",
                "Compute-2",
                "Compute-3",
            ],
            absent_node_ids: vec!["Shared", "Compute-4"],
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut params: Vec<(&str, &str)> = vec![
            ("ClusterIdentifier", row.identifier),
            ("NodeType", "ra3.xlplus"),
            ("MasterUsername", "admin"),
        ];
        params.extend(row.params.iter().copied());
        let created = emulator.call("CreateCluster", &params).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateCluster to answer 200, got {} with body {}",
            created.status, created.body
        );

        // The nodes the CONTROL PLANE reports for this cluster, in the spelling the
        // dimension names them by: the series below must be exactly these.
        let reported = poll_until_available(&emulator, case, row.identifier).await;
        let reported_ids: BTreeSet<String> =
            reported.iter().map(|role| state::node_id(role)).collect();
        let expected_ids: BTreeSet<String> =
            row.node_ids.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids, expected_ids,
            "case {case:?}: expected the cluster to report the nodes {expected_ids:?}, \
             got {reported_ids:?} from the roles {reported:?}"
        );

        // The cluster-wide series: it reaches the healthy flag, every value is the
        // 0/1 flag, and consecutive datapoints are one minute apart — the cadence
        // the health check publishes at.
        let cluster_points = poll_for_points(&emulator, case, row.identifier, None, 2).await;
        assert!(
            cluster_points
                .iter()
                .any(|point| (point.maximum - state::HEALTHY).abs() < TOLERANCE),
            "case {case:?}: expected the cluster-wide series to reach {}, got {cluster_points:?}",
            state::HEALTHY
        );
        for point in &cluster_points {
            for (statistic, value) in [
                ("Average", point.average),
                ("Maximum", point.maximum),
                ("Minimum", point.minimum),
            ] {
                assert!(
                    (value - state::HEALTHY).abs() < TOLERANCE
                        || (value - state::UNHEALTHY).abs() < TOLERANCE,
                    "case {case:?}: expected {statistic} to be the 0/1 health flag, \
                     got {value} in {point:?}"
                );
            }
        }
        for pair in cluster_points.windows(2) {
            let spacing = pair[1].timestamp - pair[0].timestamp;
            assert!(
                (spacing - f64::from(PERIOD_SECONDS)).abs() < TOLERANCE,
                "case {case:?}: expected consecutive datapoints one minute apart, \
                 got {spacing} between {:?} and {:?}",
                pair[0],
                pair[1]
            );
        }

        // Every node of the cluster publishes its own series, covering every minute
        // the cluster-wide series above already had.
        let cluster_minutes: BTreeSet<i64> = cluster_points
            .iter()
            .map(|point| point.timestamp as i64)
            .collect();
        for node_id in &row.node_ids {
            let node_points =
                poll_for_points(&emulator, case, row.identifier, Some(node_id), 1).await;
            let node_minutes: BTreeSet<i64> = node_points
                .iter()
                .map(|point| point.timestamp as i64)
                .collect();
            assert!(
                cluster_minutes.is_subset(&node_minutes),
                "case {case:?}: expected the {node_id:?} series to cover the cluster-wide \
                 minutes {cluster_minutes:?}, got {node_minutes:?}"
            );
            assert!(
                node_points
                    .iter()
                    .any(|point| (point.maximum - state::HEALTHY).abs() < TOLERANCE),
                "case {case:?}: expected the {node_id:?} series to reach {}, got {node_points:?}",
                state::HEALTHY
            );
        }

        // And a dimension set no node of this cluster has is a series nobody
        // published: an empty list, not an error.
        for absent in &row.absent_node_ids {
            let points = health_points(&emulator, case, row.identifier, Some(absent)).await;
            assert!(
                points.is_empty(),
                "case {case:?}: expected no {absent:?} series for this cluster, got {points:?}"
            );
        }
    }

    // A cluster nobody created publishes nothing at all, cluster-wide or per node:
    // the series exists because a cluster was brought up, not because a read asked.
    for node_id in [None, Some("Shared"), Some("Leader")] {
        let points = health_points(
            &emulator,
            "a cluster identifier nothing created",
            "health-never-created",
            node_id,
        )
        .await;
        assert!(
            points.is_empty(),
            "case \"a cluster identifier nothing created\": expected no {node_id:?} series, \
             got {points:?}"
        );
    }
}
