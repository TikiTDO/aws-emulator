// SPDX-License-Identifier: CC0-1.0

//! The KeyspacesStreams consumer read loop, end to end over the wire.
//!
//! Every test drives the service through the production router with awsJson1_0
//! requests (`POST /` + `X-Amz-Target: KeyspacesStreams.<Op>`), so what is asserted is
//! the HTTP behaviour a client sees, not a handler's return value. Streams are seeded
//! through `crate::state`'s write side — the Keyspaces control plane's seam — because
//! this service binds no operation that creates one.
//!
//! The tables below span the classes the rules generalise over (record count and page
//! size, every `StreamViewType`, every `ShardIteratorType`, every refusal), rather than
//! pinning one recorded request.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::ops::types::{
    KeyspacesCell, KeyspacesCellValue, KeyspacesRow,
};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::state::{
    service_state, ChangeRecord, PageCursor, PageScope, MAX_TRACKED_ITERATORS,
    MAX_TRACKED_PAGE_TOKENS, PAGE_TOKEN_LIFETIME, SHARD_ITERATOR_LIFETIME,
    STATUS_TRANSITION_DELAY,
};
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Every `StreamViewType` the model declares — the crate's own generated catalog
/// (`src/ops/types/shared_types.rs`, from the model's `StreamViewType` enum), so this
/// test cannot drift from the model.
use service_com_amazonaws_keyspacesstreams_keyspacesstreams::ops::types::STREAM_VIEW_TYPE_VALUES;

/// Every `ShardIteratorType` the model declares, from
/// `smithy_query(query="operation", operation="GetShardIterator")`.
const SHARD_ITERATOR_TYPES: &[&str] = &[
    "AFTER_SEQUENCE_NUMBER",
    "AT_SEQUENCE_NUMBER",
    "LATEST",
    "TRIM_HORIZON",
];

/// One second, the margin by which a test pushes a token past its lifetime.
const SECOND: Duration = Duration::from_secs(1);

/// The emulator under test: one shared engine, one service, authentication off (the
/// requests here are unsigned).
fn emulator() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// One awsJson1_0 call, returning the HTTP status and the parsed body.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("https://cassandra-streams.us-east-1.amazonaws.com/")
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("KeyspacesStreams.{operation}"))
        .body(bytes::Bytes::from(body.to_string()))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A successful call, or a panic naming the status and body that came back instead.
async fn ok(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert_eq!(
        status, 200,
        "{operation} {body} must succeed, got HTTP {status} {parsed}"
    );
    parsed
}

/// A text cell, the shape a captured column value has on the wire.
fn text_cell(value: &str) -> KeyspacesCellValue {
    KeyspacesCellValue::textT(value.to_string())
}

/// A row image holding one value column.
fn row(marker: &str) -> KeyspacesRow {
    let mut value_cells = HashMap::new();
    value_cells.insert(
        "name".to_string(),
        KeyspacesCell {
            value: Some(text_cell(marker)),
            metadata: None,
        },
    );
    KeyspacesRow {
        value_cells: Some(value_cells),
        static_cells: None,
        row_metadata: None,
    }
}

/// One captured change to the row `key`, carrying both images.
fn change(key: &str) -> ChangeRecord {
    let mut partition_keys = BTreeMap::new();
    partition_keys.insert("id".to_string(), text_cell(key));
    let mut clustering_keys = BTreeMap::new();
    clustering_keys.insert("seq".to_string(), text_cell(key));
    ChangeRecord {
        origin: "USER".to_string(),
        partition_keys,
        clustering_keys,
        new_image: Some(row(&format!("{key}-after"))),
        old_image: Some(row(&format!("{key}-before"))),
    }
}

/// Enable CDC for one table and capture `records` changes on it, as Keyspaces does
/// when the table is created with a CDC specification and then written to.
fn seed_stream(
    engine: &engine::InMemoryEngine,
    region: &str,
    keyspace_name: &str,
    table_name: &str,
    stream_view_type: &str,
    records: usize,
) -> String {
    let state = service_state(engine).expect("the shared engine holds this service's state");
    let mut state = state.lock();
    let stream_arn = state
        .enable_stream(region, keyspace_name, table_name, stream_view_type)
        .unwrap_or_else(|| {
            panic!("seeding {keyspace_name}.{table_name}: {stream_view_type} must be accepted")
        });
    for index in 0..records {
        let sequence_number = state.append_change_record(&stream_arn, change(&format!("r{index}")));
        assert!(
            sequence_number.is_some(),
            "seeding {keyspace_name}.{table_name}: record {index} must be captured"
        );
    }
    stream_arn
}

/// The one shard of a freshly enabled stream, read back through `GetStream`.
async fn only_shard_id(router: &MultiServiceRouter, stream_arn: &str) -> String {
    let stream = ok(router, "GetStream", &json!({ "streamArn": stream_arn })).await;
    let shards = stream["shards"]
        .as_array()
        .unwrap_or_else(|| panic!("GetStream must report shards, got {stream}"));
    assert_eq!(
        shards.len(),
        1,
        "a freshly enabled stream has one shard, got {stream}"
    );
    shards[0]["shardId"]
        .as_str()
        .unwrap_or_else(|| panic!("a shard must carry its shardId, got {stream}"))
        .to_string()
}

/// A `TRIM_HORIZON` iterator on a shard.
async fn trim_horizon(router: &MultiServiceRouter, stream_arn: &str, shard_id: &str) -> String {
    let minted = ok(
        router,
        "GetShardIterator",
        &json!({
            "streamArn": stream_arn,
            "shardId": shard_id,
            "shardIteratorType": "TRIM_HORIZON",
        }),
    )
    .await;
    minted["shardIterator"]
        .as_str()
        .unwrap_or_else(|| panic!("GetShardIterator must mint an iterator, got {minted}"))
        .to_string()
}

