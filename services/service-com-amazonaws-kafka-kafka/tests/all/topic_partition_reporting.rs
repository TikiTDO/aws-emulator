// SPDX-License-Identifier: CC0-1.0

//! `DescribeTopicPartitions` on a provisioned cluster, driven end to end over the
//! same `MultiServiceRouter` the emulator serves from.
//!
//! The RULE under test: a topic of `PartitionCount` partitions reports exactly
//! the partition ids `0 .. PartitionCount - 1`, in ascending id order, each with a
//! replica set of `min(ReplicationFactor, brokers)` DISTINCT broker ids of the
//! cluster that holds it, led by its first replica — Apache Kafka's own default
//! assignment, whose leaders walk the brokers round-robin so the leader of
//! partition `p` is the cluster's `p % brokers`-th broker — and with every replica
//! in sync. The reader pages that sequence under `MaxResults` / `NextToken`, and
//! it reports, so reading it twice says the same thing and leaves every other
//! reader of the topic alone.
//!
//! Each suite asserts that over a TABLE rather than one recorded request: the rows
//! differ along what the rule generalises over — the cluster's broker count, the
//! topic's partition count (from one up past a whole default page), its
//! replication factor (one, two, and every broker), the form of the topic name,
//! and the page size the caller asks for.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the `CreateCluster`
//! request builder come from [`crate::kafka_harness`], the crate's shared harness.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{MAX_RESULTS_LIMIT, RESOURCE_NOT_FOUND_MESSAGE};

use crate::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// Stand a provisioned cluster of `brokers` nodes up and hand back its ARN. The
/// cluster is only the context a topic needs, so the row is the harness's minimal
/// `CreateCluster` request with the broker count set.
async fn cluster(emulator: &Emulator, name: &str, brokers: i64) -> String {
    let mut request = ClusterRequest::named(name);
    request.brokers = brokers;
    emulator.create_ok(&request).await
}

async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    replication: i64,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(json!({
                "topicName": name,
                "partitionCount": partitions,
                "replicationFactor": replication,
            })),
        )
        .await
}

/// `DescribeTopicPartitions` on `topic`, with `query` carrying whichever of
/// `maxResults` / `nextToken` the row exercises.
async fn describe_partitions(
    emulator: &Emulator,
    cluster_arn: &str,
    topic: &str,
    query: &str,
) -> Reply {
    describe_partitions_in(emulator, REGION, cluster_arn, topic, query).await
}

/// The same call, signed for `region` — which decides which endpoint's clusters
/// the ARN is resolved against.
async fn describe_partitions_in(
    emulator: &Emulator,
    region: &str,
    cluster_arn: &str,
    topic: &str,
    query: &str,
) -> Reply {
    emulator
        .call_in(
            region,
            "GET",
            &format!(
                "/v1/clusters/{}/topics/{}/partitions{query}",
                pct(cluster_arn),
                pct(topic)
            ),
            None,
        )
        .await
}

async fn describe_topic(emulator: &Emulator, cluster_arn: &str, topic: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/topics/{}", pct(cluster_arn), pct(topic)),
            None,
        )
        .await
}

/// Every partition the reader reports for `topic`, following `NextToken` to the
/// end. `page_size` is the `MaxResults` each request asks for, or `None` to let
/// the service page at its own size.
async fn all_partitions(
    emulator: &Emulator,
    cluster_arn: &str,
    topic: &str,
    page_size: Option<i64>,
    case: &str,
) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut query = String::new();
        if let Some(size) = page_size {
            query.push_str(&format!("?maxResults={size}"));
        }
        if let Some(resume) = &token {
            query.push(if query.is_empty() { '?' } else { '&' });
            query.push_str(&format!("nextToken={}", pct(resume)));
        }
        let reply = describe_partitions(emulator, cluster_arn, topic, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected DescribeTopicPartitions to answer 200 for {query:?}, got {} {}",
            reply.status, reply.body
        );
        let page = reply
            .at("/partitions")
            .as_array()
            .cloned()
            .unwrap_or_default();
        let bound = page_size.unwrap_or(i64::from(MAX_RESULTS_LIMIT));
        assert!(
            i64::try_from(page.len()).unwrap_or(i64::MAX) <= bound,
            "case {case}: a page may hold at most {bound} partitions, got {}",
            page.len()
        );
        reported.extend(page);
        token = reply.at("/nextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        pages += 1;
        assert!(
            pages < 1_000,
            "case {case}: the paginated read did not terminate after {pages} pages"
        );
    }
    reported
}

/// One row of the table: a cluster of `brokers` nodes holding a topic of
/// `partitions` partitions at `replication`.
#[derive(Debug, Clone, Copy)]
struct Row {
    case: &'static str,
    topic: &'static str,
    brokers: i64,
    partitions: i64,
    replication: i64,
}

