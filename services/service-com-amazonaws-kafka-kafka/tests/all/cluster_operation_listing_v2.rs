// SPDX-License-Identifier: CC0-1.0

//! `ListClusterOperationsV2`, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from, through the shared harness in
//! `tests/all/kafka_harness.rs`.
//!
//! RULE under test: `ListClusterOperationsV2` reports the operations the cluster
//! the request names has recorded — every one of them and nothing else — latest
//! first, each as the summary of the record the by-ARN V2 reader reports for the
//! same ARN, and `MaxResults` / `NextToken` page that history without dropping
//! or repeating an entry, the last page carrying no token. It serves every kind
//! of cluster, because the summary reports only the members each kind shares and
//! names the kind itself. Reading the history changes nothing a later caller
//! sees. A `ClusterArn` that is not an MSK cluster ARN is a
//! `BadRequestException`, a well-formed one this endpoint's region holds no
//! cluster for is a `NotFoundException`, and a `NextToken` this listing did not
//! mint for this cluster is a `BadRequestException`.
//!
//! The rows differ along what the rule generalises over: how many operations a
//! cluster has recorded (none, one, two, three), which updates filed them (so
//! the operation type each entry reports differs per row), the kind of cluster
//! the history belongs to, and the page size the listing is walked at (one, two,
//! and the whole history in one page).

use serde_json::{json, Value};

use crate::kafka_harness::{
    pct, reported_instant_secs, sleep_past_activation, sleep_past_update, ClusterRequest, Emulator,
    Reply, ACCOUNT, REGION,
};

/// The wire values this suite expects, written out rather than imported from
/// the implementation, so an assertion cannot agree with a constant that is
/// wrong.
///
/// `UPDATE_COMPLETE` is the state Amazon MSK reports for an update that has
/// finished; `PROVISIONED` and `SERVERLESS` are the members of the model's
/// `ClusterType` enum, which `ClusterOperationV2Summary$ClusterType` targets;
/// the not-found wording is the one this service answers for a resource that
/// does not exist (its apostrophe is U+2019, as the service writes it).
const OPERATION_STATE_COMPLETE: &str = "UPDATE_COMPLETE";
const CLUSTER_TYPE_PROVISIONED: &str = "PROVISIONED";
const CLUSTER_TYPE_SERVERLESS: &str = "SERVERLESS";
const NOT_FOUND_MESSAGE: &str = "The requested resource doesn\u{2019}t exist.";
const INVALID_TOKEN_MESSAGE: &str = "The pagination token is not valid.";

/// The members `ClusterOperationV2Summary` declares, from
/// `smithy_query(query="shape", shape="com.amazonaws.kafka#ClusterOperationV2Summary")`:
/// exactly the members of `ClusterOperationV2` that every cluster kind shares,
/// so the summary of an operation and the full V2 record of it must agree on
/// each one.
const SHARED_MEMBERS: [&str; 7] = [
    "/clusterArn",
    "/clusterType",
    "/startTime",
    "/endTime",
    "/operationArn",
    "/operationState",
    "/operationType",
];

/// One update a row files: the route under the cluster its `http` binding
/// names, the body it submits beside `currentVersion`, and the `operationType`
/// the operation it files must report.
struct Update {
    path: &'static str,
    body: Value,
    operation_type: &'static str,
}

/// One row of the table: a cluster, the updates whose operations it must
/// report, and the page size its history is walked at.
struct Row {
    /// The cluster name, which is also this row's case name.
    name: &'static str,
    updates: Vec<Update>,
    /// The `MaxResults` the paged walk asks for, or `None` to let the listing
    /// page at the size it uses when the request names none.
    page_size: Option<i32>,
}

fn storage_update() -> Update {
    Update {
        path: "storage",
        body: json!({ "volumeSizeGB": 200 }),
        operation_type: "UPDATE_STORAGE",
    }
}

fn broker_storage_update() -> Update {
    Update {
        path: "nodes/storage",
        body: json!({
            "targetBrokerEBSVolumeInfo": [{ "kafkaBrokerNodeId": "ALL", "volumeSizeGB": 500 }]
        }),
        operation_type: "UPDATE_BROKER_STORAGE",
    }
}