/// The `sequenceNumber` of every record in a `GetRecords` response.
fn sequence_numbers(records: &Value) -> Vec<String> {
    records["changeRecords"]
        .as_array()
        .unwrap_or_else(|| panic!("GetRecords must report changeRecords, got {records}"))
        .iter()
        .map(|record| {
            record["sequenceNumber"]
                .as_str()
                .unwrap_or_else(|| panic!("every record carries a sequenceNumber, got {record}"))
                .to_string()
        })
        .collect()
}

/// RULE: a consumer polling from `TRIM_HORIZON` with the returned `nextShardIterator`
/// sees every captured record exactly once, in sequence-number order, in pages of at
/// most `maxResults`; the position is `BEHIND_TIP` while records remain and `AT_TIP`
/// once they are drained, and an open shard always continues.
#[tokio::test]
async fn polling_delivers_every_record_once_in_sequence_order() {
    // (captured records, maxResults asked for) — the two dimensions the rule
    // generalises over, including an empty shard and a page larger than the shard.
    let cases: &[(usize, Option<i32>)] = &[
        (0, Some(1)),
        (1, Some(1)),
        (3, Some(2)),
        (5, None),
        (7, Some(3)),
        (4, Some(10)),
    ];

    for &(captured, max_results) in cases {
        let case = format!("{captured} records, maxResults {max_results:?}");
        let (engine, router) = emulator();
        let stream_arn = seed_stream(
            &engine,
            "us-east-1",
            "mykeyspace",
            "mytable",
            "NEW_AND_OLD_IMAGES",
            captured,
        );
        let shard_id = only_shard_id(&router, &stream_arn).await;
        let mut iterator = trim_horizon(&router, &stream_arn, &shard_id).await;

        let mut delivered: Vec<String> = Vec::new();
        let mut polls = 0;
        loop {
            polls += 1;
            assert!(
                polls <= captured + 2,
                "case {case}: the read loop must terminate, it polled {polls} times"
            );
            let mut request = json!({ "shardIterator": iterator });
            if let Some(limit) = max_results {
                request["maxResults"] = json!(limit);
            }
            let page = ok(&router, "GetRecords", &request).await;
            let page_sequences = sequence_numbers(&page);
            let page_size = page_sequences.len();
            assert!(
                page_size <= max_results.unwrap_or(100).unsigned_abs() as usize,
                "case {case}: a page must not exceed maxResults, got {page_size}"
            );
            let next = page["nextShardIterator"].as_str().unwrap_or_else(|| {
                panic!("case {case}: an open shard always continues, got {page}")
            });
            assert_ne!(
                next, iterator,
                "case {case}: the next iterator must differ from the one just used"
            );
            delivered.extend(page_sequences);

            let position = page["iteratorDescription"]["iteratorPosition"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case}: an iteratorPosition is required, got {page}")
                });
            let drained = delivered.len() == captured;
            let expected_position = if drained { "AT_TIP" } else { "BEHIND_TIP" };
            assert_eq!(
                position, expected_position,
                "case {case}: after {} of {captured} records the position must be \
                 {expected_position}, got {position}",
                delivered.len()
            );
            iterator = next.to_string();
            if drained {
                // One further poll at the tip: no record is re-delivered.
                let tip = ok(&router, "GetRecords", &json!({ "shardIterator": iterator })).await;
                assert!(
                    sequence_numbers(&tip).is_empty(),
                    "case {case}: a poll at the tip must deliver nothing, got {tip}"
                );
                assert_eq!(
                    tip["iteratorDescription"]["iteratorPosition"], "AT_TIP",
                    "case {case}: a poll at the tip is AT_TIP, got {tip}"
                );
                break;
            }
        }

        assert_eq!(
            delivered.len(),
            captured,
            "case {case}: every captured record must be delivered exactly once, got {delivered:?}"
        );
        let mut sorted = delivered.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted, delivered,
            "case {case}: records must arrive in sequence-number order with no repeat, got \
             {delivered:?}"
        );
    }
}

/// RULE: an iterator is a position, not a cursor the service consumes — re-reading
/// with the same iterator returns the same records.
#[tokio::test]
async fn re_reading_with_the_same_iterator_returns_the_same_records() {
    let (engine, router) = emulator();
    let stream_arn = seed_stream(&engine, "eu-west-1", "ks", "tbl", "NEW_IMAGE", 4);
    let shard_id = only_shard_id(&router, &stream_arn).await;
    let iterator = trim_horizon(&router, &stream_arn, &shard_id).await;

    let request = json!({ "shardIterator": iterator, "maxResults": 2 });
    let first = sequence_numbers(&ok(&router, "GetRecords", &request).await);
    let again = sequence_numbers(&ok(&router, "GetRecords", &request).await);
    assert_eq!(
        first, again,
        "case re-read: the same iterator must return the same records, got {first:?} then {again:?}"
    );
    assert_eq!(
        first.len(),
        2,
        "case re-read: maxResults 2 must deliver two records, got {first:?}"
    );
}

