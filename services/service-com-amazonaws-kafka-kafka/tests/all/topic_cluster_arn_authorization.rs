// SPDX-License-Identifier: CC0-1.0

//! The two ORDERED gates every Topic Management call passes before it reads
//! anything, driven end to end over the same `MultiServiceRouter` the emulator
//! serves from (the shared `kafka_harness`).
//!
//! RULE under test: a topic call is authorized on the `ClusterArn` it names before
//! any resource is resolved, and a permission for it is scoped to a CLUSTER
//! resource — so a value that IS an ARN but names no MSK cluster of this account
//! matches no permission, reaches no grant, and is refused HTTP 403
//! `AccessDeniedException` naming the principal, the action and the resource the
//! decision was taken over; a well-formed cluster ARN passes that gate and is
//! answered by the resolution behind it (`NotFoundException` where this endpoint
//! holds no such cluster). A value that is not an ARN at all names no resource for
//! a policy to be matched against and keeps the service's parameter refusal, HTTP
//! 400 `BadRequestException`.
//!
//! The rows differ along what the rule generalises over: what makes the value name
//! no cluster (another vendor's namespace, another resource kind of this service, a
//! cluster resource field missing its id or carrying an extra segment, an empty
//! account or region segment), what makes it no ARN at all (free text, a truncated
//! ARN, a non-ASCII value), and which of the six topic operations answers — the
//! gate is the surface's, not one handler's.

use serde_json::{json, Value};

use service_com_amazonaws_kafka_kafka::state::RESOURCE_NOT_FOUND_MESSAGE;

use super::kafka_harness::{pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION};

/// A cluster id of the form this service mints, for an ARN naming a cluster no
/// registry of this emulator holds.
const GHOST_ID: &str = "11111111-2222-4333-8444-555555555555-1";

/// The IAM action a topic call is authorized under, and the wire code and status an
/// authorization refusal carries — spelled out here rather than read from the
/// service, so a drift in its own constants fails this suite.
const CLUSTER_CONNECT_ACTION: &str = "kafka-cluster:Connect";
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The principal the account's own published credential speaks for, which is the
/// identity a refusal of these calls names.
fn caller() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/emulator")
}

/// The sentence a request that reaches no grant is refused with: fixed connectives
/// around the principal, the action and the resource the decision was taken over,
/// closed by IAM's default-deny clause naming the action nothing allowed.
fn not_authorized(resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {CLUSTER_CONNECT_ACTION} on resource: {resource} \
         because no identity-based policy allows the {CLUSTER_CONNECT_ACTION} action",
        caller()
    )
}

/// One topic operation, as a request this suite can make for any `ClusterArn`.
struct TopicCall {
    name: &'static str,
    method: &'static str,
    /// The URI, built from the percent-encoded cluster ARN.
    uri: fn(&str) -> String,
    body: Option<Value>,
}

/// Every operation of the Topic Management surface, with the route the model binds
/// it to (`smithy_query(query="operation",operation="<op>")`).
fn topic_calls() -> Vec<TopicCall> {
    vec![
        TopicCall {
            name: "CreateTopic",
            method: "POST",
            uri: |arn| format!("/v1/clusters/{}/topics", pct(arn)),
            body: Some(json!({
                "topicName": "orders",
                "partitionCount": 1,
                "replicationFactor": 1,
            })),
        },
        TopicCall {
            name: "ListTopics",
            method: "GET",
            uri: |arn| format!("/v1/clusters/{}/topics", pct(arn)),
            body: None,
        },
        TopicCall {
            name: "DescribeTopic",
            method: "GET",
            uri: |arn| format!("/v1/clusters/{}/topics/orders", pct(arn)),
            body: None,
        },
        TopicCall {
            name: "DescribeTopicPartitions",
            method: "GET",
            uri: |arn| format!("/v1/clusters/{}/topics/orders/partitions", pct(arn)),
            body: None,
        },
        TopicCall {
            name: "UpdateTopic",
            method: "PUT",
            uri: |arn| format!("/v1/clusters/{}/topics/orders", pct(arn)),
            body: Some(json!({ "partitionCount": 2 })),
        },
        TopicCall {
            name: "DeleteTopic",
            method: "DELETE",
            uri: |arn| format!("/v1/clusters/{}/topics/orders", pct(arn)),
            body: None,
        },
    ]
}

/// Make `call` for `cluster_arn`.
async fn make(emulator: &Emulator, call: &TopicCall, cluster_arn: &str) -> Reply {
    emulator
        .call(call.method, &(call.uri)(cluster_arn), call.body.clone())
        .await
}

/// Values that ARE ARNs and name no MSK cluster of this account: nothing can hold
/// a permission scoped to them, so the authorization gate refuses every topic call
/// carrying one — naming the value as the resource the decision was taken over.
fn arns_naming_no_cluster() -> Vec<(&'static str, String)> {
    vec![
        (
            "another vendor's namespace",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:instance/i-0a1b2c3d"),
        ),
        (
            "another resource kind of this service",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/a-configuration/{GHOST_ID}"),
        ),
        (
            "a cluster resource field missing its id",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/only-a-name"),
        ),
        (
            "a cluster resource field with an extra segment",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/a-cluster/{GHOST_ID}/extra"),
        ),
        (
            "an empty region segment",
            format!("arn:aws:kafka::{ACCOUNT}:cluster/a-cluster/{GHOST_ID}"),
        ),
        (
            "an empty account segment",
            format!("arn:aws:kafka:{REGION}::cluster/a-cluster/{GHOST_ID}"),
        ),
    ]
}

