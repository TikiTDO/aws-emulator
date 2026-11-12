// SPDX-License-Identifier: CC0-1.0

//! What EVERY `TableDescription` a table operation returns must carry, and where each
//! member's value comes from.
//!
//! `CreateTable`, `DescribeTable`, `UpdateTable` and `DeleteTable` all return the same
//! projection of the stored table, so the rules here are asserted over a TABLE of table
//! shapes (capacity mode, warm throughput asked for in full, in part or not at all,
//! secondary indexes) rather than one example:
//!
//! * `TableId` is the UUID minted at create — present on every description of the table,
//!   the same value each time, and different for a different table;
//! * `ProvisionedThroughput.NumberOfDecreasesToday` is the number of decreases applied
//!   in this UTC day, reported from the first description (0) onwards, and
//!   `LastDecreaseDateTime`/`LastIncreaseDateTime` appear with the first move of each
//!   kind;
//! * `BillingModeSummary.LastUpdateToPayPerRequestDateTime` is when `PAY_PER_REQUEST`
//!   was LAST set as the capacity mode — absent until it has been, and kept after a
//!   switch back to `PROVISIONED`;
//! * `WarmThroughput` is the pair in force (the documented default when no request named
//!   one), with the status of that pair;
//! * `ItemCount`/`TableSizeBytes`, and an index's `ItemCount`/`IndexSizeBytes`, are the
//!   figures of the last metering refresh, which a write never moves.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    service_state, COUNTER_REFRESH_INTERVAL, DEFAULT_WARM_READ_UNITS_PER_SECOND,
    DEFAULT_WARM_WRITE_UNITS_PER_SECOND, STATUS_TRANSITION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call, emulator, emulator_and_engine, REGION};

/// One row of the description table: the table to create, and what its description must
/// then report.
struct TableRow {
    case: &'static str,
    name: &'static str,
    /// The `BillingMode` the create asks for; `None` leaves the member out, which
    /// resolves to `PROVISIONED`.
    billing_mode: Option<&'static str>,
    /// The `ProvisionedThroughput` a `PROVISIONED` table is created with.
    throughput: Option<(i64, i64)>,
    /// The `WarmThroughput` the create asks for, member by member; `None` leaves the
    /// member out.
    warm: Option<(Option<i64>, Option<i64>)>,
    /// The capacity mode the table ends up in.
    expected_billing_mode: &'static str,
    /// The read and write capacity units the description must report (0 and 0 for an
    /// on-demand table: "For a table ... that uses on-demand capacity mode
    /// (PAY_PER_REQUEST), this value is 0" — `ProvisionedThroughputDescription`).
    expected_capacity: (i64, i64),
    /// The warm throughput pair the description must report.
    expected_warm: (i64, i64),
    /// Whether `LastUpdateToPayPerRequestDateTime` must be there, which it is only once
    /// `PAY_PER_REQUEST` has been set as the mode.
    pay_per_request_set: bool,
}

/// The rows differ along everything the rules generalise over: the capacity mode (named
/// or defaulted), the capacity values, and whether the request names a warm throughput
/// in full, in part or not at all.
const TABLE_ROWS: &[TableRow] = &[
    TableRow {
        case: "an on-demand table, no warm throughput named",
        name: "OnDemandPlain",
        billing_mode: Some("PAY_PER_REQUEST"),
        throughput: None,
        warm: None,
        expected_billing_mode: "PAY_PER_REQUEST",
        expected_capacity: (0, 0),
        expected_warm: (
            DEFAULT_WARM_READ_UNITS_PER_SECOND,
            DEFAULT_WARM_WRITE_UNITS_PER_SECOND,
        ),
        pay_per_request_set: true,
    },
    TableRow {
        case: "a provisioned table, no warm throughput named",
        name: "ProvisionedPlain",
        billing_mode: Some("PROVISIONED"),
        throughput: Some((7, 3)),
        warm: None,
        expected_billing_mode: "PROVISIONED",
        expected_capacity: (7, 3),
        // Warm throughput is "the minimum provisioned capacity maintained for immediate
        // data access" (`TableWarmThroughputDescription`), so a table nobody named one
        // for keeps warm the capacity it IS provisioned for — the recorded description of
        // a provisioned table reports its own units, not the on-demand default pair.
        expected_warm: (7, 3),
        pay_per_request_set: false,
    },
    TableRow {
        case: "a table with no BillingMode at all and both warm members named",
        name: "DefaultedMode",
        billing_mode: None,
        throughput: Some((1, 1)),
        warm: Some((Some(24_000), Some(8_000))),
        expected_billing_mode: "PROVISIONED",
        expected_capacity: (1, 1),
        expected_warm: (24_000, 8_000),
        pay_per_request_set: false,
    },
    TableRow {
        case: "an on-demand table naming only the read half of its warm throughput",
        name: "OnDemandWarmRead",
        billing_mode: Some("PAY_PER_REQUEST"),
        throughput: None,
        warm: Some((Some(36_000), None)),
        expected_billing_mode: "PAY_PER_REQUEST",
        // The member the request leaves out keeps the documented default.
        expected_warm: (36_000, DEFAULT_WARM_WRITE_UNITS_PER_SECOND),
        expected_capacity: (0, 0),
        pay_per_request_set: true,
    },
];