/// The rows the rule is checked over. They differ along the broker count, the
/// partition count (one, a count below / equal to / above the broker count, and
/// one past a whole default page of [`MAX_RESULTS_LIMIT`]), the replication
/// factor (one, two, and every broker the cluster has) and the form of the topic
/// name.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "one broker, one partition, one replica",
            topic: "orders",
            brokers: 1,
            partitions: 1,
            replication: 1,
        },
        Row {
            case: "three brokers, a partition each, replicated onto every broker",
            topic: "orders.v2",
            brokers: 3,
            partitions: 3,
            replication: 3,
        },
        Row {
            case: "three brokers, more partitions than brokers, two replicas",
            topic: "orders_v2-2025",
            brokers: 3,
            partitions: 7,
            replication: 2,
        },
        Row {
            case: "six brokers, a wide topic, three replicas",
            topic: "events.stream-6",
            brokers: 6,
            partitions: 12,
            replication: 3,
        },
        Row {
            case: "two brokers, more partitions than one default page",
            topic: "wide.audit_trail",
            brokers: 2,
            partitions: i64::from(MAX_RESULTS_LIMIT) + 1,
            replication: 2,
        },
    ]
}

/// Assert that `reported` is the whole partition list the row's topic must have:
/// the ids `0 .. partitions - 1` in ascending order, each with a replica set the
/// row's cluster can place and a leader that walks the brokers round-robin.
fn assert_partition_layout(reported: &[Value], row: &Row) {
    let case = row.case;
    assert_eq!(
        i64::try_from(reported.len()).unwrap_or(i64::MAX),
        row.partitions,
        "case {case:?}: expected {} partitions, got {}",
        row.partitions,
        reported.len()
    );
    // Apache Kafka cannot place more replicas of a partition than there are
    // brokers, so this is the replica count the cluster can actually hold.
    let replicas_expected = row.replication.min(row.brokers);
    for (index, partition) in reported.iter().enumerate() {
        let id = i64::try_from(index).unwrap_or(i64::MAX);
        assert_eq!(
            partition.get("partition").and_then(Value::as_i64),
            Some(id),
            "case {case:?}: expected partition {id} at position {index}, got {partition}"
        );
        let replicas: Vec<i64> = partition
            .get("replicas")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        assert_eq!(
            i64::try_from(replicas.len()).unwrap_or(i64::MAX),
            replicas_expected,
            "case {case:?}: partition {id} must have {replicas_expected} replicas, got {partition}"
        );
        for replica in &replicas {
            assert!(
                (1..=row.brokers).contains(replica),
                "case {case:?}: partition {id} names replica {replica}, which is not one of the \
                 cluster's {} brokers: {partition}",
                row.brokers
            );
        }
        let mut distinct = replicas.clone();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(
            distinct.len(),
            replicas.len(),
            "case {case:?}: partition {id} places two replicas on one broker: {partition}"
        );
        // The leader is the partition's first replica, and the first replicas walk
        // the cluster's brokers round-robin, so consecutive partitions are led by
        // different brokers.
        assert_eq!(
            partition.get("leader").and_then(Value::as_i64),
            replicas.first().copied(),
            "case {case:?}: partition {id}'s leader must be its first replica, got {partition}"
        );
        assert_eq!(
            replicas.first().copied(),
            Some(id % row.brokers + 1),
            "case {case:?}: expected partition {id} to be led by broker {}, got {partition}",
            id % row.brokers + 1
        );
        // Nothing in this emulator can put a broker behind, so every replica of
        // every partition is in sync.
        let isr: Vec<i64> = partition
            .get("isr")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        assert_eq!(
            isr, replicas,
            "case {case:?}: partition {id}'s in-sync set must be its replica set, got {partition}"
        );
    }
}

