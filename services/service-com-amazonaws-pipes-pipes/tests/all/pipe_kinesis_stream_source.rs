// SPDX-License-Identifier: CC0-1.0

//! A pipe whose SOURCE is an Amazon Kinesis data stream, end to end over the wire.
//!
//! Pipes, Kinesis and Amazon SQS are mounted on the SAME engine and every call goes
//! through `MultiServiceRouter::dispatch` — the one path a socket-served request takes —
//! because a stream-to-queue topology is asserted where a client sees it: the records are
//! put with `PutRecords` and they arrive at the target queue, with nothing in between asked
//! to do anything.
//!
//! The rules asserted, each over a TABLE whose rows differ along what the rule generalises
//! over:
//!
//! 1. A RUNNING pipe whose `Source` names a Kinesis data stream reads that stream itself,
//!    with no client action, and delivers every record its position covers to the target as
//!    the record envelope EventBridge Pipes documents for a Kinesis source — the record's
//!    own data base64-encoded, its partition key, its sequence number, the shard and
//!    sequence number that identify it, and the stream it was read off. The rows differ
//!    along the position the pipe reads from (and so along which records that position
//!    covers), the batch size and batching window the records are polled under, how many
//!    shards the stream has, and what the records carry.
//! 2. Every reader that looks INSIDE a polled record reads the record's payload member as
//!    its producer sent it — a Kinesis record's `data` base64-decoded, and as the JSON
//!    document that text spells when it spells one — so a `FilterCriteria` pattern admits
//!    on a field of the decoded data and a `TargetParameters.InputTemplate` placeholder
//!    renders that field's own value. The rows differ along what the pattern reads and what
//!    the template renders: a string field, a number compared numerically, a negation, a
//!    member of the envelope beside the data, a whole decoded document, and a payload that
//!    spells no document at all.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_body_paths::canonical;
use crate::pipe_data_plane::{rpc, LOOK_AGAIN, QUIET};
use crate::pipe_lifecycle::{call, describe, Reply, REGION};

/// The role every pipe below names. Nothing in this suite turns on its policies: the
/// engine carries no IAM store, which shows nothing about what the role may do.
const ROLE: &str = "arn:aws:iam::000000000000:role/pipes-kinesis-stream";

/// How long a wait for work the pipe has to do is given before it is a failure. Well above
/// the data plane's poll interval and the span a pipe takes to reach RUNNING, so a row
/// fails only when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(25);

/// The `CurrentState` a pipe that is reading its source rests in.
const RUNNING: &str = "RUNNING";

/// How far in the future the position of the row that must read nothing lies. Far enough
/// that a record put while the row runs is well before it however slow or fast the host is.
const FAR_AHEAD: Duration = Duration::from_secs(3_600);

/// Pipes and the services a Kinesis-sourced pipe reaches, on one engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One `awsJson` RPC call against Kinesis.
async fn kinesis(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "kinesis", "Kinesis_20131202", operation, body).await
}

/// One `awsJson` RPC call against Amazon SQS.
async fn sqs(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    rpc(router, "sqs", "AmazonSQS", operation, body).await
}