/// Send one operation that must answer 200.
async fn ok(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let (status, parsed) = call(router, operation, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
    );
    parsed
}

/// The wire error code (`__type`) of a refusal.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Wait out a transition the clock owes, so what follows observes the settled state.
fn wait_out_transition() {
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
}

/// Whether `value` is in the format `TableDescription$TableId` declares: the member's own
/// pattern `^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$` (from
/// `smithy_query(query="shape", shape="TableDescription")`).
fn is_uuid(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
}

/// The create request one row asks for.
fn create_request(row: &TableRow) -> Value {
    let mut body = json!({
        "TableName": row.name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
    });
    if let Some(mode) = row.billing_mode {
        body["BillingMode"] = json!(mode);
    }
    if let Some((read, write)) = row.throughput {
        body["ProvisionedThroughput"] =
            json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write});
    }
    if let Some((read, write)) = row.warm {
        let mut warm = json!({});
        if let Some(read) = read {
            warm["ReadUnitsPerSecond"] = json!(read);
        }
        if let Some(write) = write {
            warm["WriteUnitsPerSecond"] = json!(write);
        }
        body["WarmThroughput"] = warm;
    }
    body
}

/// Every rule above, over one description of one row's table. `origin` names the
/// operation the description came from, so a failure says which of the four diverged.
fn assert_description(row: &TableRow, origin: &str, description: &Value) {
    let case = row.case;
    assert!(
        is_uuid(&description["TableId"]),
        "case {case:?} from {origin}: expected a UUID TableId, got {description}"
    );
    assert_eq!(
        description["ProvisionedThroughput"]["NumberOfDecreasesToday"],
        json!(0),
        "case {case:?} from {origin}: a table that has had no decrease reports 0, got \
         {description}"
    );
    assert_eq!(
        (
            description["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            description["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
        ),
        (
            json!(row.expected_capacity.0),
            json!(row.expected_capacity.1)
        ),
        "case {case:?} from {origin}: expected the capacity it was created with, got \
         {description}"
    );
    assert_eq!(
        description["TableThroughputModeSummary"]["TableThroughputMode"],
        json!(row.expected_billing_mode),
        "case {case:?} from {origin}: expected the resolved capacity mode, got {description}"
    );
    // "You may need to switch to on-demand mode at least once in order to return a
    // BillingModeSummary response" (that shape's own note): the summary — and the instant
    // it carries — is there exactly for a table that has been on-demand.
    let billing = description.get("BillingModeSummary");
    assert_eq!(
        billing.is_some(),
        row.pay_per_request_set,
        "case {case:?} from {origin}: expected BillingModeSummary \
         present={}, got {description}",
        row.pay_per_request_set
    );
    if let Some(billing) = billing {
        assert_eq!(
            billing["BillingMode"],
            json!(row.expected_billing_mode),
            "case {case:?} from {origin}: expected the resolved capacity mode, got \
             {description}"
        );
        assert_eq!(
            billing["LastUpdateToPayPerRequestDateTime"],
            description["CreationDateTime"],
            "case {case:?} from {origin}: a table created on-demand had PAY_PER_REQUEST \
             set when it was created, got {description}"
        );
    }
    assert_eq!(
        (
            description["WarmThroughput"]["ReadUnitsPerSecond"].clone(),
            description["WarmThroughput"]["WriteUnitsPerSecond"].clone(),
            description["WarmThroughput"]["Status"].clone(),
        ),
        (
            json!(row.expected_warm.0),
            json!(row.expected_warm.1),
            json!("ACTIVE"),
        ),
        "case {case:?} from {origin}: expected the warm throughput in force, got \
         {description}"
    );
    assert_eq!(
        (
            description["ItemCount"].clone(),
            description["TableSizeBytes"].clone(),
        ),
        (json!(0), json!(0)),
        "case {case:?} from {origin}: a table that has been metered once, empty, reports \
         zeroes, got {description}"
    );
}

#[tokio::test]
async fn every_table_description_carries_the_members_read_from_the_stored_table() {
    let router = emulator();
    let mut minted: Vec<String> = Vec::new();

    for row in TABLE_ROWS {
        let case = row.case;
        let created = ok(&router, "CreateTable", create_request(row)).await;
        let created = &created["TableDescription"];
        // The create response reports the table CREATING, and the capacity mode it was
        // created with is the one it has had from that instant.
        assert_description(row, "CreateTable", created);

        wait_out_transition();
        let described = ok(&router, "DescribeTable", json!({"TableName": row.name})).await;
        let described = &described["Table"];
        assert_description(row, "DescribeTable", described);
        assert_eq!(
            described["TableId"], created["TableId"],
            "case {case:?}: DescribeTable must echo the TableId the create minted, got \
             {described}"
        );

        // An UpdateTable that changes nothing about the throughput still reports the
        // same stored members.
        let updated = ok(
            &router,
            "UpdateTable",
            json!({"TableName": row.name, "DeletionProtectionEnabled": false}),
        )
        .await;
        let updated = &updated["TableDescription"];
        assert_description(row, "UpdateTable", updated);
        assert_eq!(
            updated["TableId"], created["TableId"],
            "case {case:?}: UpdateTable must echo the same TableId, got {updated}"
        );

        wait_out_transition();
        let deleted = ok(&router, "DeleteTable", json!({"TableName": row.name})).await;
        let deleted = &deleted["TableDescription"];
        assert_description(row, "DeleteTable", deleted);
        assert_eq!(
            deleted["TableId"], created["TableId"],
            "case {case:?}: DeleteTable must report the id of the table it is deleting, \
             got {deleted}"
        );

        let id = created["TableId"].as_str().unwrap_or_default().to_string();
        assert!(
            !minted.contains(&id),
            "case {case:?}: every table gets its own TableId, but {id} was minted before \
             ({minted:?})"
        );
        minted.push(id);
    }
}

#[tokio::test]
async fn last_update_to_pay_per_request_records_when_on_demand_was_last_set() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "ModeSwitch",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "owner", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            // An index carries capacity of its own, which the mode switch withdraws too.
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByOwner",
                "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 2},
            }],
        }),
    )
    .await;
    wait_out_transition();

    // A table created PROVISIONED has never had PAY_PER_REQUEST set, so it carries no
    // BillingModeSummary at all ("you may need to switch to on-demand mode at least once
    // in order to return a BillingModeSummary response").
    let described = ok(&router, "DescribeTable", json!({"TableName": "ModeSwitch"})).await;
    assert_eq!(
        described["Table"].get("BillingModeSummary"),
        None,
        "case \"never on-demand\": expected no BillingModeSummary, got {described}"
    );

    let switched = ok(
        &router,
        "UpdateTable",
        json!({"TableName": "ModeSwitch", "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    // The instant the member reports is the instant of the CALL that set the mode, so
    // this response — the switch's own echo, with the capacity move still settling —
    // already carries it.
    assert!(
        switched["TableDescription"]["BillingModeSummary"]
            ["LastUpdateToPayPerRequestDateTime"]
            .as_f64()
            .is_some_and(|instant| instant > 0.0),
        "case \"the switch's own echo\": expected the instant the switch was made, got \
         {switched}"
    );
    assert_eq!(
        switched["TableDescription"]["ProvisionedThroughput"]["ReadCapacityUnits"],
        json!(0),
        "case \"switched to on-demand\": an on-demand table reports no provisioned \
         capacity, got {switched}"
    );
    // The reported capacity went from 5/5 to 0/0: the last time it went down is now,
    // while the day's COUNTED decreases (the documented daily allowance) stay unspent.
    let throughput = &switched["TableDescription"]["ProvisionedThroughput"];
    assert_eq!(
        throughput["NumberOfDecreasesToday"],
        json!(0),
        "case \"switched to on-demand\": withdrawing capacity spends none of the day's \
         counted decreases, got {throughput}"
    );
    assert!(
        throughput
            .get("LastDecreaseDateTime")
            .and_then(Value::as_f64)
            .is_some_and(|instant| instant > 0.0),
        "case \"switched to on-demand\": expected LastDecreaseDateTime, got {throughput}"
    );
    // An index's capacity is withdrawn with the table's, and reads the same way.
    let index = &switched["TableDescription"]["GlobalSecondaryIndexes"][0];
    assert_eq!(
        (
            index["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            index["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
            index["ProvisionedThroughput"]["NumberOfDecreasesToday"].clone(),
        ),
        (json!(0), json!(0), json!(0)),
        "case \"switched to on-demand\": an on-demand index reports no provisioned \
         capacity and no counted decrease, got {index}"
    );
    assert!(
        index["ProvisionedThroughput"]
            .get("LastDecreaseDateTime")
            .and_then(Value::as_f64)
            .is_some_and(|instant| instant > 0.0),
        "case \"switched to on-demand\": expected the index's LastDecreaseDateTime, got \
         {index}"
    );
    wait_out_transition();

    // Every later description reports the same instant.
    let on_demand = ok(&router, "DescribeTable", json!({"TableName": "ModeSwitch"})).await;
    let set_at = on_demand["Table"]["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"]
        .as_f64();
    assert_eq!(
        set_at,
        switched["TableDescription"]["BillingModeSummary"]
            ["LastUpdateToPayPerRequestDateTime"]
            .as_f64(),
        "case \"switched to on-demand\": expected the instant the switch reported to \
         stand, got {on_demand}"
    );

    // A request that RESTATES the mode already in force sets nothing: the mode was
    // already PAY_PER_REQUEST, so the instant it was last set stands.
    let restated = ok(
        &router,
        "UpdateTable",
        json!({"TableName": "ModeSwitch", "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    assert_eq!(
        restated["TableDescription"]["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"]
            .as_f64(),
        set_at,
        "case \"on-demand restated\": expected the instant it was last SET to stand, got \
         {restated}"
    );
    wait_out_transition();

    // "Represents the time when PAY_PER_REQUEST was LAST set as the read/write capacity
    // mode": switching back to PROVISIONED does not erase it.
    let back = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "ModeSwitch",
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
            // An index of a PROVISIONED table must be given capacity too: "initial
            // provisioned capacity values must be set" (`UpdateTable$BillingMode`).
            "GlobalSecondaryIndexUpdates": [{
                "Update": {
                    "IndexName": "ByOwner",
                    "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
                },
            }],
        }),
    )
    .await;
    assert_eq!(
        back["TableDescription"]["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"]
            .as_f64(),
        set_at,
        "case \"switched back to provisioned\": expected the instant on-demand was last \
         set to stand, got {back}"
    );
    assert_eq!(
        back["TableDescription"]["BillingModeSummary"]["BillingMode"],
        json!("PROVISIONED"),
        "case \"switched back to provisioned\": expected the new mode, got {back}"
    );
    // Coming back from on-demand raises the capacity the table REPORTS from zero ("on-demand
    // mode does not use provisioned throughput" — `ProvisionedThroughputDescription`), so it
    // is an increase and is stamped as one; the day's decrease count stands, because
    // withdrawing capacity never spent one of them.
    let throughput = &back["TableDescription"]["ProvisionedThroughput"];
    assert_eq!(
        (
            throughput["NumberOfDecreasesToday"].clone(),
            throughput
                .get("LastIncreaseDateTime")
                .and_then(Value::as_f64)
                .is_some(),
        ),
        (json!(0), true),
        "case \"switched back to provisioned\": expected no counted decrease and the \
         increase from zero recorded, got {throughput}"
    );
}

/// One capacity move, and what the throughput description must report after it.
struct CapacityMove {
    case: &'static str,
    read: i64,
    write: i64,
    /// `NumberOfDecreasesToday` after the move: a decrease in EITHER dimension counts.
    decreases: i64,
    increased: bool,
    decreased: bool,
}

/// The moves differ in which dimension moves, in which direction, and in how many
/// decreases have accumulated by then.
const CAPACITY_MOVES: &[CapacityMove] = &[
    CapacityMove {
        case: "the read dimension lowered",
        read: 4,
        write: 10,
        decreases: 1,
        increased: false,
        decreased: true,
    },
    CapacityMove {
        case: "the write dimension lowered too",
        read: 4,
        write: 4,
        decreases: 2,
        increased: false,
        decreased: true,
    },
    CapacityMove {
        case: "one dimension raised, the other left alone",
        read: 9,
        write: 4,
        decreases: 2,
        increased: true,
        decreased: true,
    },
    CapacityMove {
        case: "one dimension raised and the other lowered in one call",
        read: 11,
        write: 2,
        decreases: 3,
        increased: true,
        decreased: true,
    },
];

#[tokio::test]
async fn number_of_decreases_today_counts_the_decreases_applied_today() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Capacity",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 10, "WriteCapacityUnits": 10},
        }),
    )
    .await;
    wait_out_transition();

    for step in CAPACITY_MOVES {
        let case = step.case;
        let updated = ok(
            &router,
            "UpdateTable",
            json!({
                "TableName": "Capacity",
                "ProvisionedThroughput": {
                    "ReadCapacityUnits": step.read, "WriteCapacityUnits": step.write,
                },
            }),
        )
        .await;
        let throughput = &updated["TableDescription"]["ProvisionedThroughput"];
        assert_eq!(
            throughput["NumberOfDecreasesToday"],
            json!(step.decreases),
            "case {case:?}: expected {} decreases applied today, got {throughput}",
            step.decreases
        );
        assert_eq!(
            throughput
                .get("LastDecreaseDateTime")
                .and_then(Value::as_f64)
                .is_some(),
            step.decreased,
            "case {case:?}: expected LastDecreaseDateTime present={}, got {throughput}",
            step.decreased
        );
        assert_eq!(
            throughput
                .get("LastIncreaseDateTime")
                .and_then(Value::as_f64)
                .is_some(),
            step.increased,
            "case {case:?}: expected LastIncreaseDateTime present={}, got {throughput}",
            step.increased
        );

        // The same figures come back from a plain read of the stored table.
        wait_out_transition();
        let described = ok(&router, "DescribeTable", json!({"TableName": "Capacity"})).await;
        assert_eq!(
            described["Table"]["ProvisionedThroughput"]["NumberOfDecreasesToday"],
            json!(step.decreases),
            "case {case:?}: DescribeTable reports the same count, got {described}"
        );
    }
}