/// Values that are no ARN at all, which name no resource for a policy to be
/// matched against and are answered as the parameter they are.
fn values_that_are_no_arn() -> Vec<(&'static str, String)> {
    vec![
        ("free text", "not-a-cluster-arn".to_string()),
        ("free text with spaces", "not an arn at all".to_string()),
        ("a truncated ARN", "arn:aws:kafka".to_string()),
        ("a non-ASCII value", "ニセのクラスター-arn".to_string()),
        (
            "a long opaque value",
            format!("not-a-cluster-arn-{}", "x".repeat(200)),
        ),
    ]
}

/// RULE: every topic call carrying an ARN that names no cluster of this account is
/// refused by authorization, with the one sentence and the one code — whichever
/// operation answers and whatever makes the value name no cluster.
#[tokio::test]
async fn an_arn_that_names_no_cluster_is_refused_by_authorization_on_every_topic_call() {
    let emulator = Emulator::new();

    for (case, value) in arns_naming_no_cluster() {
        for call in topic_calls() {
            let reply = make(&emulator, &call, &value).await;
            assert_eq!(
                (reply.status, reply.error_code(), reply.error_message()),
                (
                    ACCESS_DENIED_STATUS,
                    ACCESS_DENIED_CODE,
                    not_authorized(&value).as_str()
                ),
                "case {case:?}/{}: expected the authorization refusal, got {} {}",
                call.name,
                reply.status,
                reply.body
            );
        }
    }
}

/// RULE: a value that is no ARN at all names no resource to decide over, so it
/// keeps the service's parameter refusal — the gate does not swallow a mistyped
/// path segment.
#[tokio::test]
async fn a_value_that_is_no_arn_at_all_stays_a_parameter_refusal() {
    let emulator = Emulator::new();

    for (case, value) in values_that_are_no_arn() {
        for call in topic_calls() {
            let reply = make(&emulator, &call, &value).await;
            assert_eq!(
                (reply.status, reply.error_code()),
                (400, "BadRequestException"),
                "case {case:?}/{}: expected 400 BadRequestException, got {} {}",
                call.name,
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some("clusterArn"),
                "case {case:?}/{}: expected the refusal to name clusterArn, got {}",
                call.name,
                reply.body
            );
        }
    }
}

/// RULE: the gates are ORDERED and distinct — a well-formed cluster ARN passes
/// authorization and reaches the resolution behind it, which answers
/// `NotFoundException` for a cluster this endpoint does not hold, while the same
/// call on an ARN naming no cluster never gets that far. A cluster of another
/// ACCOUNT is refused by the resource-policy half of the same gate, so it is an
/// authorization answer too, not a miss.
#[tokio::test]
async fn a_well_formed_cluster_arn_passes_the_gate_and_reaches_the_resolution() {
    let emulator = Emulator::new();

    let absent = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/{GHOST_ID}");
    let unmatchable = format!("arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/only-a-name");
    let foreign = format!("arn:aws:kafka:{REGION}:111122223333:cluster/foreign/{GHOST_ID}");

    for call in topic_calls() {
        // Past the authorization gate, into the resolution: this endpoint holds no
        // such cluster.
        let reply = make(&emulator, &call, &absent).await;
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
            "case \"an absent cluster\"/{}: expected the not-found refusal, got {} {}",
            call.name,
            reply.status,
            reply.body
        );

        // The same call one segment away from that ARN never reaches the
        // resolution: the gate answers first, and says nothing about what the
        // registry holds.
        let denied = make(&emulator, &call, &unmatchable).await;
        assert_eq!(
            (denied.status, denied.error_code()),
            (ACCESS_DENIED_STATUS, ACCESS_DENIED_CODE),
            "case \"an unmatchable cluster ARN\"/{}: expected the authorization refusal, got {} {}",
            call.name,
            denied.status,
            denied.body
        );
        assert!(
            !denied.error_message().contains(RESOURCE_NOT_FOUND_MESSAGE),
            "case \"an unmatchable cluster ARN\"/{}: the refusal must not report whether the \
             cluster exists, got {}",
            call.name,
            denied.body
        );

        // A cluster of another account is a resource the caller holds no policy
        // over, which is the other half of the same gate.
        let other_account = make(&emulator, &call, &foreign).await;
        assert_eq!(
            (other_account.status, other_account.error_code()),
            (ACCESS_DENIED_STATUS, ACCESS_DENIED_CODE),
            "case \"another account's cluster\"/{}: expected the authorization refusal, got {} {}",
            call.name,
            other_account.status,
            other_account.body
        );
    }
}

/// RULE: the gate refuses only what names no cluster — a cluster this endpoint
/// DOES hold is served, so the refusals above are about the value and not about a
/// surface that refuses everything.
#[tokio::test]
async fn the_cluster_this_endpoint_holds_is_still_served() {
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("topic-gate-control"))
        .await;
    sleep_past_activation();

    let created = emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(&cluster)),
            Some(json!({
                "topicName": "orders",
                "partitionCount": 1,
                "replicationFactor": 1,
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"the held cluster\": expected CreateTopic to be served, got {} {}",
        created.status, created.body
    );

    let listed = emulator
        .call("GET", &format!("/v1/clusters/{}/topics", pct(&cluster)), None)
        .await;
    assert_eq!(
        listed.status, 200,
        "case \"the held cluster\": expected ListTopics to be served, got {} {}",
        listed.status, listed.body
    );
    assert_eq!(
        listed
            .at("/topics")
            .as_array()
            .map(|topics| topics
                .iter()
                .filter_map(|topic| topic.get("topicName").and_then(Value::as_str))
                .count()),
        Some(1),
        "case \"the held cluster\": expected the created topic to be listed, got {}",
        listed.body
    );
}
