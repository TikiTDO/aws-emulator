// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION a table's Kinesis streaming destination belongs to, and
//! for the inventory of streaming tables a client derives per endpoint.
//!
//! The rules pinned here:
//!
//! 1. a table an accepted `CreateTable` registered is resolved by `DescribeTable` at the
//!    endpoint that created it FROM THAT MOMENT — before its own `CREATING` span has
//!    settled — and at no other endpoint, where the same name is refused with DynamoDB's
//!    missing-table sentence; the destination commands resolve their table by that same
//!    rule;
//! 2. `StreamArn` carries a length bound and no other constraint
//!    (`smithy_query(query="operation",operation="EnableKinesisStreamingDestination")`:
//!    `StreamArn`, len 37..1024), so every value that clears the bound is ACCEPTED, at
//!    whichever Region's endpoint, and answered the transient status; the destination then
//!    settles `ACTIVE` only where the value names a data stream the SAME Region's Kinesis
//!    endpoint holds, and `ENABLE_FAILED` otherwise — a value below the bound is the one
//!    thing the call refuses, by the boundary validator's own length clause;
//! 3. the destinations a table reports are its whole replication history, so the inventory
//!    of the tables STREAMING in a Region is the ones whose reported `DestinationStatus` is
//!    `ACTIVE`: a destination that failed, or that has been stopped, is reported and
//!    excluded, and a table no enable named reports none at all;
//! 4. a destination that replicates delivers its table's item changes to the data stream of
//!    its OWN Region's Kinesis endpoint;
//! 5. a table replicates to at most one stream in every Region alike: while its destination
//!    is live every further enable is refused with DynamoDB's enable-state sentence,
//!    whichever stream it names.
//!
//! Each rule is asserted over a SET of inputs — two Regions' endpoints, and per Region what
//! the `StreamArn` names (a stream of this Region, a stream of the other one, a value at the
//! modelled length floor, a longer value that is no ARN, a stream whose replication was
//! stopped, and no destination at all) — rather than one example. Every command and every
//! read is driven over the wire (`POST /` with `X-Amz-Target: <Service>.<OperationName>`)
//! through the same `MultiServiceRouter` the emulator's front doors use, with the DynamoDB
//! and the Kinesis service mounted on ONE shared engine and nothing seeded through either
//! store.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::contributor_insights::{create_table_in, settle};
use crate::emulator::{call_in, dispatch_as, emulator, error_code, message, ok_in, REGION};

/// The two Regions whose endpoints every row below is driven against: the suite's own, and
/// one of another partition's continents. A table name and a stream name are unique within
/// ONE Region, so both Regions use the SAME names — which is what makes the scope visible.
const REGIONS: [&str; 2] = [REGION, "eu-west-3"];

/// A third Region nothing here creates anything in: the endpoint a name created elsewhere
/// is NOT resolved at.
const EMPTY_REGION: &str = "ap-south-1";

/// The name of the data stream each Region's endpoint holds.
const STREAM: &str = "cdc";

/// The smallest `StreamArn` the member admits: "StreamArn, len 37..1024"
/// (`smithy_query(query="operation",operation="EnableKinesisStreamingDestination")`), so a
/// value of this length is accepted and one character shorter is refused.
const STREAM_ARN_MIN_LENGTH: usize = 37;

/// The SigV4 signing name of the service that owns the destination's data stream, which is
/// what routes a request to it.
const KINESIS_SIGNING_NAME: &str = "kinesis";

/// The `X-Amz-Target` prefix that service names its operations under.
const KINESIS_TARGET_PREFIX: &str = "Kinesis_20131202";

/// The statuses this suite reads by name.
const ACTIVE: &str = "ACTIVE";
const ENABLING: &str = "ENABLING";

