// SPDX-License-Identifier: CC0-1.0

//! What a write to a table carrying VECTOR INDEXES is charged for.
//!
//! The rule pinned here: a vector is held as 32-bit floats, four bytes per dimension, so a
//! write of an item carrying one is METERED on that stored form rather than on the decimal
//! text the request spelled each coordinate with — both in the bytes each index reports
//! ("VectorWriteRequestBytes", `ConsumedCapacity$VectorIndexes`, held to a one-kilobyte
//! per-write floor) and in the capacity the item's own write consumes
//! (`ConsumedCapacity$CapacityUnits` and `ConsumedCapacity$Table`, one unit per kilobyte).
//! The per-dimension cost is therefore part of what the write costs, and what it costs
//! does not depend on how many digits a coordinate was written with.
//!
//! The rows differ along what the rule generalises over: the dimension count of the index
//! the item reaches (one small enough to meter the floor and one far past it), which of
//! the table's vector indexes the item reaches at all, and the number of digits each
//! coordinate carries (which moves the item's stored text by kilobytes while leaving the
//! metered figures where they are).
//!
//! `ConsumedCapacity$VectorIndexes` is a map of `VectorCapacity`, whose members are
//! `VectorSearchRequestBytes` and `VectorWriteRequestBytes`
//! (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#VectorCapacity")`), so a
//! write reports the byte figure and no search figure.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{describe_when_active, emulator, ok};

/// The table every row writes to.
const TABLE: &str = "VectorWriteCost";

/// The dimensions of the index whose entry is smaller than the per-write floor, so its
/// bytes are the floor however the coordinates were spelled.
const SMALL_DIMENSIONS: usize = 4;

/// The dimensions of the index whose entry is far past that floor, so its bytes grow with
/// the dimension count.
const LARGE_DIMENSIONS: usize = 1024;

/// What one dimension of a stored vector costs: a vector attribute is a list of 32-bit
/// IEEE-754 floating point numbers (`SearchVectors$SearchVector`), so each dimension is
/// four bytes once it is held as the index holds it.
const BYTES_PER_DIMENSION: i64 = 4;

/// The bytes one write capacity unit covers, and the per-write floor a vector write is
/// held to: "one write capacity unit represents one write per second for an item up to 1
/// KB in size" (the DynamoDB Developer Guide, "Read/write capacity mode").
const UNIT_BYTES: i64 = 1024;

/// The stored size of the partition key every row is written under: the attribute's name
/// (`pk`) plus its two-character value, by the sizing rule of the Developer Guide's "Item
/// sizes and formats". Each row writes its OWN key, so the capacity a write consumes is
/// what its own item costs rather than the larger of it and whatever stood there before
/// ("the item size that matters is the larger of the two").
const KEY_BYTES: i64 = 2 + 2;

/// The two-character partition key row `row` writes under.
fn key_of(row: usize) -> String {
    format!("k{row}")
}

/// One vector of `dimensions` coordinates, each spelled with `digits` significant digits,
/// so the TEXT of the attribute grows with `digits` while the vector it denotes does not.
fn vector(dimensions: usize, digits: usize) -> Value {
    let coordinate = format!("0.{}", "1".repeat(digits));
    Value::Array(
        (0..dimensions)
            .map(|_| json!({"N": coordinate.clone()}))
            .collect(),
    )
}

/// The bytes a vector index over `attribute` with `dimensions` dimensions meters for a
/// write of an item this test writes: four bytes per dimension, the attribute's name and
/// the item's key, held to the one-kilobyte per-write floor.
fn expected_index_bytes(attribute: &str, dimensions: usize) -> f64 {
    let dimensions = i64::try_from(dimensions).expect("the dimension count fits");
    let name = i64::try_from(attribute.len()).expect("the attribute name fits");
    let bytes = (dimensions * BYTES_PER_DIMENSION + name + KEY_BYTES).max(UNIT_BYTES);
    bytes as f64
}

/// The capacity units the write of an item carrying `vectors` — each entry an attribute
/// name and its dimension count — consumes: the item as the store holds it, with every
/// vector counted at four bytes per dimension, at one unit per kilobyte.
fn expected_units(vectors: &[(&str, usize)]) -> f64 {
    let mut size = KEY_BYTES;
    for (attribute, dimensions) in vectors {
        let dimensions = i64::try_from(*dimensions).expect("the dimension count fits");
        let name = i64::try_from(attribute.len()).expect("the attribute name fits");
        size += name + dimensions * BYTES_PER_DIMENSION;
    }
    let units = (size / UNIT_BYTES + i64::from(size % UNIT_BYTES != 0)).max(1);
    units as f64
}

