// SPDX-License-Identifier: CC0-1.0

//! The Topic Management surface against a SERVERLESS cluster, driven end to end
//! over the same router the emulator serves from.
//!
//! The RULE under test: the Topic Management operations serve a cluster whose
//! brokers the AWS API manages, so every one of them refuses a cluster whose
//! `ClusterType` is `SERVERLESS` on the cluster's KIND alone — with
//! [`SERVERLESS_TOPIC_APIS_CODE`] and the whole-fixed-text
//! [`SERVERLESS_TOPIC_APIS_MESSAGE`] — immediately after the cluster is resolved
//! and before any further state-dependent check or any registration, so the answer
//! does not depend on the rest of the request and a refused call changes nothing.
//!
//! Each suite asserts that over a TABLE rather than one request: the rows differ
//! along what the rule generalises over — which operation is called, each
//! operation's own parameters (replication factor from 1 up past any broker count,
//! partition count, `Configs` presence, topic-name form, `MaxResults` /
//! `TopicNameFilter`), and the serverless cluster's own configuration (VPC
//! configurations, client authentication). The controls in the same suites pin the
//! two boundaries of the rule: the SAME calls serve a provisioned cluster, and a
//! request that resolves NO cluster is still answered by the checks that precede
//! the gate.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the request builders
//! come from [`crate::kafka_harness`], the crate's shared harness.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    RESOURCE_NOT_FOUND_MESSAGE, SERVERLESS_TOPIC_APIS_CODE, SERVERLESS_TOPIC_APIS_MESSAGE,
    SERVERLESS_TOPIC_APIS_STATUS,
};

use crate::kafka_harness::{pct, sleep_past_activation, ClusterRequest, Emulator, ACCOUNT, REGION};

/// One Topic Management call: how the model binds it, and the parameters this row
/// exercises.
#[derive(Debug, Clone)]
struct TopicCall {
    case: &'static str,
    method: &'static str,
    /// The path and query under `/v1/clusters/{ClusterArn}`, as the model's `http`
    /// bindings on the topic operations name them.
    suffix: String,
    body: Option<Value>,
}

impl TopicCall {
    fn new(case: &'static str, method: &'static str, suffix: String, body: Option<Value>) -> Self {
        Self {
            case,
            method,
            suffix,
            body,
        }
    }

    fn uri(&self, cluster_arn: &str) -> String {
        format!("/v1/clusters/{}{}", pct(cluster_arn), self.suffix)
    }
}

/// `CreateTopic`'s body, with the members the row exercises.
fn create_body(name: &str, partitions: i64, replication: i64, configs: Option<&str>) -> Value {
    let mut body = json!({
        "topicName": name,
        "partitionCount": partitions,
        "replicationFactor": replication,
    });
    if let Some(configs) = configs {
        body["configs"] = json!(configs);
    }
    body
}

/// Every implemented Topic Management call, as a table: the rows differ along which
/// operation is called and along each operation's own parameters, which is what the
/// rule generalises over. The replication factors span from 1 — the value a
/// provisioned cluster of any size accepts, and the one a broker-count bound would
/// have let through — up past any broker count a cluster could have.
fn topic_calls() -> Vec<TopicCall> {
    vec![
        TopicCall::new(
            "CreateTopic replicationFactor=1",
            "POST",
            "/topics".to_string(),
            Some(create_body("orders", 1, 1, None)),
        ),
        TopicCall::new(
            "CreateTopic replicationFactor=2 with configs",
            "POST",
            "/topics".to_string(),
            Some(create_body(
                "orders.v2",
                12,
                2,
                Some("cmV0ZW50aW9uLm1zPTg2NDAwMDAw"),
            )),
        ),
        TopicCall::new(
            "CreateTopic replicationFactor=3",
            "POST",
            "/topics".to_string(),
            Some(create_body("orders_v2-2025", 3, 3, None)),
        ),
        TopicCall::new(
            "CreateTopic replicationFactor=99",
            "POST",
            "/topics".to_string(),
            Some(create_body("__audit.trail", 100, 99, Some(""))),
        ),
        TopicCall::new(
            "DescribeTopic",
            "GET",
            format!("/topics/{}", pct("orders")),
            None,
        ),
        TopicCall::new(
            "DescribeTopic of a dotted name",
            "GET",
            format!("/topics/{}", pct("orders.v2")),
            None,
        ),
        TopicCall::new(
            "DeleteTopic",
            "DELETE",
            format!("/topics/{}", pct("orders")),
            None,
        ),
        TopicCall::new(
            "UpdateTopic raising the partition count",
            "PUT",
            format!("/topics/{}", pct("orders")),
            Some(json!({ "partitionCount": 24 })),
        ),
        TopicCall::new(
            "UpdateTopic submitting configs",
            "PUT",
            format!("/topics/{}", pct("orders.v2")),
            Some(json!({ "configs": "cmV0ZW50aW9uLm1zPTYwMDAwMA==" })),
        ),
        TopicCall::new("ListTopics", "GET", "/topics".to_string(), None),
        TopicCall::new(
            "ListTopics with a page size and a filter",
            "GET",
            "/topics?maxResults=10&topicNameFilter=ord".to_string(),
            None,
        ),
        TopicCall::new(
            "DescribeTopicPartitions",
            "GET",
            format!("/topics/{}/partitions", pct("orders")),
            None,
        ),
        TopicCall::new(
            "DescribeTopicPartitions with a page size",
            "GET",
            format!("/topics/{}/partitions?maxResults=10", pct("orders.v2")),
            None,
        ),
    ]
}