/// RULE: every `ShardIteratorType` positions the read where its documented meaning
/// says — the oldest record, just after the newest, at a sequence number, or after it.
#[tokio::test]
async fn every_shard_iterator_type_starts_where_it_says() {
    let (engine, router) = emulator();
    let stream_arn = seed_stream(&engine, "us-west-2", "ks", "tbl", "KEYS_ONLY", 5);
    let shard_id = only_shard_id(&router, &stream_arn).await;
    let all = sequence_numbers(
        &ok(
            &router,
            "GetRecords",
            &json!({ "shardIterator": trim_horizon(&router, &stream_arn, &shard_id).await }),
        )
        .await,
    );
    assert_eq!(all.len(), 5, "the seeded shard holds five records, got {all:?}");

    // (iterator type, sequenceNumber to ask for, the records the read must start with)
    let cases: Vec<(&str, Option<&str>, Vec<String>)> = vec![
        ("TRIM_HORIZON", None, all.clone()),
        ("LATEST", None, Vec::new()),
        ("AT_SEQUENCE_NUMBER", Some(&all[0]), all.clone()),
        ("AT_SEQUENCE_NUMBER", Some(&all[3]), all[3..].to_vec()),
        ("AFTER_SEQUENCE_NUMBER", Some(&all[0]), all[1..].to_vec()),
        ("AFTER_SEQUENCE_NUMBER", Some(&all[4]), Vec::new()),
    ];
    for (iterator_type, sequence_number, expected) in cases {
        let case = format!("{iterator_type} at {sequence_number:?}");
        assert!(
            SHARD_ITERATOR_TYPES.contains(&iterator_type),
            "case {case}: the type must be one the model declares"
        );
        let mut request = json!({
            "streamArn": stream_arn,
            "shardId": shard_id,
            "shardIteratorType": iterator_type,
        });
        if let Some(sequence_number) = sequence_number {
            request["sequenceNumber"] = json!(sequence_number);
        }
        let minted = ok(&router, "GetShardIterator", &request).await;
        let iterator = minted["shardIterator"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: an iterator must be minted, got {minted}"));
        let read = sequence_numbers(
            &ok(
                &router,
                "GetRecords",
                &json!({ "shardIterator": iterator }),
            )
            .await,
        );
        assert_eq!(
            read, expected,
            "case {case}: the read must start at the resolved position, got {read:?}"
        );
    }
}

/// RULE: `LATEST` positions just after the newest record at the time it is minted, so
/// it delivers what the stream captures afterwards.
#[tokio::test]
async fn latest_delivers_records_captured_after_it_was_minted() {
    let (engine, router) = emulator();
    let stream_arn = seed_stream(&engine, "ap-south-1", "ks", "tbl", "OLD_IMAGE", 2);
    let shard_id = only_shard_id(&router, &stream_arn).await;
    let minted = ok(
        &router,
        "GetShardIterator",
        &json!({
            "streamArn": stream_arn,
            "shardId": shard_id,
            "shardIteratorType": "LATEST",
        }),
    )
    .await;
    let iterator = minted["shardIterator"].as_str().expect("an iterator").to_string();

    let before = ok(&router, "GetRecords", &json!({ "shardIterator": iterator })).await;
    assert!(
        sequence_numbers(&before).is_empty(),
        "case LATEST before capture: nothing is past the tip yet, got {before}"
    );

    {
        let state = service_state(&*engine).expect("state");
        let mut guard = state.lock();
        for index in 0..3 {
            guard.append_change_record(&stream_arn, change(&format!("late{index}")));
        }
    }

    let after = ok(&router, "GetRecords", &json!({ "shardIterator": iterator })).await;
    assert_eq!(
        sequence_numbers(&after).len(),
        3,
        "case LATEST after capture: the three later records must arrive, got {after}"
    );
}

/// RULE: a stream's records carry exactly the images its `streamViewType` publishes,
/// and `GetStream` reports the view type (and every other field) from the stored
/// stream.
#[tokio::test]
async fn records_carry_the_images_the_view_type_publishes() {
    // (view type, newImage expected, oldImage expected) — the whole catalog, with the
    // projection each one documents.
    let cases: &[(&str, bool, bool)] = &[
        ("KEYS_ONLY", false, false),
        ("NEW_IMAGE", true, false),
        ("OLD_IMAGE", false, true),
        ("NEW_AND_OLD_IMAGES", true, true),
    ];
    for &(view_type, expect_new, expect_old) in cases {
        let case = format!("view type {view_type}");
        assert!(
            STREAM_VIEW_TYPE_VALUES.contains(&view_type),
            "case {case}: the view type must be one the model declares"
        );
        let (engine, router) = emulator();
        let stream_arn = seed_stream(&engine, "us-east-2", "shop", "orders", view_type, 1);

        let stream = ok(&router, "GetStream", &json!({ "streamArn": stream_arn })).await;
        assert_eq!(
            stream["streamViewType"], view_type,
            "case {case}: GetStream must report the stored view type, got {stream}"
        );
        assert_eq!(
            stream["keyspaceName"], "shop",
            "case {case}: GetStream must report the stored keyspace, got {stream}"
        );
        assert_eq!(
            stream["tableName"], "orders",
            "case {case}: GetStream must report the stored table, got {stream}"
        );

        let shard_id = only_shard_id(&router, &stream_arn).await;
        let iterator = trim_horizon(&router, &stream_arn, &shard_id).await;
        let page = ok(&router, "GetRecords", &json!({ "shardIterator": iterator })).await;
        let record = &page["changeRecords"][0];
        assert_eq!(
            record["newImage"].is_object(),
            expect_new,
            "case {case}: newImage presence must follow the view type, got {record}"
        );
        assert_eq!(
            record["oldImage"].is_object(),
            expect_old,
            "case {case}: oldImage presence must follow the view type, got {record}"
        );
        assert_eq!(
            record["partitionKeys"]["id"]["textT"], "r0",
            "case {case}: the keys of the changed row are always reported, got {record}"
        );
        assert_eq!(
            record["origin"], "USER",
            "case {case}: the record's origin comes from the capture, got {record}"
        );
        assert!(
            record["createdAt"].is_number(),
            "case {case}: a record carries its capture time, got {record}"
        );
    }
}

/// RULE: a stream is reported with the ARN, label and creation time it was created
/// with — the same values on every read and through every operation that reports them.
#[tokio::test]
async fn reported_identity_comes_from_the_stored_stream() {
    // (region, keyspace, table) — the identity is built from the table the stream
    // follows, so it varies with all three.
    let cases: &[(&str, &str, &str)] = &[
        ("us-east-1", "mykeyspace", "mytable"),
        ("eu-central-1", "analytics", "events_2026"),
        ("ap-northeast-1", "a", "b"),
    ];
    for &(region, keyspace_name, table_name) in cases {
        let case = format!("{region} {keyspace_name}.{table_name}");
        let (engine, router) = emulator();
        let stream_arn = seed_stream(&engine, region, keyspace_name, table_name, "NEW_IMAGE", 0);

        let listed = ok(&router, "ListStreams", &json!({})).await;
        let streams = listed["streams"].as_array().expect("streams");
        assert_eq!(
            streams.len(),
            1,
            "case {case}: the one enabled stream must be listed, got {listed}"
        );
        assert_eq!(
            streams[0]["streamArn"], stream_arn.as_str(),
            "case {case}: ListStreams reports the stored ARN, got {listed}"
        );
        assert!(
            stream_arn.starts_with(&format!("arn:aws:cassandra:{region}:")),
            "case {case}: the ARN names the region the table lives in, got {stream_arn}"
        );
        assert!(
            stream_arn.contains(&format!("/keyspace/{keyspace_name}/table/{table_name}/stream/")),
            "case {case}: the ARN names the table the stream follows, got {stream_arn}"
        );

        let first = ok(&router, "GetStream", &json!({ "streamArn": stream_arn })).await;
        let label = first["streamLabel"].as_str().unwrap_or_default().to_string();
        assert!(
            !label.is_empty() && stream_arn.ends_with(&label),
            "case {case}: the stream label identifies the stream in its ARN, got {label}"
        );
        assert_eq!(
            streams[0]["streamLabel"], label.as_str(),
            "case {case}: ListStreams and GetStream report one label, got {listed}"
        );
        let created = first["creationRequestDateTime"].as_f64().unwrap_or_default();
        assert!(
            created > 0.0,
            "case {case}: the creation time must be a real timestamp, got {created}"
        );

        let second = ok(&router, "GetStream", &json!({ "streamArn": stream_arn })).await;
        assert_eq!(
            first["streamLabel"], second["streamLabel"],
            "case {case}: a read must not change what a later read sees, got {second}"
        );
        assert_eq!(
            first["creationRequestDateTime"], second["creationRequestDateTime"],
            "case {case}: the creation time is stable across reads, got {second}"
        );
    }
}

/// RULE: a stream Keyspaces has just enabled is reported in its transient status and
/// reaches `ENABLED` on the clock alone, with no further call.
#[tokio::test]
async fn a_new_stream_is_enabling_until_the_clock_enables_it() {
    let (engine, router) = emulator();
    let stream_arn = seed_stream(&engine, "us-east-1", "ks", "tbl", "KEYS_ONLY", 0);

    let fresh = ok(&router, "GetStream", &json!({ "streamArn": stream_arn })).await;
    assert_eq!(
        fresh["streamStatus"], "ENABLING",
        "case fresh: a stream reports its transient status first, got {fresh}"
    );

    tokio::time::sleep(STATUS_TRANSITION_DELAY + Duration::from_millis(200)).await;
    let settled = ok(&router, "GetStream", &json!({ "streamArn": stream_arn })).await;
    assert_eq!(
        settled["streamStatus"], "ENABLED",
        "case settled: the clock must move the stream to ENABLED, got {settled}"
    );
}

/// RULE: `ListStreams` enumerates the streams state holds, keeps only those matching
/// the `keyspaceName`/`tableName` filter, and pages with a token that resumes where the
/// previous page stopped.
#[tokio::test]
async fn list_streams_filters_and_pages_over_what_state_holds() {
    let (engine, router) = emulator();
    let seeded: Vec<(String, String, String)> = [
        ("shop", "orders"),
        ("shop", "customers"),
        ("shop", "invoices"),
        ("analytics", "events"),
    ]
    .iter()
    .map(|(keyspace_name, table_name)| {
        let arn = seed_stream(
            &engine,
            "us-east-1",
            keyspace_name,
            table_name,
            "NEW_AND_OLD_IMAGES",
            0,
        );
        ((*keyspace_name).to_string(), (*table_name).to_string(), arn)
    })
    .collect();

    // (keyspaceName, tableName, how many streams the filter selects)
    let cases: &[(Option<&str>, Option<&str>, usize)] = &[
        (None, None, 4),
        (Some("shop"), None, 3),
        (Some("analytics"), None, 1),
        (Some("shop"), Some("orders"), 1),
        (Some("shop"), Some("absent"), 0),
        (Some("absent"), None, 0),
    ];
    for &(keyspace_name, table_name, expected) in cases {
        let case = format!("filter {keyspace_name:?}/{table_name:?}");
        let expected_arns: Vec<String> = {
            let mut arns: Vec<String> = seeded
                .iter()
                .filter(|(keyspace, table, _)| {
                    keyspace_name.is_none_or(|wanted| wanted == keyspace)
                        && table_name.is_none_or(|wanted| wanted == table)
                })
                .map(|(_, _, arn)| arn.clone())
                .collect();
            arns.sort();
            arns
        };
        assert_eq!(
            expected_arns.len(),
            expected,
            "case {case}: the fixture must select {expected} streams"
        );

        // Walk every page with maxResults 1, the smallest the model allows, so the
        // token has to resume the enumeration rather than restart it.
        let mut seen: Vec<String> = Vec::new();
        let mut next_token: Option<String> = None;
        for page_index in 0..=expected_arns.len() {
            let mut request = json!({ "maxResults": 1 });
            if let Some(keyspace_name) = keyspace_name {
                request["keyspaceName"] = json!(keyspace_name);
            }
            if let Some(table_name) = table_name {
                request["tableName"] = json!(table_name);
            }
            if let Some(token) = &next_token {
                request["nextToken"] = json!(token);
            }
            let page = ok(&router, "ListStreams", &request).await;
            let arns: Vec<String> = page["streams"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: streams is a list, got {page}"))
                .iter()
                .map(|stream| stream["streamArn"].as_str().unwrap_or_default().to_string())
                .collect();
            assert!(
                arns.len() <= 1,
                "case {case} page {page_index}: maxResults 1 bounds the page, got {arns:?}"
            );
            seen.extend(arns);
            match page["nextToken"].as_str() {
                Some(token) => {
                    assert!(
                        (80..=3000).contains(&token.len()),
                        "case {case}: a nextToken must be within the modelled 80..3000 \
                         bound, got {} characters",
                        token.len()
                    );
                    next_token = Some(token.to_string());
                }
                None => {
                    next_token = None;
                    break;
                }
            }
        }
        assert!(
            next_token.is_none(),
            "case {case}: the enumeration must end, it still offered a token after \
             {} streams",
            seen.len()
        );
        assert_eq!(
            seen, expected_arns,
            "case {case}: paging must yield exactly the selected streams, in order"
        );
    }
}

/// RULE: `GetStream` pages its shard list the same way, and `CHILD_SHARDS` selects
/// exactly the shards that evolved from the shard named in the filter; a closed shard
/// reports the end of its range and stops continuing once drained.
#[tokio::test]
async fn get_stream_pages_shards_and_selects_children() {
    let (engine, router) = emulator();
    let stream_arn = seed_stream(&engine, "us-east-1", "ks", "tbl", "NEW_IMAGE", 2);
    let parent_id = only_shard_id(&router, &stream_arn).await;

    // Reshard twice: parent -> child -> grandchild, so a child selection has both a
    // match and a non-match to tell apart.
    let (child_id, grandchild_id) = {
        let state = service_state(&*engine).expect("state");
        let mut guard = state.lock();
        let child = guard.split_shard(&stream_arn).expect("the parent splits");
        guard.append_change_record(&stream_arn, change("c0"));
        let grandchild = guard.split_shard(&stream_arn).expect("the child splits");
        (child, grandchild)
    };

    let all = ok(&router, "GetStream", &json!({ "streamArn": stream_arn })).await;
    let shard_ids: Vec<&str> = all["shards"]
        .as_array()
        .expect("shards")
        .iter()
        .map(|shard| shard["shardId"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        shard_ids,
        vec![parent_id.as_str(), child_id.as_str(), grandchild_id.as_str()],
        "case all shards: every shard is reported, oldest first, got {all}"
    );
    assert!(
        all["shards"][0]["sequenceNumberRange"]["endingSequenceNumber"].is_string(),
        "case closed parent: a closed shard's range has an end, got {all}"
    );
    assert!(
        all["shards"][2]["sequenceNumberRange"]["endingSequenceNumber"].is_null(),
        "case open shard: an open shard's range has no end, got {all}"
    );

    // (filter shard id, the shards CHILD_SHARDS selects)
    let cases: &[(&str, &[&str])] = &[
        (parent_id.as_str(), &[child_id.as_str()]),
        (child_id.as_str(), &[grandchild_id.as_str()]),
        (grandchild_id.as_str(), &[]),
    ];
    for &(filter_shard_id, expected) in cases {
        let case = format!("CHILD_SHARDS of {filter_shard_id}");
        let filtered = ok(
            &router,
            "GetStream",
            &json!({
                "streamArn": stream_arn,
                "shardFilter": { "type": "CHILD_SHARDS", "shardId": filter_shard_id },
            }),
        )
        .await;
        let selected: Vec<&str> = filtered["shards"]
            .as_array()
            .expect("shards")
            .iter()
            .map(|shard| shard["shardId"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            selected, expected,
            "case {case}: only the shards that evolved from it are selected, got {filtered}"
        );
    }

    // Pages of one shard, resumed by the token, cover the shard list exactly once.
    let mut paged: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut request = json!({ "streamArn": stream_arn, "maxResults": 1 });
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let page = ok(&router, "GetStream", &request).await;
        for shard in page["shards"].as_array().expect("shards") {
            paged.push(shard["shardId"].as_str().unwrap_or_default().to_string());
        }
        match page["nextToken"].as_str() {
            Some(token) => next_token = Some(token.to_string()),
            None => break,
        }
        assert!(
            paged.len() <= shard_ids.len(),
            "case shard paging: the walk must terminate, got {paged:?}"
        );
    }
    assert_eq!(
        paged, shard_ids,
        "case shard paging: paging must cover the shard list once, in order"
    );

    // The parent shard is closed: once its records are drained it stops continuing.
    let iterator = trim_horizon(&router, &stream_arn, &parent_id).await;
    let drained = ok(&router, "GetRecords", &json!({ "shardIterator": iterator })).await;
    assert_eq!(
        sequence_numbers(&drained).len(),
        2,
        "case closed parent: its two records are still served, got {drained}"
    );
    assert!(
        drained["nextShardIterator"].is_null(),
        "case closed parent: a drained closed shard returns no next iterator, got {drained}"
    );
    assert_eq!(
        drained["iteratorDescription"]["iteratorPosition"], "AT_TIP",
        "case closed parent: a drained shard is AT_TIP, got {drained}"
    );
}

/// RULE: a request naming something this service does not hold is refused with the
/// modelled 404, and one this service cannot satisfy with the modelled 400 — never
/// with a success and never with the scaffold's 501.
#[tokio::test]
async fn refusals_use_the_modelled_errors() {
    let (engine, router) = emulator();
    let stream_arn = seed_stream(&engine, "us-east-1", "ks", "tbl", "NEW_IMAGE", 2);
    let shard_id = only_shard_id(&router, &stream_arn).await;
    let known_sequence = sequence_numbers(
        &ok(
            &router,
            "GetRecords",
            &json!({ "shardIterator": trim_horizon(&router, &stream_arn, &shard_id).await }),
        )
        .await,
    )[0]
    .clone();
    let absent_arn = format!("{stream_arn}-absent");
    let absent_shard = format!("shardId-{:020}-deadbeef", 1);
    let fabricated_iterator = format!("ksstreams-iterator-{}", "0".repeat(80));
    let fabricated_token = "z".repeat(120);
    // One below the shard's own starting sequence number, read back from the shard
    // rather than assumed, so the case stays "older than the shard retains" whatever
    // order ids are minted in.
    let trimmed_sequence = {
        let stream = ok(&router, "GetStream", &json!({ "streamArn": stream_arn })).await;
        let starting = stream["shards"][0]["sequenceNumberRange"]["startingSequenceNumber"]
            .as_str()
            .unwrap_or_else(|| panic!("a shard reports its range, got {stream}"))
            .parse::<u64>()
            .unwrap_or_else(|error| panic!("a sequence number is decimal: {error}"));
        assert!(
            starting > 0,
            "the shard's starting sequence number must leave room below it, got {starting}"
        );
        format!("{:021}", starting - 1)
    };

    // (operation, request, expected status, expected error code, expected errorCode member)
    let cases: Vec<(&str, Value, u16, &str, Option<&str>)> = vec![
        (
            "GetStream",
            json!({ "streamArn": absent_arn }),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "GetShardIterator",
            json!({
                "streamArn": absent_arn,
                "shardId": shard_id,
                "shardIteratorType": "TRIM_HORIZON",
            }),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "GetShardIterator",
            json!({
                "streamArn": stream_arn,
                "shardId": absent_shard,
                "shardIteratorType": "LATEST",
            }),
            404,
            "ResourceNotFoundException",
            None,
        ),
        (
            "GetShardIterator",
            json!({
                "streamArn": stream_arn,
                "shardId": shard_id,
                "shardIteratorType": "AT_SEQUENCE_NUMBER",
            }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "GetShardIterator",
            json!({
                "streamArn": stream_arn,
                "shardId": shard_id,
                "shardIteratorType": "AFTER_SEQUENCE_NUMBER",
            }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "GetShardIterator",
            json!({
                "streamArn": stream_arn,
                "shardId": shard_id,
                "shardIteratorType": "AT_SEQUENCE_NUMBER",
                "sequenceNumber": trimmed_sequence,
            }),
            400,
            "ValidationException",
            Some("TrimmedDataAccess"),
        ),
        (
            "GetShardIterator",
            json!({
                "streamArn": stream_arn,
                "shardId": shard_id,
                "shardIteratorType": "AT_SEQUENCE_NUMBER",
                "sequenceNumber": "999999999999999999999",
            }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "GetRecords",
            json!({ "shardIterator": fabricated_iterator }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "ListStreams",
            json!({ "tableName": "tbl" }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "ListStreams",
            json!({ "nextToken": fabricated_token }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "GetStream",
            json!({ "streamArn": stream_arn, "nextToken": fabricated_token }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "GetStream",
            json!({ "streamArn": stream_arn, "shardFilter": { "type": "CHILD_SHARDS" } }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "GetStream",
            json!({ "streamArn": stream_arn, "shardFilter": { "shardId": shard_id } }),
            400,
            "ValidationException",
            Some("InvalidFormat"),
        ),
        (
            "GetStream",
            json!({
                "streamArn": stream_arn,
                "shardFilter": { "type": "CHILD_SHARDS", "shardId": absent_shard },
            }),
            404,
            "ResourceNotFoundException",
            None,
        ),
    ];

    for (operation, request, expected_status, expected_code, expected_error_code) in cases {
        let case = format!("{operation} {request}");
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            status, expected_status,
            "case {case}: must answer HTTP {expected_status}, got {status} {body}"
        );
        assert_eq!(
            body["__type"], expected_code,
            "case {case}: must answer {expected_code}, got {body}"
        );
        if let Some(expected_error_code) = expected_error_code {
            assert_eq!(
                body["errorCode"], expected_error_code,
                "case {case}: must carry errorCode {expected_error_code}, got {body}"
            );
        }
    }

    // A sequence number the shard really holds is NOT refused — the refusals above
    // are about the request, not about every sequence number.
    let accepted = ok(
        &router,
        "GetShardIterator",
        &json!({
            "streamArn": stream_arn,
            "shardId": shard_id,
            "shardIteratorType": "AT_SEQUENCE_NUMBER",
            "sequenceNumber": known_sequence,
        }),
    )
    .await;
    assert!(
        accepted["shardIterator"].is_string(),
        "case accepted sequence number: a known sequence number mints an iterator, got {accepted}"
    );
}

/// RULE: a pagination token resumes the query it was issued for and no other.
#[tokio::test]
async fn a_page_token_is_refused_for_a_different_query() {
    let (engine, router) = emulator();
    for table_name in ["t1", "t2", "t3"] {
        seed_stream(&engine, "us-east-1", "shop", table_name, "NEW_IMAGE", 0);
    }
    seed_stream(&engine, "us-east-1", "analytics", "events", "NEW_IMAGE", 0);

    let page = ok(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "shop", "maxResults": 1 }),
    )
    .await;
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first of three streams must offer a token, got {page}"))
        .to_string();

    let (status, body) = call(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "analytics", "nextToken": token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case token reuse: a token from another filter must be refused, got {status} {body}"
    );
    assert_eq!(
        body["__type"], "ValidationException",
        "case token reuse: the refusal is a ValidationException, got {body}"
    );

    let resumed = ok(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "shop", "maxResults": 1, "nextToken": token }),
    )
    .await;
    assert_eq!(
        resumed["streams"].as_array().map(Vec::len),
        Some(1),
        "case token resume: its own query resumes with one more stream, got {resumed}"
    );
    assert_eq!(
        resumed["streams"][0]["tableName"], "t2",
        "case token resume: it resumes after the stream the first page returned, got {resumed}"
    );
}

/// RULE: the registries a request mints into — shard iterators and pagination cursors
/// — are bounded, so no sequence of requests can make this service hold memory
/// without limit. Eviction is unconditional (first-minted first), so it happens even
/// when every entry is still inside its lifetime, and an evicted token is refused
/// rather than resumed from a wrong position.
#[tokio::test]
async fn the_registries_a_request_mints_into_are_bounded() {
    let (engine, router) = emulator();
    for table_name in ["t1", "t2", "t3"] {
        seed_stream(&engine, "us-east-1", "shop", table_name, "NEW_IMAGE", 2);
    }
    let stream_arn = {
        let state = service_state(&*engine).expect("state");
        let guard = state.lock();
        guard
            .stream_for_table("us-east-1", "shop", "t1")
            .expect("the seeded stream")
            .stream_arn
            .clone()
    };
    let shard_id = only_shard_id(&router, &stream_arn).await;

    // A pagination cursor issued now, then the cap minted on top of it.
    let page = ok(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "shop", "maxResults": 1 }),
    )
    .await;
    let evicted_token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("the first of three streams must offer a token, got {page}"))
        .to_string();
    // A shard iterator issued now, then the cap minted on top of it.
    let evicted_iterator = trim_horizon(&router, &stream_arn, &shard_id).await;
    {
        let state = service_state(&*engine).expect("state");
        let mut guard = state.lock();
        for offset in 0..MAX_TRACKED_PAGE_TOKENS {
            guard.mint_page_token(PageCursor {
                scope: PageScope::Streams {
                    region: None,
                    keyspace_name: Some("shop".to_string()),
                    table_name: None,
                },
                after: format!("arn:aws:cassandra:us-east-1:000000000000:filler-{offset}"),
            });
        }
        for next_index in 0..MAX_TRACKED_ITERATORS {
            guard.mint_shard_iterator(&stream_arn, &shard_id, next_index);
        }
    }

    let (status, body) = call(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "shop", "maxResults": 1, "nextToken": evicted_token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case evicted cursor: a cursor past the registry's cap must be refused, got \
         {status} {body}"
    );
    assert_eq!(
        body["errorCode"], "InvalidFormat",
        "case evicted cursor: the refusal says the token is not one this service holds, got {body}"
    );

    let (status, body) = call(
        &router,
        "GetRecords",
        &json!({ "shardIterator": evicted_iterator }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case evicted iterator: an iterator past the registry's cap must be refused, got \
         {status} {body}"
    );
    assert_eq!(
        body["__type"], "ValidationException",
        "case evicted iterator: an iterator this service holds no registration for names no \
         position, so the refusal is a ValidationException, got {body}"
    );
    assert_eq!(
        body["errorCode"], "InvalidFormat",
        "case evicted iterator: the refusal says the iterator is not one this service issued, \
         got {body}"
    );

    // And the read loop still works after the eviction: a freshly minted iterator and
    // cursor serve normally.
    let fresh_iterator = trim_horizon(&router, &stream_arn, &shard_id).await;
    let fresh_read = ok(
        &router,
        "GetRecords",
        &json!({ "shardIterator": fresh_iterator }),
    )
    .await;
    assert_eq!(
        sequence_numbers(&fresh_read).len(),
        2,
        "case after eviction: a fresh iterator still serves the shard, got {fresh_read}"
    );
    let fresh_page = ok(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "shop", "maxResults": 1 }),
    )
    .await;
    assert!(
        fresh_page["nextToken"].is_string(),
        "case after eviction: a fresh page still offers a cursor, got {fresh_page}"
    );
}

/// RULE: a `nextToken` resumes after the item the previous page ended at (the model
/// types it `StreamArnToken`), so a stream enabled between two pages neither hides a
/// stream from the walk nor delivers one twice.
#[tokio::test]
async fn paging_is_anchored_to_the_last_item_not_to_a_count() {
    // (the table enabled between the two pages, where its ARN sorts relative to the
    // page the walk has already returned) — a cursor that counted items would skip an
    // item for the "before" row and repeat one for the "after" row.
    let cases: &[(&str, &str)] = &[("aaa", "before the cursor"), ("zzz", "after the cursor")];
    for &(late_table, placement) in cases {
        let case = format!("a stream enabled {placement} ({late_table})");
        let (engine, router) = emulator();
        let mut expected: Vec<String> = ["m1", "m2", "m3"]
            .iter()
            .map(|table_name| seed_stream(&engine, "us-east-1", "shop", table_name, "NEW_IMAGE", 0))
            .collect();

        let first = ok(
            &router,
            "ListStreams",
            &json!({ "keyspaceName": "shop", "maxResults": 1 }),
        )
        .await;
        let mut seen: Vec<String> = vec![first["streams"][0]["streamArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the first page returns a stream, got {first}"))
            .to_string()];
        let mut next_token = first["nextToken"].as_str().map(str::to_string);

        // CDC is enabled for another table while the walk is in flight.
        let late_arn = seed_stream(&engine, "us-east-1", "shop", late_table, "NEW_IMAGE", 0);
        expected.push(late_arn);
        expected.sort();

        while let Some(token) = next_token {
            let page = ok(
                &router,
                "ListStreams",
                &json!({ "keyspaceName": "shop", "maxResults": 1, "nextToken": token }),
            )
            .await;
            for stream in page["streams"].as_array().expect("streams") {
                seen.push(stream["streamArn"].as_str().unwrap_or_default().to_string());
            }
            next_token = page["nextToken"].as_str().map(str::to_string);
            assert!(
                seen.len() <= expected.len(),
                "case {case}: the walk must terminate without re-delivering, got {seen:?}"
            );
        }

        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: no stream may be delivered twice, got {seen:?}"
        );
        for arn in &expected {
            // The stream enabled mid-walk may or may not be reached, depending on where
            // it sorts; every stream that existed when the walk started must be.
            if arn.contains(&format!("/table/{late_table}/")) {
                continue;
            }
            assert!(
                seen.contains(arn),
                "case {case}: the walk must not skip {arn}, got {seen:?}"
            );
        }
    }
}