#[tokio::test]
async fn warm_throughput_reports_the_pair_in_force_and_its_own_status() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Warm",
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    wait_out_transition();

    // A change is in flight for the span the clock owes it, so the SETTING is UPDATING
    // while the new pair is already what the table reports.
    let updated = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "Warm",
            "WarmThroughput": {"ReadUnitsPerSecond": 48_000, "WriteUnitsPerSecond": 16_000},
        }),
    )
    .await;
    let warm = &updated["TableDescription"]["WarmThroughput"];
    assert_eq!(
        (
            warm["ReadUnitsPerSecond"].clone(),
            warm["WriteUnitsPerSecond"].clone(),
            warm["Status"].clone(),
        ),
        (json!(48_000), json!(16_000), json!("UPDATING")),
        "case \"a warm throughput change in flight\": expected the new pair, UPDATING, \
         got {updated}"
    );

    wait_out_transition();
    let described = ok(&router, "DescribeTable", json!({"TableName": "Warm"})).await;
    let warm = &described["Table"]["WarmThroughput"];
    assert_eq!(
        (
            warm["ReadUnitsPerSecond"].clone(),
            warm["WriteUnitsPerSecond"].clone(),
            warm["Status"].clone(),
        ),
        (json!(48_000), json!(16_000), json!("ACTIVE")),
        "case \"the change has settled\": expected the stored pair, ACTIVE, got {described}"
    );

    // Asking for the pair already in force asks for no change, so the setting stays
    // ACTIVE even while the table itself is UPDATING.
    let unchanged = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "Warm",
            "WarmThroughput": {"ReadUnitsPerSecond": 48_000, "WriteUnitsPerSecond": 16_000},
        }),
    )
    .await;
    assert_eq!(
        (
            unchanged["TableDescription"]["WarmThroughput"]["Status"].clone(),
            unchanged["TableDescription"]["TableStatus"].clone(),
        ),
        (json!("ACTIVE"), json!("UPDATING")),
        "case \"a warm throughput re-stated\": expected the setting ACTIVE and the table \
         UPDATING, got {unchanged}"
    );
    wait_out_transition();

    // The floor the MODEL states is 1 (`PositiveLongObject`, range min 1): below it no
    // description could carry the value, so it is refused — on the bound itself, one past
    // it and well past it. A value between 1 and the documented default pair is NOT
    // refused: nothing in the model or the docs makes that pair a minimum, so a caller
    // asking for less gets what it asked for back.
    let refusals: [(&str, i64, i64); 3] = [
        ("both members at the bound minus one", 0, 0),
        ("the read member below the bound", 0, 4_000),
        ("the write member below the bound", 12_000, -1),
    ];
    for (case, read, write) in refusals {
        let (status, refused) = call(
            &router,
            "UpdateTable",
            json!({
                "TableName": "Warm",
                "WarmThroughput": {"ReadUnitsPerSecond": read, "WriteUnitsPerSecond": write},
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a warm throughput under the modelled minimum is refused, got \
             {status} {refused}"
        );
    }
    // The bound itself, and a value under the documented default pair, are stored and
    // reported back.
    for (case, read, write) in [
        ("the modelled bound itself", 1, 1),
        (
            "a pair under the documented default",
            DEFAULT_WARM_READ_UNITS_PER_SECOND - 1,
            DEFAULT_WARM_WRITE_UNITS_PER_SECOND - 1,
        ),
    ] {
        let accepted = ok(
            &router,
            "UpdateTable",
            json!({
                "TableName": "Warm",
                "WarmThroughput": {"ReadUnitsPerSecond": read, "WriteUnitsPerSecond": write},
            }),
        )
        .await;
        assert_eq!(
            (
                accepted["TableDescription"]["WarmThroughput"]["ReadUnitsPerSecond"].clone(),
                accepted["TableDescription"]["WarmThroughput"]["WriteUnitsPerSecond"].clone(),
            ),
            (json!(read), json!(write)),
            "case {case:?}: expected the requested pair to be stored and reported, got \
             {accepted}"
        );
        wait_out_transition();
    }
}

#[tokio::test]
async fn item_and_size_figures_come_from_the_metering_refresh_not_from_the_writes() {
    let (router, engine) = emulator_and_engine();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Metered",
            "KeySchema": [
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "sk", "AttributeType": "S"},
                {"AttributeName": "owner", "AttributeType": "S"},
                {"AttributeName": "label", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [
                {
                    "IndexName": "ByOwnerAll",
                    "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "ALL"},
                },
                {
                    "IndexName": "ByOwnerKeysOnly",
                    "KeySchema": [{"AttributeName": "owner", "KeyType": "HASH"}],
                    "Projection": {"ProjectionType": "KEYS_ONLY"},
                },
            ],
            "LocalSecondaryIndexes": [
                {
                    "IndexName": "ByLabel",
                    "KeySchema": [
                        {"AttributeName": "pk", "KeyType": "HASH"},
                        {"AttributeName": "label", "KeyType": "RANGE"},
                    ],
                    "Projection": {"ProjectionType": "ALL"},
                },
            ],
        }),
    )
    .await;
    wait_out_transition();

    // Two items reach the global indexes (they carry `owner`) and one reaches the local
    // one (it carries `label`); the third reaches neither.
    for (sk, owner, label) in [
        ("one", Some("ada"), Some("x")),
        ("two", Some("bob"), None),
        ("three", None, None),
    ] {
        let mut item = json!({
            "pk": {"S": "p"}, "sk": {"S": sk},
            "payload": {"S": "a longer value than the keys"},
        });
        if let Some(owner) = owner {
            item["owner"] = json!({"S": owner});
        }
        if let Some(label) = label {
            item["label"] = json!({"S": label});
        }
        ok(
            &router,
            "PutItem",
            json!({"TableName": "Metered", "Item": item}),
        )
        .await;
    }

    let described = ok(&router, "DescribeTable", json!({"TableName": "Metered"})).await;
    assert_eq!(
        (
            described["Table"]["ItemCount"].clone(),
            described["Table"]["TableSizeBytes"].clone(),
        ),
        (json!(0), json!(0)),
        "case \"right after three writes\": expected the figures of the last refresh, \
         which a write does not move, got {described}"
    );
    for member in ["GlobalSecondaryIndexes", "LocalSecondaryIndexes"] {
        for index in described["Table"][member]
            .as_array()
            .cloned()
            .unwrap_or_default()
        {
            let name = index["IndexName"].clone();
            assert_eq!(
                (index["ItemCount"].clone(), index["IndexSizeBytes"].clone()),
                (json!(0), json!(0)),
                "case \"{member} {name} right after three writes\": expected the figures \
                 of the last refresh, got {index}"
            );
        }
    }

    // The refresh is owed every six hours, which no test can wait for: it is driven here
    // the way a later request drives it, through the stored table, at an instant one
    // interval on. What it must then report is what the store actually holds.
    {
        let state = service_state(&*engine).expect("this service's store");
        let mut store = state.lock();
        let table = store
            .table_mut(REGION, "Metered")
            .expect("the table that was just created");
        table.refresh_due_counters(SystemTime::now() + COUNTER_REFRESH_INTERVAL);
    }

    let refreshed = ok(&router, "DescribeTable", json!({"TableName": "Metered"})).await;
    let table = &refreshed["Table"];
    assert_eq!(
        table["ItemCount"],
        json!(3),
        "case \"after a refresh fell due\": expected every stored item counted, got {table}"
    );
    let size = table["TableSizeBytes"].as_i64().unwrap_or_default();
    assert!(
        size > 0,
        "case \"after a refresh fell due\": expected the stored bytes measured, got {table}"
    );
    let index_named = |member: &str, name: &str| -> Value {
        table[member]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|index| index["IndexName"] == json!(name))
            .unwrap_or(Value::Null)
    };
    let all = index_named("GlobalSecondaryIndexes", "ByOwnerAll");
    let keys_only = index_named("GlobalSecondaryIndexes", "ByOwnerKeysOnly");
    let local = index_named("LocalSecondaryIndexes", "ByLabel");
    // Each index counts exactly the items carrying its own key attributes.
    for (index, expected) in [(&all, 2), (&keys_only, 2), (&local, 1)] {
        assert_eq!(
            index["ItemCount"],
            json!(expected),
            "case \"after a refresh fell due\": only the items carrying the index key are \
             projected, got {index}"
        );
        assert!(
            index["IndexSizeBytes"].as_i64().unwrap_or_default() > 0,
            "case \"after a refresh fell due\": projected items give the index a size, \
             got {index}"
        );
    }
    assert!(
        all["IndexSizeBytes"].as_i64().unwrap_or_default()
            > keys_only["IndexSizeBytes"].as_i64().unwrap_or_default(),
        "case \"after a refresh fell due\": an ALL projection measures more than a \
         KEYS_ONLY one for the same items, got {all} and {keys_only}"
    );
}