/// RULE: every Topic Management operation refuses a `SERVERLESS` cluster with
/// [`SERVERLESS_TOPIC_APIS_CODE`] and the whole-fixed-text
/// [`SERVERLESS_TOPIC_APIS_MESSAGE`], whatever else the request carries — the
/// replication factor in particular never enters the answer — and the message
/// carries no value from the request or from state.
#[tokio::test]
async fn every_topic_api_refuses_a_serverless_cluster() {
    let emulator = Emulator::new();

    // Serverless clusters differing in what a serverless create can differ in, so
    // the refusal is shown to rest on the cluster's KIND and not on its
    // configuration.
    let clusters: Vec<(&str, String)> = vec![
        (
            "one VPC configuration, no client authentication",
            emulator
                .create_serverless_ok("sl-topics-minimal", &[vec!["subnet-0a1b2c3d"]], None)
                .await,
        ),
        (
            "two VPC configurations with SASL/IAM",
            emulator
                .create_serverless_ok(
                    "sl-topics-iam",
                    &[
                        vec!["subnet-0a1b2c3d", "subnet-1b2c3d4e"],
                        vec!["subnet-2c3d4e5f"],
                    ],
                    Some(true),
                )
                .await,
        ),
    ];
    // A serverless cluster is ACTIVE once the clock owes it, which is the state the
    // Topic Management calls below are made against.
    sleep_past_activation();

    for (cluster_case, arn) in &clusters {
        let described = emulator.describe_v2(arn).await;
        assert_eq!(
            described.at("/clusterInfo/clusterType").as_str(),
            Some("SERVERLESS"),
            "case {cluster_case:?}: the suite's cluster must be SERVERLESS, got {}",
            described.body
        );

        for call in topic_calls() {
            let reply = emulator
                .call(call.method, &call.uri(arn), call.body.clone())
                .await;
            assert_eq!(
                reply.status, SERVERLESS_TOPIC_APIS_STATUS,
                "case {cluster_case:?} / {}: expected HTTP {SERVERLESS_TOPIC_APIS_STATUS}, got {} {}",
                call.case, reply.status, reply.body
            );
            assert!(
                reply.error_code().ends_with(SERVERLESS_TOPIC_APIS_CODE),
                "case {cluster_case:?} / {}: expected {SERVERLESS_TOPIC_APIS_CODE}, got {:?}",
                call.case,
                reply.error_code()
            );
            assert_eq!(
                reply.error_message(),
                SERVERLESS_TOPIC_APIS_MESSAGE,
                "case {cluster_case:?} / {}: expected the fixed serverless refusal message, got {:?}",
                call.case,
                reply.error_message()
            );
            // The message is fixed text end to end: nothing of the request or of
            // the stored cluster reaches it.
            for value in [arn.as_str(), "orders", "99", "100", "12"] {
                assert!(
                    !reply.error_message().contains(value),
                    "case {cluster_case:?} / {}: the refusal message must carry no request or \
                     state value, but {value:?} is in {:?}",
                    call.case,
                    reply.error_message()
                );
            }
        }

        // None of the refusals disturbed the cluster, and none of them registered a
        // topic: the cluster is still the ACTIVE serverless record it was.
        let after = emulator.describe_v2(arn).await;
        assert_eq!(
            after.at("/clusterInfo/state").as_str(),
            Some("ACTIVE"),
            "case {cluster_case:?}: the refusals must leave the cluster ACTIVE, got {}",
            after.body
        );
        assert_eq!(
            after.at("/clusterInfo/clusterType").as_str(),
            Some("SERVERLESS"),
            "case {cluster_case:?}: the refusals must leave the cluster SERVERLESS, got {}",
            after.body
        );
    }
}