fn broker_type_update() -> Update {
    Update {
        path: "nodes/type",
        body: json!({ "targetInstanceType": "kafka.m5.2xlarge" }),
        operation_type: "UPDATE_BROKER_TYPE",
    }
}

fn connectivity_update() -> Update {
    Update {
        path: "connectivity",
        body: json!({ "zookeeperAccess": { "enabled": false } }),
        operation_type: "UPDATE_CONNECTIVITY",
    }
}

/// An Apache Kafka version upgrade. `3.7.x` is the next ACTIVE version above
/// the `3.6.0` every cluster this harness creates runs, so it is a target the
/// service accepts.
fn kafka_version_update() -> Update {
    Update {
        path: "version",
        body: json!({ "targetKafkaVersion": "3.7.x" }),
        operation_type: "UPDATE_CLUSTER_KAFKA_VERSION",
    }
}

/// The table: the history length, the updates that built it and the page size
/// it is read at all differ per row, so the rule is asserted over a set of
/// histories rather than over one.
fn rows() -> Vec<Row> {
    vec![
        Row {
            name: "no-operations-v2",
            updates: Vec::new(),
            page_size: None,
        },
        Row {
            name: "one-operation-v2",
            updates: vec![broker_type_update()],
            page_size: Some(1),
        },
        Row {
            name: "two-operations-v2",
            updates: vec![storage_update(), kafka_version_update()],
            page_size: Some(1),
        },
        Row {
            name: "three-operations-v2",
            updates: vec![
                broker_storage_update(),
                connectivity_update(),
                broker_type_update(),
            ],
            page_size: Some(2),
        },
        Row {
            name: "unpaged-history-v2",
            updates: vec![connectivity_update(), storage_update()],
            page_size: None,
        },
    ]
}

/// `ListClusterOperationsV2`: `GET /api/v2/clusters/{ClusterArn}/operations`,
/// per the operation's `http` binding in the model. `query` is the whole query
/// string including its `?`, or empty.
async fn list_v2(emulator: &Emulator, cluster_arn: &str, query: &str) -> Reply {
    list_v2_in(emulator, REGION, cluster_arn, query).await
}

/// The same listing, as the endpoint of `region` serves it.
async fn list_v2_in(emulator: &Emulator, region: &str, cluster_arn: &str, query: &str) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!("/api/v2/clusters/{}/operations{query}", pct(cluster_arn)),
            None,
        )
        .await
}

/// `ListClusterOperations`: the V1 listing of the SAME history, whose page
/// tokens this listing must not accept.
async fn list_v1(emulator: &Emulator, cluster_arn: &str, query: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/operations{query}", pct(cluster_arn)),
            None,
        )
        .await
}

/// The reported page, as a list of `ClusterOperationV2Summary` values.
fn entries(reply: &Reply, case: &str) -> Vec<Value> {
    reply
        .at("/clusterOperationInfoList")
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a clusterOperationInfoList array, got {}",
                reply.body
            )
        })
        .clone()
}

/// A member of a reported value, or `Value::Null` when it is absent.
fn member(entry: &Value, pointer: &str) -> Value {
    entry.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Submit `update` against `cluster_arn`, assert it was accepted, wait for the
/// clock to owe its completion, and hand back the ARN of the operation it
/// filed.
async fn file_update(emulator: &Emulator, case: &str, cluster_arn: &str, update: &Update) -> String {
    let version = emulator
        .describe(cluster_arn)
        .await
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let mut body = update.body.clone();
    body["currentVersion"] = json!(version);
    let reply = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/{}", pct(cluster_arn), update.path),
            Some(body),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected the {} update to be accepted, got {} {}",
        update.path, reply.status, reply.body
    );
    let operation_arn = reply
        .at("/clusterOperationArn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        operation_arn.starts_with(&format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/")),
        "case {case}: expected the {} update to answer with a cluster-operation ARN, got \
         {operation_arn:?}",
        update.path
    );
    // The next update is only accepted once this one has landed, and a settled
    // operation is also what lets the listing be asserted against a finished
    // state.
    sleep_past_update();
    operation_arn
}

