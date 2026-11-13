// SPDX-License-Identifier: CC0-1.0

//! What decides whether `ExportTableToPointInTime` is served: the point-in-time recovery
//! state of the table the request addresses, as it stands at the instant of the call.
//!
//! The rule pinned here, over a set of inputs rather than one example:
//!
//! * a table that has a point in time to export from is SERVED — the export is recorded
//!   and `DescribeExport` reports the very record the call answered;
//! * a table that has none is refused with the modelled
//!   `PointInTimeRecoveryUnavailableException` ("Point in time recovery is not enabled for
//!   table '<name>'") and records nothing;
//! * which of the two it is depends on nothing else: not on the other members the request
//!   carries, not on the recovery state of the OTHER tables the endpoint holds, and not on
//!   the addressed table's recovery history — the same table flips between refused and
//!   served as its own recovery setting is turned off and on again.
//!
//! Every call is driven over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same `MultiServiceRouter`
//! the emulator's front doors use, with Amazon S3 mounted beside this service on ONE engine
//! so the export has a destination that really exists.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state;

use crate::emulator::{
    call, create_bucket, describe_when_active, emulator_with_s3, error_code, export_arn, message,
    ok, restorable_window, REGION,
};

/// The bucket every export of this suite writes to.
const BUCKET: &str = "export-recovery-precondition";

/// One on-demand table with one item in it, ready to be exported.
async fn table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
    ok(
        router,
        "PutItem",
        json!({"TableName": name, "Item": {"Part": {"S": "only"}}}),
    )
    .await;
}

/// Turn point-in-time recovery on (or off) for `name`.
async fn set_recovery(router: &MultiServiceRouter, name: &str, enabled: bool) {
    ok(
        router,
        "UpdateContinuousBackups",
        json!({
            "TableName": name,
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": enabled},
        }),
    )
    .await;
}

/// How many exports the readers hold for `name`.
async fn recorded_exports(router: &MultiServiceRouter, name: &str) -> usize {
    let listed = ok(
        router,
        "ListExports",
        json!({"TableArn": state::table_arn(REGION, name)}),
    )
    .await;
    listed["ExportSummaries"]
        .as_array()
        .map_or(0, Vec::len)
}

/// Assert `table` is SERVED an export asking for `extra`, and that the record the call
/// answered is the one `DescribeExport` reports — the round trip through the reader.
async fn assert_served(router: &MultiServiceRouter, case: &str, name: &str, extra: Value) -> String {
    let mut body = json!({"TableArn": state::table_arn(REGION, name), "S3Bucket": BUCKET});
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    let (status, answered) = call(router, "ExportTableToPointInTime", body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a table with a point in time to export from is served, got {status} \
         with {answered:?}"
    );
    let arn = export_arn(&answered);
    assert!(
        arn.starts_with(&format!("{}/export/", state::table_arn(REGION, name))),
        "case {case:?}: expected an ExportArn under the exported table, got {arn:?}"
    );
    let described = ok(router, "DescribeExport", json!({"ExportArn": arn.clone()})).await;
    assert_eq!(
        described["ExportDescription"]["ExportArn"],
        json!(arn),
        "case {case:?}: DescribeExport reports the export the call recorded, got {described:?}"
    );
    assert_eq!(
        described["ExportDescription"]["TableArn"],
        json!(state::table_arn(REGION, name)),
        "case {case:?}: the record names the table it was taken from, got {described:?}"
    );
    arn
}

/// Assert `table` is REFUSED an export asking for `extra`, with the modelled refusal of a
/// table that has no point in time to export from.
async fn assert_refused(router: &MultiServiceRouter, case: &str, name: &str, extra: Value) {
    let mut body = json!({"TableArn": state::table_arn(REGION, name), "S3Bucket": BUCKET});
    for (member, value) in extra.as_object().cloned().unwrap_or_default() {
        body[member] = value;
    }
    let (status, answered) = call(router, "ExportTableToPointInTime", body.clone()).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with {answered:?}"
    );
    assert_eq!(
        error_code(&answered),
        "PointInTimeRecoveryUnavailableException",
        "case {case:?}: expected the modelled refusal of a table with no point in time to \
         export from, got {answered:?}"
    );
    assert_eq!(
        message(&answered),
        format!("Point in time recovery is not enabled for table '{name}'"),
        "case {case:?}: the refusal names the table the request addressed, got {answered:?}"
    );
}