/// RULE: a stream is enabled only for a view type the model declares — the write side
/// refuses any other value instead of storing one that would publish no image.
#[tokio::test]
async fn cdc_is_enabled_only_for_a_modelled_view_type() {
    let (engine, _router) = emulator();
    let state = service_state(&*engine).expect("state");
    let mut guard = state.lock();

    for view_type in STREAM_VIEW_TYPE_VALUES {
        let enabled = guard.enable_stream("us-east-1", "shop", view_type, view_type);
        assert!(
            enabled.is_some(),
            "case {view_type}: a view type the model declares must be accepted"
        );
    }
    // Values the model does not declare: a near-miss, a case variant, and empty.
    for view_type in ["NEW_AND_OLD_IMAGE", "new_image", ""] {
        let enabled = guard.enable_stream("us-east-1", "shop", "other_table", view_type);
        assert!(
            enabled.is_none(),
            "case {view_type:?}: a view type the model does not declare must be refused, \
             got {enabled:?}"
        );
    }
    // Re-asserting CDC on an already-enabled table keeps its stream whatever the
    // re-asserted view type says: idempotence is per region/keyspace/table.
    let first = guard
        .enable_stream("us-east-1", "shop", "KEYS_ONLY", "KEYS_ONLY")
        .expect("the stream enabled above");
    let again = guard.enable_stream("us-east-1", "shop", "KEYS_ONLY", "NEW_IMAGE");
    assert_eq!(
        again.as_deref(),
        Some(first.as_str()),
        "case re-enable: an already-enabled table keeps its stream, got {again:?}"
    );
}

