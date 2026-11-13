// SPDX-License-Identifier: CC0-1.0

//! The per-item `Errors` entry `BatchGetServiceLevelObjectiveBudgetReport` (POST
//! `/budget-report`) states for an `SloIds` entry it could not resolve.
//!
//! The rule under test: a per-item failure of this batch is stated as the HTTP STATUS of the
//! failure class rather than as the name of an exception — an item the CALLER got wrong
//! carries the client-error code `400` — and the entry identifies its item by the `Arn` it
//! was asked about alone: an id that resolves to no stored record names no objective to read
//! a `Name` off, so the member is absent rather than parsed back out of the request.
//!
//! The rule is asserted over a TABLE whose rows differ along what it generalises over: the
//! form of the unresolvable id (a bare name, an ARN of this scope, an ARN of another Region,
//! an ARN of another account, a name at either bound of the name class), how many such ids one
//! request carries, and whether the request also names ids that DO resolve.
//!
//! The harness is the one `budget_reports` drives this surface with, so every suite speaks to
//! one set of helpers.

use serde_json::{json, Value};

use crate::budget_reports::{budget_report, now_seconds, CALLER_ERROR_CODE};
use crate::slo_lifecycle::{create, router, sli_config, ACCOUNT, REGION};

/// The members a per-item entry of this operation carries, in the order a reader may expect
/// none other: `Arn`, `ErrorCode` and `ErrorMessage`.
///
/// `ServiceLevelObjectiveBudgetReportError` models `Name`, `Arn`, `ErrorCode` and
/// `ErrorMessage` (`smithy_query(query="shape",shape="ServiceLevelObjectiveBudgetReportError")`),
/// and the entry this service serves carries every one of them but `Name`.
const ENTRY_MEMBERS: [&str; 3] = ["Arn", "ErrorCode", "ErrorMessage"];

/// The ARN of the SLO `name` in the partition, Region and account every request of this suite
/// is signed for.
fn slo_arn(name: &str) -> String {
    format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{name}")
}

/// The shortest and longest `ServiceLevelObjectiveName` the modelled pattern admits
/// (`smithy_query(query="shape",shape="ServiceLevelObjectiveName")`:
/// `^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$`): two anchored alphanumerics, plus up to
/// 126 characters between them.
const NAME_LENGTH_MIN: usize = 2;
const NAME_LENGTH_MAX: usize = 128;

/// A name of exactly `length` characters that the pattern admits.
fn name_of_length(length: usize) -> String {
    format!("a{}z", "b".repeat(length - 2))
}

/// One row: an id that names no stored objective, and the `Arn` the entry for it must carry.
struct Row {
    label: &'static str,
    id: String,
    arn: String,
}

/// The table: every id form the modelled `ServiceLevelObjectiveId` admits for a resource this
/// store does not hold.
fn unresolvable_ids() -> Vec<Row> {
    vec![
        Row {
            label: "a bare name",
            id: "no-such-objective".to_string(),
            // Nothing stored to read an ARN off, so the entry names the ARN that name would
            // carry in the scope the request was signed for.
            arn: slo_arn("no-such-objective"),
        },
        Row {
            label: "an ARN of the signed-for scope",
            id: slo_arn("absent-by-arn"),
            arn: slo_arn("absent-by-arn"),
        },
        Row {
            label: "an ARN of another Region",
            id: "arn:aws:application-signals:us-east-1:000000000000:slo/absent-elsewhere"
                .to_string(),
            arn: "arn:aws:application-signals:us-east-1:000000000000:slo/absent-elsewhere"
                .to_string(),
        },
        Row {
            label: "an ARN of another account",
            id: "arn:aws:application-signals:eu-west-2:999999999999:slo/absent-other-account"
                .to_string(),
            arn: "arn:aws:application-signals:eu-west-2:999999999999:slo/absent-other-account"
                .to_string(),
        },
        Row {
            label: "an ARN of the other partition the pattern admits",
            id: "arn:aws-us-gov:application-signals:us-gov-west-1:000000000000:slo/absent-gov"
                .to_string(),
            arn: "arn:aws-us-gov:application-signals:us-gov-west-1:000000000000:slo/absent-gov"
                .to_string(),
        },
        Row {
            label: "a name at the length minimum",
            id: name_of_length(NAME_LENGTH_MIN),
            arn: slo_arn(&name_of_length(NAME_LENGTH_MIN)),
        },
        Row {
            label: "a name at the length maximum",
            id: name_of_length(NAME_LENGTH_MAX),
            arn: slo_arn(&name_of_length(NAME_LENGTH_MAX)),
        },
    ]
}

