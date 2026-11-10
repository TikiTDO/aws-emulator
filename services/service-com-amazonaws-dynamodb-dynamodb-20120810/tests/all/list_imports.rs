// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListImports`.
//!
//! The rule pinned here: a listing reports one `ImportSummary` per import record the
//! store holds that the request SELECTS — the region the request reached, the table its
//! `TableArn` names, and the documented window "within the past 90 days" — in ascending
//! `ImportArn` order, `PageSize` entries at a time, with `NextToken` reported exactly
//! when more remain and resuming strictly after the entry the page stopped on. Every
//! member of a summary is read from the record, so nothing is canned.
//!
//! Each rule is asserted over a SET of inputs: the records held, the table named, the
//! region, the page size (on each bound the member publishes and one past it), the age of
//! a record, and the cursor.
//!
//! `ImportTable` — the operation that WRITES an import record — creates a table of its
//! own, so the suites below seed the records they are about through the same per-service
//! store the handler reads (the shared engine `emulator_and_engine` hands back), which is
//! what lets one case hold a record of any age, status or region; `tests/all/table_imports`
//! is where the records an import really writes are pinned. Every read here is driven over
//! the wire through the router the emulator's front doors use.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use service_com_amazonaws_dynamodb_dynamodb_20120810::ops::types::{
    S3BucketSource, IMPORT_STATUS_VALUES, INPUT_FORMAT_VALUES,
};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::{
    self, Import, ImportDetails, IMPORT_HISTORY_WINDOW,
};

use crate::emulator::{call, call_in, emulator_and_engine, error_code, message, ok, REGION};

/// A second region, for the rule that a listing answers over the region its request
/// reached.
const OTHER_REGION: &str = "eu-west-1";

/// One day, the unit the documented 90-day window is stated in.
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// A whole-second instant `ago` before now: a record stored at whole seconds reports a
/// timestamp a test can compare exactly, and every instant here stays inside the
/// documented window unless a case puts it outside on purpose.
fn seconds_ago(ago: Duration) -> SystemTime {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch");
    UNIX_EPOCH + Duration::from_secs(now.as_secs().saturating_sub(ago.as_secs()))
}

/// Record one import in the store the handler reads, answering the `ImportArn` it was
/// recorded under.
fn record_import(engine: &Arc<engine::InMemoryEngine>, import: Import) -> String {
    let arn = import.arn.clone();
    state::service_state(&**engine)
        .expect("the shared engine holds this service's store")
        .lock()
        .insert_import(import);
    arn
}

/// One finished import of `table` in `region`, begun at `started_at`.
fn completed_import(region: &str, table: &str, started_at: SystemTime) -> Import {
    Import {
        arn: state::new_import_arn(region, table, started_at),
        region: region.to_string(),
        table_name: table.to_string(),
        status: "COMPLETED".to_string(),
        s3_bucket_source: S3BucketSource {
            s3_bucket: "import-source".to_string(),
            s3_bucket_owner: None,
            s3_key_prefix: None,
        },
        cloud_watch_log_group_arn: None,
        input_format: "DYNAMODB_JSON".to_string(),
        started_at,
        ended_at: Some(started_at + Duration::from_secs(30)),
        // The members only the two single-import readers report, which this suite is not
        // about: a listing reads none of them.
        details: ImportDetails::default(),
    }
}