#[tokio::test]
async fn the_addressed_tables_recovery_state_decides_every_export() {
    let router = emulator_with_s3();
    create_bucket(&router, BUCKET).await;
    // Two tables of ONE endpoint, differing only in whether they have a point in time to
    // export from: the decision is per-table, so one being exportable says nothing about
    // the other.
    table(&router, "RecoveryOn").await;
    table(&router, "RecoveryOff").await;
    set_recovery(&router, "RecoveryOn", true).await;
    let (_, latest) = restorable_window(&router, "RecoveryOn").await;

    // Rows differ in what ELSE the request asks for — the members it carries beside the
    // two required ones — because none of them can turn the decision around. The point in
    // time named is the newest the exportable table's window reaches, read from the
    // continuous-backups reader that publishes the bounds.
    let asked: [(&str, Value); 5] = [
        ("only the required members", json!({})),
        (
            "a prefix and a bucket owner",
            json!({
                "S3Prefix": "nightly/snapshots",
                "S3BucketOwner": engine::emulator_account_id(),
            }),
        ),
        ("the format named", json!({"ExportFormat": "DYNAMODB_JSON"})),
        ("a point in time named", json!({"ExportTime": latest})),
        (
            "a client token",
            json!({"ClientToken": "recovery-precondition-token"}),
        ),
    ];

    let mut served = 0usize;
    for (asked_for, extra) in asked {
        // The same request, member for member, against each of the two tables: what
        // differs between the two answers is only the addressed table's recovery state.
        //
        // A client token names ONE export, so the served row carries a token of its own
        // per row; the refused row's token is beside the point, since nothing is recorded
        // under it.
        let mut on = extra.clone();
        if let Some(token) = on.get("ClientToken").and_then(Value::as_str) {
            on["ClientToken"] = json!(format!("{token}-on"));
        }
        assert_served(
            &router,
            &format!("recovery enabled, {asked_for}"),
            "RecoveryOn",
            on,
        )
        .await;
        served += 1;
        assert_refused(
            &router,
            &format!("recovery disabled, {asked_for}"),
            "RecoveryOff",
            extra,
        )
        .await;
        // Every refusal so far recorded nothing, and every success recorded exactly one
        // export — read back from the listing rather than assumed.
        assert_eq!(
            recorded_exports(&router, "RecoveryOff").await,
            0,
            "case {asked_for:?}: a refused export records nothing"
        );
        assert_eq!(
            recorded_exports(&router, "RecoveryOn").await,
            served,
            "case {asked_for:?}: each served export is recorded once"
        );
    }
}

#[tokio::test]
async fn turning_recovery_off_and_on_flips_the_same_table() {
    let router = emulator_with_s3();
    create_bucket(&router, BUCKET).await;
    table(&router, "Toggled").await;
    // A sibling whose recovery stays on throughout, so each step below shows the decision
    // follows the ADDRESSED table rather than the endpoint.
    table(&router, "AlwaysOn").await;
    set_recovery(&router, "AlwaysOn", true).await;

    // Rows are the recovery states the table is put through, in order: a table that never
    // had recovery, one that has it, one that had it and lost it again, and one that has
    // it back. Whether the export is served follows the state the table is in at the
    // instant of the call, never the states it was in before.
    let steps: [(&str, Option<bool>, bool); 4] = [
        ("recovery never enabled", None, false),
        ("recovery enabled", Some(true), true),
        ("recovery enabled and then disabled", Some(false), false),
        ("recovery enabled again", Some(true), true),
    ];

    let mut recorded = 0usize;
    for (case, setting, servable) in steps {
        if let Some(enabled) = setting {
            set_recovery(&router, "Toggled", enabled).await;
        }
        if servable {
            assert_served(&router, case, "Toggled", json!({})).await;
            recorded += 1;
        } else {
            assert_refused(&router, case, "Toggled", json!({})).await;
        }
        assert_eq!(
            recorded_exports(&router, "Toggled").await,
            recorded,
            "case {case:?}: only a served export is recorded"
        );
        // The sibling is unaffected by any of it: its own recovery is on, so it is served
        // at every step.
        assert_served(
            &router,
            &format!("{case}, sibling with its own recovery on"),
            "AlwaysOn",
            json!({}),
        )
        .await;
    }
}
