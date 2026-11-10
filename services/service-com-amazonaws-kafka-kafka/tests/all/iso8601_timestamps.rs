// SPDX-License-Identifier: CC0-1.0

//! The wire FORM of every timestamp this service reports.
//!
//! RULE: every response member of this service that targets
//! `com.amazonaws.kafka#__timestampIso8601` — the one timestamp shape this model has, and
//! it carries `@timestampFormat("date-time")` (`smithy_query(query="shape",
//! shape="com.amazonaws.kafka#__timestampIso8601")`) — travels on this restJson1 wire as
//! an RFC 3339 / ISO-8601 date-time STRING in UTC, never as a bare epoch number, and the
//! instant it names is the one the emulator stored for that resource, whichever reader
//! renders it.
//!
//! The rows differ along what the rule generalises over: which resource family carries the
//! timestamp (configurations and their revisions, clusters in both their v1 and v2 shapes,
//! VPC connections, cluster operations in both shapes), whether it is read from a create,
//! an update, a by-ARN reader or a page of a listing, how deeply the member is nested in
//! the response (top level, inside a structure, inside each element of a list), and which
//! member of the model it is (`creationTime`, `startTime`, `endTime`).
//!
//! The instant itself is state-derived and differs per run, so no row asserts a fixed
//! value: each asserts the TYPE and format, that the instant falls inside the window the
//! call ran in, and that every other reader of the same resource reports the SAME text.

use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::kafka_harness::{
    configuration_rows, pct, reported_instant_secs, sleep_past_activation, sleep_past_update,
    sleep_past_vpc_connection_activation, ConnectionRequest, Emulator, Reply,
};

/// Now, in seconds since the epoch: the clock a row's window is taken from.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
}

/// A member of a reply, or `Value::Null` when it is absent.
fn member(body: &Value, pointer: &str) -> Value {
    body.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The instant one reported date-time names, in seconds since the epoch.
fn instant_secs(case: &str, text: &str) -> f64 {
    reported_instant_secs(&json!(text), case)
}

/// Assert the member at `pointer` is the modelled date-time and hand back the instant it
/// names: a JSON STRING (the defect this pins is a JSON NUMBER of epoch seconds), an
/// RFC 3339 reader accepts it, it is spelled in UTC, and it falls inside `window` — the
/// span between the call that minted the resource and now, so a hardcoded or echoed
/// instant fails here.
fn assert_date_time(case: &str, body: &Value, pointer: &str, window: (f64, f64)) -> String {
    let value = member(body, pointer);
    assert!(
        !value.is_number(),
        "case {case}: {pointer} is a bare epoch-seconds number ({value}), not the modelled \
         ISO-8601 date-time string"
    );
    let text = value
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case}: expected an ISO-8601 date-time string at {pointer}, got {value}")
        })
        .to_string();
    // AWS renders a `date-time` member in UTC, which the `Z` designator is.
    assert!(
        text.ends_with('Z'),
        "case {case}: {pointer} must be a UTC date-time, got {text:?}"
    );
    let secs = reported_instant_secs(&value, case);
    assert!(
        (window.0..=window.1).contains(&secs),
        "case {case}: {pointer} must name an instant inside the window the call ran in \
         ({window:?}), got {text:?} ({secs})"
    );
    text
}

/// Assert every element of the list at `list_pointer` reports the modelled date-time at
/// `member_pointer`, and hand back the texts in the order the list reported them.
fn assert_date_times_in_list(
    case: &str,
    body: &Value,
    list_pointer: &str,
    member_pointer: &str,
    window: (f64, f64),
) -> Vec<String> {
    let entries = member(body, list_pointer);
    let entries = entries
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a list at {list_pointer}, got {body}"));
    assert!(
        !entries.is_empty(),
        "case {case}: expected {list_pointer} to report at least one entry, got {body}"
    );
    entries
        .iter()
        .enumerate()
        .map(|(position, entry)| {
            let case = format!("{case} {list_pointer}[{position}]");
            assert_date_time(&case, entry, member_pointer, window)
        })
        .collect()
}