/// Create the table the rows write to: two vector indexes over two attributes, of very
/// different dimension counts, so one write can reach either or both.
async fn vector_table(router: &MultiServiceRouter) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "BillingMode": "PAY_PER_REQUEST",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "VectorIndexes": [
                {
                    "IndexName": "idx-small",
                    "VectorAttribute": {"AttributeName": "emb_small"},
                    "Dimensions": SMALL_DIMENSIONS,
                    "DistanceFunction": "COSINE",
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "idx-large",
                    "VectorAttribute": {"AttributeName": "emb_large"},
                    "Dimensions": LARGE_DIMENSIONS,
                    "DistanceFunction": "COSINE",
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
        }),
    )
    .await;
    describe_when_active(router, TABLE, TABLE).await;
}

/// A number the response carried, or `None` where the member is absent.
fn figure(consumed: &Value, path: &[&str]) -> Option<f64> {
    let mut value = consumed;
    for member in path {
        value = value.get(*member)?;
    }
    value.as_f64()
}

/// One row: the case, the vector attributes the item carries with their dimension counts,
/// and how many significant digits each coordinate is spelled with.
struct WriteRow {
    case: &'static str,
    vectors: Vec<(&'static str, usize)>,
    digits: usize,
}

#[tokio::test]
async fn a_write_is_charged_for_the_vector_it_stores() {
    let router = emulator();
    vector_table(&router).await;

    // Rows differ in which indexes the item reaches and in how many digits each coordinate
    // carries: the two 18-digit rows write kilobytes more TEXT than their 1-digit
    // counterparts for the same vectors, and must be metered and charged identically.
    let both = vec![
        ("emb_small", SMALL_DIMENSIONS),
        ("emb_large", LARGE_DIMENSIONS),
    ];
    let small = vec![("emb_small", SMALL_DIMENSIONS)];
    let large = vec![("emb_large", LARGE_DIMENSIONS)];
    let cases: [WriteRow; 6] = [
        WriteRow {
            case: "both vectors, one digit a coordinate",
            vectors: both.clone(),
            digits: 1,
        },
        WriteRow {
            case: "both vectors, eighteen digits a coordinate",
            vectors: both,
            digits: 18,
        },
        WriteRow {
            case: "the small vector alone, one digit",
            vectors: small.clone(),
            digits: 1,
        },
        WriteRow {
            case: "the small vector alone, eighteen digits",
            vectors: small,
            digits: 18,
        },
        WriteRow {
            case: "the large vector alone, one digit",
            vectors: large.clone(),
            digits: 1,
        },
        WriteRow {
            case: "the large vector alone, eighteen digits",
            vectors: large,
            digits: 18,
        },
    ];

    for (row, WriteRow { case, vectors, digits }) in cases.into_iter().enumerate() {
        let mut item = json!({"pk": {"S": key_of(row)}});
        for (attribute, dimensions) in &vectors {
            item[*attribute] = json!({"L": vector(*dimensions, digits)});
        }
        let answered = ok(
            &router,
            "PutItem",
            json!({"TableName": TABLE, "Item": item,
                   "ReturnConsumedCapacity": "INDEXES"}),
        )
        .await;
        let consumed = &answered["ConsumedCapacity"];

        // Each index the write reached reports the bytes it metered, which is the stored
        // vector and nothing of the text it was written as; an index the item does not
        // reach reports nothing at all.
        for (index, attribute, dimensions) in [
            ("idx-small", "emb_small", SMALL_DIMENSIONS),
            ("idx-large", "emb_large", LARGE_DIMENSIONS),
        ] {
            let reported = figure(consumed, &["VectorIndexes", index, "VectorWriteRequestBytes"]);
            let expected = vectors
                .iter()
                .any(|(written, _)| *written == attribute)
                .then(|| expected_index_bytes(attribute, dimensions));
            assert_eq!(
                reported, expected,
                "case {case:?}: expected {index} to meter {expected:?} write-request \
                 byte(s), got {reported:?} in {consumed}"
            );
            assert_eq!(
                figure(consumed, &["VectorIndexes", index, "VectorSearchRequestBytes"]),
                None,
                "case {case:?}: a write searches nothing, so {index} reports no search \
                 figure, got {consumed}"
            );
        }

        // And the capacity the write consumed counts the vector at that same stored size,
        // on the aggregate and on the table's own line alike.
        let expected = expected_units(&vectors);
        for member in [vec!["CapacityUnits"], vec!["Table", "CapacityUnits"], vec!["WriteCapacityUnits"]] {
            let reported = figure(consumed, &member);
            assert_eq!(
                reported,
                Some(expected),
                "case {case:?}: expected {member:?} to be {expected} unit(s) for the stored \
                 vector, got {reported:?} in {consumed}"
            );
        }
    }
}