/// Walk the whole history at `page_size`, asserting each page's size and token,
/// and hand back the entries in the order the pages reported them.
async fn walk_pages(
    emulator: &Emulator,
    case: &str,
    cluster_arn: &str,
    page_size: Option<i32>,
    expected_total: usize,
) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(ref next) = token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={}", pct(next)));
        }
        let reply = list_v2(emulator, cluster_arn, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected 200 for the listing at {query:?}, got {} {}",
            reply.status, reply.body
        );
        let page = entries(&reply, case);
        if let Some(size) = page_size {
            assert!(
                page.len() <= size as usize,
                "case {case}: expected at most {size} entries per page, got {}",
                page.len()
            );
        }
        collected.extend(page);
        match reply.at("/nextToken").as_str() {
            Some(next) => {
                assert!(
                    collected.len() < expected_total,
                    "case {case}: expected no token once the {expected_total} recorded \
                     operations have been reported, got {next:?} after {}",
                    collected.len()
                );
                token = Some(next.to_string());
            }
            None => break,
        }
    }
    collected
}

/// Every row: the cluster's whole operation history is reported, latest first,
/// in one page and paged, each entry the summary of the record the by-ARN V2
/// reader reports for the same ARN.
#[tokio::test(flavor = "current_thread")]
async fn pages_the_operations_a_cluster_recorded_latest_first() {
    let emulator = Emulator::new();
    for row in rows() {
        let case = row.name;
        let cluster_arn = emulator.create_ok(&ClusterRequest::named(case)).await;
        sleep_past_activation();

        let mut filed: Vec<(String, &'static str)> = Vec::new();
        for update in &row.updates {
            let operation_arn = file_update(&emulator, case, &cluster_arn, update).await;
            filed.push((operation_arn, update.operation_type));
        }
        // The history is reported latest first, so the expected order is the
        // order the updates were filed in, reversed.
        let expected: Vec<(String, &'static str)> = filed.iter().rev().cloned().collect();

        let whole = list_v2(&emulator, &cluster_arn, "").await;
        assert_eq!(
            whole.status, 200,
            "case {case}: expected 200 for the whole history, got {} {}",
            whole.status, whole.body
        );
        assert_eq!(
            whole.at("/nextToken"),
            &Value::Null,
            "case {case}: expected no token on a page that reported the whole history, got {}",
            whole.at("/nextToken")
        );
        let listed = entries(&whole, case);
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {case}: expected the {} recorded operations to be reported, got {}",
            expected.len(),
            whole.body
        );
        for (position, ((operation_arn, operation_type), entry)) in
            expected.iter().zip(listed.iter()).enumerate()
        {
            assert_eq!(
                member(entry, "/operationArn"),
                json!(operation_arn),
                "case {case}: expected the operation at position {position} to be \
                 {operation_arn:?}, got {:?}",
                member(entry, "/operationArn")
            );
            assert_eq!(
                member(entry, "/clusterArn"),
                json!(cluster_arn),
                "case {case}: expected the operation at position {position} to name its own \
                 cluster, got {:?}",
                member(entry, "/clusterArn")
            );
            assert_eq!(
                member(entry, "/clusterType"),
                json!(CLUSTER_TYPE_PROVISIONED),
                "case {case}: expected clusterType {CLUSTER_TYPE_PROVISIONED} at position \
                 {position}, got {:?}",
                member(entry, "/clusterType")
            );
            assert_eq!(
                member(entry, "/operationType"),
                json!(operation_type),
                "case {case}: expected operationType {operation_type} at position {position}, \
                 got {:?}",
                member(entry, "/operationType")
            );
            assert_eq!(
                member(entry, "/operationState"),
                json!(OPERATION_STATE_COMPLETE),
                "case {case}: expected operationState {OPERATION_STATE_COMPLETE} at position \
                 {position}, once the update span has passed, got {:?}",
                member(entry, "/operationState")
            );
            assert!(
                reported_instant_secs(&member(entry, "/startTime"), case) > 0.0,
                "case {case}: expected the operation at position {position} to report a \
                 startTime, got {:?}",
                member(entry, "/startTime")
            );
            assert!(
                reported_instant_secs(&member(entry, "/endTime"), case) > 0.0,
                "case {case}: expected a finished operation to report an endTime at position \
                 {position}, got {:?}",
                member(entry, "/endTime")
            );

            // The summary is the shared part of the one stored record the
            // by-ARN V2 reader reports, so the two cannot disagree about any
            // member both of them carry — and the summary carries no per-kind
            // detail arm at all.
            let described = emulator
                .call(
                    "GET",
                    &format!("/api/v2/operations/{}", pct(operation_arn)),
                    None,
                )
                .await;
            assert_eq!(
                described.status, 200,
                "case {case}: expected DescribeClusterOperationV2 to return 200 for \
                 {operation_arn:?}, got {} {}",
                described.status, described.body
            );
            let full = described.at("/clusterOperationInfo").clone();
            for pointer in SHARED_MEMBERS {
                assert_eq!(
                    member(entry, pointer),
                    member(&full, pointer),
                    "case {case}: expected the listing and the by-ARN V2 reader to agree on \
                     {pointer} for {operation_arn:?}, got {:?} and {:?}",
                    member(entry, pointer),
                    member(&full, pointer)
                );
            }
            for arm in ["/provisioned", "/serverless"] {
                assert_eq!(
                    member(entry, arm),
                    Value::Null,
                    "case {case}: expected no {arm} detail on a summary, got {:?}",
                    member(entry, arm)
                );
            }
        }

        // Paged: the pages concatenate to exactly the same history, in the same
        // order, however small a page the caller asks for.
        let paged = walk_pages(&emulator, case, &cluster_arn, row.page_size, listed.len()).await;
        assert_eq!(
            paged, listed,
            "case {case}: expected the paged walk at {:?} per page to report the same history, \
             got {paged:?} against {listed:?}",
            row.page_size
        );

        // Listing changes nothing a later caller sees.
        let again = list_v2(&emulator, &cluster_arn, "").await;
        assert_eq!(
            again.body, whole.body,
            "case {case}: expected a second listing to report the same history, got {} then {}",
            whole.body, again.body
        );
    }
}

/// This view reports an operation through the members every cluster kind shares
/// and names the kind, so a cluster with no customer broker nodes is SERVED
/// rather than refused on its kind: it has recorded nothing, which is an empty
/// history. The rows differ along the serverless cluster's own configuration:
/// one VPC configuration without client authentication, and two with SASL/IAM
/// on.
#[tokio::test(flavor = "current_thread")]
async fn serves_a_cluster_with_no_broker_nodes() {
    let emulator = Emulator::new();
    let serverless = [
        (
            "one-vpc-v2",
            vec![vec!["subnet-0a1b2c3d", "subnet-1a2b3c4d"]],
            None,
        ),
        (
            "two-vpcs-iam-v2",
            vec![
                vec!["subnet-0a1b2c3d", "subnet-1a2b3c4d"],
                vec!["subnet-2a3b4c5d", "subnet-3a4b5c6d"],
            ],
            Some(true),
        ),
    ];
    for (case, vpc_configs, iam) in serverless {
        let cluster_arn = emulator.create_serverless_ok(case, &vpc_configs, iam).await;
        let described = emulator.describe_v2(&cluster_arn).await;
        assert_eq!(
            described.at("/clusterInfo/clusterType"),
            &json!(CLUSTER_TYPE_SERVERLESS),
            "case {case}: expected the cluster this row stands up to be serverless, got {:?}",
            described.at("/clusterInfo/clusterType")
        );
        for query in ["", "?maxResults=1"] {
            let reply = list_v2(&emulator, &cluster_arn, query).await;
            assert_eq!(
                reply.status, 200,
                "case {case}: expected 200 for the listing at {query:?}, got {} {}",
                reply.status, reply.body
            );
            assert_eq!(
                entries(&reply, case),
                Vec::<Value>::new(),
                "case {case}: expected an empty history for a cluster that has recorded \
                 nothing, got {}",
                reply.body
            );
            assert_eq!(
                reply.at("/nextToken"),
                &Value::Null,
                "case {case}: expected no token on a page that reported the whole history, got {}",
                reply.at("/nextToken")
            );
        }
    }
}

/// A `ClusterArn` that names no cluster of this endpoint, and a `NextToken`
/// this listing did not mint for the cluster being listed, are refused rather
/// than answered with a page. The rows differ along how the value fails: the
/// wrong resource type, a truncated resource path, a foreign service, something
/// that is no ARN at all, a cluster this region does not hold, and a real
/// cluster read through another region's endpoint.
#[tokio::test(flavor = "current_thread")]
async fn refuses_a_cluster_arn_that_names_no_cluster() {
    let emulator = Emulator::new();
    let malformed = [
        (
            "operation-arn",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster-operation/a-cluster/\
                 11111111-2222-3333-4444-555555555555-1/66666666-7777-8888-9999-000000000000"
            ),
        ),
        (
            "missing-cluster-id",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/a-cluster"),
        ),
        (
            "other-service",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:cluster/a-cluster/cluster-id"),
        ),
        ("not-an-arn", "a-cluster".to_string()),
    ];
    for (case, value) in malformed {
        let reply = list_v2(&emulator, &value, "").await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400 for {value:?}, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("BadRequestException"),
            "case {case}: expected a BadRequestException for {value:?}, got {:?}",
            reply.error_code()
        );
    }

    // Well formed, and held by nobody: the cluster does not exist, so there is
    // no history to serve — which is not the same answer as an empty one.
    let cluster_arn = emulator
        .create_ok(&ClusterRequest::named("listed-elsewhere-v2"))
        .await;
    sleep_past_activation();
    let unheld = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/no-such-cluster/\
         11111111-2222-3333-4444-555555555555-1"
    );
    for (case, value, region) in [
        ("unheld-cluster", unheld, REGION),
        ("another-region", cluster_arn.clone(), "eu-central-1"),
    ] {
        let reply = list_v2_in(&emulator, region, &value, "").await;
        assert_eq!(
            reply.status, 404,
            "case {case}: expected 404 for {value:?} at the {region} endpoint, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("NotFoundException"),
            "case {case}: expected a NotFoundException for {value:?}, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            reply.error_message(),
            NOT_FOUND_MESSAGE,
            "case {case}: expected the missing-resource wording, got {:?}",
            reply.error_message()
        );
    }

    // A token this listing did not mint for THIS cluster resumes nothing: the
    // rows are a value that is no token at all, a token the V1 listing of this
    // very history minted, and a token this listing minted for another
    // cluster's history.
    file_update(
        &emulator,
        "listed-elsewhere-v2",
        &cluster_arn,
        &storage_update(),
    )
    .await;
    file_update(
        &emulator,
        "listed-elsewhere-v2",
        &cluster_arn,
        &broker_type_update(),
    )
    .await;
    let other_arn = emulator
        .create_ok(&ClusterRequest::named("another-history-v2"))
        .await;
    sleep_past_activation();
    file_update(&emulator, "another-history-v2", &other_arn, &storage_update()).await;
    file_update(
        &emulator,
        "another-history-v2",
        &other_arn,
        &connectivity_update(),
    )
    .await;
    let v1_token = list_v1(&emulator, &cluster_arn, "?maxResults=1")
        .await
        .at("/nextToken")
        .as_str()
        .map(str::to_string);
    let other_token = list_v2(&emulator, &other_arn, "?maxResults=1")
        .await
        .at("/nextToken")
        .as_str()
        .map(str::to_string);
    let mut tokens: Vec<(&str, String)> = vec![("not-a-token", "zzzz".to_string())];
    tokens.push((
        "the-v1-listings-token",
        v1_token.expect("the V1 listing of a two-operation history mints a token at maxResults=1"),
    ));
    tokens.push((
        "another-clusters-token",
        other_token
            .expect("this listing of a two-operation history mints a token at maxResults=1"),
    ));
    for (case, token) in tokens {
        let reply = list_v2(&emulator, &cluster_arn, &format!("?nextToken={}", pct(&token))).await;
        assert_eq!(
            reply.status, 400,
            "case {case}: expected 400 for the token {token:?}, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with("BadRequestException"),
            "case {case}: expected a BadRequestException for the token {token:?}, got {:?}",
            reply.error_code()
        );
        assert_eq!(
            reply.error_message(),
            INVALID_TOKEN_MESSAGE,
            "case {case}: expected the invalid-token wording, got {:?}",
            reply.error_message()
        );
    }
}
