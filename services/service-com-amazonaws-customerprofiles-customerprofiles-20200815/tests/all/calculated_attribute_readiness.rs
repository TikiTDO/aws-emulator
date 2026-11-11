// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the readiness a calculated attribute definition reports while the
//! indexing of its historical data is still owed.
//!
//! The rule under test: a calculated attribute definition's `Status` and `Readiness` are a
//! function of the clock alone. A definition with historical data to index reports
//! `PREPARING` with `ProgressPercentage` below full for the whole span the indexing takes,
//! then `COMPLETED` at full progress once that span has passed, with no further call made
//! to make it so; a definition with nothing to index is `COMPLETED` at full progress from
//! its first read and stays so. Progress stays inside the 0..=100 the model declares for
//! `Readiness$ProgressPercentage` and never goes backwards, and reading a definition
//! changes nothing a later caller sees.
//!
//! The sibling suite `calculated_attribute_definitions` asserts the two endpoints of that
//! span (the status at the create, and the status after it); this one asserts what holds AT
//! EVERY instant in between, over a set of sample points rather than one.
//!
//! Every case goes through the real router, so what is asserted is the wire behaviour a
//! client sees. The HTTP plumbing is this crate's one test harness, in
//! `tests/all/domain_profile_lifecycle.rs`, and the request helpers are the ones
//! `tests/all/calculated_attribute_definitions.rs` already defines.

use serde_json::{json, Value};
use std::time::Duration;

use service_com_amazonaws_customerprofiles_customerprofiles_20200815::state::HISTORICAL_DATA_INDEXING_SPAN;

use crate::calculated_attribute_definitions::{attribute_details, create, definition, get};
use crate::domain_profile_lifecycle::Emulator;

/// The `Status` of a definition whose historical data is still being indexed, and of one
/// whose indexing is done — the two values of the modelled `ReadinessStatus` this service
/// reports for a definition that was accepted.
const PREPARING: &str = "PREPARING";
const COMPLETED: &str = "COMPLETED";

/// Full `Readiness$ProgressPercentage`: the maximum of the range the model declares for it.
const FULL_PROGRESS: i64 = 100;

/// The fractions of the indexing span the definitions below are read at: the start, and
/// three points inside it. Each is a different instant in the SAME span, which is what the
/// rule generalises over, and all of them are inside its first half so a loaded machine
/// cannot carry a read past the span.
const SAMPLE_FRACTIONS: [f64; 4] = [0.0, 0.15, 0.3, 0.45];

/// The `Status` a response reports.
fn status_of(body: &Value) -> Option<&str> {
    body.get("Status").and_then(Value::as_str)
}

/// The `Readiness$ProgressPercentage` a response reports.
fn progress_of(body: &Value) -> Option<i64> {
    body.get("Readiness")
        .and_then(|readiness| readiness.get("ProgressPercentage"))
        .and_then(Value::as_i64)
}

/// A request that asks for historical data to be included, so the definition has something
/// to index and therefore a span to be `PREPARING` for.
fn historical(statistic: &str) -> Value {
    json!({
        "AttributeDetails": attribute_details("Orders", &["Amount"]),
        "Statistic": statistic,
        "UseHistoricalData": true,
    })
}

/// One row of the table: a definition to create, and whether it has historical data to
/// index — which is the only thing the rule's two halves turn on.
struct Row {
    case: &'static str,
    name: &'static str,
    body: Value,
    indexing: bool,
}

/// The definitions the table below creates: rows with historical data to index, spanning
/// statistic and how much optional configuration the request carries, and rows with nothing
/// to index, spanning the two ways a request declines historical data.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the required members and historical data",
            name: "Span_Minimal",
            body: historical("SUM"),
            indexing: true,
        },
        Row {
            case: "historical data under a different statistic",
            name: "Span_Count",
            body: historical("COUNT"),
            indexing: true,
        },
        Row {
            case: "historical data alongside every optional member",
            name: "Span_Configured",
            body: json!({
                "AttributeDetails": attribute_details("Orders", &["Amount", "Discount"]),
                "Statistic": "AVERAGE",
                "DisplayName": "Order spread",
                "Description": "the spread of every order",
                "Conditions": { "Range": { "Value": 30, "Unit": "DAYS" } },
                "UseHistoricalData": true,
                "Tags": { "team": "crm" },
            }),
            indexing: true,
        },
        Row {
            case: "UseHistoricalData omitted, so nothing to index",
            name: "Instant_Omitted",
            body: definition("SUM"),
            indexing: false,
        },
        Row {
            case: "UseHistoricalData false, so nothing to index",
            name: "Instant_False",
            body: json!({
                "AttributeDetails": attribute_details("Orders", &["Amount"]),
                "Statistic": "MAXIMUM",
                "UseHistoricalData": false,
            }),
            indexing: false,
        },
    ]
}