/// Send one KINESIS operation at `region`'s endpoint, asserting it succeeded.
async fn kinesis_in(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: Value,
) -> Value {
    let (status, parsed) = dispatch_as(
        router,
        region,
        KINESIS_SIGNING_NAME,
        &format!("{KINESIS_TARGET_PREFIX}.{operation}"),
        body.clone(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case kinesis {operation} in {region} {body}: expected HTTP 200, got {status} with \
         {parsed}"
    );
    parsed
}

/// Make sure the data stream `name` exists at `region`'s Kinesis endpoint, and answer the
/// ARN THAT service reports for it — the value a table's destination then names.
///
/// Idempotent: a stream the Region already holds answers `ResourceInUseException`, which is
/// this call's success too. The ARN is read back off that service's own description rather
/// than built here, so the suite does not depend on how the Kinesis crate mints one.
async fn ensure_stream_in(router: &MultiServiceRouter, region: &str, name: &str) -> String {
    let (status, body) = dispatch_as(
        router,
        region,
        KINESIS_SIGNING_NAME,
        &format!("{KINESIS_TARGET_PREFIX}.CreateStream"),
        json!({"StreamName": name, "ShardCount": 1}),
    )
    .await;
    assert!(
        status == 200 || error_code(&body) == "ResourceInUseException",
        "case the stream {name:?} in {region}: expected it created or already there, got \
         {status} with {body}"
    );
    let described = kinesis_in(
        router,
        region,
        "DescribeStreamSummary",
        json!({"StreamName": name}),
    )
    .await;
    described["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Every change record the data stream `name` of `region`'s endpoint holds, as the JSON
/// document each record's `Data` blob carries — what a consumer of a table's streaming
/// destination reads, read where Kinesis holds it.
///
/// The walk is bounded, so a delivery that never advances an iterator fails rather than
/// hanging.
async fn delivered_in(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    name: &str,
) -> Vec<Value> {
    let listed = kinesis_in(router, region, "ListShards", json!({"StreamName": name})).await;
    let shards = listed["Shards"].as_array().cloned().unwrap_or_default();
    assert!(
        !shards.is_empty(),
        "case {case:?}: the destination's stream must list at least one shard, got {listed}"
    );
    let mut documents: Vec<Value> = Vec::new();
    for shard in &shards {
        let shard_id = shard["ShardId"].as_str().unwrap_or_default().to_string();
        let minted = kinesis_in(
            router,
            region,
            "GetShardIterator",
            json!({
                "StreamName": name,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        let mut iterator = minted["ShardIterator"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !iterator.is_empty(),
            "case {case:?}: GetShardIterator must mint a position in {shard_id:?}, got {minted}"
        );
        for _ in 0..16 {
            let page = kinesis_in(
                router,
                region,
                "GetRecords",
                json!({"ShardIterator": iterator}),
            )
            .await;
            let batch = page["Records"].as_array().cloned().unwrap_or_default();
            for record in &batch {
                let encoded = record["Data"].as_str().unwrap_or_else(|| {
                    panic!("case {case:?}: every record carries Data, got {record}")
                });
                let decoded = aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
                    panic!("case {case:?}: a record's Data is base64, got {error}")
                });
                documents.push(serde_json::from_slice(&decoded).unwrap_or_else(|error| {
                    panic!("case {case:?}: a delivered record carries JSON, got {error}")
                }));
            }
            match page["NextShardIterator"].as_str() {
                Some(next) if !batch.is_empty() => iterator = next.to_string(),
                _ => break,
            }
        }
    }
    documents
}

/// Start replication of `table` to `stream_arn` at `region`'s endpoint, answering the HTTP
/// status and the parsed body the command reported.
async fn enable_in(
    router: &MultiServiceRouter,
    region: &str,
    table: &str,
    stream_arn: &str,
) -> (u16, Value) {
    call_in(
        router,
        region,
        "EnableKinesisStreamingDestination",
        json!({"TableName": table, "StreamArn": stream_arn}),
    )
    .await
}

/// Every destination `table` reports at `region`'s endpoint, in the order it reports them.
async fn destinations_in(router: &MultiServiceRouter, region: &str, table: &str) -> Vec<Value> {
    ok_in(
        router,
        region,
        "DescribeKinesisStreamingDestination",
        json!({"TableName": table}),
    )
    .await["KinesisDataStreamDestinations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The status `table` reports for its one destination at `region`'s endpoint, or `None`
/// where it reports no destination at all.
async fn reported_status(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    table: &str,
) -> Option<String> {
    let destinations = destinations_in(router, region, table).await;
    assert!(
        destinations.len() <= 1,
        "case {case:?}: a table replicates to at most one stream, got {destinations:?}"
    );
    destinations.first().map(|destination| {
        destination["DestinationStatus"]
            .as_str()
            .unwrap_or("<no status>")
            .to_string()
    })
}

/// The inventory a client derives at `region`'s endpoint: every table there whose reported
/// destination is `ACTIVE`, sorted, through `ListTables` and the destination reader alone.
async fn streaming_tables_in(router: &MultiServiceRouter, region: &str) -> Vec<String> {
    let listed = ok_in(router, region, "ListTables", json!({})).await;
    let names = listed["TableNames"].as_array().cloned().unwrap_or_default();
    let mut streaming: Vec<String> = Vec::new();
    for name in names {
        let Some(table) = name.as_str() else { continue };
        let replicating = destinations_in(router, region, table)
            .await
            .into_iter()
            .any(|destination| destination["DestinationStatus"] == json!(ACTIVE));
        if replicating {
            streaming.push(table.to_string());
        }
    }
    streaming.sort();
    streaming
}

/// What a row's enable names, and what it does afterwards.
#[derive(Clone, Copy, Debug)]
enum Named {
    /// No enable at all: the table replicates to nothing.
    Nothing,
    /// The data stream this Region's own endpoint holds.
    ThisRegionsStream,
    /// The data stream the OTHER Region's endpoint holds.
    AnotherRegionsStream,
    /// A value exactly as long as the member's floor, naming no stream at all.
    AtTheLengthFloor,
    /// A longer value that is no ARN either.
    PastTheLengthFloor,
    /// This Region's stream, with replication then stopped.
    ThisRegionsStreamThenStopped,
}

impl Named {
    /// The status the destination reports once every span the clock owed it has elapsed, or
    /// `None` where the row registers no destination.
    fn settled_status(self) -> Option<&'static str> {
        match self {
            Named::Nothing => None,
            Named::ThisRegionsStream => Some(ACTIVE),
            Named::AnotherRegionsStream
            | Named::AtTheLengthFloor
            | Named::PastTheLengthFloor => Some("ENABLE_FAILED"),
            Named::ThisRegionsStreamThenStopped => Some("DISABLED"),
        }
    }

    /// Whether the row's table belongs in the Region's streaming inventory.
    fn in_inventory(self) -> bool {
        self.settled_status() == Some(ACTIVE)
    }

    /// The `StreamArn` the row's enable carries, over the stream ARN of this Region's
    /// endpoint and of the other one, or `None` where the row makes no enable.
    fn stream_arn(self, here: &str, elsewhere: &str) -> Option<String> {
        match self {
            Named::Nothing => None,
            Named::ThisRegionsStream | Named::ThisRegionsStreamThenStopped => {
                Some(here.to_string())
            }
            Named::AnotherRegionsStream => Some(elsewhere.to_string()),
            Named::AtTheLengthFloor => Some("x".repeat(STREAM_ARN_MIN_LENGTH)),
            Named::PastTheLengthFloor => {
                Some("this-is-not-an-arn-but-clears-the-length-floor".to_string())
            }
        }
    }
}

/// The table every row of a Region drives, named for what its enable names.
fn table_of(named: Named) -> String {
    format!("{named:?}")
}

/// DynamoDB's refusal of an enable the table's streaming state cannot serve: the statuses
/// an enable may start from, naming neither the table nor the stream.
fn not_enableable_message() -> String {
    "Table is not in a valid state to enable Kinesis Streaming Destination: \
     EnableKinesisStreamingDestination must be DISABLED or ENABLE_FAILED to perform ENABLE \
     operation."
        .to_string()
}

/// DynamoDB's refusal of a command naming a table the endpoint does not hold.
fn not_found_message(table: &str) -> String {
    format!("Requested resource not found: Table: {table} not found")
}

/// Rules 2, 3 and 4: at each Region's endpoint every value that clears the member's length
/// bound is accepted, the destination settles on whether it names a stream of THAT Region,
/// the Region's streaming inventory is exactly the tables whose destination is `ACTIVE`, and
/// a replicating destination delivers into its own Region's stream.
#[tokio::test]
async fn each_region_settles_and_inventories_its_own_destinations() {
    let router = emulator();
    let rows = [
        Named::Nothing,
        Named::ThisRegionsStream,
        Named::AnotherRegionsStream,
        Named::AtTheLengthFloor,
        Named::PastTheLengthFloor,
        Named::ThisRegionsStreamThenStopped,
    ];

    // One data stream per Region, under the same name: the ARNs differ in the Region they
    // name, which is what the cross-Region rows are stated over.
    let mut arns: Vec<String> = Vec::new();
    for region in REGIONS {
        arns.push(ensure_stream_in(&router, region, STREAM).await);
    }

    for (index, region) in REGIONS.into_iter().enumerate() {
        let here = arns[index].clone();
        let elsewhere = arns[(index + 1) % REGIONS.len()].clone();
        for named in rows {
            let table = table_of(named);
            let case = format!("{named:?} at {region}");
            create_table_in(&router, region, &table, &[]).await;
            let Some(stream_arn) = named.stream_arn(&here, &elsewhere) else {
                continue;
            };
            let (status, body) = enable_in(&router, region, &table, &stream_arn).await;
            assert_eq!(
                (status, body["DestinationStatus"].clone()),
                (200, json!(ENABLING)),
                "case {case:?}: every StreamArn that clears the length bound is accepted with \
                 the transient status, got {status} with {body}"
            );
            if matches!(named, Named::ThisRegionsStreamThenStopped) {
                settle().await;
                let (status, stopped) = call_in(
                    &router,
                    region,
                    "DisableKinesisStreamingDestination",
                    json!({"TableName": table, "StreamArn": stream_arn}),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: an ACTIVE destination serves a stop, got {status} with \
                     {stopped}"
                );
            }
        }

        // A value one character SHORTER than the member's floor is the one thing the call
        // refuses for the value itself, before any destination is registered.
        let short = "x".repeat(STREAM_ARN_MIN_LENGTH - 1);
        let table = table_of(Named::Nothing);
        let (status, body) = enable_in(&router, region, &table, &short).await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException".to_string()),
            "case \"one character below the length floor at {region}\": expected the length \
             refusal, got {status} with {body}"
        );
    }

    settle().await;

    for region in REGIONS {
        for named in rows {
            let table = table_of(named);
            let case = format!("{named:?} at {region}");
            let reported = reported_status(&router, &case, region, &table).await;
            let wanted = named.settled_status().map(str::to_string);
            assert_eq!(
                reported, wanted,
                "case {case:?}: expected the destination to settle on {wanted:?}, got {reported:?}"
            );
        }

        let inventory = streaming_tables_in(&router, region).await;
        let mut wanted: Vec<String> = rows
            .into_iter()
            .filter(|named| named.in_inventory())
            .map(table_of)
            .collect();
        wanted.sort();
        assert_eq!(
            inventory, wanted,
            "case \"the streaming inventory of {region}\": expected {wanted:?}, got {inventory:?}"
        );
    }

    // Rule 4: a write to the one replicating table of each Region is delivered into THAT
    // Region's stream, and into no other.
    let replicating = table_of(Named::ThisRegionsStream);
    for region in REGIONS {
        ok_in(
            &router,
            region,
            "PutItem",
            json!({
                "TableName": replicating,
                "Item": {"Part": {"S": "row"}, "Value": {"S": region}},
            }),
        )
        .await;
    }
    for region in REGIONS {
        let case = format!("the records delivered in {region}");
        let records = delivered_in(&router, &case, region, STREAM).await;
        let described: Vec<(Value, Value, Value)> = records
            .iter()
            .map(|record| {
                (
                    record["eventName"].clone(),
                    record["tableName"].clone(),
                    record["awsRegion"].clone(),
                )
            })
            .collect();
        assert_eq!(
            described,
            vec![(json!("INSERT"), json!(replicating), json!(region))],
            "case {case:?}: expected one record from this Region's own table, got {records:?}"
        );
    }
}

/// Rule 1: a table is resolved at the endpoint that created it from the moment the create
/// was accepted, and nowhere else — for the reader and for the destination commands alike.
#[tokio::test]
async fn a_table_is_resolved_only_at_the_endpoint_that_created_it() {
    let router = emulator();
    let table = "HomeStreaming";

    for region in REGIONS {
        let case = format!("{table} created at {region}");
        create_table_in(&router, region, table, &[]).await;
        // No span is waited out here: the table EXISTS from the moment the create answered,
        // whatever its own `TableStatus` reports.
        let (status, body) = call_in(&router, region, "DescribeTable", json!({"TableName": table}))
            .await;
        assert_eq!(
            (status, body["Table"]["TableName"].clone()),
            (200, json!(table)),
            "case {case:?}: the table the create accepted is describable at once, got {status} \
             with {body}"
        );
        let reported = body["Table"]["TableStatus"]
            .as_str()
            .unwrap_or("<no status>")
            .to_string();
        assert!(
            reported == "CREATING" || reported == ACTIVE,
            "case {case:?}: expected the table in its creating or settled status, got {reported}"
        );
        // The destination reader resolves the same table at the same endpoint, and reports
        // no destination for it yet.
        let destinations = destinations_in(&router, region, table).await;
        assert!(
            destinations.is_empty(),
            "case {case:?}: a table no enable named reports no destination, got {destinations:?}"
        );
    }

    // The same name at an endpoint nothing created it at: every command that resolves a
    // table refuses it, with DynamoDB's missing-table sentence.
    let stream_arn = ensure_stream_in(&router, EMPTY_REGION, STREAM).await;
    let refusals: Vec<(&str, Value)> = vec![
        ("DescribeTable", json!({"TableName": table})),
        (
            "DescribeKinesisStreamingDestination",
            json!({"TableName": table}),
        ),
        (
            "EnableKinesisStreamingDestination",
            json!({"TableName": table, "StreamArn": stream_arn}),
        ),
    ];
    for (operation, body) in refusals {
        let case = format!("{operation} at {EMPTY_REGION}");
        let (status, answered) = call_in(&router, EMPTY_REGION, operation, body).await;
        assert_eq!(
            (status, error_code(&answered), message(&answered)),
            (
                400,
                "ResourceNotFoundException".to_string(),
                not_found_message(table)
            ),
            "case {case:?}: a name this endpoint does not hold is refused, got {status} with \
             {answered}"
        );
    }

    // And the table each endpoint DOES hold takes an enable there, for its own Region's
    // stream: the scope is the endpoint's, not the suite's.
    for region in REGIONS {
        let case = format!("an enable at {region}");
        let here = ensure_stream_in(&router, region, STREAM).await;
        let (status, body) = enable_in(&router, region, table, &here).await;
        assert_eq!(
            (status, body["DestinationStatus"].clone()),
            (200, json!(ENABLING)),
            "case {case:?}: the endpoint's own table takes the enable, got {status} with {body}"
        );
    }
}

/// Rule 5: a table replicates to at most one stream in every Region alike — while its
/// destination is live, an enable naming the same stream or another one is refused with the
/// state sentence, and the table still holds its one record.
#[tokio::test]
async fn a_live_destination_refuses_every_further_enable_in_each_region() {
    let router = emulator();
    let table = "SingleDestination";

    for region in REGIONS {
        let live = ensure_stream_in(&router, region, STREAM).await;
        let other = ensure_stream_in(&router, region, "audit").await;
        create_table_in(&router, region, table, &[]).await;
        let (status, body) = enable_in(&router, region, table, &live).await;
        assert_eq!(
            status, 200,
            "case \"the first enable at {region}\": expected HTTP 200, got {status} with {body}"
        );
        settle().await;

        for (addressed, arn) in [("the same stream", &live), ("another stream", &other)] {
            let case = format!("{addressed} at {region}");
            let (status, body) = enable_in(&router, region, table, arn).await;
            let wanted = not_enableable_message();
            assert_eq!(
                (status, error_code(&body), message(&body)),
                (400, "ValidationException".to_string(), wanted.clone()),
                "case {case:?}: expected {wanted:?}, got {status} with {body}"
            );
        }

        settle().await;
        let destinations = destinations_in(&router, region, table).await;
        assert_eq!(
            destinations.len(),
            1,
            "case \"after the refusals at {region}\": a refused enable registers no destination, \
             got {destinations:?}"
        );
        assert_eq!(
            (
                destinations[0]["StreamArn"].clone(),
                destinations[0]["DestinationStatus"].clone(),
            ),
            (json!(live), json!(ACTIVE)),
            "case \"after the refusals at {region}\": the live destination is untouched, got \
             {destinations:?}"
        );
    }
}