/// RULE: a topic's partitions are the ids `0 .. PartitionCount - 1` in ascending
/// order — the same count `DescribeTopic` reports — each placed on
/// `min(ReplicationFactor, brokers)` distinct brokers of its cluster and led by
/// its first replica, with every replica in sync.
#[tokio::test]
async fn a_topic_reports_one_partition_per_partition_count() {
    let emulator = Emulator::new();
    let rows = rows();
    let mut arns: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        arns.push(cluster(&emulator, &format!("tp-layout-{index}"), row.brokers).await);
    }
    // One wait for the whole table: every cluster is ACTIVE once the clock owes it.
    sleep_past_activation();

    for (row, arn) in rows.iter().zip(&arns) {
        let created = create_topic(&emulator, arn, row.topic, row.partitions, row.replication).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateTopic to return 200, got {} {}",
            row.case, created.status, created.body
        );

        let reported = all_partitions(&emulator, arn, row.topic, None, row.case).await;
        assert_partition_layout(&reported, row);

        // The two readers of one topic agree about how many partitions it has.
        let described = describe_topic(&emulator, arn, row.topic).await;
        assert_eq!(
            described.at("/partitionCount").as_i64(),
            Some(row.partitions),
            "case {:?}: expected DescribeTopic to report {} partitions, got {}",
            row.case,
            row.partitions,
            described.body
        );

        // Reporting only: reading the partitions changes nothing a later caller
        // sees, so the second read is the first one again.
        let again = all_partitions(&emulator, arn, row.topic, None, row.case).await;
        assert_eq!(
            again, reported,
            "case {:?}: a second read must report the same partitions",
            row.case
        );
        let described_again = describe_topic(&emulator, arn, row.topic).await;
        assert_eq!(
            (
                described_again.status,
                described_again.at("/partitionCount").clone()
            ),
            (described.status, described.at("/partitionCount").clone()),
            "case {:?}: reading the partitions must leave DescribeTopic unchanged, got {}",
            row.case,
            described_again.body
        );
    }
}

/// RULE: `MaxResults` / `NextToken` page that one sequence — every page is at most
/// the asked-for size, the pages concatenate back to the whole list in the same
/// order, and the page that exhausts the partitions carries no token.
#[tokio::test]
async fn max_results_pages_the_same_partition_sequence() {
    let emulator = Emulator::new();
    let rows = rows();
    let mut arns: Vec<String> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        arns.push(cluster(&emulator, &format!("tp-pages-{index}"), row.brokers).await);
    }
    sleep_past_activation();

    for (row, arn) in rows.iter().zip(&arns) {
        let created = create_topic(&emulator, arn, row.topic, row.partitions, row.replication).await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected CreateTopic to return 200, got {} {}",
            row.case, created.status, created.body
        );
        let whole = all_partitions(&emulator, arn, row.topic, None, row.case).await;

        // Page sizes spanning the model's range: the smallest page, sizes that
        // divide the partition count and sizes that do not, and the largest page
        // the range allows.
        for size in [1_i64, 2, 3, 50, i64::from(MAX_RESULTS_LIMIT)] {
            let case = format!("{} maxResults={size}", row.case);
            let paged = all_partitions(&emulator, arn, row.topic, Some(size), &case).await;
            assert_eq!(
                paged, whole,
                "case {case}: paging must report the same partitions in the same order"
            );
        }

        // The first page of a topic with more partitions than fit in it promises
        // another page; a page size at least the partition count closes the list.
        let one = describe_partitions(&emulator, arn, row.topic, "?maxResults=1").await;
        let expect_token = row.partitions > 1;
        assert_eq!(
            one.at("/nextToken").as_str().is_some(),
            expect_token,
            "case {:?}: a one-partition page of a {}-partition topic must {}carry a token, got {}",
            row.case,
            row.partitions,
            if expect_token { "" } else { "not " },
            one.body
        );
    }
}

/// RULE: a `NextToken` carries the list it was minted from, so a token is only
/// accepted on the topic it was minted for; anything else is a bad request, and a
/// refusal reports no partitions at all.
#[tokio::test]
async fn a_token_from_another_list_is_refused() {
    let emulator = Emulator::new();
    let arn = cluster(&emulator, "tp-tokens", 3).await;
    sleep_past_activation();
    for (topic, partitions) in [("alpha", 5_i64), ("beta", 5), ("gamma.v2", 9)] {
        let created = create_topic(&emulator, &arn, topic, partitions, 2).await;
        assert_eq!(
            created.status, 200,
            "case {topic:?}: expected CreateTopic to return 200, got {} {}",
            created.status, created.body
        );
    }
    let other = cluster(&emulator, "tp-tokens-other", 3).await;
    sleep_past_activation();
    let created = create_topic(&emulator, &other, "alpha", 5, 2).await;
    assert_eq!(
        created.status, 200,
        "expected CreateTopic on the second cluster to return 200, got {} {}",
        created.status, created.body
    );

    let page = describe_partitions(&emulator, &arn, "alpha", "?maxResults=2").await;
    let token = page
        .at("/nextToken")
        .as_str()
        .expect("the first page of a five-partition topic carries a token")
        .to_string();

    // The same token resumes the list it was minted from.
    let resumed = describe_partitions(
        &emulator,
        &arn,
        "alpha",
        &format!("?maxResults=2&nextToken={}", pct(&token)),
    )
    .await;
    assert_eq!(
        resumed.at("/partitions/0/partition").as_i64(),
        Some(2),
        "expected the token to resume after partition 1, got {}",
        resumed.body
    );

    // And nowhere else: another topic of the same cluster, the same topic name on
    // another cluster, and a value that is not a token this service minted.
    let rejected: Vec<(&str, String, String, String)> = vec![
        (
            "another topic of the same cluster",
            arn.clone(),
            "beta".to_string(),
            token.clone(),
        ),
        (
            "a topic of a longer list on the same cluster",
            arn.clone(),
            "gamma.v2".to_string(),
            token.clone(),
        ),
        (
            "the same topic name on another cluster",
            other.clone(),
            "alpha".to_string(),
            token.clone(),
        ),
        (
            "a value that is not base64",
            arn.clone(),
            "alpha".to_string(),
            "not-a-token".to_string(),
        ),
        (
            "base64 of someone else's plaintext",
            arn.clone(),
            "alpha".to_string(),
            "a2Fma2E6dG9waWNzOmFybiNhbHBoYQ==".to_string(),
        ),
    ];
    for (case, cluster_arn, topic, candidate) in &rejected {
        let reply = describe_partitions(
            &emulator,
            cluster_arn,
            topic,
            &format!("?nextToken={}", pct(candidate)),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case:?}: expected a 400 BadRequestException for a token of another list, got {} {}",
            reply.status,
            reply.body
        );
        assert!(
            reply.at("/partitions").as_array().is_none(),
            "case {case:?}: a refused read must report no partitions, got {}",
            reply.body
        );
    }
}