/// A `NextToken` in the form the model declares for the member (`ImportNextToken`:
/// groups of 16 hex digits, at least 112 characters), carrying `cursor`.
///
/// A client never builds one — it copies the value a page reported — so this is only how
/// a case here names a cursor the store holds no record after.
fn token_for(cursor: &str) -> String {
    let mut bytes = cursor.as_bytes().to_vec();
    while bytes.len() < 56 || !bytes.len().is_multiple_of(8) {
        bytes.push(0);
    }
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The `ImportSummaryList` of a response, as a list.
fn summaries(body: &Value) -> Vec<Value> {
    body["ImportSummaryList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `ImportArn`s a response reports, in the order it reported them.
fn listed_arns(body: &Value) -> Vec<String> {
    summaries(body)
        .iter()
        .map(|summary| {
            summary["ImportArn"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// `body`'s `NextToken`, or `None` where it reported none.
fn next_token(body: &Value) -> Option<String> {
    body["NextToken"].as_str().map(str::to_string)
}

/// One `CreateTable` body with a simple key, for a case that is not about the table.
fn simple_table(name: &str) -> Value {
    json!({
        "TableName": name,
        "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
        "BillingMode": "PAY_PER_REQUEST",
    })
}

#[tokio::test]
async fn an_account_that_has_run_no_import_is_answered_an_empty_page() {
    let (router, _engine) = emulator_and_engine();
    ok(&router, "CreateTable", simple_table("Imported")).await;

    // Rows differ in what the request asks for: the table it names, the page it asks
    // for, and the cursor it resumes from. None of them can conjure a record.
    let cases: [(&str, Value); 7] = [
        ("no member at all", json!({})),
        (
            "the ARN of a table that exists",
            json!({"TableArn": state::table_arn(REGION, "Imported")}),
        ),
        (
            "the ARN of a table that was never created",
            json!({"TableArn": state::table_arn(REGION, "NeverCreated")}),
        ),
        // The two page sizes the member publishes (`ListImportsMaxLimit`, range 1..=25).
        ("the smallest page the member allows", json!({"PageSize": 1})),
        ("a page size inside the range", json!({"PageSize": 13})),
        ("the largest page the member allows", json!({"PageSize": 25})),
        (
            "a cursor no record can follow",
            json!({"NextToken": token_for(&format!(
                "{}/import/99999999999999-ffffffff",
                state::table_arn(REGION, "Zzz")
            ))}),
        ),
    ];

    for (case, body) in cases {
        let answered = ok(&router, "ListImports", body.clone()).await;
        assert!(
            answered["ImportSummaryList"].is_array(),
            "case {case:?}: the listing must report an ImportSummaryList, got {answered}"
        );
        assert!(
            summaries(&answered).is_empty(),
            "case {case:?}: expected no import summaries, got {answered}"
        );
        assert_eq!(
            next_token(&answered),
            None,
            "case {case:?}: a last page reports no NextToken, got {answered}"
        );
    }
}

#[tokio::test]
async fn every_member_of_a_recorded_import_is_reported() {
    let (router, engine) = emulator_and_engine();

    // One record per `ImportStatus` the model declares, each with an `InputFormat` from
    // the model's own list — the two catalogues this service exposes as
    // `IMPORT_STATUS_VALUES` and `INPUT_FORMAT_VALUES` (the `ImportStatus` and
    // `InputFormat` enums; `smithy_query(query="shape",shape="ImportStatus,InputFormat")`)
    // — so no status or format the API admits goes untested. The optional members
    // (`S3BucketOwner`, `S3KeyPrefix`, `CloudWatchLogGroupArn`, `EndTime`) alternate, so
    // a reported summary is pinned both with and without each.
    let mut recorded: Vec<Import> = Vec::new();
    for (index, status) in IMPORT_STATUS_VALUES.iter().enumerate() {
        let format = INPUT_FORMAT_VALUES[index % INPUT_FORMAT_VALUES.len()];
        let started = seconds_ago(Duration::from_secs(3_600 * (index as u64 + 1)));
        let finished = *status != "IN_PROGRESS";
        let detailed = index % 2 == 0;
        recorded.push(Import {
            arn: state::new_import_arn(REGION, "Imported", started),
            region: REGION.to_string(),
            table_name: "Imported".to_string(),
            status: (*status).to_string(),
            s3_bucket_source: S3BucketSource {
                s3_bucket: format!("import-source-{index}"),
                s3_bucket_owner: detailed.then(|| engine::emulator_account_id().to_string()),
                s3_key_prefix: (!detailed).then(|| format!("exports/{index}/")),
            },
            cloud_watch_log_group_arn: detailed.then(|| {
                engine::emulator_arn(
                    "logs",
                    REGION,
                    &format!("log-group:/aws-dynamodb/imports/{index}"),
                )
            }),
            input_format: format.to_string(),
            started_at: started,
            ended_at: finished.then(|| started + Duration::from_secs(45)),
            details: ImportDetails::default(),
        });
    }
    for import in &recorded {
        record_import(&engine, import.clone());
    }

    let answered = ok(&router, "ListImports", json!({})).await;
    assert_eq!(
        summaries(&answered).len(),
        recorded.len(),
        "every recorded import must be reported, got {answered}"
    );

    for import in &recorded {
        let case = format!("{} / {}", import.status, import.input_format);
        let reported = summaries(&answered)
            .into_iter()
            .find(|summary| summary["ImportArn"] == json!(import.arn))
            .unwrap_or(Value::Null);
        assert_eq!(
            reported["ImportStatus"],
            json!(import.status),
            "case {case:?}: the reported status is the one recorded, got {reported}"
        );
        assert_eq!(
            reported["TableArn"],
            json!(state::table_arn(&import.region, &import.table_name)),
            "case {case:?}: the reported TableArn is the table's own, got {reported}"
        );
        assert_eq!(
            reported["InputFormat"],
            json!(import.input_format),
            "case {case:?}: the reported InputFormat is the one recorded, got {reported}"
        );
        assert_eq!(
            reported["S3BucketSource"]["S3Bucket"],
            json!(import.s3_bucket_source.s3_bucket),
            "case {case:?}: the reported source bucket is the one recorded, got {reported}"
        );
        assert_eq!(
            reported["S3BucketSource"]["S3BucketOwner"].as_str(),
            import.s3_bucket_source.s3_bucket_owner.as_deref(),
            "case {case:?}: the source bucket owner is reported exactly when recorded, \
             got {reported}"
        );
        assert_eq!(
            reported["S3BucketSource"]["S3KeyPrefix"].as_str(),
            import.s3_bucket_source.s3_key_prefix.as_deref(),
            "case {case:?}: the source key prefix is reported exactly when recorded, got \
             {reported}"
        );
        assert_eq!(
            reported["CloudWatchLogGroupArn"].as_str(),
            import.cloud_watch_log_group_arn.as_deref(),
            "case {case:?}: the log group is reported exactly when recorded, got {reported}"
        );
        assert_eq!(
            reported["StartTime"].as_f64(),
            Some(epoch_seconds(import.started_at)),
            "case {case:?}: the reported StartTime is the instant recorded, got {reported}"
        );
        assert_eq!(
            reported["EndTime"].as_f64(),
            import.ended_at.map(epoch_seconds),
            "case {case:?}: an EndTime is reported exactly when the import has finished, \
             got {reported}"
        );
    }
}

/// An instant as the wire carries it: epoch seconds.
fn epoch_seconds(instant: SystemTime) -> f64 {
    instant
        .duration_since(UNIX_EPOCH)
        .expect("the instant is after the epoch")
        .as_secs_f64()
}

#[tokio::test]
async fn a_listing_reports_the_records_the_request_selects() {
    let (router, engine) = emulator_and_engine();
    // One table that still exists and one that never did: an import record outlives the
    // table it wrote into, so the listing is answered from the records and not from the
    // tables.
    ok(&router, "CreateTable", simple_table("Alpha")).await;

    let alpha: Vec<String> = (1..=2)
        .map(|hours| {
            record_import(
                &engine,
                completed_import(REGION, "Alpha", seconds_ago(Duration::from_secs(3_600 * hours))),
            )
        })
        .collect();
    let beta = vec![record_import(
        &engine,
        completed_import(REGION, "Beta", seconds_ago(Duration::from_secs(600))),
    )];
    let elsewhere = vec![record_import(
        &engine,
        completed_import(OTHER_REGION, "Alpha", seconds_ago(Duration::from_secs(900))),
    )];

    let mut here = [alpha.clone(), beta.clone()].concat();
    here.sort();
    let mut alpha_sorted = alpha.clone();
    alpha_sorted.sort();

    // Rows differ in the region the request reaches and the table its `TableArn` names.
    let cases: [(&str, &str, Value, Vec<String>); 5] = [
        (
            "no table named, in the region the records are in",
            REGION,
            json!({}),
            here,
        ),
        (
            "the table that still exists",
            REGION,
            json!({"TableArn": state::table_arn(REGION, "Alpha")}),
            alpha_sorted,
        ),
        (
            "a table whose own records outlived it",
            REGION,
            json!({"TableArn": state::table_arn(REGION, "Beta")}),
            beta,
        ),
        (
            "a table of this region with no record of its own",
            REGION,
            json!({"TableArn": state::table_arn(REGION, "Gamma")}),
            Vec::new(),
        ),
        (
            "another region, where only its own record is addressable",
            OTHER_REGION,
            json!({}),
            elsewhere,
        ),
    ];

    for (case, region, body, expected) in cases {
        let (status, answered) = call_in(&router, region, "ListImports", body.clone()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answered}"
        );
        assert_eq!(
            listed_arns(&answered),
            expected,
            "case {case:?}: expected the selected records in ascending ImportArn order, \
             got {answered}"
        );
    }
}

#[tokio::test]
async fn a_page_carries_page_size_records_and_a_token_that_resumes_after_it() {
    let (router, engine) = emulator_and_engine();

    // Seeded newest first, so a listing in ascending ARN order cannot be insertion order.
    let mut all: Vec<String> = (1..=7)
        .rev()
        .map(|hours| {
            record_import(
                &engine,
                completed_import(REGION, "Paged", seconds_ago(Duration::from_secs(3_600 * hours))),
            )
        })
        .collect();
    all.sort();

    // Rows differ in the page size asked for: both bounds the member publishes
    // (`ListImportsMaxLimit`, range 1..=25), sizes inside it either side of the record
    // count, and a request that names none.
    let cases: [(&str, Option<i32>); 6] = [
        ("one record at a time", Some(1)),
        ("two records at a time", Some(2)),
        ("a page smaller than the records held", Some(3)),
        ("a page exactly as large as the records held", Some(7)),
        ("the largest page the member allows", Some(25)),
        ("no page size named", None),
    ];

    for (case, page_size) in cases {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..=all.len() {
            let mut body = json!({});
            if let Some(size) = page_size {
                body["PageSize"] = json!(size);
            }
            if let Some(cursor) = token.as_deref() {
                body["NextToken"] = json!(cursor);
            }
            let answered = ok(&router, "ListImports", body.clone()).await;
            let page = listed_arns(&answered);
            let remaining = all.len() - walked.len();
            let expected_page = page_size
                .map_or(remaining, |size| usize::try_from(size).unwrap_or(0))
                .min(remaining);
            assert_eq!(
                page.len(),
                expected_page,
                "case {case:?}: expected {expected_page} records on this page, got {answered}"
            );
            walked.extend(page);
            token = next_token(&answered);
            match token.as_deref() {
                Some(minted) => {
                    assert!(
                        walked.len() < all.len(),
                        "case {case:?}: a NextToken is reported only while records remain, \
                         got {answered}"
                    );
                    // The form the member models (`ImportNextToken`, len 112..=1024,
                    // pattern ^([0-9a-f]{16})+$), so the token is one a client may send
                    // straight back.
                    assert!(
                        (112..=1024).contains(&minted.len())
                            && minted.len().is_multiple_of(16)
                            && minted.bytes().all(|byte| byte.is_ascii_digit()
                                || (b'a'..=b'f').contains(&byte)),
                        "case {case:?}: the minted token {minted:?} must satisfy the \
                         ImportNextToken form"
                    );
                }
                None => break,
            }
        }
        assert_eq!(
            walked, all,
            "case {case:?}: paging must walk every record exactly once, in ascending \
             ImportArn order"
        );
        assert_eq!(
            token, None,
            "case {case:?}: the walk must end on a page that reports no NextToken"
        );
    }
}

#[tokio::test]
async fn a_record_outside_the_documented_window_is_not_reported() {
    let (router, engine) = emulator_and_engine();

    // Rows differ in the AGE of the record: inside the documented 90-day window, exactly
    // on it, and past it. "Lists completed imports within the past 90 days" (the
    // `ListImports` doc), measured from the instant the import finished — or, for one
    // still running, from the instant it began.
    let window = IMPORT_HISTORY_WINDOW;
    let cases: [(&str, Duration, bool, bool); 5] = [
        ("finished a day ago", DAY, true, true),
        ("finished a day inside the window", window - DAY, true, true),
        ("finished exactly a window ago", window, true, false),
        ("finished a day past the window", window + DAY, true, false),
        ("still running, begun two days ago", 2 * DAY, false, true),
    ];

    let mut expected: Vec<String> = Vec::new();
    for (case, age, finished, reported) in cases {
        let marker = seconds_ago(age);
        let started = marker - Duration::from_secs(60);
        let import = Import {
            ended_at: finished.then_some(marker),
            status: if finished { "COMPLETED" } else { "IN_PROGRESS" }.to_string(),
            ..completed_import(REGION, "Aged", started)
        };
        let arn = record_import(&engine, import);
        if reported {
            expected.push(arn);
        } else {
            // Named so a failure below says which row was wrong.
            assert!(!arn.is_empty(), "case {case:?}: the record was given an ImportArn");
        }
    }
    expected.sort();

    let answered = ok(&router, "ListImports", json!({})).await;
    assert_eq!(
        listed_arns(&answered),
        expected,
        "only records inside the documented window are reported, got {answered}"
    );
}

#[tokio::test]
async fn a_table_arn_that_addresses_no_table_of_this_endpoint_is_refused() {
    let (router, engine) = emulator_and_engine();
    record_import(
        &engine,
        completed_import(REGION, "Alpha", seconds_ago(Duration::from_secs(600))),
    );

    // Rows differ in HOW the value fails to be an ARN of this service naming a table of
    // the endpoint the request reached: not an ARN at all, a bare table name, a deeper
    // sub-resource, another service, and another region's table.
    let index_arn = format!("{}/index/ByPart", state::table_arn(REGION, "Alpha"));
    let cases: [(&str, String); 5] = [
        ("not an ARN at all", "Alpha".to_string()),
        ("an ARN of another service", engine::emulator_arn("s3", REGION, "Alpha")),
        ("an index of the table", index_arn),
        (
            "a table of another region",
            state::table_arn(OTHER_REGION, "Alpha"),
        ),
        (
            "an ARN with no resource path",
            format!(
                "arn:aws:dynamodb:{REGION}:{}:Alpha",
                engine::emulator_account_id()
            ),
        ),
    ];

    for (case, value) in cases {
        let (status, answered) =
            call(&router, "ListImports", json!({"TableArn": value.clone()})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("Invalid TableArn: Invalid TableArn provided as input {value}"),
            "case {case:?}: the refusal states the member and echoes the value sent"
        );
    }
}

#[tokio::test]
async fn a_request_outside_the_members_own_bounds_is_refused() {
    let (router, engine) = emulator_and_engine();
    record_import(
        &engine,
        completed_import(REGION, "Alpha", seconds_ago(Duration::from_secs(600))),
    );

    // Rows differ in which member is out of bounds: the page size either side of the
    // range the model publishes (`ListImportsMaxLimit`, range 1..=25), and a `NextToken`
    // that is not a token this operation mints — too short for the member's 112-character
    // floor, not hexadecimal, and well-formed but standing for no cursor at all.
    let cases: [(&str, Value); 6] = [
        ("a page smaller than the member allows", json!({"PageSize": 0})),
        ("a negative page", json!({"PageSize": -1})),
        ("a page larger than the member allows", json!({"PageSize": 26})),
        ("a token under the member's length floor", json!({"NextToken": "0".repeat(96)})),
        ("a token that is not hexadecimal", json!({"NextToken": "z".repeat(112)})),
        ("a well-formed token carrying no cursor", json!({"NextToken": "0".repeat(112)})),
    ];

    for (case, body) in cases {
        let (status, answered) = call(&router, "ListImports", body.clone()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with {answered}"
        );
        assert_eq!(
            error_code(&answered),
            "ValidationException",
            "case {case:?}: expected a ValidationException, got {answered}"
        );
    }

    // A token this operation did not mint is refused in the wording this service states
    // for that case, rather than taken for the first page.
    let (_, answered) = call(
        &router,
        "ListImports",
        json!({"NextToken": "0".repeat(112)}),
    )
    .await;
    assert_eq!(
        message(&answered),
        "Invalid NextToken",
        "a token standing for no cursor is refused as an invalid NextToken, got {answered}"
    );
}