/// RULE (the other boundary of the same gate): the refusal is about the cluster's
/// kind, so the SAME calls serve a provisioned cluster — and a request that resolves
/// no cluster at all is answered by the checks that run before the gate, since a
/// cluster that is not there has no kind to refuse.
#[tokio::test]
async fn the_same_calls_serve_a_provisioned_cluster() {
    let emulator = Emulator::new();
    let serverless = emulator
        .create_serverless_ok("sl-topics-neighbour", &[vec!["subnet-0a1b2c3d"]], None)
        .await;

    // Provisioned clusters of different sizes: the topic calls below are accepted on
    // each, so nothing about the gate depends on a cluster's broker count.
    let mut rows: Vec<(i64, String)> = Vec::new();
    for brokers in [1_i64, 3, 6] {
        let mut request = ClusterRequest::named(&format!("pv-topics-{brokers}"));
        request.brokers = brokers;
        // Three subnets are what the harness's minimal request carries; a
        // one-broker cluster is created from the same body.
        rows.push((brokers, emulator.create_ok(&request).await));
    }
    sleep_past_activation();

    for (brokers, arn) in &rows {
        let created = emulator
            .call(
                "POST",
                &format!("/v1/clusters/{}/topics", pct(arn)),
                Some(create_body("orders", 3, 1, None)),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case brokers={brokers}: expected CreateTopic to serve a provisioned cluster, got {} {}",
            created.status, created.body
        );
        for (case, method, suffix) in [
            ("DescribeTopic", "GET", format!("/topics/{}", pct("orders"))),
            (
                "DescribeTopicPartitions",
                "GET",
                format!("/topics/{}/partitions", pct("orders")),
            ),
            ("ListTopics", "GET", "/topics".to_string()),
            ("DeleteTopic", "DELETE", format!("/topics/{}", pct("orders"))),
        ] {
            let reply = emulator
                .call(method, &format!("/v1/clusters/{}{suffix}", pct(arn)), None)
                .await;
            assert_eq!(
                reply.status, 200,
                "case brokers={brokers} / {case}: expected 200 on a provisioned cluster, got {} {}",
                reply.status, reply.body
            );
            assert_ne!(
                reply.error_code(),
                SERVERLESS_TOPIC_APIS_CODE,
                "case brokers={brokers} / {case}: a provisioned cluster must not be refused as \
                 serverless, got {}",
                reply.body
            );
        }
    }

    // A request that resolves NO cluster never reaches the gate: an ARN that is not
    // an MSK cluster ARN is the bad request it always was, and a well-formed one
    // this endpoint holds no cluster for is the modelled 404 — neither is the
    // serverless refusal, because neither names a cluster with a kind.
    let unresolvable: Vec<(&str, String, u16, &str)> = vec![
        (
            "an ARN of another service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:queue-not-a-cluster"),
            400,
            "BadRequestException",
        ),
        (
            "an absent cluster",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-3333-4444-555555555555-1"),
            404,
            "NotFoundException",
        ),
        (
            "a serverless cluster of another region's endpoint",
            serverless.clone(),
            404,
            "NotFoundException",
        ),
    ];
    for (case, arn, status, code) in &unresolvable {
        // The last row is the serverless cluster as an endpoint that does not hold
        // it sees it: no cluster resolves there, so no kind can be refused.
        let region = if case.contains("another region") {
            "eu-west-1"
        } else {
            REGION
        };
        let reply = emulator
            .call_in(
                region,
                "POST",
                &format!("/v1/clusters/{}/topics", pct(arn)),
                Some(create_body("orders", 1, 1, None)),
            )
            .await;
        assert_eq!(
            reply.status, *status,
            "case {case:?}: expected HTTP {status} from CreateTopic, got {} {}",
            reply.status, reply.body
        );
        assert!(
            reply.error_code().ends_with(code),
            "case {case:?}: expected {code}, got {:?}",
            reply.error_code()
        );
        if *status == 404 {
            assert_eq!(
                reply.error_message(),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the service's not-found wording, got {:?}",
                reply.error_message()
            );
        }
    }
}