/// RULE: the reader resolves the cluster and then the topic, so a value that is
/// not an MSK cluster ARN and a name Apache Kafka itself would refuse are bad
/// requests, while a well-formed ARN this endpoint holds no cluster for and a
/// legal name the cluster holds no topic for are both the missing resource they
/// are — HTTP 404 `NotFoundException` with the service's fixed not-found
/// sentence.
#[tokio::test]
async fn an_unresolvable_cluster_or_topic_is_refused() {
    let emulator = Emulator::new();
    let arn = cluster(&emulator, "tp-refusals", 3).await;
    sleep_past_activation();
    let created = create_topic(&emulator, &arn, "kept", 4, 2).await;
    assert_eq!(
        created.status, 200,
        "expected the sibling topic to be created, got {} {}",
        created.status, created.body
    );

    // Topic names the cluster does not hold, in every form a legal Apache Kafka
    // name takes.
    let absent = [
        "ghost",
        "ghost.v2",
        "ghost_v2-2025",
        "g",
        "...",
        "KEPT",
    ];
    for name in absent {
        let reply = describe_partitions(&emulator, &arn, name, "").await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (404, "NotFoundException"),
            "case {name:?}: expected a 404 NotFoundException for a topic the cluster does not \
             hold, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.error_message(),
            RESOURCE_NOT_FOUND_MESSAGE,
            "case {name:?}: expected the service's fixed not-found sentence, got {:?}",
            reply.error_message()
        );
    }

    // Names Apache Kafka's own rule refuses are bad requests, not misses.
    for name in [".", "..", "bad*name", "spaced name"] {
        let reply = describe_partitions(&emulator, &arn, name, "").await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {name:?}: expected a 400 BadRequestException for an illegal topic name, got {} {}",
            reply.status,
            reply.body
        );
    }

    // And the cluster the partitions are read FROM is resolved the same way every
    // other topic reader resolves it.
    let unresolvable: Vec<(&str, String, u16, &str)> = vec![
        (
            // An ARN this account holds no cluster resource under: the
            // authorization gate refuses it ahead of any lookup, because no
            // resource-scoped permission can be matched against it.
            "an ARN of another service",
            format!("arn:aws:sqs:{REGION}:{ACCOUNT}:queue-not-a-cluster"),
            403,
            "AccessDeniedException",
        ),
        (
            "a value that is no ARN at all",
            "tp-refusals".to_string(),
            400,
            "BadRequestException",
        ),
        (
            "a well-formed ARN no cluster is registered under",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/\
                 11111111-2222-3333-4444-555555555555-1"
            ),
            404,
            "NotFoundException",
        ),
    ];
    for (case, candidate, status, code) in &unresolvable {
        let reply = describe_partitions(&emulator, candidate, "kept", "").await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (*status, *code),
            "case {case:?}: expected HTTP {status} {code}, got {} {}",
            reply.status,
            reply.body
        );
    }

    // A cluster of this endpoint is not a cluster of another one: the same ARN
    // read in another region resolves nothing.
    let elsewhere = describe_partitions_in(&emulator, "eu-west-1", &arn, "kept", "").await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (404, "NotFoundException"),
        "expected another region's endpoint to hold no such cluster, got {} {}",
        elsewhere.status,
        elsewhere.body
    );

    // None of the refusals disturbed the topic the cluster really holds.
    let kept = all_partitions(&emulator, &arn, "kept", None, "kept after refusals").await;
    assert_eq!(
        kept.len(),
        4,
        "expected the sibling topic to keep its four partitions, got {kept:?}"
    );
}