/// A reply that must have succeeded.
fn ok(case: &str, what: &str, reply: Reply) -> Value {
    assert_eq!(
        reply.status, 200,
        "case {case}: expected {what} to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.body
}

/// RULE, over the configuration family: `creationTime` — the configuration's own and the
/// one inside `latestRevision` — is an ISO-8601 date-time string on every path that
/// reports it, and every reader of one stored configuration reports the same text.
#[tokio::test(flavor = "current_thread")]
async fn every_configuration_path_reports_iso8601_date_times() {
    let emulator = Emulator::new();
    for row in configuration_rows() {
        let case = row.name.clone();
        let before = now_secs();
        let created = ok(
            &case,
            "CreateConfiguration",
            emulator
                .call("POST", "/v1/configurations", Some(row.to_json()))
                .await,
        );
        let window = (before - 1.0, now_secs() + 1.0);
        let arn = member(&created, "/arn")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: CreateConfiguration returned no arn: {created}"))
            .to_string();

        // Cause 1: the create's own two members.
        let created_at = assert_date_time(&case, &created, "/creationTime", window);
        let revision_one_at =
            assert_date_time(&case, &created, "/latestRevision/creationTime", window);

        // Cause 2: the by-ARN reader renders the same stored instants.
        let described = ok(
            &case,
            "DescribeConfiguration",
            emulator.describe_configuration(&arn).await,
        );
        assert_eq!(
            assert_date_time(&case, &described, "/creationTime", window),
            created_at,
            "case {case}: DescribeConfiguration must report the stored creation instant, got {described}"
        );
        assert_eq!(
            assert_date_time(&case, &described, "/latestRevision/creationTime", window),
            revision_one_at,
            "case {case}: DescribeConfiguration must report the stored revision instant, got {described}"
        );

        // Cause 3: every element of the listing, and the revision nested in it.
        let listed = ok(
            &case,
            "ListConfigurations",
            emulator.call("GET", "/v1/configurations", None).await,
        );
        let entries = member(&listed, "/configurations");
        let entry = entries
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a configurations list, got {listed}"))
            .iter()
            .find(|entry| member(entry, "/arn") == json!(arn))
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: {arn} is missing from the listing: {listed}"));
        assert_eq!(
            assert_date_time(&case, &entry, "/creationTime", window),
            created_at,
            "case {case}: the listing must report the stored creation instant, got {entry}"
        );
        assert_eq!(
            assert_date_time(&case, &entry, "/latestRevision/creationTime", window),
            revision_one_at,
            "case {case}: the listing must report the stored revision instant, got {entry}"
        );

        // Cause 4: the revision reader.
        let revision = ok(
            &case,
            "DescribeConfigurationRevision",
            emulator.describe_configuration_revision(&arn, 1).await,
        );
        assert_eq!(
            assert_date_time(&case, &revision, "/creationTime", window),
            revision_one_at,
            "case {case}: the revision reader must report the stored instant, got {revision}"
        );

        // Cause 5: the update's `latestRevision`, which is a revision minted after the
        // create, so its instant is at or after the configuration's own.
        let updated = ok(
            &case,
            "UpdateConfiguration",
            emulator
                .update_configuration(&arn, Some("a second revision"), b"num.partitions=2\n")
                .await,
        );
        let window = (window.0, now_secs() + 1.0);
        let revision_two_at =
            assert_date_time(&case, &updated, "/latestRevision/creationTime", window);
        assert!(
            instant_secs(&case, &revision_two_at) >= instant_secs(&case, &revision_one_at),
            "case {case}: revision 2 was minted after revision 1, got {revision_two_at} then \
             {revision_one_at}"
        );

        // Cause 6: every element of the revision listing, in the order it reports them.
        let revisions = ok(
            &case,
            "ListConfigurationRevisions",
            emulator
                .call(
                    "GET",
                    &format!("/v1/configurations/{}/revisions", pct(&arn)),
                    None,
                )
                .await,
        );
        let listed_revisions =
            assert_date_times_in_list(&case, &revisions, "/revisions", "/creationTime", window);
        assert_eq!(
            listed_revisions,
            vec![revision_one_at.clone(), revision_two_at.clone()],
            "case {case}: the revision listing must report each stored revision's instant, got \
             {revisions}"
        );
    }
}

/// RULE, over the resource families outside the configuration one: the same member kind is
/// the same date-time string wherever this service reports it, so the rows below read a
/// cluster, a VPC connection and a cluster operation through every operation that renders
/// one — which is what makes the rule a rule rather than a fix to the configuration
/// responses.
#[tokio::test(flavor = "current_thread")]
async fn every_other_timestamp_member_is_an_iso8601_date_time() {
    let emulator = Emulator::new();
    let before = now_secs();
    let cluster_arn = emulator.vpc_connection_target("timestamp-forms").await;
    sleep_past_activation();
    let window = (before - 1.0, now_secs() + 1.0);

    // `ClusterInfo.creationTime`, from the v1 reader and the v1 listing.
    let described = ok(
        "cluster",
        "DescribeCluster",
        emulator.describe(&cluster_arn).await,
    );
    let cluster_at = assert_date_time("cluster", &described, "/clusterInfo/creationTime", window);
    let listed = ok("cluster", "ListClusters", emulator.list("").await);
    assert_eq!(
        assert_date_times_in_list(
            "cluster",
            &listed,
            "/clusterInfoList",
            "/creationTime",
            window
        ),
        vec![cluster_at.clone()],
        "case cluster: the listing must report the stored creation instant, got {listed}"
    );

    // `Cluster.creationTime`, the v2 shape of the same stored cluster.
    let described_v2 = ok(
        "cluster-v2",
        "DescribeClusterV2",
        emulator.describe_v2(&cluster_arn).await,
    );
    assert_eq!(
        assert_date_time(
            "cluster-v2",
            &described_v2,
            "/clusterInfo/creationTime",
            window
        ),
        cluster_at,
        "case cluster-v2: the two shapes render one stored instant, got {described_v2}"
    );
    let listed_v2 = ok("cluster-v2", "ListClustersV2", emulator.list_v2("").await);
    assert_eq!(
        assert_date_times_in_list(
            "cluster-v2",
            &listed_v2,
            "/clusterInfoList",
            "/creationTime",
            window
        ),
        vec![cluster_at.clone()],
        "case cluster-v2: the listing must report the stored creation instant, got {listed_v2}"
    );

    // `VpcConnection`/`ClientVpcConnection`/the two VPC-connection responses.
    let before = now_secs();
    let connection = ok(
        "vpc-connection",
        "CreateVpcConnection",
        emulator
            .create_vpc_connection(&ConnectionRequest::to(&cluster_arn))
            .await,
    );
    let connection_window = (before - 1.0, now_secs() + 1.0);
    let connection_arn = member(&connection, "/vpcConnectionArn")
        .as_str()
        .unwrap_or_else(|| panic!("CreateVpcConnection returned no ARN: {connection}"))
        .to_string();
    let connection_at = assert_date_time(
        "vpc-connection",
        &connection,
        "/creationTime",
        connection_window,
    );
    sleep_past_vpc_connection_activation();
    let described_connection = ok(
        "vpc-connection",
        "DescribeVpcConnection",
        emulator.describe_vpc_connection(&connection_arn).await,
    );
    assert_eq!(
        assert_date_time(
            "vpc-connection",
            &described_connection,
            "/creationTime",
            connection_window
        ),
        connection_at,
        "case vpc-connection: the reader must report the stored instant, got {described_connection}"
    );
    let listed_connections = ok(
        "vpc-connection",
        "ListVpcConnections",
        emulator.list_vpc_connections("").await,
    );
    assert_eq!(
        assert_date_times_in_list(
            "vpc-connection",
            &listed_connections,
            "/vpcConnections",
            "/creationTime",
            connection_window
        ),
        vec![connection_at.clone()],
        "case vpc-connection: the listing must report the stored instant, got {listed_connections}"
    );
    let client_connections = ok(
        "client-vpc-connection",
        "ListClientVpcConnections",
        emulator
            .list_client_vpc_connections(&cluster_arn, "")
            .await,
    );
    assert_eq!(
        assert_date_times_in_list(
            "client-vpc-connection",
            &client_connections,
            "/clientVpcConnections",
            "/creationTime",
            connection_window
        ),
        vec![connection_at],
        "case client-vpc-connection: the listing must report the stored instant, got \
         {client_connections}"
    );

    // `ClusterOperationInfo.creationTime`/`.endTime` and the v2 shape's
    // `startTime`/`endTime`: a finished operation reports both ends of its span.
    let version = member(&described, "/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let before = now_secs();
    let update = ok(
        "operation",
        "UpdateClusterKafkaVersion",
        emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/version", pct(&cluster_arn)),
                Some(json!({ "currentVersion": version, "targetKafkaVersion": "3.7.x" })),
            )
            .await,
    );
    let operation_arn = member(&update, "/clusterOperationArn")
        .as_str()
        .unwrap_or_else(|| panic!("the update filed no cluster operation: {update}"))
        .to_string();
    sleep_past_update();
    let operation_window = (before - 1.0, now_secs() + 1.0);

    let operation = ok(
        "operation",
        "DescribeClusterOperation",
        emulator
            .call("GET", &format!("/v1/operations/{}", pct(&operation_arn)), None)
            .await,
    );
    let operation_at = assert_date_time(
        "operation",
        &operation,
        "/clusterOperationInfo/creationTime",
        operation_window,
    );
    let operation_end = assert_date_time(
        "operation",
        &operation,
        "/clusterOperationInfo/endTime",
        operation_window,
    );
    let operations = ok(
        "operation",
        "ListClusterOperations",
        emulator
            .call(
                "GET",
                &format!("/v1/clusters/{}/operations", pct(&cluster_arn)),
                None,
            )
            .await,
    );
    assert_eq!(
        assert_date_times_in_list(
            "operation",
            &operations,
            "/clusterOperationInfoList",
            "/creationTime",
            operation_window
        ),
        vec![operation_at.clone()],
        "case operation: the listing must report the stored creation instant, got {operations}"
    );
    assert_eq!(
        assert_date_times_in_list(
            "operation",
            &operations,
            "/clusterOperationInfoList",
            "/endTime",
            operation_window
        ),
        vec![operation_end.clone()],
        "case operation: the listing must report the stored end instant, got {operations}"
    );

    // The v2 shape dates the same operation from `startTime`.
    let operation_v2 = ok(
        "operation-v2",
        "DescribeClusterOperationV2",
        emulator
            .call(
                "GET",
                &format!("/api/v2/operations/{}", pct(&operation_arn)),
                None,
            )
            .await,
    );
    assert_eq!(
        assert_date_time(
            "operation-v2",
            &operation_v2,
            "/clusterOperationInfo/startTime",
            operation_window
        ),
        operation_at,
        "case operation-v2: the two shapes render one stored instant, got {operation_v2}"
    );
    assert_eq!(
        assert_date_time(
            "operation-v2",
            &operation_v2,
            "/clusterOperationInfo/endTime",
            operation_window
        ),
        operation_end,
        "case operation-v2: the two shapes render one stored end instant, got {operation_v2}"
    );
    // `ListClusterOperationsV2` renders `ClusterOperationV2Summary`, whose two timestamp
    // members are the same date-time shape, so the listing must report the same two stored
    // instants the readers above do.
    let operations_v2 = ok(
        "operation-v2",
        "ListClusterOperationsV2",
        emulator
            .call(
                "GET",
                &format!("/api/v2/clusters/{}/operations", pct(&cluster_arn)),
                None,
            )
            .await,
    );
    assert_eq!(
        assert_date_times_in_list(
            "operation-v2",
            &operations_v2,
            "/clusterOperationInfoList",
            "/startTime",
            operation_window
        ),
        vec![operation_at],
        "case operation-v2: the listing must report the stored start instant, got {operations_v2}"
    );
    assert_eq!(
        assert_date_times_in_list(
            "operation-v2",
            &operations_v2,
            "/clusterOperationInfoList",
            "/endTime",
            operation_window
        ),
        vec![operation_end],
        "case operation-v2: the listing must report the stored end instant, got {operations_v2}"
    );
}