/// RULE, over the whole span rather than at its endpoints: for as long as the indexing of a
/// definition's historical data is owed, every read reports `PREPARING` with progress inside
/// 0..100 and never below what an earlier read reported, while a definition with nothing to
/// index is already `COMPLETED` at full progress; once the span has passed every read
/// reports `COMPLETED` at full progress, with no call made to bring that about. No read
/// moves anything the create stored.
///
/// Every row is created at the start and read at each sample point, so the one span covers
/// the whole table instead of a span being waited out per row.
#[tokio::test]
async fn readiness_follows_the_clock_for_the_whole_indexing_span() {
    let emulator = Emulator::new();
    emulator.create_domain("readydom", 30).await;

    let table = rows();
    let mut created: Vec<Value> = Vec::new();
    for row in &table {
        let (status, response) = create(&emulator, "readydom", row.name, row.body.clone()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for {:?}, got {status} {response:?}",
            row.case, row.body
        );
        // A definition with nothing to index has nothing to wait for, so it is ready in the
        // create's own answer.
        if !row.indexing {
            assert_eq!(
                (status_of(&response), progress_of(&response)),
                (Some(COMPLETED), Some(FULL_PROGRESS)),
                "case {:?}: expected the create to report {COMPLETED} at full progress, \
                 got {response:?}",
                row.case
            );
        }
        created.push(response);
    }

    // Progress may only ever rise, so the highest value each definition has reported so far
    // is the floor its next read has to clear.
    let mut floor: Vec<i64> = vec![0; table.len()];
    let mut elapsed = Duration::ZERO;

    for fraction in SAMPLE_FRACTIONS {
        let target = HISTORICAL_DATA_INDEXING_SPAN.mul_f64(fraction);
        if target > elapsed {
            tokio::time::sleep(target - elapsed).await;
            elapsed = target;
        }

        for (index, row) in table.iter().enumerate() {
            let case = row.case;
            let (status, read) = get(&emulator, "readydom", row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} at {fraction} of the span: expected 200, got {status} {read:?}"
            );

            let progress = progress_of(&read).unwrap_or_else(|| {
                panic!("case {case:?} at {fraction} of the span: expected a ProgressPercentage, got {read:?}")
            });
            if row.indexing {
                assert_eq!(
                    status_of(&read),
                    Some(PREPARING),
                    "case {case:?} at {fraction} of the span: expected Status {PREPARING} \
                     while the indexing is still owed, got {:?}",
                    status_of(&read)
                );
                assert!(
                    (0..FULL_PROGRESS).contains(&progress),
                    "case {case:?} at {fraction} of the span: expected progress inside \
                     0..{FULL_PROGRESS} while the indexing is still owed, got {progress:?}"
                );
            } else {
                assert_eq!(
                    (status_of(&read), Some(progress)),
                    (Some(COMPLETED), Some(FULL_PROGRESS)),
                    "case {case:?} at {fraction} of the span: a definition reported ready \
                     stays ready, got {read:?}"
                );
            }
            assert!(
                progress >= floor[index],
                "case {case:?} at {fraction} of the span: progress must never go backwards, \
                 got {progress:?} after {:?}",
                floor[index]
            );
            floor[index] = progress;

            // The read is a report: it moves neither the instant the definition was created
            // nor anything else the create stored.
            for member in ["CreatedAt", "AttributeDetails", "Statistic"] {
                assert_eq!(
                    read.get(member),
                    created[index].get(member),
                    "case {case:?} at {fraction} of the span: expected the {member} the \
                     create stored, got {:?} after {:?}",
                    read.get(member),
                    created[index].get(member)
                );
            }
        }
    }

    // Past the span the indexing is due for every row, with no further call made to make it
    // so, and it stays due.
    tokio::time::sleep(HISTORICAL_DATA_INDEXING_SPAN - elapsed + Duration::from_millis(250)).await;

    for row in &table {
        let case = row.case;
        for round in ["the first read past the span", "the read after that"] {
            let (status, read) = get(&emulator, "readydom", row.name).await;
            assert_eq!(
                status, 200,
                "case {case:?} / {round}: expected 200, got {status} {read:?}"
            );
            assert_eq!(
                (status_of(&read), progress_of(&read)),
                (Some(COMPLETED), Some(FULL_PROGRESS)),
                "case {case:?} / {round}: expected {COMPLETED} at full progress once the \
                 indexing is due, got {read:?}"
            );
        }
    }
}