/// RULE: a table's streams are distinct resources, so turning CDC off and straight back
/// on — with no wait at all, inside one tick of the stream label's resolution — gives
/// the table a NEW, live stream of its own, and leaves the disabled one exactly as it
/// was, records and all.
///
/// No sleep anywhere: the point is that two streams minted in the same instant cannot
/// collide, so an enable can never be answered with the stream that was just disabled.
#[tokio::test]
async fn disabling_and_immediately_re_enabling_mints_a_distinct_live_stream() {
    let (engine, _router) = emulator();
    let state = service_state(&*engine).expect("state");
    let mut guard = state.lock();

    let mut minted: Vec<String> = Vec::new();
    // Several cycles back to back, so the case the clock's resolution makes likely —
    // two of them inside one millisecond — is actually exercised.
    for cycle in 0..4_u32 {
        let view_type = STREAM_VIEW_TYPE_VALUES[cycle as usize % STREAM_VIEW_TYPE_VALUES.len()];
        let case = format!("cycle {cycle} ({view_type})");
        let stream_arn = guard
            .enable_stream("us-east-1", "shop", "orders", view_type)
            .unwrap_or_else(|| panic!("case {case}: a modelled view type must be accepted"));
        assert!(
            !minted.contains(&stream_arn),
            "case {case}: a re-enabled table must get a stream of its own, got \
             {stream_arn} again"
        );
        let live = guard
            .live_stream_for_table("us-east-1", "shop", "orders")
            .expect("the stream just enabled is live");
        assert_eq!(
            (live.stream_arn.as_str(), live.stream_view_type.as_str()),
            (stream_arn.as_str(), view_type),
            "case {case}: the live stream is the one just enabled, publishing its own view"
        );
        // One captured change, so a stream that was overwritten rather than kept would
        // lose a record and be caught below.
        assert!(
            guard
                .append_change_record(&stream_arn, change(&format!("c{cycle}")))
                .is_some(),
            "case {case}: a live stream must capture a change"
        );

        let disabled = guard.disable_stream("us-east-1", "shop", "orders");
        assert_eq!(
            disabled.as_deref(),
            Some(stream_arn.as_str()),
            "case {case}: disabling must answer with the stream it stopped"
        );
        assert!(
            guard
                .live_stream_for_table("us-east-1", "shop", "orders")
                .is_none(),
            "case {case}: a disabled table has no live stream left"
        );
        minted.push(stream_arn);
    }

    // Every stream of the cycle is still there, each with the one record it captured.
    for (cycle, stream_arn) in minted.iter().enumerate() {
        let case = format!("cycle {cycle}");
        let stream = guard
            .stream(stream_arn)
            .unwrap_or_else(|| panic!("case {case}: the stream must still be held"));
        assert_eq!(
            stream.shards.iter().map(|shard| shard.records.len()).sum::<usize>(),
            1,
            "case {case}: a stream keeps the records it captured, got {:?}",
            stream.shards
        );
        assert!(
            !stream.is_live(),
            "case {case}: a disabled stream must not be live, got {}",
            stream.stream_status
        );
    }
}