/// Assert that `entry` is the per-item failure of `arn`: the client-error code, a message
/// naming that ARN, and no member beyond [`ENTRY_MEMBERS`] — in particular no `Name`.
fn assert_entry(label: &str, entry: &Value, arn: &str) {
    assert_eq!(
        entry["Arn"].as_str(),
        Some(arn),
        "case {label:?}: expected the entry to carry the ARN asked about, got {entry}"
    );
    assert_eq!(
        entry["ErrorCode"].as_str(),
        Some(CALLER_ERROR_CODE),
        "case {label:?}: expected ErrorCode {CALLER_ERROR_CODE:?}, got {entry}"
    );
    let message = entry["ErrorMessage"].as_str().unwrap_or_default();
    assert!(
        message.contains(arn),
        "case {label:?}: expected the message to name the ARN {arn:?}, got {message:?}"
    );
    let members: Vec<String> = entry
        .as_object()
        .map(|members| members.keys().cloned().collect())
        .unwrap_or_default();
    let mut sorted = members.clone();
    sorted.sort();
    let mut expected: Vec<String> = ENTRY_MEMBERS.iter().map(ToString::to_string).collect();
    expected.sort();
    assert_eq!(
        sorted, expected,
        "case {label:?}: expected exactly the members {expected:?}, got {members:?}"
    );
}

/// Every unresolvable id earns one entry stating the client-error code and identifying the
/// item by its ARN alone, whichever form the id took.
#[tokio::test]
async fn an_unresolvable_id_is_stated_as_a_client_error_against_its_arn() {
    let router = router();
    let rows = unresolvable_ids();

    // One id at a time, so each row's own entry is the whole `Errors` list.
    for row in &rows {
        let (status, response) =
            budget_report(&router, std::slice::from_ref(&row.id), now_seconds()).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 with the failure stated per item, got {status} with \
             {response}",
            row.label
        );
        let errors = response["Errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            errors.len(),
            1,
            "case {:?}: expected one entry for the one id, got {response}",
            row.label
        );
        assert_entry(row.label, &errors[0], &row.arn);
    }

    // And all of them in ONE request: every id is answered for, each by its own entry.
    let ids: Vec<String> = rows.iter().map(|row| row.id.clone()).collect();
    let (status, response) = budget_report(&router, &ids, now_seconds()).await;
    assert_eq!(
        status, 200,
        "case the whole batch: expected HTTP 200, got {status} with {response}"
    );
    let errors = response["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        rows.len(),
        "case the whole batch: expected one entry per id, got {response}"
    );
    for (row, entry) in rows.iter().zip(errors.iter()) {
        assert_entry(row.label, entry, &row.arn);
    }
}

/// A batch that names stored objectives too states each failure beside the reports, and only
/// the failures lose the `Name` member: a REPORT of a stored objective carries its name, since
/// there is a record to read one off.
#[tokio::test]
async fn a_report_carries_the_name_a_failed_entry_has_none_of() {
    let router = router();
    let stored = "mixed-batch-objective";
    let (status, created) = create(
        &router,
        json!({
            "Name": stored,
            "SliConfig": sli_config(stored, "GET /orders", 250.0)
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case setup: expected HTTP 200 creating the objective, got {status} with {created}"
    );

    let absent = unresolvable_ids();
    let mut ids = vec![stored.to_string()];
    ids.extend(absent.iter().map(|row| row.id.clone()));

    let (status, response) = budget_report(&router, &ids, now_seconds()).await;
    assert_eq!(
        status, 200,
        "case mixed: expected HTTP 200, got {status} with {response}"
    );
    let reports = response["Reports"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        reports.len(),
        1,
        "case mixed: expected the one stored objective to be reported, got {response}"
    );
    assert_eq!(
        reports[0]["Name"].as_str(),
        Some(stored),
        "case mixed: expected the report to name the objective it is of, got {:?}",
        reports[0]
    );
    assert_eq!(
        reports[0]["Arn"].as_str(),
        Some(slo_arn(stored).as_str()),
        "case mixed: expected the report to carry the stored ARN, got {:?}",
        reports[0]
    );

    let errors = response["Errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        errors.len(),
        absent.len(),
        "case mixed: expected one entry per unresolvable id, got {response}"
    );
    for (row, entry) in absent.iter().zip(errors.iter()) {
        assert_entry(row.label, entry, &row.arn);
    }
}