/// A stream called `name` with `shards` shards, waited out until it is ACTIVE; answers its
/// ARN.
async fn given_stream(router: &MultiServiceRouter, name: &str, shards: usize) -> String {
    let (status, body) = kinesis(
        router,
        "CreateStream",
        &json!({ "StreamName": name, "ShardCount": shards }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: CreateStream answered {status} {body}"
    );
    let deadline = Instant::now() + BUDGET;
    loop {
        let (status, described) = kinesis(
            router,
            "DescribeStreamSummary",
            &json!({ "StreamName": name }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name}: DescribeStreamSummary answered {status} {described}"
        );
        let summary = &described["StreamDescriptionSummary"];
        let active = summary["StreamStatus"].as_str() == Some("ACTIVE");
        let arn = summary["StreamARN"].as_str().unwrap_or_default().to_string();
        if active && !arn.is_empty() {
            return arn;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: expected the stream to reach ACTIVE with an ARN, got {described}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// A queue called `name`; answers its url and the ARN a pipe names it by.
async fn given_queue(router: &MultiServiceRouter, name: &str, fifo: bool) -> (String, String) {
    let mut request = json!({ "QueueName": if fifo { format!("{name}.fifo") } else { name.to_string() } });
    if fifo {
        request["Attributes"] = json!({ "FifoQueue": "true" });
    }
    let (status, body) = sqs(router, "CreateQueue", &request).await;
    assert_eq!(
        status, 200,
        "case {name}: CreateQueue answered {status} {body}"
    );
    let url = body["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueUrl in {body}"))
        .to_string();
    let (status, body) = sqs(
        router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {name}: GetQueueAttributes answered {status} {body}"
    );
    let arn = body["Attributes"]["QueueArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: no QueueArn in {body}"))
        .to_string();
    (url, arn)
}

/// Put `records` — (partition key, the bytes the producer puts) pairs — as ONE batch, so
/// the order they were put in is the order one shard holds them in.
async fn put_records(
    router: &MultiServiceRouter,
    stream: &str,
    case: &str,
    records: &[(&str, &str)],
) {
    let entries: Vec<Value> = records
        .iter()
        .map(|(key, payload)| {
            json!({
                // A record's `Data` is bytes, which travel base64-encoded on the wire.
                "Data": aws_smithy_types::base64::encode(payload.as_bytes()),
                "PartitionKey": key,
            })
        })
        .collect();
    let (status, body) = kinesis(
        router,
        "PutRecords",
        &json!({ "StreamARN": stream, "Records": entries }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutRecords answered {status} {body}"
    );
    assert_eq!(
        body["FailedRecordCount"].as_i64(),
        Some(0),
        "case {case:?}: every record must be stored, got {body}"
    );
}

/// `CreatePipe` for the pipe `name` under `body`.
async fn create_pipe(router: &MultiServiceRouter, name: &str, body: Value) -> Reply {
    call(router, "POST", &format!("/v1/pipes/{name}"), Some(body), REGION).await
}

/// Wait until the pipe `name` reports it RUNS, which is the state it reads its source in.
async fn wait_until_running(router: &MultiServiceRouter, name: &str) {
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = describe(router, name).await;
        if described.string("CurrentState") == RUNNING {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {name:?}: expected the pipe to reach {RUNNING}, got {described:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Receive up to ten messages from the queue at `url` and DELETE each one, so the next call
/// hands over what is behind them.
async fn drain(router: &MultiServiceRouter, url: &str, case: &str) -> Vec<String> {
    let (status, body) = sqs(
        router,
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ReceiveMessage answered {status} {body}"
    );
    let mut taken: Vec<String> = Vec::new();
    for message in body["Messages"].as_array().cloned().unwrap_or_default() {
        taken.push(message["Body"].as_str().unwrap_or_default().to_string());
        if let Some(handle) = message["ReceiptHandle"].as_str() {
            let (status, answer) = sqs(
                router,
                "DeleteMessage",
                &json!({ "QueueUrl": url, "ReceiptHandle": handle }),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: DeleteMessage answered {status} {answer}"
            );
        }
    }
    taken
}

/// Every payload delivered to `url` until there are `want` of them or the budget lapses.
async fn delivered(
    router: &MultiServiceRouter,
    url: &str,
    case: &str,
    want: usize,
) -> Vec<String> {
    let deadline = Instant::now() + BUDGET;
    let mut collected: Vec<String> = Vec::new();
    loop {
        collected.extend(drain(router, url, case).await);
        if collected.len() >= want || Instant::now() >= deadline {
            return collected;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Every payload that arrives at `url` during the whole of `QUIET` — what makes the absence
/// of a delivery an answer rather than a race.
async fn nothing_more(router: &MultiServiceRouter, url: &str, case: &str) -> Vec<String> {
    let deadline = Instant::now() + QUIET;
    let mut collected: Vec<String> = Vec::new();
    while Instant::now() < deadline {
        collected.extend(drain(router, url, case).await);
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    collected
}

/// Where a row's records are put relative to the pipe: the position a pipe reads from
/// decides which of them it covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PutWhen {
    /// Before the pipe exists at all, so what it reads is a backlog.
    BeforeThePipe,
    /// Once the pipe reports it runs, so what it reads is a live record.
    WhileItRuns,
}

/// One row of the delivery table.
struct Delivery {
    case: &'static str,
    /// `StartingPosition`: where the pipe starts reading the stream.
    starting_position: &'static str,
    /// `StartingPositionTimestamp`, as an offset from the row's own start — `None` for a
    /// position that carries no instant.
    at: Option<Offset>,
    /// When the records are put.
    put: PutWhen,
    /// `BatchSize`: how many records one poll takes.
    batch_size: usize,
    /// `MaximumBatchingWindowInSeconds`: how long a poll that has not filled its batch
    /// keeps collecting, so a batch can span several reads.
    window: usize,
    /// How many shards the stream is created with.
    shards: usize,
    /// The records the row puts, as (partition key, payload) pairs.
    records: &'static [(&'static str, &'static str)],
    /// Whether the pipe's position covers those records at all.
    covered: bool,
}

/// Which side of the row's start a configured instant lies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Offset {
    /// Before every record of the stream, so the position covers all of them.
    WellBefore,
    /// After every record the row puts, so the position covers none of them.
    WellAfter,
}

/// RULE: a RUNNING pipe whose `Source` names a Kinesis data stream reads that stream
/// itself, with no client action, and delivers every record the position it reads from
/// covers to its target as the record envelope EventBridge Pipes documents for a Kinesis
/// source — the record's own data base64-encoded, its partition key and sequence number,
/// the shard and sequence number that identify it, and the stream it was read off — in the
/// order the records of one shard were put.
///
/// The rows differ along what the rule generalises over: the position the pipe reads from
/// and therefore which records it covers (the oldest retained record, the tip, an instant
/// before every record, an instant after all of them), whether the records were put before
/// the pipe existed or while it was running, the batch size and batching window they are
/// polled under (a window makes a batch span several reads), how many shards the stream
/// has, how many records there are and what they carry.
#[tokio::test]
async fn a_running_kinesis_sourced_pipe_delivers_every_record_it_covers() {
    let rows = vec![
        Delivery {
            case: "trim-horizon-backlog",
            starting_position: "TRIM_HORIZON",
            at: None,
            put: PutWhen::BeforeThePipe,
            batch_size: 1,
            window: 0,
            shards: 1,
            records: &[
                ("orders", r#"{"order":"a"}"#),
                ("orders", r#"{"order":"b"}"#),
                ("orders", r#"{"order":"c"}"#),
            ],
            covered: true,
        },
        Delivery {
            case: "latest-live-records",
            starting_position: "LATEST",
            at: None,
            put: PutWhen::WhileItRuns,
            batch_size: 10,
            window: 0,
            shards: 1,
            records: &[("orders", "first"), ("orders", "second")],
            covered: true,
        },
        Delivery {
            case: "batched-over-a-window",
            starting_position: "TRIM_HORIZON",
            at: None,
            put: PutWhen::WhileItRuns,
            batch_size: 3,
            window: 2,
            shards: 1,
            records: &[
                ("orders", "r0"),
                ("orders", "r1"),
                ("orders", "r2"),
                ("orders", "r3"),
                ("orders", "r4"),
            ],
            covered: true,
        },
        Delivery {
            case: "two-shards-two-partition-keys",
            starting_position: "TRIM_HORIZON",
            at: None,
            put: PutWhen::BeforeThePipe,
            batch_size: 2,
            window: 0,
            shards: 2,
            records: &[
                ("left", "l0"),
                ("right", "r0"),
                ("left", "l1"),
                ("right", "r1"),
            ],
            covered: true,
        },
        Delivery {
            case: "at-timestamp-before-every-record",
            starting_position: "AT_TIMESTAMP",
            at: Some(Offset::WellBefore),
            put: PutWhen::BeforeThePipe,
            batch_size: 10,
            window: 0,
            shards: 1,
            records: &[("orders", "aged-0"), ("orders", "aged-1")],
            covered: true,
        },
        Delivery {
            // A position no record of the stream reaches: the pipe runs, reads and delivers
            // nothing at all.
            case: "at-timestamp-after-every-record",
            starting_position: "AT_TIMESTAMP",
            at: Some(Offset::WellAfter),
            put: PutWhen::WhileItRuns,
            batch_size: 10,
            window: 0,
            shards: 1,
            records: &[("orders", "unreached-0")],
            covered: false,
        },
        Delivery {
            case: "payloads-of-every-kind",
            starting_position: "TRIM_HORIZON",
            at: None,
            put: PutWhen::BeforeThePipe,
            batch_size: 10,
            window: 0,
            shards: 1,
            records: &[
                ("mixed", r#"{"a":1}"#),
                ("mixed", "plain text"),
                ("mixed", "[1,2,3]"),
                ("mixed", "42"),
            ],
            covered: true,
        },
    ];

    for row in rows {
        let router = emulator();
        let case = row.case;
        let stream = given_stream(&router, case, row.shards).await;
        let (url, target) = given_queue(&router, case, true).await;
        if row.put == PutWhen::BeforeThePipe {
            put_records(&router, &stream, case, row.records).await;
        }

        let mut parameters = json!({
            "StartingPosition": row.starting_position,
            "BatchSize": row.batch_size,
            "MaximumBatchingWindowInSeconds": row.window,
        });
        if let Some(offset) = row.at {
            let now = std::time::SystemTime::now();
            let instant = match offset {
                Offset::WellBefore => std::time::UNIX_EPOCH,
                Offset::WellAfter => now + FAR_AHEAD,
            };
            let seconds = instant
                .duration_since(std::time::UNIX_EPOCH)
                .expect("the instant is after the epoch")
                .as_secs();
            parameters["StartingPositionTimestamp"] = json!(seconds);
        }
        let created = create_pipe(
            &router,
            case,
            json!({
                "Source": stream,
                "Target": target,
                "RoleArn": ROLE,
                "DesiredState": "RUNNING",
                "SourceParameters": { "KinesisStreamParameters": parameters },
                // A FIFO target keeps the order the pipe delivered in, and each record's
                // own event id is what makes two deliveries two messages.
                "TargetParameters": {
                    "SqsQueueParameters": {
                        "MessageGroupId": "$.partitionKey",
                        "MessageDeduplicationId": "$.eventID",
                    },
                },
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );
        wait_until_running(&router, case).await;
        if row.put == PutWhen::WhileItRuns {
            put_records(&router, &stream, case, row.records).await;
        }

        if !row.covered {
            let seen = nothing_more(&router, &url, case).await;
            assert!(
                seen.is_empty(),
                "case {case:?}: a position no record reaches must deliver nothing, got {seen:?}"
            );
            continue;
        }

        let bodies = delivered(&router, &url, case, row.records.len()).await;
        assert_eq!(
            bodies.len(),
            row.records.len(),
            "case {case:?}: expected {} delivered record(s), got {bodies:?}",
            row.records.len()
        );
        let envelopes: Vec<Value> = bodies
            .iter()
            .map(|body| {
                serde_json::from_str(body).unwrap_or_else(|error| {
                    panic!("case {case:?}: a delivered body is not a record ({error}): {body:?}")
                })
            })
            .collect();

        for envelope in &envelopes {
            let key = envelope["partitionKey"].as_str().unwrap_or_default();
            let encoded = envelope["data"].as_str().unwrap_or_default();
            let decoded = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                panic!("case {case:?}: data must be base64 ({error}): {envelope}")
            });
            let payload = String::from_utf8(decoded).unwrap_or_else(|error| {
                panic!("case {case:?}: data must decode to what was put ({error}): {envelope}")
            });
            assert!(
                row.records
                    .iter()
                    .any(|(put_key, put_payload)| *put_key == key && *put_payload == payload),
                "case {case:?}: expected a record that was put, got key {key:?} payload \
                 {payload:?}"
            );
            let sequence_number = envelope["sequenceNumber"].as_str().unwrap_or_default();
            assert!(
                !sequence_number.is_empty(),
                "case {case:?}: expected the record's own sequence number, got {envelope}"
            );
            assert_eq!(
                envelope["eventSource"],
                json!("aws:kinesis"),
                "case {case:?}: expected the Kinesis event source, got {envelope}"
            );
            assert_eq!(
                envelope["eventName"],
                json!("aws:kinesis:record"),
                "case {case:?}: expected the Kinesis event name, got {envelope}"
            );
            assert_eq!(
                envelope["kinesisSchemaVersion"],
                json!("1.0"),
                "case {case:?}: expected the record's schema version, got {envelope}"
            );
            assert_eq!(
                envelope["eventSourceARN"].as_str(),
                Some(stream.as_str()),
                "case {case:?}: expected the stream it was read off, got {envelope}"
            );
            assert_eq!(
                envelope["awsRegion"],
                json!(REGION),
                "case {case:?}: expected the Region the stream is in, got {envelope}"
            );
            let event_id = envelope["eventID"].as_str().unwrap_or_default();
            assert_eq!(
                event_id.rsplit_once(':').map(|(shard, sequence)| (
                    shard.starts_with("shardId-"),
                    sequence == sequence_number
                )),
                Some((true, true)),
                "case {case:?}: expected the shard and sequence number as the event id, got \
                 {envelope}"
            );
        }

        // Every record exactly once, and the records of ONE partition key in the order they
        // were put: one key's records are all in one shard, and a shard is read in order.
        let ids: BTreeSet<String> = envelopes
            .iter()
            .map(|envelope| envelope["eventID"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids.len(),
            envelopes.len(),
            "case {case:?}: every delivery must be a distinct record, got {ids:?}"
        );
        for key in row.records.iter().map(|(key, _)| *key).collect::<BTreeSet<&str>>() {
            let put: Vec<&str> = row
                .records
                .iter()
                .filter(|(put_key, _)| *put_key == key)
                .map(|(_, payload)| *payload)
                .collect();
            let arrived: Vec<String> = envelopes
                .iter()
                .filter(|envelope| envelope["partitionKey"].as_str() == Some(key))
                .map(|envelope| decoded_data(envelope, case))
                .collect();
            assert_eq!(
                arrived,
                put.iter().map(|payload| (*payload).to_string()).collect::<Vec<String>>(),
                "case {case:?}: expected the records of {key:?} in the order they were put, \
                 got {arrived:?}"
            );
        }

        let extra = nothing_more(&router, &url, case).await;
        assert!(
            extra.is_empty(),
            "case {case:?}: expected no further delivery, got {extra:?}"
        );
    }
}

/// The payload one delivered envelope carries: its `data` member, base64-decoded.
fn decoded_data(envelope: &Value, case: &str) -> String {
    let encoded = envelope["data"].as_str().unwrap_or_default();
    let decoded = aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case {case:?}: data must be base64 ({error}): {envelope}"));
    String::from_utf8(decoded)
        .unwrap_or_else(|error| panic!("case {case:?}: data must be text ({error}): {envelope}"))
}

/// One record a filter row puts, and what the target must receive for it.
struct Filtered {
    /// The `PartitionKey` the record is put under.
    key: &'static str,
    /// The bytes the producer puts, which travel base64-encoded.
    payload: &'static str,
    /// What the target must receive for it, or `None` for a record the filter drops.
    rendered: Option<&'static str>,
}

/// One row of the decoded-payload table.
struct Reading {
    case: &'static str,
    /// The patterns `FilterCriteria.Filters` carries, when the row filters at all.
    patterns: &'static [&'static str],
    /// `TargetParameters.InputTemplate`: what the target is handed instead of the record.
    template: &'static str,
    /// The records the row puts, in put order.
    records: &'static [Filtered],
}

/// RULE: every reader that looks inside a polled record reads the record's payload member
/// as its producer sent it — a Kinesis record's `data` base64-decoded, and as the JSON
/// document that text spells when it spells one — so a `FilterCriteria` pattern admits on a
/// field of the decoded data, a record no pattern admits reaches no stage at all, and an
/// `InputTemplate` placeholder renders the value its path selects in that same reading.
///
/// The rows differ along what the rule generalises over: what the pattern reads and what
/// the template renders — a string field of the decoded document, a number compared
/// numerically, a negation, a member of the envelope beside the data, the whole decoded
/// document, and a payload spelling no document at all, which a pattern written for a
/// document cannot admit.
#[tokio::test]
async fn a_pattern_and_a_template_read_the_decoded_data_of_a_kinesis_record() {
    let rows = vec![
        Reading {
            case: "a-field-of-the-decoded-data",
            patterns: &[r#"{"data": {"city": ["Seattle"]}}"#],
            template: "<$.data.order>",
            records: &[
                Filtered {
                    key: "orders",
                    payload: r#"{"city": "Portland", "order": "dropped-1"}"#,
                    rendered: None,
                },
                Filtered {
                    key: "orders",
                    payload: r#"{"city": "Seattle", "order": "admitted-1"}"#,
                    rendered: Some("admitted-1"),
                },
            ],
        },
        Reading {
            case: "a-number-of-the-decoded-data",
            patterns: &[r#"{"data": {"amount": [{"numeric": [">", 10]}]}}"#],
            template: r#"{"amount": <$.data.amount>, "key": "<$.partitionKey>"}"#,
            records: &[
                Filtered {
                    key: "payments",
                    payload: r#"{"amount": 5}"#,
                    rendered: None,
                },
                Filtered {
                    key: "payments",
                    payload: r#"{"amount": 25}"#,
                    rendered: Some(r#"{"amount": 25, "key": "payments"}"#),
                },
            ],
        },
        Reading {
            case: "a-negation-over-the-decoded-data",
            patterns: &[r#"{"data": {"kind": [{"anything-but": "ignore"}]}}"#],
            template: "<$.data.kind>",
            records: &[
                Filtered {
                    key: "kinds",
                    payload: r#"{"kind": "ignore"}"#,
                    rendered: None,
                },
                Filtered {
                    key: "kinds",
                    payload: r#"{"kind": "keep"}"#,
                    rendered: Some("keep"),
                },
            ],
        },
        Reading {
            case: "a-member-of-the-envelope-beside-the-data",
            patterns: &[r#"{"partitionKey": ["keep"]}"#],
            template: "<$.data>",
            records: &[
                Filtered {
                    key: "drop",
                    payload: r#"{"id": "dropped-2"}"#,
                    rendered: None,
                },
                Filtered {
                    key: "keep",
                    payload: r#"{"id": "kept-2"}"#,
                    // The document the decoded data spells, re-emitted as the JSON it is.
                    rendered: Some(r#"{"id": "kept-2"}"#),
                },
            ],
        },
        Reading {
            // A payload spelling no document is the text it carries, so a clause written
            // for a document admits it no more than one whose values disagreed — and a
            // clause written for a value reads it.
            case: "a-payload-that-spells-no-document",
            patterns: &[r#"{"data": ["plain-marker"]}"#],
            template: "<$.data>",
            records: &[
                Filtered {
                    key: "texts",
                    payload: r#"{"data": "plain-marker"}"#,
                    rendered: None,
                },
                Filtered {
                    key: "texts",
                    payload: "plain-marker",
                    rendered: Some("plain-marker"),
                },
            ],
        },
        Reading {
            // No filter at all: every record enters the pipe and the template renders a
            // nested field of each one's decoded data.
            case: "no-filter-with-a-nested-path",
            patterns: &[],
            template: "<$.data.order.id>",
            records: &[
                Filtered {
                    key: "nested",
                    payload: r#"{"order": {"id": "deep-1"}}"#,
                    rendered: Some("deep-1"),
                },
                Filtered {
                    key: "nested",
                    payload: r#"{"order": {"id": "deep-2"}}"#,
                    rendered: Some("deep-2"),
                },
            ],
        },
    ];

    for row in rows {
        let router = emulator();
        let case = row.case;
        let stream = given_stream(&router, case, 1).await;
        let (url, target) = given_queue(&router, case, false).await;

        let mut source_parameters = json!({
            "KinesisStreamParameters": { "StartingPosition": "TRIM_HORIZON", "BatchSize": 1 },
        });
        if !row.patterns.is_empty() {
            let filters: Vec<Value> = row
                .patterns
                .iter()
                .map(|pattern| json!({ "Pattern": pattern }))
                .collect();
            source_parameters["FilterCriteria"] = json!({ "Filters": filters });
        }
        let created = create_pipe(
            &router,
            case,
            json!({
                "Source": stream,
                "Target": target,
                "RoleArn": ROLE,
                "DesiredState": "RUNNING",
                "SourceParameters": source_parameters,
                "TargetParameters": { "InputTemplate": row.template },
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreatePipe must succeed, got {created:?}"
        );

        let put: Vec<(&str, &str)> = row
            .records
            .iter()
            .map(|record| (record.key, record.payload))
            .collect();
        put_records(&router, &stream, case, &put).await;

        let wanted: BTreeSet<String> = row
            .records
            .iter()
            .filter_map(|record| record.rendered)
            .map(canonical)
            .collect();
        let mut bodies = delivered(&router, &url, case, wanted.len()).await;
        // Everything the pipe must deliver has arrived; a record it must DROP would arrive
        // late, so the quiet window is what makes its absence an answer rather than a race.
        bodies.extend(nothing_more(&router, &url, case).await);
        let seen: BTreeSet<String> = bodies.iter().map(|body| canonical(body)).collect();
        assert_eq!(
            seen, wanted,
            "case {case:?}: expected the target to receive exactly {wanted:?}, got {bodies:?}"
        );
        assert_eq!(
            bodies.len(),
            wanted.len(),
            "case {case:?}: expected {} delivery(ies), got {bodies:?}",
            wanted.len()
        );
    }
}