/// RULE: a token this service issues stops being usable once it is older than the
/// lifetime the service documents for it, and each refusal carries the
/// `ValidationExceptionType` that names which lifetime ran out.
#[tokio::test]
async fn an_issued_token_stops_working_once_it_is_older_than_its_lifetime() {
    let (engine, router) = emulator();
    for table_name in ["t1", "t2"] {
        seed_stream(&engine, "us-east-1", "shop", table_name, "NEW_IMAGE", 2);
    }
    let stream_arn = {
        let state = service_state(&*engine).expect("state");
        let guard = state.lock();
        guard
            .stream_for_table("us-east-1", "shop", "t1")
            .expect("the seeded stream")
            .stream_arn
            .clone()
    };
    let shard_id = only_shard_id(&router, &stream_arn).await;
    let iterator = trim_horizon(&router, &stream_arn, &shard_id).await;
    let page = ok(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "shop", "maxResults": 1 }),
    )
    .await;
    let token = page["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("two streams must offer a token, got {page}"))
        .to_string();

    // Both are still inside their lifetime.
    let fresh = ok(
        &router,
        "GetRecords",
        &json!({ "shardIterator": iterator }),
    )
    .await;
    assert_eq!(
        sequence_numbers(&fresh).len(),
        2,
        "case fresh iterator: it serves the shard, got {fresh}"
    );

    // Age every issued token past the longer of the two documented lifetimes.
    {
        let state = service_state(&*engine).expect("state");
        let mut guard = state.lock();
        guard.backdate_issued_tokens(SHARD_ITERATOR_LIFETIME.max(PAGE_TOKEN_LIFETIME) + SECOND);
    }

    let (status, body) = call(
        &router,
        "GetRecords",
        &json!({ "shardIterator": iterator }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case aged iterator: an iterator past its 15-minute lifetime must be refused, got \
         {status} {body}"
    );
    assert_eq!(
        body["errorCode"], "ExpiredIterator",
        "case aged iterator: the refusal names the expired iterator, got {body}"
    );

    let (status, body) = call(
        &router,
        "ListStreams",
        &json!({ "keyspaceName": "shop", "maxResults": 1, "nextToken": token }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case aged cursor: a cursor past its lifetime must be refused, got {status} {body}"
    );
    assert_eq!(
        body["errorCode"], "ExpiredNextToken",
        "case aged cursor: the refusal names the expired token, got {body}"
    );

    // A newly minted iterator works again: the refusal is about the token's age, not
    // about the shard.
    let renewed = trim_horizon(&router, &stream_arn, &shard_id).await;
    let read = ok(&router, "GetRecords", &json!({ "shardIterator": renewed })).await;
    assert_eq!(
        sequence_numbers(&read).len(),
        2,
        "case renewed iterator: a fresh iterator serves the shard again, got {read}"
    );
}
